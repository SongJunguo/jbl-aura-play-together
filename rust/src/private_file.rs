use std::fs::File;
#[cfg(not(target_os = "linux"))]
use std::fs::OpenOptions;
use std::io::{self, Read};
use std::path::Path;

use crate::error::JblError;

#[derive(Debug, Clone, Copy)]
pub(crate) enum PrivateFileKind {
    Config,
    Certificate,
    PrivateKey,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OpenPrivateError {
    Missing,
    PermissionDenied,
    SymlinkRejected,
    Unsupported,
    Other,
}

fn unavailable(kind: PrivateFileKind) -> JblError {
    match kind {
        PrivateFileKind::Config => JblError::ConfigUnavailable,
        PrivateFileKind::Certificate => JblError::CertificateUnavailable,
        PrivateFileKind::PrivateKey => JblError::PrivateKeyUnavailable,
    }
}

fn permissions(kind: PrivateFileKind) -> JblError {
    match kind {
        PrivateFileKind::Config => JblError::ConfigPermissions,
        PrivateFileKind::Certificate => JblError::CertificatePermissions,
        PrivateFileKind::PrivateKey => JblError::PrivateKeyPermissions,
    }
}

fn invalid(kind: PrivateFileKind) -> JblError {
    match kind {
        PrivateFileKind::Config => JblError::InvalidConfig,
        PrivateFileKind::Certificate | PrivateFileKind::PrivateKey => {
            JblError::CredentialFileInvalid
        }
    }
}

fn too_large(kind: PrivateFileKind) -> JblError {
    match kind {
        PrivateFileKind::Config => JblError::ConfigTooLarge,
        PrivateFileKind::Certificate | PrivateFileKind::PrivateKey => JblError::CredentialTooLarge,
    }
}

#[cfg(target_os = "linux")]
fn classify_linux_open_error(error: &io::Error) -> OpenPrivateError {
    match error.raw_os_error() {
        Some(libc::ENOENT) => OpenPrivateError::Missing,
        Some(libc::EACCES) => OpenPrivateError::PermissionDenied,
        Some(libc::ELOOP) => OpenPrivateError::SymlinkRejected,
        Some(libc::ENOSYS) | Some(libc::EINVAL) | Some(libc::E2BIG) | Some(libc::EPERM) => {
            OpenPrivateError::Unsupported
        }
        _ => OpenPrivateError::Other,
    }
}

#[cfg(all(unix, not(target_os = "linux")))]
fn classify_open_error(error: &io::Error) -> OpenPrivateError {
    if error.kind() == io::ErrorKind::NotFound {
        OpenPrivateError::Missing
    } else if error.kind() == io::ErrorKind::PermissionDenied {
        OpenPrivateError::PermissionDenied
    } else if error.raw_os_error() == Some(libc::ELOOP) {
        OpenPrivateError::SymlinkRejected
    } else {
        OpenPrivateError::Other
    }
}

#[cfg(not(unix))]
fn classify_open_error(error: &io::Error) -> OpenPrivateError {
    if error.kind() == io::ErrorKind::NotFound {
        OpenPrivateError::Missing
    } else if error.kind() == io::ErrorKind::PermissionDenied {
        OpenPrivateError::PermissionDenied
    } else {
        OpenPrivateError::Other
    }
}

#[cfg(target_os = "linux")]
fn open_private(path: &Path) -> Result<File, OpenPrivateError> {
    use std::ffi::CString;
    use std::os::fd::FromRawFd;
    use std::os::unix::ffi::OsStrExt;

    let path = CString::new(path.as_os_str().as_bytes()).map_err(|_| OpenPrivateError::Other)?;
    // SAFETY: open_how is a plain kernel ABI value; zero is valid for every
    // field and provides forward-compatible zeroed trailing fields.
    let mut how: libc::open_how = unsafe { std::mem::zeroed() };
    // O_NONBLOCK prevents a malicious FIFO or device node from blocking before
    // fstat can reject it. Linux ignores this flag for regular files.
    how.flags = (libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NONBLOCK) as u64;
    how.resolve = libc::RESOLVE_NO_SYMLINKS;
    // SAFETY: path is a live NUL-terminated string, how points to an initialized
    // open_how with the kernel ABI size, and a successful descriptor is uniquely
    // transferred into File below.
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
        return Err(classify_linux_open_error(&io::Error::last_os_error()));
    }
    // SAFETY: openat2 returned a new owned descriptor and no other owner exists.
    Ok(unsafe { File::from_raw_fd(descriptor as libc::c_int) })
}

