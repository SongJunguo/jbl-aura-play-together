//! Crash-persistent, non-identifying uncertainty journal.
//!
//! A pending record is durably committed before a whole-pair backend action is
//! allowed to begin. It is replaced by a durable clean record only after the
//! action acknowledgement and fresh membership postcondition both succeed.

#[cfg(test)]
use std::cell::Cell;
use std::env;
use std::ffi::{CStr, CString};
use std::fmt;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{DirBuilderExt, MetadataExt};
use std::path::{Path, PathBuf};

use openssl::rand::rand_bytes;

const APPLICATION_DIRECTORY: &str = "jbl-aura-link-rust";
const JOURNAL_FILE: &str = "uncertainty.state";
const PENDING_FILE: &str = "uncertainty.pending";
const MAX_JOURNAL_BYTES: u64 = 128;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum JournalAction {
    Start,
    Stop,
    Shutdown,
    RecoverStop,
}

impl JournalAction {
    const fn label(self) -> &'static str {
        match self {
            Self::Start => "start",
            Self::Stop => "stop",
            Self::Shutdown => "shutdown",
            Self::RecoverStop => "recover-stop",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        match value {
            "start" => Some(Self::Start),
            "stop" => Some(Self::Stop),
            "shutdown" => Some(Self::Shutdown),
            "recover-stop" => Some(Self::RecoverStop),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum JournalState {
    Clean,
    Pending(JournalAction),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum JournalError {
    StateDirectoryUnavailable,
    StateDirectoryUntrusted,
    JournalUntrusted,
    JournalInvalid,
    JournalWriteFailed,
}

impl fmt::Display for JournalError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::StateDirectoryUnavailable => "controller state directory is unavailable",
            Self::StateDirectoryUntrusted => "controller state directory failed trust checks",
            Self::JournalUntrusted => "controller uncertainty journal failed trust checks",
            Self::JournalInvalid => "controller uncertainty journal is invalid",
            Self::JournalWriteFailed => "controller uncertainty journal could not be committed",
        };
        formatter.write_str(message)
    }
}

impl std::error::Error for JournalError {}

pub(crate) trait UncertaintyJournal {
    fn is_pending(&self) -> bool;
    fn mark_pending(&mut self, action: JournalAction) -> Result<(), JournalError>;
    fn clear(&mut self) -> Result<(), JournalError>;
}

#[derive(Debug, Clone, Copy)]
#[cfg(test)]
pub(crate) struct MemoryJournal {
    state: JournalState,
}

#[cfg(test)]
impl MemoryJournal {
    #[cfg(test)]
    pub(crate) const fn clean() -> Self {
        Self {
            state: JournalState::Clean,
        }
    }

    #[cfg(test)]
    pub(crate) const fn pending(action: JournalAction) -> Self {
        Self {
            state: JournalState::Pending(action),
        }
    }
}

#[cfg(test)]
impl UncertaintyJournal for MemoryJournal {
    fn is_pending(&self) -> bool {
        matches!(self.state, JournalState::Pending(_))
    }

    fn mark_pending(&mut self, action: JournalAction) -> Result<(), JournalError> {
        self.state = JournalState::Pending(action);
        Ok(())
    }

    fn clear(&mut self) -> Result<(), JournalError> {
        self.state = JournalState::Clean;
        Ok(())
    }
}

/// Owner-only journal used by the real service.
pub(crate) struct FileUncertaintyJournal {
    directory: File,
    state: JournalState,
    #[cfg(test)]
    directory_sync_calls: Cell<usize>,
    #[cfg(test)]
    fail_directory_sync_on: Cell<Option<usize>>,
}

impl FileUncertaintyJournal {
    pub(crate) fn open_default() -> Result<Self, JournalError> {
        let root = default_state_root()?;
        Self::open_under(&root)
    }

