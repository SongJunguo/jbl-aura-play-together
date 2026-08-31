//! Linux process boundary for the single-owner Rust controller service.

use std::cell::Cell;
use std::env;
use std::ffi::{CString, OsStr};
use std::fmt;
use std::fs::File;
use std::io;
use std::marker::PhantomData;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::ffi::OsStrExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicPtr, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::aura_bluez::AuraBluezConfig;
use crate::backend::PairBackend;
use crate::backend_native::{JblPairConfigurationProbe, NativePairBackend};
use crate::capability::authentics_300_capabilities;
use crate::client::JblLanClient;
use crate::config::RuntimeConfig;
use crate::controller::{
    ControllerActionOutcome, ControllerActionResult, ExternalPrepareFailure,
    PairConfigurationProbe, PairController,
};
use crate::eq::EqPresetWriteResult;
use crate::error::JblError;
use crate::inspection::InspectionReadError;
use crate::journal::{FileUncertaintyJournal, UncertaintyJournal};
use crate::local_client::{LocalClientError, LocalServiceClient};
use crate::media::{AudioSourceWriteResult, MuteWriteResult, VolumeWriteResult};
use crate::web::{RevisionConflict, WebActor, WebMutation};
use crate::web_device::{
    DirectActionOutcome, DirectActionResult, DirectFailure, DirectMutation, DirectObservation,
    DirectSnapshot,
};

// v0.4 and Rust share this owner-only lock namespace. Rust holds both files
// for its complete service lifetime: `operation.lock` excludes every public
// v0.4 entry before device I/O, while `session.lock` excludes the persistent
// v0.4 Python supervisor (including a supervisor launched directly).
const RUNTIME_DIRECTORY: &[u8] = b"jbl-aura-link\0";
const OPERATION_LOCK_FILE: &[u8] = b"operation.lock\0";
const SESSION_LOCK_FILE: &[u8] = b"session.lock\0";
const V04_LAUNCH_RESERVATION: &[u8] = b"launch.reservation\0";
const USER_UNIT: &str = "jbl-aura-link-rust.service";
const SYSTEMCTL_TIMEOUT: Duration = Duration::from_secs(5);
const SERVICE_READY_TIMEOUT: Duration = Duration::from_secs(15);
const READY_POLL_INTERVAL: Duration = Duration::from_millis(100);
const WEB_DIRECT_REQUEST_TIMEOUT: Duration = Duration::from_millis(500);
const WEB_DIRECT_CACHE_TTL: Duration = Duration::from_secs(2);
const WEB_DIRECT_RETRY_DELAY: Duration = Duration::from_millis(100);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceRuntimeError {
    RuntimeDirectoryUnavailable,
    RuntimeDirectoryUntrusted,
    AlreadyRunning,
    SignalHandlerUnavailable,
    ServiceManagerUnavailable,
    ServiceStartFailed,
    ServiceReadyTimedOut,
    LocalServiceInvalid,
    ControllerInitializationFailed,
    GracefulShutdownRejected,
    GracefulShutdownOutcomeUnknown,
    GracefulShutdownPostconditionFailed,
    TransportTeardownFailed,
}

impl fmt::Display for ServiceRuntimeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::RuntimeDirectoryUnavailable => {
                "the private user runtime directory is unavailable"
            }
            Self::RuntimeDirectoryUntrusted => {
                "the private user runtime directory failed trust checks"
            }
            Self::AlreadyRunning => {
                "another v0.4 or Rust Play Together controller already owns the devices"
            }
            Self::SignalHandlerUnavailable => "termination handlers could not be installed",
            Self::ServiceManagerUnavailable => "the user service manager is unavailable",
            Self::ServiceStartFailed => "the Rust Play Together user service could not be started",
            Self::ServiceReadyTimedOut => {
                "the Rust Play Together user service did not become ready in time"
            }
            Self::LocalServiceInvalid => {
                "the local Rust Play Together service failed its readiness check"
            }
            Self::ControllerInitializationFailed => {
                "the native Play Together controller could not be initialized"
            }
            Self::GracefulShutdownRejected => {
                "graceful Play Together shutdown was rejected before completion"
            }
            Self::GracefulShutdownOutcomeUnknown => {
                "graceful Play Together shutdown outcome is unknown"
            }
            Self::GracefulShutdownPostconditionFailed => {
                "graceful Play Together shutdown postcondition failed"
            }
            Self::TransportTeardownFailed => "Play Together transport teardown failed",
        })
    }
}

impl std::error::Error for ServiceRuntimeError {}

pub(crate) trait DirectDeviceSurface {
    fn snapshot(&mut self) -> Result<DirectSnapshot, DirectFailure>;
    fn mutate(
        &mut self,
        lock: &mut DirectControlLock,
        mutation: DirectMutation,
    ) -> DirectActionResult;
}

struct JblDirectSurface {
    client: JblLanClient,
    expected_model: String,
    cached_snapshot: Option<(Instant, DirectSnapshot)>,
}

impl DirectDeviceSurface for JblDirectSurface {
    fn snapshot(&mut self) -> Result<DirectSnapshot, DirectFailure> {
        if let Some((observed_at, snapshot)) = &self.cached_snapshot {
            if observed_at.elapsed() <= WEB_DIRECT_CACHE_TTL {
                return Ok(snapshot.clone());
            }
        }
        let observe = || {
            self.client
                .direct_read(&self.expected_model)
                .map_err(map_inspection_error)
        };
        let observed = match observe() {
            Ok(observed) => observed,
            Err(DirectFailure::Unavailable | DirectFailure::DeviceRejected) => {
                std::thread::sleep(WEB_DIRECT_RETRY_DELAY);
                observe()?
            }
            Err(failure) => return Err(failure),
        };
        let snapshot = DirectSnapshot {
            media: observed.media,
            inspection: observed.inspection,
            capabilities: authentics_300_capabilities().to_vec(),
            source_targets: observed.source_targets,
            active_eq: observed.active_eq,
            revision: 0,
        };
        self.cached_snapshot = Some((Instant::now(), snapshot.clone()));
        Ok(snapshot)
    }

    fn mutate(
        &mut self,
        lock: &mut DirectControlLock,
        mutation: DirectMutation,
    ) -> DirectActionResult {
        self.cached_snapshot = None;
        match mutation {
            DirectMutation::Volume(value) => {
                map_volume(self.client.set_volume(lock, &self.expected_model, value))
            }
            DirectMutation::Mute(target) => {
                map_mute(self.client.set_mute(lock, &self.expected_model, target))
            }
            DirectMutation::Source(target) => map_source(self.client.set_audio_source(
                lock,
                &self.expected_model,
                target,
            )),
            DirectMutation::EqPreset(target) => map_eq(self.client.set_eq_preset(
                lock,
                &self.expected_model,
                target,
            )),
        }
    }
}