#[cfg(all(unix, not(target_os = "linux")))]
fn open_private(path: &Path) -> Result<File, OpenPrivateError> {
    use std::os::unix::fs::OpenOptionsExt;

    OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .map_err(|error| classify_open_error(&error))
}

#[cfg(not(unix))]
fn open_private(path: &Path) -> Result<File, OpenPrivateError> {
    OpenOptions::new()
        .read(true)
        .open(path)
        .map_err(|error| classify_open_error(&error))
}

#[cfg(unix)]
fn validate_metadata(file: &File, kind: PrivateFileKind, max: u64) -> Result<(), JblError> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    let metadata = file.metadata().map_err(|_| invalid(kind))?;
    if !metadata.file_type().is_file() {
        return Err(invalid(kind));
    }
    if metadata.len() > max {
        return Err(too_large(kind));
    }
    // SAFETY: geteuid takes no arguments and has no memory-safety preconditions.
    let effective_uid = unsafe { libc::geteuid() };
    if metadata.uid() != effective_uid {
        return Err(permissions(kind));
    }
    if metadata.permissions().mode() & 0o077 != 0 {
        return Err(permissions(kind));
    }
    Ok(())
}

#[cfg(not(unix))]
fn validate_metadata(file: &File, kind: PrivateFileKind, max: u64) -> Result<(), JblError> {
    let metadata = file.metadata().map_err(|_| invalid(kind))?;
    if !metadata.file_type().is_file() {
        return Err(invalid(kind));
    }
    if metadata.len() > max {
        return Err(too_large(kind));
    }
    Ok(())
}

pub(crate) fn read_private_file(
    path: &Path,
    max: u64,
    kind: PrivateFileKind,
) -> Result<Vec<u8>, JblError> {
    read_private_file_if_present(path, max, kind)?.ok_or_else(|| unavailable(kind))
}