    #[cfg_attr(test, allow(dead_code))]
    pub(crate) fn open_under(root: &Path) -> Result<Self, JournalError> {
        if !root.is_absolute() {
            return Err(JournalError::StateDirectoryUntrusted);
        }
        let mut root_builder = fs::DirBuilder::new();
        root_builder.mode(0o700).recursive(true);
        root_builder
            .create(root)
            .map_err(|_| JournalError::StateDirectoryUnavailable)?;
        let directory = root.join(APPLICATION_DIRECTORY);
        match fs::symlink_metadata(&directory) {
            Ok(metadata) => validate_directory_metadata(&metadata)?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let mut builder = fs::DirBuilder::new();
                builder.mode(0o700);
                builder
                    .create(&directory)
                    .map_err(|_| JournalError::StateDirectoryUnavailable)?;
                validate_directory_metadata(
                    &fs::symlink_metadata(&directory)
                        .map_err(|_| JournalError::StateDirectoryUnavailable)?,
                )?;
            }
            Err(_) => return Err(JournalError::StateDirectoryUnavailable),
        }
        let directory = open_directory(&directory)?;
        validate_directory_metadata(
            &directory
                .metadata()
                .map_err(|_| JournalError::StateDirectoryUntrusted)?,
        )?;

        let snapshot = read_state(&directory, JOURNAL_FILE)?;
        let marker = read_state(&directory, PENDING_FILE)?;
        let state = match marker {
            Some(JournalState::Pending(action)) => JournalState::Pending(action),
            Some(JournalState::Clean) => return Err(JournalError::JournalInvalid),
            None => snapshot.unwrap_or(JournalState::Clean),
        };
        let journal = Self {
            directory,
            state,
            #[cfg(test)]
            directory_sync_calls: Cell::new(0),
            #[cfg(test)]
            fail_directory_sync_on: Cell::new(None),
        };

        // Migrate a journal written by the original one-file implementation
        // before returning a controller that may perform recovery. The marker
        // is the fail-closed authority during every later pending -> clean
        // transition.
        if marker.is_none() && matches!(snapshot, Some(JournalState::Pending(_))) {
            journal.commit(PENDING_FILE, state)?;
        }
        Ok(journal)
    }

    fn commit(&self, target: &str, state: JournalState) -> Result<(), JournalError> {
        let payload = encode_state(state);
        let mut random = [0_u8; 8];
        rand_bytes(&mut random).map_err(|_| JournalError::JournalWriteFailed)?;
        let mut suffix = String::with_capacity(16);
        for byte in random {
            use std::fmt::Write as _;
            write!(&mut suffix, "{byte:02x}").map_err(|_| JournalError::JournalWriteFailed)?;
        }
        let temporary = CString::new(format!(".uncertainty.{suffix}.tmp"))
            .map_err(|_| JournalError::JournalWriteFailed)?;
        let target = CString::new(target).map_err(|_| JournalError::JournalWriteFailed)?;
        let mut file = open_entry(
            &self.directory,
            &temporary,
            libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_CLOEXEC,
            0o600,
        )
        .map_err(|_| JournalError::JournalWriteFailed)?;
        validate_journal_metadata(
            &file
                .metadata()
                .map_err(|_| JournalError::JournalWriteFailed)?,
            false,
        )
        .map_err(|_| JournalError::JournalWriteFailed)?;
        let commit_result = (|| {
            file.write_all(payload.as_bytes())
                .map_err(|_| JournalError::JournalWriteFailed)?;
            file.sync_all()
                .map_err(|_| JournalError::JournalWriteFailed)?;
            drop(file);
            rename_entry(&self.directory, &temporary, &target)?;
            self.sync_directory()
        })();
        if commit_result.is_err() {
            let _ = unlink_entry(&self.directory, &temporary);
        }
        commit_result
    }

    fn sync_directory(&self) -> Result<(), JournalError> {
        #[cfg(test)]
        {
            let call = self.directory_sync_calls.get().saturating_add(1);
            self.directory_sync_calls.set(call);
            if self.fail_directory_sync_on.get() == Some(call) {
                self.fail_directory_sync_on.set(None);
                return Err(JournalError::JournalWriteFailed);
            }
        }
        self.directory
            .sync_all()
            .map_err(|_| JournalError::JournalWriteFailed)
    }

    #[cfg(test)]
    pub(crate) fn fail_directory_sync_on_nth_for_test(&self, call: usize) {
        assert!(call > 0, "fault call is one-indexed");
        self.directory_sync_calls.set(0);
        self.fail_directory_sync_on.set(Some(call));
    }
}

impl fmt::Debug for FileUncertaintyJournal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("FileUncertaintyJournal")
            .field("state", &self.state)
            .field("directory", &"redacted")
            .finish()
    }
}

impl UncertaintyJournal for FileUncertaintyJournal {
    fn is_pending(&self) -> bool {
        matches!(self.state, JournalState::Pending(_))
    }