fn map_direct_error(error: JblError) -> DirectFailure {
    match error {
        JblError::PeerCertificateMismatch
        | JblError::UnexpectedDeviceModel
        | JblError::DeviceInfoMissing
        | JblError::ControlDeviceInfoMissing => DirectFailure::InvalidState,
        JblError::NetworkUnreachable | JblError::HttpStatus(_) => DirectFailure::Unavailable,
        JblError::ControlCommandRejected
        | JblError::DeviceReportedError
        | JblError::UpnpActionRejected => DirectFailure::DeviceRejected,
        JblError::InvalidVolume
        | JblError::VolumeSafetyLimitExceeded
        | JblError::MediaVolumeMissing
        | JblError::MediaMuteMissing => DirectFailure::SafetyGate,
        JblError::UnsupportedMediaSource | JblError::EqPresetInvalid => {
            DirectFailure::UnsupportedTarget
        }
        JblError::PlaybackPreconditionFailed => DirectFailure::InvalidState,
        JblError::ConfigUnavailable
        | JblError::ConfigPermissions
        | JblError::ConfigTooLarge
        | JblError::InvalidConfig
        | JblError::MissingSetting(_)
        | JblError::InvalidTimeout
        | JblError::InvalidAddress
        | JblError::CertificateUnavailable
        | JblError::CertificatePermissions
        | JblError::PrivateKeyUnavailable
        | JblError::PrivateKeyPermissions
        | JblError::InvalidTlsFingerprint
        | JblError::InvalidClientIdentity
        | JblError::CredentialFileInvalid
        | JblError::CredentialTooLarge
        | JblError::TlsConfiguration => DirectFailure::Unavailable,
        _ => DirectFailure::InvalidState,
    }
}

fn map_inspection_error(error: InspectionReadError) -> DirectFailure {
    match error {
        InspectionReadError::Device(error) => map_direct_error(error),
        InspectionReadError::Response(_) => DirectFailure::InvalidState,
    }
}

fn map_volume(result: VolumeWriteResult) -> DirectActionResult {
    match result {
        VolumeWriteResult::AlreadyAtTarget(value) => direct_result(
            DirectActionOutcome::AlreadyAtTarget,
            Some(DirectObservation::Volume {
                volume: value.volume,
                muted: value.muted,
            }),
            None,
        ),
        VolumeWriteResult::Applied(value) => direct_result(
            DirectActionOutcome::Applied,
            Some(DirectObservation::Volume {
                volume: value.volume,
                muted: value.muted,
            }),
            None,
        ),
        VolumeWriteResult::TargetObservedAfterUnknownWrite(value) => direct_result(
            DirectActionOutcome::TargetObservedAfterUnknownWrite,
            Some(DirectObservation::Volume {
                volume: value.volume,
                muted: value.muted,
            }),
            Some(DirectFailure::OutcomeUnknown),
        ),
        VolumeWriteResult::PostconditionFailed(value) => direct_result(
            DirectActionOutcome::PostconditionFailed,
            Some(DirectObservation::Volume {
                volume: value.volume,
                muted: value.muted,
            }),
            None,
        ),
        VolumeWriteResult::RejectedBeforeSend(error) => rejected_before(error),
        VolumeWriteResult::OutcomeUnknown(error) => unknown(error),
    }
}

fn map_mute(result: MuteWriteResult) -> DirectActionResult {
    match result {
        MuteWriteResult::AlreadyAtTarget(value) => direct_result(
            DirectActionOutcome::AlreadyAtTarget,
            Some(DirectObservation::Mute { muted: value.muted }),
            None,
        ),
        MuteWriteResult::Applied(value) => direct_result(
            DirectActionOutcome::Applied,
            Some(DirectObservation::Mute { muted: value.muted }),
            None,
        ),
        MuteWriteResult::TargetObservedAfterUnknownWrite(value) => direct_result(
            DirectActionOutcome::TargetObservedAfterUnknownWrite,
            Some(DirectObservation::Mute { muted: value.muted }),
            Some(DirectFailure::OutcomeUnknown),
        ),
        MuteWriteResult::PostconditionFailed(value) => direct_result(
            DirectActionOutcome::PostconditionFailed,
            Some(DirectObservation::Mute { muted: value.muted }),
            None,
        ),
        MuteWriteResult::RejectedBeforeSend(error) => rejected_before(error),
        MuteWriteResult::OutcomeUnknown(error) => unknown(error),
    }
}

fn map_source(result: AudioSourceWriteResult) -> DirectActionResult {
    match result {
        AudioSourceWriteResult::AlreadyAtTarget(source) => direct_result(
            DirectActionOutcome::AlreadyAtTarget,
            Some(DirectObservation::Source { source }),
            None,
        ),
        AudioSourceWriteResult::Applied(source) => direct_result(
            DirectActionOutcome::Applied,
            Some(DirectObservation::Source { source }),
            None,
        ),
        AudioSourceWriteResult::RejectedByDevice(source) => direct_result(
            DirectActionOutcome::RejectedByDevice,
            Some(DirectObservation::Source { source }),
            Some(DirectFailure::DeviceRejected),
        ),
        AudioSourceWriteResult::TargetObservedAfterUnknownWrite(source) => direct_result(
            DirectActionOutcome::TargetObservedAfterUnknownWrite,
            Some(DirectObservation::Source { source }),
            Some(DirectFailure::OutcomeUnknown),
        ),
        AudioSourceWriteResult::PostconditionFailed(source) => direct_result(
            DirectActionOutcome::PostconditionFailed,
            Some(DirectObservation::Source { source }),
            None,
        ),
        AudioSourceWriteResult::RejectedBeforeSend(error) => rejected_before(error),
        AudioSourceWriteResult::OutcomeUnknown(error) => unknown(error),
    }
}

fn map_eq(result: EqPresetWriteResult) -> DirectActionResult {
    match result {
        EqPresetWriteResult::AlreadyAtTarget(preset) => direct_result(
            DirectActionOutcome::AlreadyAtTarget,
            Some(DirectObservation::EqPreset {
                preset: Some(preset),
            }),
            None,
        ),
        EqPresetWriteResult::Applied(preset) => direct_result(
            DirectActionOutcome::Applied,
            Some(DirectObservation::EqPreset {
                preset: Some(preset),
            }),
            None,
        ),
        EqPresetWriteResult::RejectedByDevice(preset) => direct_result(
            DirectActionOutcome::RejectedByDevice,
            Some(DirectObservation::EqPreset {
                preset: Some(preset),
            }),
            Some(DirectFailure::DeviceRejected),
        ),
        EqPresetWriteResult::TargetObservedAfterUnknownWrite(preset) => direct_result(
            DirectActionOutcome::TargetObservedAfterUnknownWrite,
            Some(DirectObservation::EqPreset {
                preset: Some(preset),
            }),
            Some(DirectFailure::OutcomeUnknown),
        ),
        EqPresetWriteResult::PostconditionFailed(preset) => direct_result(
            DirectActionOutcome::PostconditionFailed,
            Some(DirectObservation::EqPreset { preset }),
            None,
        ),
        EqPresetWriteResult::RejectedBeforeSend(error) => rejected_before(error),
        EqPresetWriteResult::OutcomeUnknown(error) => unknown(error),
    }
}

const fn direct_result(
    outcome: DirectActionOutcome,
    observation: Option<DirectObservation>,
    failure: Option<DirectFailure>,
) -> DirectActionResult {
    DirectActionResult::new(outcome, observation, failure)
}

fn rejected_before(error: JblError) -> DirectActionResult {
    direct_result(
        DirectActionOutcome::RejectedBeforeSend,
        None,
        Some(map_direct_error(error)),
    )
}

fn unknown(error: JblError) -> DirectActionResult {
    let _ = error;
    direct_result(
        DirectActionOutcome::OutcomeUnknown,
        None,
        Some(DirectFailure::OutcomeUnknown),
    )
}

/// Web adapter around the only in-process controller owner.
pub(crate) struct ControllerWebActor<
    B: PairBackend,
    P: PairConfigurationProbe,
    J: UncertaintyJournal,
    D: DirectDeviceSurface,
> {
    controller: PairController<B, P, J>,
    direct: D,
    teardown_done: bool,
    direct_lock: DirectControlLock,
}