pub(crate) fn read_private_file_if_present(
    path: &Path,
    max: u64,
    kind: PrivateFileKind,
) -> Result<Option<Vec<u8>>, JblError> {
    let file = match open_private(path) {
        Ok(file) => file,
        Err(OpenPrivateError::Missing) => return Ok(None),
        Err(OpenPrivateError::PermissionDenied) => return Err(permissions(kind)),
        Err(OpenPrivateError::SymlinkRejected) => return Err(invalid(kind)),
        Err(OpenPrivateError::Unsupported | OpenPrivateError::Other) => {
            return Err(unavailable(kind));
        }
    };
    validate_metadata(&file, kind, max)?;
    let read_limit = max.checked_add(1).ok_or_else(|| too_large(kind))?;
    let mut payload = Vec::new();
    file.take(read_limit)
        .read_to_end(&mut payload)
        .map_err(|_| invalid(kind))?;
    if payload.len() as u64 > max {
        return Err(too_large(kind));
    }
    Ok(Some(payload))
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    #[cfg(target_os = "linux")]
    use std::ffi::CString;
    use std::fs;
    #[cfg(target_os = "linux")]
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::{symlink, PermissionsExt};
    #[cfg(target_os = "linux")]
    use std::process::Command;
    #[cfg(target_os = "linux")]
    use std::thread;
    #[cfg(target_os = "linux")]
    use std::time::{Duration, Instant};
    use std::time::{SystemTime, UNIX_EPOCH};

    #[cfg(target_os = "linux")]
    const FIFO_CHILD_PATH: &str = "JBL_AURA_PRIVATE_FILE_FIFO_CHILD_PATH";
    #[cfg(target_os = "linux")]
    const FIFO_CHILD_MARKER: &str = "JBL_AURA_PRIVATE_FILE_FIFO_CHILD_MARKER";

    fn temporary_directory() -> std::path::PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock should follow Unix epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "jbl-aura-private-file-{}-{unique}",
            std::process::id()
        ));
        fs::create_dir(&path).expect("temporary directory should be created");
        path
    }

    #[test]
    fn accepts_owner_only_regular_file_and_rejects_broad_mode() {
        let directory = temporary_directory();
        let path = directory.join("private.env");
        fs::write(&path, b"safe").expect("fixture should be written");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))
            .expect("fixture mode should be set");
        assert_eq!(
            read_private_file(&path, 16, PrivateFileKind::Config)
                .expect("owner-only file should load"),
            b"safe"
        );
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644))
            .expect("fixture mode should be broadened");
        assert_eq!(
            read_private_file(&path, 16, PrivateFileKind::Config).unwrap_err(),
            JblError::ConfigPermissions
        );
        fs::remove_dir_all(directory).expect("fixture directory should be removed");
    }

    #[test]
    fn rejects_symlink_and_oversized_file() {
        let directory = temporary_directory();
        let target = directory.join("target.env");
        let link = directory.join("link.env");
        fs::write(&target, b"0123456789").expect("fixture should be written");
        fs::set_permissions(&target, fs::Permissions::from_mode(0o600))
            .expect("fixture mode should be set");
        symlink(&target, &link).expect("fixture symlink should be created");
        assert_eq!(
            read_private_file(&link, 32, PrivateFileKind::Config).unwrap_err(),
            JblError::InvalidConfig
        );
        assert_eq!(
            read_private_file(&target, 4, PrivateFileKind::Config).unwrap_err(),
            JblError::ConfigTooLarge
        );
        fs::remove_dir_all(directory).expect("fixture directory should be removed");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn rejects_symlink_in_parent_path() {
        let directory = temporary_directory();
        let real_parent = directory.join("real");
        let linked_parent = directory.join("linked");
        fs::create_dir(&real_parent).expect("real parent should be created");
        let target = real_parent.join("private.env");
        fs::write(&target, b"safe").expect("fixture should be written");
        fs::set_permissions(&target, fs::Permissions::from_mode(0o600))
            .expect("fixture mode should be set");
        symlink(&real_parent, &linked_parent).expect("parent symlink should be created");

        assert_eq!(
            read_private_file(
                &linked_parent.join("private.env"),
                16,
                PrivateFileKind::Config
            )
            .unwrap_err(),
            JblError::InvalidConfig
        );
        fs::remove_dir_all(directory).expect("fixture directory should be removed");
    }

    #[test]
    fn rejects_directory_before_applying_size_classification() {
        let directory = temporary_directory();
        let path = directory.join("not-a-file");
        fs::create_dir(&path).expect("fixture directory should be created");
        assert_eq!(
            read_private_file(&path, 0, PrivateFileKind::Config).unwrap_err(),
            JblError::InvalidConfig
        );
        fs::remove_dir_all(directory).expect("fixture directory should be removed");
    }

    #[test]
    fn accepts_exact_size_limit_and_rejects_one_byte_over() {
        let directory = temporary_directory();
        let path = directory.join("boundary.env");
        fs::write(&path, b"safe").expect("fixture should be written");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))
            .expect("fixture mode should be set");
        assert_eq!(
            read_private_file(&path, 4, PrivateFileKind::Config)
                .expect("exact-size fixture should load"),
            b"safe"
        );
        assert_eq!(
            read_private_file(&path, 3, PrivateFileKind::Config).unwrap_err(),
            JblError::ConfigTooLarge
        );
        fs::remove_dir_all(directory).expect("fixture directory should be removed");
    }

    #[cfg(target_os = "linux")]
    fn create_fifo(path: &Path) {
        let path = CString::new(path.as_os_str().as_bytes()).expect("fixture path has no NUL");
        // SAFETY: path is NUL-terminated and points to a live pathname for the
        // duration of mkfifo.
        let result = unsafe { libc::mkfifo(path.as_ptr(), 0o600) };
        assert_eq!(result, 0, "FIFO fixture should be created");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn fifo_child_entrypoint() {
        let Some(path) = std::env::var_os(FIFO_CHILD_PATH) else {
            return;
        };
        let marker = std::env::var_os(FIFO_CHILD_MARKER)
            .expect("FIFO child marker should accompany the fixture path");
        assert_eq!(
            read_private_file(Path::new(&path), 16, PrivateFileKind::Config).unwrap_err(),
            JblError::InvalidConfig
        );
        fs::write(marker, b"rejected").expect("FIFO child should record completion");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn rejects_fifo_without_waiting_for_a_writer() {
        let directory = temporary_directory();
        let path = directory.join("private.fifo");
        let marker = directory.join("child-complete");
        create_fifo(&path);

        let mut child = Command::new(std::env::current_exe().expect("test executable exists"))
            .arg("--exact")
            .arg("private_file::tests::fifo_child_entrypoint")
            .env(FIFO_CHILD_PATH, &path)
            .env(FIFO_CHILD_MARKER, &marker)
            .spawn()
            .expect("FIFO helper should start");
        let deadline = Instant::now() + Duration::from_secs(2);
        let status = loop {
            if let Some(status) = child.try_wait().expect("FIFO helper should be observable") {
                break status;
            }
            if Instant::now() >= deadline {
                child.kill().expect("blocked FIFO helper should be killed");
                child.wait().expect("killed FIFO helper should be reaped");
                fs::remove_dir_all(&directory).expect("fixture directory should be removed");
                panic!("private FIFO open blocked before type validation");
            }
            thread::sleep(Duration::from_millis(10));
        };
        let marker_contents = fs::read(&marker).ok();
        fs::remove_dir_all(directory).expect("fixture directory should be removed");
        assert!(status.success(), "FIFO helper should reject the fixture");
        assert_eq!(
            marker_contents.as_deref(),
            Some(b"rejected".as_slice()),
            "the exact FIFO helper test must have executed"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn classifies_linux_open_errors_without_fallback() {
        for (errno, expected) in [
            (libc::ENOENT, OpenPrivateError::Missing),
            (libc::EACCES, OpenPrivateError::PermissionDenied),
            (libc::ELOOP, OpenPrivateError::SymlinkRejected),
            (libc::ENOSYS, OpenPrivateError::Unsupported),
            (libc::EINVAL, OpenPrivateError::Unsupported),
            (libc::EIO, OpenPrivateError::Other),
        ] {
            assert_eq!(
                classify_linux_open_error(&io::Error::from_raw_os_error(errno)),
                expected
            );
        }
    }

    #[test]
    fn credential_kinds_preserve_permission_and_size_errors() {
        let directory = temporary_directory();
        let path = directory.join("credential.pem");
        fs::write(&path, b"0123456789").expect("fixture should be written");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644))
            .expect("broad fixture mode should be set");

        assert_eq!(
            read_private_file(&path, 32, PrivateFileKind::Certificate).unwrap_err(),
            JblError::CertificatePermissions
        );
        assert_eq!(
            read_private_file(&path, 32, PrivateFileKind::PrivateKey).unwrap_err(),
            JblError::PrivateKeyPermissions
        );

        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))
            .expect("owner-only fixture mode should be set");
        for kind in [PrivateFileKind::Certificate, PrivateFileKind::PrivateKey] {
            assert_eq!(
                read_private_file(&path, 4, kind).unwrap_err(),
                JblError::CredentialTooLarge
            );
        }
        fs::remove_dir_all(directory).expect("fixture directory should be removed");
    }
}