    fn mark_pending(&mut self, action: JournalAction) -> Result<(), JournalError> {
        let state = JournalState::Pending(action);
        // Latch memory first. Even a partially published marker must stop all
        // later writes in this process, while a pre-publication failure is a
        // conservative false pending only (the backend was not called).
        self.state = state;
        self.commit(PENDING_FILE, state)?;
        // Retain the original human-readable snapshot for compatibility. The
        // separate marker, not this replaceable snapshot, is authoritative.
        self.commit(JOURNAL_FILE, state)?;
        Ok(())
    }

    fn clear(&mut self) -> Result<(), JournalError> {
        // Publish and durably sync the clean snapshot while the independent
        // pending marker still exists. Thus every failure through the final
        // directory fsync reopens as pending even if the clean rename happened.
        self.commit(JOURNAL_FILE, JournalState::Clean)?;
        let marker = CString::new(PENDING_FILE).map_err(|_| JournalError::JournalWriteFailed)?;
        match unlink_entry(&self.directory, &marker) {
            Ok(()) => {}
            Err(error) if error.raw_os_error() == Some(libc::ENOENT) => {}
            Err(_) => return Err(JournalError::JournalWriteFailed),
        }
        // Deliberately do not fsync after unlink. The unlink is the final
        // publication step and has no later fallible operation. A host crash
        // may conservatively resurrect the already-durable marker, but can
        // never turn a reported failure into a clean restart.
        self.state = JournalState::Clean;
        Ok(())
    }
}