impl<B: PairBackend, P: PairConfigurationProbe, J: UncertaintyJournal, D: DirectDeviceSurface> Drop
    for ControllerWebActor<B, P, J, D>
{
    fn drop(&mut self) {
        // Covers listener accept errors, unwinding, and any other path that
        // consumes the actor before main can perform graceful shutdown. This
        // method releases only process-owned transport; it never sends role
        // commands and never clears the persistent uncertainty journal.
        let _teardown = self.teardown_transport_once();
    }
}

impl<B: PairBackend, P: PairConfigurationProbe, J: UncertaintyJournal, D: DirectDeviceSurface>
    ControllerWebActor<B, P, J, D>
{
    const fn new(
        controller: PairController<B, P, J>,
        direct: D,
        direct_lock: DirectControlLock,
    ) -> Self {
        Self {
            controller,
            direct,
            teardown_done: false,
            direct_lock,
        }
    }

    pub(crate) fn direct_snapshot(&mut self) -> Result<DirectSnapshot, DirectFailure> {
        if self.controller.has_unresolved_action() {
            return Err(DirectFailure::InvalidState);
        }
        self.direct
            .snapshot()
            .map(|snapshot| snapshot.with_revision(self.controller.revision()))
    }

    pub(crate) fn mutate_direct_if_revision(
        &mut self,
        expected_revision: u64,
        mutation: DirectMutation,
    ) -> Result<DirectActionResult, RevisionConflict> {
        if self.controller.revision() != expected_revision {
            return Err(RevisionConflict);
        }
        if let Err(failure) = self.controller.prepare_external_action() {
            let revision = self.controller.note_external_action();
            return Ok(match failure {
                ExternalPrepareFailure::UnresolvedPriorAction => DirectActionResult::new(
                    DirectActionOutcome::RejectedBeforeSend,
                    None,
                    Some(DirectFailure::InvalidState),
                ),
                ExternalPrepareFailure::JournalUnavailable => DirectActionResult::new(
                    DirectActionOutcome::RejectedBeforeSend,
                    None,
                    Some(DirectFailure::Unavailable),
                ),
                ExternalPrepareFailure::JournalCommitFailed => DirectActionResult::new(
                    DirectActionOutcome::OutcomeUnknown,
                    None,
                    Some(DirectFailure::OutcomeUnknown),
                ),
            }
            .with_revision(revision));
        }
        let result = self.direct.mutate(&mut self.direct_lock, mutation);
        let uncertain = matches!(
            result.outcome,
            DirectActionOutcome::OutcomeUnknown
                | DirectActionOutcome::TargetObservedAfterUnknownWrite
        );
        let finished = self.controller.finish_external_action(uncertain);
        Ok(if finished.journal_failed {
            DirectActionResult::new(
                DirectActionOutcome::OutcomeUnknown,
                None,
                Some(DirectFailure::OutcomeUnknown),
            )
            .with_revision(finished.revision)
        } else {
            result.with_revision(finished.revision)
        })
    }

    /// One bounded graceful shutdown.  If the controller has latched an
    /// uncertain prior write, its own invariant rejects this before touching
    /// either device; dropping the returned actor then releases only local
    /// resources.
    #[must_use]
    fn shutdown(&mut self) -> ControllerActionResult {
        self.controller.shutdown()
    }

    fn teardown_transport_once(&mut self) -> Result<(), crate::backend::PairBackendError> {
        if self.teardown_done {
            return Ok(());
        }
        let result = self.controller.teardown_transport_for_exit();
        if result.is_ok() {
            self.teardown_done = true;
        }
        result
    }
}

/// Minimal lifecycle needed by the service host after the listener returns.
/// The concrete controller/backend types stay behind this safe factory.
pub trait ServiceActor: WebActor {
    fn shutdown_for_exit(&mut self) -> Result<(), ServiceRuntimeError>;
}

impl<B: PairBackend, P: PairConfigurationProbe, J: UncertaintyJournal, D: DirectDeviceSurface>
    ServiceActor for ControllerWebActor<B, P, J, D>
{
    fn shutdown_for_exit(&mut self) -> Result<(), ServiceRuntimeError> {
        let shutdown_error = shutdown_error(self.shutdown());
        let teardown_error = self
            .teardown_transport_once()
            .err()
            .map(|_| ServiceRuntimeError::TransportTeardownFailed);

        // Device-role outcome has priority when both phases fail, but teardown
        // is always attempted first. Both choices remain closed and sanitized.
        match (shutdown_error, teardown_error) {
            (Some(error), _) => Err(error),
            (None, Some(error)) => Err(error),
            (None, None) => Ok(()),
        }
    }
}

fn shutdown_error(result: ControllerActionResult) -> Option<ServiceRuntimeError> {
    match result.outcome() {
        ControllerActionOutcome::Accepted
        | ControllerActionOutcome::AcceptedUnconfirmed
        | ControllerActionOutcome::Idempotent => None,
        ControllerActionOutcome::RejectedBeforeSend => {
            Some(ServiceRuntimeError::GracefulShutdownRejected)
        }
        ControllerActionOutcome::OutcomeUnknown => {
            Some(ServiceRuntimeError::GracefulShutdownOutcomeUnknown)
        }
        ControllerActionOutcome::PostconditionFailed => {
            Some(ServiceRuntimeError::GracefulShutdownPostconditionFailed)
        }
    }
}

/// Construct the one native service actor. Construction performs no device
/// discovery, connection, role write, or automatic grouping.
pub fn build_native_service_actor(
    config: &RuntimeConfig,
) -> Result<impl ServiceActor, ServiceRuntimeError> {
    // Acquire before constructing any mutable backend capability, then move
    // the guard into the opaque actor so no public factory caller can create a
    // controller without holding cross-process ownership through shutdown.
    let process_lock = ControllerProcessLock::acquire()?;
    let direct_lock = DirectControlLock {
        _inner: process_lock,
        _not_sync: PhantomData,
    };
    let backend =
        NativePairBackend::new(config, AuraBluezConfig::default(), Duration::from_secs(2))
            .map_err(|_| ServiceRuntimeError::ControllerInitializationFailed)?;
    let probe = JblPairConfigurationProbe::new(config)
        .map_err(|_| ServiceRuntimeError::ControllerInitializationFailed)?;
    let journal = FileUncertaintyJournal::open_default()
        .map_err(|_| ServiceRuntimeError::ControllerInitializationFailed)?;
    let direct = JblDirectSurface {
        client: JblLanClient::new(
            &config.address,
            &config.certificate,
            &config.private_key,
            &config.tls_sha256,
            config.timeout.min(WEB_DIRECT_REQUEST_TIMEOUT),
        )
        .map_err(|_| ServiceRuntimeError::ControllerInitializationFailed)?,
        expected_model: config.expected_model.clone(),
        cached_snapshot: None,
    };
    Ok(ControllerWebActor::new(
        PairController::with_journal(backend, probe, journal),
        direct,
        direct_lock,
    ))
}

/// Read-only doctor gate for the native runtime. The created transport is
/// dropped without discovery, connection, health query, or command dispatch.
pub fn validate_native_runtime(config: &RuntimeConfig) -> Result<(), ServiceRuntimeError> {
    NativePairBackend::new(config, AuraBluezConfig::default(), Duration::from_secs(2))
        .map(drop)
        .map_err(|_| ServiceRuntimeError::ControllerInitializationFailed)
}