fn default_state_root() -> Result<PathBuf, JournalError> {
    if let Some(root) = env::var_os("XDG_STATE_HOME").filter(|value| !value.is_empty()) {
        let root = PathBuf::from(root);
        return root
            .is_absolute()
            .then_some(root)
            .ok_or(JournalError::StateDirectoryUntrusted);
    }
    env::var_os("HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .filter(|root| root.is_absolute())
        .map(|root| root.join(".local").join("state"))
        .ok_or(JournalError::StateDirectoryUnavailable)
}

fn validate_directory_metadata(metadata: &fs::Metadata) -> Result<(), JournalError> {
    if !metadata.file_type().is_dir()
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.mode() & 0o777 != 0o700
    {
        return Err(JournalError::StateDirectoryUntrusted);
    }
    Ok(())
}

fn read_state(directory: &File, name: &str) -> Result<Option<JournalState>, JournalError> {
    let name = CString::new(name).map_err(|_| JournalError::JournalUntrusted)?;
    let file = match open_entry(
        directory,
        &name,
        libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NONBLOCK,
        0,
    ) {
        Ok(file) => file,
        Err(error) if error.raw_os_error() == Some(libc::ENOENT) => return Ok(None),
        Err(_) => return Err(JournalError::JournalUntrusted),
    };
    let opened = file
        .metadata()
        .map_err(|_| JournalError::JournalUntrusted)?;
    validate_journal_metadata(&opened, true)?;
    let mut payload = String::new();
    file.take(MAX_JOURNAL_BYTES + 1)
        .read_to_string(&mut payload)
        .map_err(|_| JournalError::JournalInvalid)?;
    if payload.len() as u64 > MAX_JOURNAL_BYTES {
        return Err(JournalError::JournalInvalid);
    }
    parse_state(&payload)
        .map(Some)
        .ok_or(JournalError::JournalInvalid)
}

fn validate_journal_metadata(
    metadata: &fs::Metadata,
    enforce_size: bool,
) -> Result<(), JournalError> {
    if !metadata.file_type().is_file()
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.mode() & 0o777 != 0o600
        || metadata.nlink() != 1
        || (enforce_size && metadata.len() > MAX_JOURNAL_BYTES)
    {
        return Err(JournalError::JournalUntrusted);
    }
    Ok(())
}

fn encode_state(state: JournalState) -> String {
    match state {
        JournalState::Clean => "version=1\nstate=clean\n".to_string(),
        JournalState::Pending(action) => {
            format!("version=1\nstate=pending\naction={}\n", action.label())
        }
    }
}

fn parse_state(payload: &str) -> Option<JournalState> {
    if payload == "version=1\nstate=clean\n" {
        return Some(JournalState::Clean);
    }
    let action = payload
        .strip_prefix("version=1\nstate=pending\naction=")?
        .strip_suffix('\n')?;
    JournalAction::parse(action).map(JournalState::Pending)
}

fn open_directory(path: &Path) -> Result<File, JournalError> {
    let path = CString::new(path.as_os_str().as_bytes())
        .map_err(|_| JournalError::StateDirectoryUntrusted)?;
    let mut how: libc::open_how = unsafe { std::mem::zeroed() };
    how.flags = (libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW) as u64;
    how.resolve = libc::RESOLVE_NO_SYMLINKS;
    // SAFETY: `path` and `how` are initialized kernel ABI values. A successful
    // descriptor is uniquely transferred into File below.
    let descriptor = unsafe {
        libc::syscall(
            libc::SYS_openat2,
            libc::AT_FDCWD,
            path.as_ptr(),
            &how,
            std::mem::size_of::<libc::open_how>(),
        )
    };
    if descriptor < 0 {
        return Err(JournalError::StateDirectoryUntrusted);
    }
    // SAFETY: openat2 returned a fresh owned descriptor.
    Ok(unsafe { File::from_raw_fd(descriptor as libc::c_int) })
}

fn open_entry(
    directory: &File,
    name: &CStr,
    flags: libc::c_int,
    mode: u32,
) -> std::io::Result<File> {
    let mut how: libc::open_how = unsafe { std::mem::zeroed() };
    how.flags = flags as u64;
    how.mode = mode as u64;
    how.resolve = libc::RESOLVE_BENEATH | libc::RESOLVE_NO_SYMLINKS;
    // SAFETY: the directory descriptor and C string are live for the call;
    // `how` uses the kernel ABI and a successful descriptor is uniquely owned.
    let descriptor = unsafe {
        libc::syscall(
            libc::SYS_openat2,
            directory.as_raw_fd(),
            name.as_ptr(),
            &how,
            std::mem::size_of::<libc::open_how>(),
        )
    };
    if descriptor < 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: openat2 returned a fresh owned descriptor.
    Ok(unsafe { File::from_raw_fd(descriptor as libc::c_int) })
}

fn rename_entry(directory: &File, source: &CStr, target: &CStr) -> Result<(), JournalError> {
    // SAFETY: both names are NUL-terminated and resolved relative to the live,
    // owner-only directory descriptor.
    let result = unsafe {
        libc::renameat(
            directory.as_raw_fd(),
            source.as_ptr(),
            directory.as_raw_fd(),
            target.as_ptr(),
        )
    };
    if result == 0 {
        Ok(())
    } else {
        Err(JournalError::JournalWriteFailed)
    }
}

fn unlink_entry(directory: &File, name: &CStr) -> std::io::Result<()> {
    // SAFETY: `name` is NUL-terminated and relative to the live directory fd.
    let result = unsafe { libc::unlinkat(directory.as_raw_fd(), name.as_ptr(), 0) };
    if result == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::{symlink, PermissionsExt};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    fn temporary_root() -> PathBuf {
        let suffix = COUNTER.fetch_add(1, Ordering::Relaxed);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let root = env::temp_dir().join(format!(
            "jbl-aura-journal-{}-{nanos}-{suffix}",
            std::process::id()
        ));
        fs::create_dir(&root).expect("test root");
        root
    }

    #[test]
    fn pending_survives_reopen_and_clean_is_durably_replaced() {
        let root = temporary_root();
        let mut journal = FileUncertaintyJournal::open_under(&root).expect("open clean");
        assert!(!journal.is_pending());
        journal
            .mark_pending(JournalAction::Start)
            .expect("mark pending");
        drop(journal);
        let mut reopened = FileUncertaintyJournal::open_under(&root).expect("reopen pending");
        assert!(reopened.is_pending());
        reopened.clear().expect("clear");
        drop(reopened);
        assert!(!FileUncertaintyJournal::open_under(&root)
            .expect("reopen clean")
            .is_pending());
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn clean_snapshot_directory_sync_failure_reopens_pending() {
        let root = temporary_root();
        let mut journal = FileUncertaintyJournal::open_under(&root).expect("open clean");
        journal
            .mark_pending(JournalAction::Start)
            .expect("mark pending");

        // The injected failure happens after the clean snapshot rename. The
        // independent marker must still dominate both the live instance and a
        // newly opened process.
        journal.fail_directory_sync_on_nth_for_test(1);
        assert_eq!(journal.clear(), Err(JournalError::JournalWriteFailed));
        assert!(journal.is_pending());
        assert_eq!(
            read_state(&journal.directory, JOURNAL_FILE).expect("read snapshot"),
            Some(JournalState::Clean)
        );
        assert_eq!(
            read_state(&journal.directory, PENDING_FILE).expect("read marker"),
            Some(JournalState::Pending(JournalAction::Start))
        );
        drop(journal);

        let reopened = FileUncertaintyJournal::open_under(&root).expect("reopen pending");
        assert!(reopened.is_pending());
        drop(reopened);
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn one_file_pending_is_migrated_before_recovery_can_open() {
        let root = temporary_root();
        let directory = root.join(APPLICATION_DIRECTORY);
        fs::create_dir(&directory).expect("state dir");
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700)).expect("mode");
        let snapshot = directory.join(JOURNAL_FILE);
        fs::write(
            &snapshot,
            encode_state(JournalState::Pending(JournalAction::Stop)),
        )
        .expect("legacy snapshot");
        fs::set_permissions(&snapshot, fs::Permissions::from_mode(0o600)).expect("snapshot mode");

        let mut journal = FileUncertaintyJournal::open_under(&root).expect("migrate pending");
        assert!(journal.is_pending());
        assert_eq!(
            read_state(&journal.directory, PENDING_FILE).expect("read migrated marker"),
            Some(JournalState::Pending(JournalAction::Stop))
        );
        journal.clear().expect("clear migrated journal");
        drop(journal);
        assert!(!FileUncertaintyJournal::open_under(&root)
            .expect("reopen clean")
            .is_pending());
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn corrupt_broad_or_symlinked_journal_fails_closed() {
        for (entry, fixture) in [
            (JOURNAL_FILE, "corrupt"),
            (JOURNAL_FILE, "broad"),
            (JOURNAL_FILE, "symlink"),
            (JOURNAL_FILE, "hardlink"),
            (PENDING_FILE, "corrupt"),
            (PENDING_FILE, "broad"),
            (PENDING_FILE, "symlink"),
            (PENDING_FILE, "hardlink"),
        ] {
            let root = temporary_root();
            let directory = root.join(APPLICATION_DIRECTORY);
            fs::create_dir(&directory).expect("state dir");
            fs::set_permissions(&directory, fs::Permissions::from_mode(0o700)).expect("mode");
            let journal = directory.join(entry);
            match fixture {
                "corrupt" => fs::write(&journal, b"unknown\n").expect("fixture"),
                "broad" => {
                    fs::write(&journal, b"version=1\nstate=clean\n").expect("fixture");
                    fs::set_permissions(&journal, fs::Permissions::from_mode(0o644)).expect("mode");
                }
                "symlink" => symlink("target", &journal).expect("symlink"),
                "hardlink" => {
                    let target = root.join("hardlink-target");
                    fs::write(&target, b"version=1\nstate=clean\n").expect("fixture");
                    fs::set_permissions(&target, fs::Permissions::from_mode(0o600)).expect("mode");
                    fs::hard_link(target, &journal).expect("hardlink");
                }
                _ => unreachable!(),
            }
            assert!(FileUncertaintyJournal::open_under(&root).is_err());
            fs::remove_dir_all(root).expect("cleanup");
        }
    }

    #[test]
    fn parent_symlink_is_rejected_by_openat2_boundary() {
        let physical = temporary_root();
        let state_directory = physical.join(APPLICATION_DIRECTORY);
        fs::create_dir(&state_directory).expect("state dir");
        fs::set_permissions(&state_directory, fs::Permissions::from_mode(0o700)).expect("mode");
        let alias = physical.with_extension("alias");
        symlink(&physical, &alias).expect("parent symlink");
        assert_eq!(
            FileUncertaintyJournal::open_under(&alias).unwrap_err(),
            JournalError::StateDirectoryUntrusted
        );
        fs::remove_file(alias).expect("remove alias");
        fs::remove_dir_all(physical).expect("cleanup");
    }

    #[test]
    fn fixed_format_contains_no_identity_or_free_form_diagnostic() {
        for action in [
            JournalAction::Start,
            JournalAction::Stop,
            JournalAction::Shutdown,
            JournalAction::RecoverStop,
        ] {
            let payload = encode_state(JournalState::Pending(action));
            assert!(payload.len() <= MAX_JOURNAL_BYTES as usize);
            assert_eq!(parse_state(&payload), Some(JournalState::Pending(action)));
            assert!(!payload.contains(':'));
        }
    }
}