impl<B: PairBackend, P: PairConfigurationProbe, J: UncertaintyJournal, D: DirectDeviceSurface>
    WebActor for ControllerWebActor<B, P, J, D>
{
    fn status(&mut self) -> crate::controller::ControllerStatus {
        self.controller.status()
    }

    fn mutate_if_revision(
        &mut self,
        expected_revision: u64,
        mutation: WebMutation,
    ) -> Result<ControllerActionResult, RevisionConflict> {
        if self.controller.revision() != expected_revision {
            return Err(RevisionConflict);
        }
        Ok(match mutation {
            WebMutation::Start => self.controller.start(),
            WebMutation::Stop => self.controller.stop(),
            WebMutation::RecoverStop => self.controller.recover_stop(),
        })
    }

    fn direct_snapshot(&mut self) -> Result<DirectSnapshot, DirectFailure> {
        ControllerWebActor::direct_snapshot(self)
    }

    fn mutate_direct_if_revision(
        &mut self,
        expected_revision: u64,
        mutation: DirectMutation,
    ) -> Result<DirectActionResult, RevisionConflict> {
        ControllerWebActor::mutate_direct_if_revision(self, expected_revision, mutation)
    }
}

/// Advisory lock held from before backend construction through graceful
/// shutdown.  The lock file remains on disk, but `flock` ownership is released
/// automatically when this value or the process exits.
struct ControllerProcessLock {
    // Field order is intentionally the reverse of acquisition order so the
    // session lock is released first and the public-operation gate last.
    _session_file: File,
    _operation_file: File,
}

/// Exclusive guard for a bounded direct JBL mutation outside the daemon.
///
/// It acquires the same operation and session locks as the persistent
/// Play Together service, so a direct media write cannot race either the Rust
/// daemon or the legacy v0.4 controller.
/// The guard is intentionally not `Sync`, so one acquired capability cannot be
/// shared by safe Rust across concurrent mutation threads.
///
/// ```compile_fail
/// fn requires_sync<T: Sync>() {}
/// requires_sync::<jbl_aura_link::DirectControlLock>();
/// ```
pub struct DirectControlLock {
    _inner: ControllerProcessLock,
    _not_sync: PhantomData<Cell<()>>,
}

#[cfg(test)]
impl DirectControlLock {
    /// Constructs a type-level mutation permit for in-process protocol
    /// fixtures. Production callers can only obtain this type by acquiring
    /// both real controller locks through `acquire_direct_control_lock`.
    pub(crate) fn for_protocol_fixture() -> Self {
        let operation_file = File::open("/dev/null").expect("fixture operation file should open");
        let session_file = File::open("/dev/null").expect("fixture session file should open");
        Self {
            _inner: ControllerProcessLock {
                _session_file: session_file,
                _operation_file: operation_file,
            },
            _not_sync: PhantomData,
        }
    }
}

pub fn acquire_direct_control_lock() -> Result<DirectControlLock, ServiceRuntimeError> {
    ControllerProcessLock::acquire().map(|inner| DirectControlLock {
        _inner: inner,
        _not_sync: PhantomData,
    })
}

impl ControllerProcessLock {
    fn acquire() -> Result<Self, ServiceRuntimeError> {
        let runtime_root = env::var_os("XDG_RUNTIME_DIR")
            .filter(|value| !value.is_empty())
            .ok_or(ServiceRuntimeError::RuntimeDirectoryUnavailable)?;
        Self::acquire_at(Path::new(&runtime_root))
    }

    fn acquire_at(runtime_root: &Path) -> Result<Self, ServiceRuntimeError> {
        if !runtime_root.is_absolute() {
            return Err(ServiceRuntimeError::RuntimeDirectoryUntrusted);
        }
        let root = open_directory(runtime_root)?;
        validate_directory(root.as_raw_fd())?;
        let runtime_name = cstr(RUNTIME_DIRECTORY);
        // SAFETY: root is a verified directory descriptor and runtime_name is
        // a static NUL-terminated component without separators.
        let created = unsafe { libc::mkdirat(root.as_raw_fd(), runtime_name.as_ptr(), 0o700) };
        if created != 0 && io::Error::last_os_error().raw_os_error() != Some(libc::EEXIST) {
            return Err(ServiceRuntimeError::RuntimeDirectoryUnavailable);
        }
        let directory = openat_directory(root.as_raw_fd(), runtime_name)?;
        validate_directory(directory.as_raw_fd())?;
        // Keep this order in sync with the v0.4 public-entry boundary. The
        // reservation covers v0.4's deliberate operation-lock release while
        // systemd hands ownership to the persistent daemon; session.lock then
        // excludes that daemon for the rest of its lifetime.
        let operation_file = acquire_lock_file(directory.as_raw_fd(), OPERATION_LOCK_FILE)?;
        if path_exists_at(directory.as_raw_fd(), V04_LAUNCH_RESERVATION)? {
            return Err(ServiceRuntimeError::AlreadyRunning);
        }
        let session_file = acquire_lock_file(directory.as_raw_fd(), SESSION_LOCK_FILE)?;
        Ok(Self {
            _session_file: session_file,
            _operation_file: operation_file,
        })
    }
}

fn path_exists_at(
    directory: RawFd,
    static_name: &'static [u8],
) -> Result<bool, ServiceRuntimeError> {
    let name = cstr(static_name);
    // SAFETY: metadata is writable, directory is live, and the static name is
    // one NUL-terminated component. AT_SYMLINK_NOFOLLOW makes any marker type
    // (including a malicious symlink) block ownership without dereferencing it.
    let mut metadata: libc::stat = unsafe { std::mem::zeroed() };
    let result = unsafe {
        libc::fstatat(
            directory,
            name.as_ptr(),
            &mut metadata,
            libc::AT_SYMLINK_NOFOLLOW,
        )
    };
    if result == 0 {
        return Ok(true);
    }
    match io::Error::last_os_error().raw_os_error() {
        Some(libc::ENOENT) => Ok(false),
        _ => Err(ServiceRuntimeError::RuntimeDirectoryUnavailable),
    }
}

fn acquire_lock_file(
    directory: RawFd,
    static_name: &'static [u8],
) -> Result<File, ServiceRuntimeError> {
    let lock_name = cstr(static_name);
    // SAFETY: directory and static filename are valid. O_NOFOLLOW and the
    // following fstat reject symlinks and special files.
    let descriptor = unsafe {
        libc::openat(
            directory,
            lock_name.as_ptr(),
            libc::O_RDWR | libc::O_CREAT | libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK,
            0o600,
        )
    };
    if descriptor < 0 {
        return Err(ServiceRuntimeError::RuntimeDirectoryUntrusted);
    }
    // SAFETY: openat returned a fresh owned descriptor.
    let file = unsafe { File::from_raw_fd(descriptor) };
    validate_lock_file(file.as_raw_fd())?;
    // SAFETY: flock operates on our valid descriptor and has no borrowed
    // memory. LOCK_NB ensures a competing controller cannot block startup.
    let locked = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if locked != 0 {
        return match io::Error::last_os_error().raw_os_error() {
            Some(libc::EWOULDBLOCK) => Err(ServiceRuntimeError::AlreadyRunning),
            _ => Err(ServiceRuntimeError::RuntimeDirectoryUnavailable),
        };
    }
    Ok(file)
}

fn cstr(value: &'static [u8]) -> &'static std::ffi::CStr {
    std::ffi::CStr::from_bytes_with_nul(value).expect("static C string")
}

fn path_cstring(path: &Path) -> Result<CString, ServiceRuntimeError> {
    CString::new(path.as_os_str().as_bytes())
        .map_err(|_| ServiceRuntimeError::RuntimeDirectoryUntrusted)
}

fn open_directory(path: &Path) -> Result<OwnedFd, ServiceRuntimeError> {
    let path = path_cstring(path)?;
    // SAFETY: path is NUL terminated and flags require a real directory while
    // rejecting a final symlink.
    let descriptor = unsafe {
        libc::open(
            path.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
        )
    };
    owned_fd(descriptor)
}

fn openat_directory(parent: RawFd, name: &std::ffi::CStr) -> Result<OwnedFd, ServiceRuntimeError> {
    // SAFETY: parent is open and name is a static single component.
    let descriptor = unsafe {
        libc::openat(
            parent,
            name.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
        )
    };
    owned_fd(descriptor)
}

fn owned_fd(descriptor: RawFd) -> Result<OwnedFd, ServiceRuntimeError> {
    if descriptor < 0 {
        Err(ServiceRuntimeError::RuntimeDirectoryUnavailable)
    } else {
        // SAFETY: the successful libc open returned a new owned descriptor.
        Ok(unsafe { OwnedFd::from_raw_fd(descriptor) })
    }
}

fn stat(descriptor: RawFd) -> Result<libc::stat, ServiceRuntimeError> {
    // SAFETY: zero is a valid initial representation for stat and fstat fills
    // it before it is inspected.
    let mut metadata: libc::stat = unsafe { std::mem::zeroed() };
    // SAFETY: descriptor is live and metadata is writable.
    if unsafe { libc::fstat(descriptor, &mut metadata) } != 0 {
        return Err(ServiceRuntimeError::RuntimeDirectoryUnavailable);
    }
    Ok(metadata)
}

fn validate_directory(descriptor: RawFd) -> Result<(), ServiceRuntimeError> {
    let metadata = stat(descriptor)?;
    // SAFETY: geteuid has no preconditions.
    let euid = unsafe { libc::geteuid() };
    if metadata.st_uid != euid
        || metadata.st_mode & libc::S_IFMT != libc::S_IFDIR
        || metadata.st_mode & 0o077 != 0
    {
        return Err(ServiceRuntimeError::RuntimeDirectoryUntrusted);
    }
    Ok(())
}

fn validate_lock_file(descriptor: RawFd) -> Result<(), ServiceRuntimeError> {
    let metadata = stat(descriptor)?;
    // SAFETY: geteuid has no preconditions.
    let euid = unsafe { libc::geteuid() };
    if metadata.st_uid != euid
        || metadata.st_mode & libc::S_IFMT != libc::S_IFREG
        || metadata.st_mode & 0o077 != 0
    {
        return Err(ServiceRuntimeError::RuntimeDirectoryUntrusted);
    }
    Ok(())
}

static SIGNAL_FLAG: AtomicPtr<AtomicBool> = AtomicPtr::new(std::ptr::null_mut());

extern "C" fn termination_handler(_signal: libc::c_int) {
    let flag = SIGNAL_FLAG.load(Ordering::Relaxed);
    if !flag.is_null() {
        // SAFETY: installation intentionally leaks one Arc strong reference,
        // so the AtomicBool stays alive until process exit. The handler does
        // only one lock-free atomic store and calls no allocator or I/O API.
        unsafe { (*flag).store(true, Ordering::Release) };
    }
}

/// Installs SIGINT/SIGTERM handlers whose signal context performs only an
/// atomic store. One tiny Arc allocation is deliberately process-lifetime so a
/// signal can never race deallocation.
pub fn install_termination_handlers() -> Result<Arc<AtomicBool>, ServiceRuntimeError> {
    let shutdown = Arc::new(AtomicBool::new(false));
    let leaked = Arc::into_raw(Arc::clone(&shutdown)).cast_mut();
    if SIGNAL_FLAG
        .compare_exchange(
            std::ptr::null_mut(),
            leaked,
            Ordering::SeqCst,
            Ordering::SeqCst,
        )
        .is_err()
    {
        // SAFETY: compare_exchange failed, so ownership of the new leaked
        // strong reference remains local and can be reconstructed once.
        unsafe { drop(Arc::from_raw(leaked)) };
        return Err(ServiceRuntimeError::SignalHandlerUnavailable);
    }
    install_signal(libc::SIGINT).and_then(|()| install_signal(libc::SIGTERM))?;
    Ok(shutdown)
}

fn install_signal(signal: libc::c_int) -> Result<(), ServiceRuntimeError> {
    // SAFETY: zero initialization is valid for sigaction before all relevant
    // fields are assigned, and libc functions receive valid pointers.
    let mut action: libc::sigaction = unsafe { std::mem::zeroed() };
    action.sa_sigaction = termination_handler as *const () as usize;
    action.sa_flags = libc::SA_RESTART;
    if unsafe { libc::sigemptyset(&mut action.sa_mask) } != 0
        || unsafe { libc::sigaction(signal, &action, std::ptr::null_mut()) } != 0
    {
        return Err(ServiceRuntimeError::SignalHandlerUnavailable);
    }
    Ok(())
}

/// Ensure the existing local singleton is ready.  Only a definite connection
/// refusal/unavailability can trigger one bounded `systemctl --user start`;
/// malformed, hung, or conflicting services never cause a second controller.
pub fn ensure_user_service(client: &LocalServiceClient) -> Result<(), ServiceRuntimeError> {
    ensure_user_service_with(
        || client.ready(),
        start_user_unit_once,
        SERVICE_READY_TIMEOUT,
        READY_POLL_INTERVAL,
    )
}

fn ensure_user_service_with(
    mut ready: impl FnMut() -> Result<(), LocalClientError>,
    start_once: impl FnOnce() -> Result<(), ServiceRuntimeError>,
    ready_timeout: Duration,
    poll_interval: Duration,
) -> Result<(), ServiceRuntimeError> {
    match ready() {
        Ok(()) => return Ok(()),
        Err(error) if error.service_unavailable() => {}
        Err(_) => return Err(ServiceRuntimeError::LocalServiceInvalid),
    }
    start_once()?;
    let deadline = Instant::now() + ready_timeout;
    loop {
        match ready() {
            Ok(()) => return Ok(()),
            Err(error) if error.service_unavailable() || error == LocalClientError::TimedOut => {}
            Err(_) => return Err(ServiceRuntimeError::LocalServiceInvalid),
        }
        if Instant::now() >= deadline {
            return Err(ServiceRuntimeError::ServiceReadyTimedOut);
        }
        std::thread::sleep(poll_interval);
    }
}

fn start_user_unit_once() -> Result<(), ServiceRuntimeError> {
    let mut child = Command::new(OsStr::new("systemctl"))
        .args(["--user", "start", USER_UNIT])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| ServiceRuntimeError::ServiceManagerUnavailable)?;
    let deadline = Instant::now() + SYSTEMCTL_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => return Ok(()),
            Ok(Some(_)) => return Err(ServiceRuntimeError::ServiceStartFailed),
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(25));
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(ServiceRuntimeError::ServiceManagerUnavailable);
            }
            Err(_) => return Err(ServiceRuntimeError::ServiceManagerUnavailable),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::fs;
    use std::fs::OpenOptions;
    use std::os::unix::fs::{symlink, DirBuilderExt, PermissionsExt};
    use std::sync::atomic::AtomicU64;

    struct NoopDirect;

    impl DirectDeviceSurface for NoopDirect {
        fn snapshot(&mut self) -> Result<DirectSnapshot, DirectFailure> {
            Err(DirectFailure::Unavailable)
        }

        fn mutate(
            &mut self,
            _lock: &mut DirectControlLock,
            _mutation: DirectMutation,
        ) -> DirectActionResult {
            DirectActionResult::new(
                DirectActionOutcome::RejectedBeforeSend,
                None,
                Some(DirectFailure::Unavailable),
            )
        }
    }

    struct RecordingDirect {
        calls: Arc<AtomicU64>,
        result: DirectActionResult,
    }

    impl DirectDeviceSurface for RecordingDirect {
        fn snapshot(&mut self) -> Result<DirectSnapshot, DirectFailure> {
            Err(DirectFailure::Unavailable)
        }

        fn mutate(
            &mut self,
            _lock: &mut DirectControlLock,
            _mutation: DirectMutation,
        ) -> DirectActionResult {
            self.calls.fetch_add(1, Ordering::Relaxed);
            self.result
        }
    }

    fn direct_lock(inner: ControllerProcessLock) -> DirectControlLock {
        DirectControlLock {
            _inner: inner,
            _not_sync: PhantomData,
        }
    }
    use std::time::{SystemTime, UNIX_EPOCH};

    use crate::backend::{
        PairActionFailure, PairActionReceipt, PairActionResult, PairBackendError,
        PairBackendEvidence, PairBackendKind, PairHealth, PairLifecycle,
    };
    use crate::controller::{PairConfigurationObservation, PairProbeError};
    use crate::journal::{JournalAction, JournalError, MemoryJournal};

    use super::*;

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    fn temporary_runtime() -> std::path::PathBuf {
        let suffix = COUNTER.fetch_add(1, Ordering::Relaxed);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let path = env::temp_dir().join(format!(
            "jbl-aura-rust-runtime-{}-{nanos}-{suffix}",
            std::process::id()
        ));
        let mut builder = fs::DirBuilder::new();
        builder.mode(0o700).create(&path).unwrap();
        path
    }

    #[test]
    fn process_lock_excludes_a_second_controller_and_is_reacquirable() {
        let root = temporary_runtime();
        let first = ControllerProcessLock::acquire_at(&root).unwrap();
        assert!(matches!(
            ControllerProcessLock::acquire_at(&root),
            Err(ServiceRuntimeError::AlreadyRunning)
        ));
        drop(first);
        ControllerProcessLock::acquire_at(&root).unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn process_lock_rejects_broad_root_and_symlinked_private_directory() {
        let root = temporary_runtime();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(matches!(
            ControllerProcessLock::acquire_at(&root),
            Err(ServiceRuntimeError::RuntimeDirectoryUntrusted)
        ));
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
        let target = root.join("target");
        fs::create_dir(&target).unwrap();
        symlink(&target, root.join("jbl-aura-link")).unwrap();
        assert!(ControllerProcessLock::acquire_at(&root).is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn process_lock_excludes_both_v04_operation_and_session_owners() {
        let root = temporary_runtime();
        drop(ControllerProcessLock::acquire_at(&root).unwrap());

        for filename in ["operation.lock", "session.lock"] {
            let file = OpenOptions::new()
                .read(true)
                .write(true)
                .open(root.join("jbl-aura-link").join(filename))
                .unwrap();
            // SAFETY: file owns a live descriptor and LOCK_NB cannot block.
            assert_eq!(
                unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
                0
            );
            assert!(matches!(
                ControllerProcessLock::acquire_at(&root),
                Err(ServiceRuntimeError::AlreadyRunning)
            ));
            drop(file);
        }

        ControllerProcessLock::acquire_at(&root).unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn process_lock_honors_the_v04_systemd_launch_reservation() {
        let root = temporary_runtime();
        drop(ControllerProcessLock::acquire_at(&root).unwrap());
        let reservation = root.join("jbl-aura-link").join("launch.reservation");
        fs::create_dir(&reservation).unwrap();
        assert!(matches!(
            ControllerProcessLock::acquire_at(&root),
            Err(ServiceRuntimeError::AlreadyRunning)
        ));
        fs::remove_dir(&reservation).unwrap();
        ControllerProcessLock::acquire_at(&root).unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn signal_handler_only_sets_the_installed_atomic_flag() {
        let flag = Arc::new(AtomicBool::new(false));
        let pointer = Arc::into_raw(Arc::clone(&flag)).cast_mut();
        let old = SIGNAL_FLAG.swap(pointer, Ordering::SeqCst);
        termination_handler(libc::SIGTERM);
        assert!(flag.load(Ordering::Relaxed));
        SIGNAL_FLAG.store(old, Ordering::SeqCst);
        // SAFETY: this test installed exactly one leaked reference and has
        // removed its pointer from the global before reconstructing it.
        unsafe { drop(Arc::from_raw(pointer)) };
    }

    #[test]
    fn unavailable_listener_starts_the_user_unit_exactly_once() {
        let mut readiness = VecDeque::from([
            Err(LocalClientError::Unavailable),
            Err(LocalClientError::Unavailable),
            Ok(()),
        ]);
        let starts = AtomicU64::new(0);
        ensure_user_service_with(
            || readiness.pop_front().expect("bounded readiness fixture"),
            || {
                starts.fetch_add(1, Ordering::Relaxed);
                Ok(())
            },
            Duration::from_secs(1),
            Duration::from_millis(1),
        )
        .unwrap();
        assert_eq!(starts.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn malformed_or_hung_listener_never_starts_a_second_controller() {
        for initial in [
            LocalClientError::InvalidResponse,
            LocalClientError::TimedOut,
            LocalClientError::RevisionConflict,
        ] {
            let starts = AtomicU64::new(0);
            assert!(ensure_user_service_with(
                || Err(initial),
                || {
                    starts.fetch_add(1, Ordering::Relaxed);
                    Ok(())
                },
                Duration::from_millis(1),
                Duration::from_millis(1),
            )
            .is_err());
            assert_eq!(starts.load(Ordering::Relaxed), 0);
        }
    }

    struct ReadyProbe;

    impl PairConfigurationProbe for ReadyProbe {
        fn pair_configuration(&mut self) -> Result<PairConfigurationObservation, PairProbeError> {
            Ok(PairConfigurationObservation::ready())
        }
    }

    struct ShutdownBackend {
        result: PairActionResult,
        teardown_result: Result<(), PairBackendError>,
        shutdown_calls: Arc<AtomicU64>,
        teardown_calls: Arc<AtomicU64>,
    }

    impl PairBackend for ShutdownBackend {
        fn kind(&self) -> PairBackendKind {
            PairBackendKind::NativePair
        }

        fn health(&mut self) -> Result<PairHealth, PairBackendError> {
            Err(PairBackendError::Unavailable)
        }

        fn start(&mut self) -> PairActionResult {
            panic!("shutdown fixture must not start")
        }

        fn stop(&mut self) -> PairActionResult {
            panic!("shutdown fixture must not stop separately")
        }

        fn shutdown(&mut self) -> PairActionResult {
            self.shutdown_calls.fetch_add(1, Ordering::Relaxed);
            self.result
        }

        fn teardown_transport(&mut self) -> Result<(), PairBackendError> {
            self.teardown_calls.fetch_add(1, Ordering::Relaxed);
            self.teardown_result
        }
    }

    fn accepted_shutdown() -> PairActionResult {
        PairActionResult::Accepted(PairActionReceipt::new(
            PairBackendKind::NativePair,
            PairLifecycle::ShuttingDown,
            PairBackendEvidence::LifecycleAcknowledgement,
            false,
        ))
    }

    fn rejected_shutdown() -> PairActionResult {
        PairActionResult::RejectedBeforeSend(PairActionFailure::new(
            PairBackendKind::NativePair,
            PairBackendError::Unavailable,
            None,
        ))
    }

    fn unknown_shutdown() -> PairActionResult {
        PairActionResult::OutcomeUnknown(PairActionFailure::new(
            PairBackendKind::NativePair,
            PairBackendError::JblExitOutcomeUnknown,
            Some(PairLifecycle::ShuttingDown),
        ))
    }

    type ShutdownActor = ControllerWebActor<ShutdownBackend, ReadyProbe, MemoryJournal, NoopDirect>;
    type ShutdownFixture = (
        ShutdownActor,
        std::path::PathBuf,
        Arc<AtomicU64>,
        Arc<AtomicU64>,
    );

    fn shutdown_actor(
        result: PairActionResult,
        teardown_result: Result<(), PairBackendError>,
    ) -> ShutdownFixture {
        let root = temporary_runtime();
        let process_lock = ControllerProcessLock::acquire_at(&root).unwrap();
        let shutdown_calls = Arc::new(AtomicU64::new(0));
        let teardown_calls = Arc::new(AtomicU64::new(0));
        let controller = PairController::with_journal(
            ShutdownBackend {
                result,
                teardown_result,
                shutdown_calls: Arc::clone(&shutdown_calls),
                teardown_calls: Arc::clone(&teardown_calls),
            },
            ReadyProbe,
            MemoryJournal::clean(),
        );
        (
            ControllerWebActor::new(controller, NoopDirect, direct_lock(process_lock)),
            root,
            shutdown_calls,
            teardown_calls,
        )
    }

    fn assert_shutdown_case(
        action: PairActionResult,
        teardown: Result<(), PairBackendError>,
        expected: Result<(), ServiceRuntimeError>,
    ) {
        let drop_retry_expected = teardown.is_err();
        let (mut actor, root, shutdown_calls, teardown_calls) = shutdown_actor(action, teardown);
        assert_eq!(actor.shutdown_for_exit(), expected);
        assert_eq!(shutdown_calls.load(Ordering::Relaxed), 1);
        assert_eq!(
            teardown_calls.load(Ordering::Relaxed),
            1,
            "teardown must be attempted regardless of action result"
        );
        drop(actor);
        assert_eq!(
            teardown_calls.load(Ordering::Relaxed),
            if drop_retry_expected { 2 } else { 1 },
            "Drop must skip a completed teardown and retry only a failed one"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn graceful_shutdown_propagates_accepted_rejected_and_unknown_results() {
        assert_shutdown_case(accepted_shutdown(), Ok(()), Ok(()));
        assert_shutdown_case(
            rejected_shutdown(),
            Ok(()),
            Err(ServiceRuntimeError::GracefulShutdownRejected),
        );
        assert_shutdown_case(
            unknown_shutdown(),
            Ok(()),
            Err(ServiceRuntimeError::GracefulShutdownOutcomeUnknown),
        );
    }

    #[test]
    fn graceful_shutdown_propagates_transport_teardown_failure() {
        assert_shutdown_case(
            accepted_shutdown(),
            Err(PairBackendError::Unavailable),
            Err(ServiceRuntimeError::TransportTeardownFailed),
        );
    }

    #[test]
    fn graceful_shutdown_action_failure_has_priority_after_teardown_is_attempted() {
        assert_shutdown_case(
            rejected_shutdown(),
            Err(PairBackendError::Unavailable),
            Err(ServiceRuntimeError::GracefulShutdownRejected),
        );
        assert_shutdown_case(
            unknown_shutdown(),
            Err(PairBackendError::Unavailable),
            Err(ServiceRuntimeError::GracefulShutdownOutcomeUnknown),
        );
    }

    struct DropBackend {
        role_calls: Arc<AtomicU64>,
        teardown_calls: Arc<AtomicU64>,
    }

    impl DropBackend {
        fn unexpected_role_call(&self) -> PairActionResult {
            self.role_calls.fetch_add(1, Ordering::Relaxed);
            PairActionResult::RejectedBeforeSend(PairActionFailure::new(
                PairBackendKind::NativePair,
                PairBackendError::Unavailable,
                None,
            ))
        }
    }

    impl PairBackend for DropBackend {
        fn kind(&self) -> PairBackendKind {
            PairBackendKind::NativePair
        }

        fn health(&mut self) -> Result<PairHealth, PairBackendError> {
            Err(PairBackendError::Unavailable)
        }

        fn start(&mut self) -> PairActionResult {
            self.unexpected_role_call()
        }

        fn stop(&mut self) -> PairActionResult {
            self.unexpected_role_call()
        }

        fn shutdown(&mut self) -> PairActionResult {
            self.unexpected_role_call()
        }

        fn teardown_transport(&mut self) -> Result<(), PairBackendError> {
            self.teardown_calls.fetch_add(1, Ordering::Relaxed);
            Ok(())
        }
    }

    struct DropProbe;

    impl PairConfigurationProbe for DropProbe {
        fn pair_configuration(
            &mut self,
        ) -> Result<crate::controller::PairConfigurationObservation, PairProbeError> {
            panic!("actor Drop must not query device membership")
        }
    }

    struct DropJournal {
        pending: Arc<AtomicBool>,
        clear_calls: Arc<AtomicU64>,
    }

    impl UncertaintyJournal for DropJournal {
        fn is_pending(&self) -> bool {
            self.pending.load(Ordering::Acquire)
        }

        fn mark_pending(&mut self, _action: JournalAction) -> Result<(), JournalError> {
            panic!("actor Drop must not replace the existing pending journal")
        }

        fn clear(&mut self) -> Result<(), JournalError> {
            self.clear_calls.fetch_add(1, Ordering::Relaxed);
            self.pending.store(false, Ordering::Release);
            Ok(())
        }
    }

    #[test]
    fn actor_drop_tears_down_transport_once_without_any_role_or_probe_call() {
        let root = temporary_runtime();
        let process_lock = ControllerProcessLock::acquire_at(&root).unwrap();
        let role_calls = Arc::new(AtomicU64::new(0));
        let teardown_calls = Arc::new(AtomicU64::new(0));
        let journal_pending = Arc::new(AtomicBool::new(true));
        let journal_clear_calls = Arc::new(AtomicU64::new(0));
        let controller = PairController::with_journal(
            DropBackend {
                role_calls: Arc::clone(&role_calls),
                teardown_calls: Arc::clone(&teardown_calls),
            },
            DropProbe,
            DropJournal {
                pending: Arc::clone(&journal_pending),
                clear_calls: Arc::clone(&journal_clear_calls),
            },
        );
        drop(ControllerWebActor::new(
            controller,
            NoopDirect,
            direct_lock(process_lock),
        ));
        assert_eq!(role_calls.load(Ordering::Relaxed), 0);
        assert_eq!(teardown_calls.load(Ordering::Relaxed), 1);
        assert!(journal_pending.load(Ordering::Acquire));
        assert_eq!(journal_clear_calls.load(Ordering::Relaxed), 0);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn graceful_exit_preserves_pending_journal_and_tears_down_exactly_once() {
        let root = temporary_runtime();
        let process_lock = ControllerProcessLock::acquire_at(&root).unwrap();
        let role_calls = Arc::new(AtomicU64::new(0));
        let teardown_calls = Arc::new(AtomicU64::new(0));
        let journal_pending = Arc::new(AtomicBool::new(true));
        let journal_clear_calls = Arc::new(AtomicU64::new(0));
        let controller = PairController::with_journal(
            DropBackend {
                role_calls: Arc::clone(&role_calls),
                teardown_calls: Arc::clone(&teardown_calls),
            },
            DropProbe,
            DropJournal {
                pending: Arc::clone(&journal_pending),
                clear_calls: Arc::clone(&journal_clear_calls),
            },
        );
        let mut actor = ControllerWebActor::new(controller, NoopDirect, direct_lock(process_lock));
        assert_eq!(
            actor.shutdown_for_exit(),
            Err(ServiceRuntimeError::GracefulShutdownRejected)
        );
        assert_eq!(role_calls.load(Ordering::Relaxed), 0);
        assert_eq!(teardown_calls.load(Ordering::Relaxed), 1);
        assert!(journal_pending.load(Ordering::Acquire));
        assert_eq!(journal_clear_calls.load(Ordering::Relaxed), 0);
        drop(actor);
        assert_eq!(teardown_calls.load(Ordering::Relaxed), 1);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn actor_owned_direct_lock_excludes_competitors_and_direct_revision_bumps_once() {
        let root = temporary_runtime();
        let process_lock = ControllerProcessLock::acquire_at(&root).unwrap();
        let direct_calls = Arc::new(AtomicU64::new(0));
        let shutdown_calls = Arc::new(AtomicU64::new(0));
        let teardown_calls = Arc::new(AtomicU64::new(0));
        let controller = PairController::with_journal(
            ShutdownBackend {
                result: accepted_shutdown(),
                teardown_result: Ok(()),
                shutdown_calls: Arc::clone(&shutdown_calls),
                teardown_calls: Arc::clone(&teardown_calls),
            },
            ReadyProbe,
            MemoryJournal::clean(),
        );
        let direct = RecordingDirect {
            calls: Arc::clone(&direct_calls),
            result: DirectActionResult::new(DirectActionOutcome::Applied, None, None),
        };
        let mut actor = ControllerWebActor::new(controller, direct, direct_lock(process_lock));
        assert!(matches!(
            ControllerProcessLock::acquire_at(&root),
            Err(ServiceRuntimeError::AlreadyRunning)
        ));
        let result = actor
            .mutate_direct_if_revision(0, DirectMutation::Volume(9))
            .expect("fresh direct revision");
        assert_eq!(result.revision, 1);
        assert_eq!(direct_calls.load(Ordering::Relaxed), 1);
        assert_eq!(shutdown_calls.load(Ordering::Relaxed), 0);
        assert!(actor.controller.status().last_action().is_none());
        assert!(actor
            .mutate_direct_if_revision(0, DirectMutation::Volume(8))
            .is_err());
        assert_eq!(direct_calls.load(Ordering::Relaxed), 1);
        let next = actor
            .mutate_direct_if_revision(1, DirectMutation::Volume(8))
            .expect("known result cleared direct marker");
        assert_eq!(next.revision, 2);
        assert_eq!(direct_calls.load(Ordering::Relaxed), 2);
        drop(actor);
        drop(ControllerProcessLock::acquire_at(&root).unwrap());
        assert_eq!(teardown_calls.load(Ordering::Relaxed), 1);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn direct_unknown_latches_and_blocks_snapshot_and_second_surface_call() {
        let root = temporary_runtime();
        let process_lock = ControllerProcessLock::acquire_at(&root).unwrap();
        let direct_calls = Arc::new(AtomicU64::new(0));
        let controller = PairController::with_journal(
            ShutdownBackend {
                result: accepted_shutdown(),
                teardown_result: Ok(()),
                shutdown_calls: Arc::new(AtomicU64::new(0)),
                teardown_calls: Arc::new(AtomicU64::new(0)),
            },
            ReadyProbe,
            MemoryJournal::clean(),
        );
        let direct = RecordingDirect {
            calls: Arc::clone(&direct_calls),
            result: DirectActionResult::new(
                DirectActionOutcome::OutcomeUnknown,
                None,
                Some(DirectFailure::OutcomeUnknown),
            ),
        };
        let mut actor = ControllerWebActor::new(controller, direct, direct_lock(process_lock));
        let first = actor
            .mutate_direct_if_revision(0, DirectMutation::Volume(9))
            .expect("first direct call");
        assert_eq!(first.outcome, DirectActionOutcome::OutcomeUnknown);
        assert_eq!(first.revision, 1);
        assert_eq!(direct_calls.load(Ordering::Relaxed), 1);
        assert_eq!(actor.direct_snapshot(), Err(DirectFailure::InvalidState));
        let second = actor
            .mutate_direct_if_revision(1, DirectMutation::Volume(8))
            .expect("blocked direct result");
        assert_eq!(second.outcome, DirectActionOutcome::RejectedBeforeSend);
        assert_eq!(second.failure, Some(DirectFailure::InvalidState));
        assert_eq!(direct_calls.load(Ordering::Relaxed), 1);
        drop(actor);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn pair_pending_journal_blocks_direct_surface_before_call() {
        let root = temporary_runtime();
        let process_lock = ControllerProcessLock::acquire_at(&root).unwrap();
        let direct_calls = Arc::new(AtomicU64::new(0));
        let controller = PairController::with_journal(
            ShutdownBackend {
                result: accepted_shutdown(),
                teardown_result: Ok(()),
                shutdown_calls: Arc::new(AtomicU64::new(0)),
                teardown_calls: Arc::new(AtomicU64::new(0)),
            },
            ReadyProbe,
            MemoryJournal::pending(JournalAction::Start),
        );
        let direct = RecordingDirect {
            calls: Arc::clone(&direct_calls),
            result: DirectActionResult::new(DirectActionOutcome::Applied, None, None),
        };
        let mut actor = ControllerWebActor::new(controller, direct, direct_lock(process_lock));
        assert_eq!(actor.direct_snapshot(), Err(DirectFailure::InvalidState));
        let result = actor
            .mutate_direct_if_revision(0, DirectMutation::Mute(crate::media::MuteTarget::On))
            .expect("blocked direct result");
        assert_eq!(result.outcome, DirectActionOutcome::RejectedBeforeSend);
        assert_eq!(direct_calls.load(Ordering::Relaxed), 0);
        drop(actor);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn weak_direct_readback_latches_before_a_second_surface_call() {
        let root = temporary_runtime();
        let process_lock = ControllerProcessLock::acquire_at(&root).unwrap();
        let direct_calls = Arc::new(AtomicU64::new(0));
        let controller = PairController::with_journal(
            ShutdownBackend {
                result: accepted_shutdown(),
                teardown_result: Ok(()),
                shutdown_calls: Arc::new(AtomicU64::new(0)),
                teardown_calls: Arc::new(AtomicU64::new(0)),
            },
            ReadyProbe,
            MemoryJournal::clean(),
        );
        let direct = RecordingDirect {
            calls: Arc::clone(&direct_calls),
            result: DirectActionResult::new(
                DirectActionOutcome::TargetObservedAfterUnknownWrite,
                None,
                Some(DirectFailure::OutcomeUnknown),
            ),
        };
        let mut actor = ControllerWebActor::new(controller, direct, direct_lock(process_lock));
        let first = actor
            .mutate_direct_if_revision(0, DirectMutation::Volume(9))
            .expect("weak direct result");
        assert_eq!(
            first.outcome,
            DirectActionOutcome::TargetObservedAfterUnknownWrite
        );
        let _blocked = actor
            .mutate_direct_if_revision(1, DirectMutation::Volume(8))
            .expect("blocked direct result");
        assert_eq!(direct_calls.load(Ordering::Relaxed), 1);
        drop(actor);
        fs::remove_dir_all(root).unwrap();
    }
}
