//! Bounded Aura Studio 5 wake acquisition through its paired A2DP profile.
//!
//! This module is intentionally independent from the Play Together backend.
//! It cannot write an Aura role, start playback, or change group state.  Its
//! only mutation is a narrowly scoped BlueZ `Device1.ConnectProfile` for the
//! standard A2DP Sink UUID, followed by `DisconnectProfile` and an observed
//! release.  A caller may proceed to the existing verified control transport
//! only after a fresh FDDF advertisement binds the expected product and stable
//! identity.

use std::fmt;
use std::time::Duration;

use crate::model::DeviceIdentity;

/// Standard Audio Sink service UUID passed to BlueZ `ConnectProfile`.
pub const A2DP_SINK_PROFILE_UUID: &str = "0000110b-0000-1000-8000-00805f9b34fb";

/// Aura Studio 5 product identifier carried by Harman FDDF Service Data.
pub const AURA_STUDIO_5_PRODUCT_ID: u16 = 0x212d;

const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(20);
// A read-only hardware probe observed that repeated exact FDDF evidence can
// have a gap longer than the former 8-second window. Thirty seconds remains
// bounded by the shared outer acquisition deadline and does not retry a role
// write (this module has no role-write capability).
const DEFAULT_FDDF_TIMEOUT: Duration = Duration::from_secs(30);
const DEFAULT_RELEASE_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_CONNECT_TIMEOUT: Duration = Duration::from_secs(20);
const MAX_FDDF_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_RELEASE_TIMEOUT: Duration = Duration::from_secs(10);

/// Closed, non-identifying I/O result supplied by platform adapters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuraWakeIoFailure {
    Unavailable,
    TimedOut,
    Rejected,
}

/// Address classification for the configured stable Aura object.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StableAddressKind {
    Public,
    Other,
}

/// Sanitized BlueZ properties read from the exact configured Aura object.
///
/// A production adapter must resolve the object from `expected_identity`, not
/// from a name, alias, cached rotating address, or first scan match.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StableAuraState {
    pub exact_identity: bool,
    pub address_kind: StableAddressKind,
    pub paired: bool,
    pub trusted: bool,
    pub blocked: bool,
}

/// Address classification for a fresh Aura FDDF observation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FddfAddressKind {
    ResolvablePrivate,
    Other,
}

/// Sanitized evidence returned by an FDDF observer.
///
/// No rotating address or payload is retained.  The production observer must
/// derive these fields from an advertisement received after the wait began.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FreshFddfObservation {
    pub fresh: bool,
    pub address_kind: FddfAddressKind,
    pub product_id: u16,
    pub embedded_identity_matches: bool,
}

/// BlueZ operations required by the wake-only acquisition.
///
/// Implementations must apply the supplied timeout to the complete D-Bus
/// method/property operation and must never broaden the profile UUID.
pub trait AuraWakeBluez {
    fn inspect_stable_device(
        &mut self,
        expected_identity: DeviceIdentity,
    ) -> Result<StableAuraState, AuraWakeIoFailure>;

    fn connect_profile(
        &mut self,
        expected_identity: DeviceIdentity,
        profile_uuid: &'static str,
        timeout: Duration,
    ) -> Result<(), AuraWakeIoFailure>;

    fn disconnect_profile(
        &mut self,
        expected_identity: DeviceIdentity,
        profile_uuid: &'static str,
        timeout: Duration,
    ) -> Result<(), AuraWakeIoFailure>;

    /// Returns `true` only after BlueZ reports the A2DP profile released.
    fn wait_profile_released(
        &mut self,
        expected_identity: DeviceIdentity,
        profile_uuid: &'static str,
        timeout: Duration,
    ) -> Result<bool, AuraWakeIoFailure>;
}

/// Fresh Harman FDDF observation required after A2DP has connected.
pub trait AuraWakeFddfObserver {
    fn wait_for_fresh_fddf(
        &mut self,
        expected_identity: DeviceIdentity,
        timeout: Duration,
    ) -> Result<Option<FreshFddfObservation>, AuraWakeIoFailure>;
}

/// Validated timing bounds for one wake acquisition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AuraWakeTimings {
    connect_timeout: Duration,
    fddf_timeout: Duration,
    release_timeout: Duration,
}

impl AuraWakeTimings {
    #[cfg(test)]
    fn new(
        connect_timeout: Duration,
        fddf_timeout: Duration,
        release_timeout: Duration,
    ) -> Result<Self, AuraWakeFailure> {
        let timings = Self {
            connect_timeout,
            fddf_timeout,
            release_timeout,
        };
        timings.validate()?;
        Ok(timings)
    }

    fn validate(self) -> Result<(), AuraWakeFailure> {
        if self.connect_timeout.is_zero()
            || self.connect_timeout > MAX_CONNECT_TIMEOUT
            || self.fddf_timeout.is_zero()
            || self.fddf_timeout > MAX_FDDF_TIMEOUT
            || self.release_timeout.is_zero()
            || self.release_timeout > MAX_RELEASE_TIMEOUT
        {
            return Err(AuraWakeFailure::InvalidConfiguration);
        }
        Ok(())
    }
}

impl Default for AuraWakeTimings {
    fn default() -> Self {
        Self {
            connect_timeout: DEFAULT_CONNECT_TIMEOUT,
            fddf_timeout: DEFAULT_FDDF_TIMEOUT,
            release_timeout: DEFAULT_RELEASE_TIMEOUT,
        }
    }
}

/// Closed, sanitized wake failure vocabulary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuraWakeFailure {
    InvalidConfiguration,
    StableDeviceUnavailable,
    StableIdentityMismatch,
    StableAddressInvalid,
    StableDeviceNotPaired,
    StableDeviceNotTrusted,
    StableDeviceBlocked,
    ProfileConnectFailed,
    FreshFddfUnavailable,
    FreshFddfTimedOut,
    FreshFddfInvalid,
    ProfileReleaseFailed,
}

impl AuraWakeFailure {
    pub const fn code(self) -> &'static str {
        match self {
            Self::InvalidConfiguration => "invalid_configuration",
            Self::StableDeviceUnavailable => "stable_device_unavailable",
            Self::StableIdentityMismatch => "stable_identity_mismatch",
            Self::StableAddressInvalid => "stable_address_invalid",
            Self::StableDeviceNotPaired => "stable_device_not_paired",
            Self::StableDeviceNotTrusted => "stable_device_not_trusted",
            Self::StableDeviceBlocked => "stable_device_blocked",
            Self::ProfileConnectFailed => "profile_connect_failed",
            Self::FreshFddfUnavailable => "fresh_fddf_unavailable",
            Self::FreshFddfTimedOut => "fresh_fddf_timed_out",
            Self::FreshFddfInvalid => "fresh_fddf_invalid",
            Self::ProfileReleaseFailed => "profile_release_failed",
        }
    }
}

impl fmt::Display for AuraWakeFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.code())
    }
}

/// Cleanup evidence attached to every result after a profile-connect attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuraWakeCleanup {
    NotNeeded,
    Released,
    ReleaseFailed,
}

/// Successful wake acquisition.  It proves only that fresh, identity-bound
/// FDDF became visible and the temporary A2DP profile was released.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AuraWakeSuccess {
    pub fresh_fddf_verified: bool,
    pub cleanup: AuraWakeCleanup,
}

/// Failed wake acquisition with explicit cleanup state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AuraWakeError {
    pub failure: AuraWakeFailure,
    pub cleanup: AuraWakeCleanup,
}

impl fmt::Display for AuraWakeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.failure.code())
    }
}

impl std::error::Error for AuraWakeError {}

fn validate_stable_state(state: StableAuraState) -> Result<(), AuraWakeFailure> {
    if !state.exact_identity {
        return Err(AuraWakeFailure::StableIdentityMismatch);
    }
    if state.address_kind != StableAddressKind::Public {
        return Err(AuraWakeFailure::StableAddressInvalid);
    }
    if !state.paired {
        return Err(AuraWakeFailure::StableDeviceNotPaired);
    }
    if !state.trusted {
        return Err(AuraWakeFailure::StableDeviceNotTrusted);
    }
    if state.blocked {
        return Err(AuraWakeFailure::StableDeviceBlocked);
    }
    Ok(())
}

fn validate_fddf(observation: FreshFddfObservation) -> Result<(), AuraWakeFailure> {
    if observation.fresh
        && observation.address_kind == FddfAddressKind::ResolvablePrivate
        && observation.product_id == AURA_STUDIO_5_PRODUCT_ID
        && observation.embedded_identity_matches
    {
        Ok(())
    } else {
        Err(AuraWakeFailure::FreshFddfInvalid)
    }
}

fn release_profile<B: AuraWakeBluez>(
    bluez: &mut B,
    expected_identity: DeviceIdentity,
    timeout: Duration,
) -> AuraWakeCleanup {
    // Confirmation is authoritative even when DisconnectProfile itself
    // returns an error: a reply can be lost after BlueZ has released A2DP.
    let _ = bluez.disconnect_profile(expected_identity, A2DP_SINK_PROFILE_UUID, timeout);
    match bluez.wait_profile_released(expected_identity, A2DP_SINK_PROFILE_UUID, timeout) {
        Ok(true) => AuraWakeCleanup::Released,
        Ok(false) | Err(_) => AuraWakeCleanup::ReleaseFailed,
    }
}

/// Performs one bounded, wake-only Aura acquisition.
///
/// The observer is invoked only after BlueZ reports A2DP connected.  Once a
/// profile connection has been attempted, cleanup is always attempted before
/// returning, including connect ambiguity and FDDF timeout/error paths.
pub fn acquire_aura_wake<B, O>(
    bluez: &mut B,
    observer: &mut O,
    expected_identity: DeviceIdentity,
    timings: AuraWakeTimings,
) -> Result<AuraWakeSuccess, AuraWakeError>
where
    B: AuraWakeBluez,
    O: AuraWakeFddfObserver,
{
    timings.validate().map_err(|failure| AuraWakeError {
        failure,
        cleanup: AuraWakeCleanup::NotNeeded,
    })?;

    let stable = bluez
        .inspect_stable_device(expected_identity)
        .map_err(|_| AuraWakeError {
            failure: AuraWakeFailure::StableDeviceUnavailable,
            cleanup: AuraWakeCleanup::NotNeeded,
        })?;
    validate_stable_state(stable).map_err(|failure| AuraWakeError {
        failure,
        cleanup: AuraWakeCleanup::NotNeeded,
    })?;

    let primary = match bluez.connect_profile(
        expected_identity,
        A2DP_SINK_PROFILE_UUID,
        timings.connect_timeout,
    ) {
        Ok(()) => match observer.wait_for_fresh_fddf(expected_identity, timings.fddf_timeout) {
            Ok(Some(observation)) => validate_fddf(observation),
            Ok(None) => Err(AuraWakeFailure::FreshFddfTimedOut),
            Err(_) => Err(AuraWakeFailure::FreshFddfUnavailable),
        },
        Err(_) => Err(AuraWakeFailure::ProfileConnectFailed),
    };

    let cleanup = release_profile(bluez, expected_identity, timings.release_timeout);
    match (primary, cleanup) {
        (Ok(()), AuraWakeCleanup::Released) => Ok(AuraWakeSuccess {
            fresh_fddf_verified: true,
            cleanup,
        }),
        (Ok(()), AuraWakeCleanup::ReleaseFailed) => Err(AuraWakeError {
            failure: AuraWakeFailure::ProfileReleaseFailed,
            cleanup,
        }),
        (Err(failure), cleanup) => Err(AuraWakeError { failure, cleanup }),
        (Ok(()), AuraWakeCleanup::NotNeeded) => unreachable!("connect attempt always cleans up"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Event {
        Inspect,
        Connect(&'static str, Duration),
        Observe(Duration),
        Disconnect(&'static str, Duration),
        ConfirmReleased(&'static str, Duration),
    }

    struct MockBluez {
        events: Vec<Event>,
        state: Result<StableAuraState, AuraWakeIoFailure>,
        connect: Result<(), AuraWakeIoFailure>,
        disconnect: Result<(), AuraWakeIoFailure>,
        released: Result<bool, AuraWakeIoFailure>,
    }

    impl Default for MockBluez {
        fn default() -> Self {
            Self {
                events: Vec::new(),
                state: Ok(valid_stable_state()),
                connect: Ok(()),
                disconnect: Ok(()),
                released: Ok(true),
            }
        }
    }

    impl AuraWakeBluez for MockBluez {
        fn inspect_stable_device(
            &mut self,
            _expected_identity: DeviceIdentity,
        ) -> Result<StableAuraState, AuraWakeIoFailure> {
            self.events.push(Event::Inspect);
            self.state
        }

        fn connect_profile(
            &mut self,
            _expected_identity: DeviceIdentity,
            profile_uuid: &'static str,
            timeout: Duration,
        ) -> Result<(), AuraWakeIoFailure> {
            self.events.push(Event::Connect(profile_uuid, timeout));
            self.connect
        }

        fn disconnect_profile(
            &mut self,
            _expected_identity: DeviceIdentity,
            profile_uuid: &'static str,
            timeout: Duration,
        ) -> Result<(), AuraWakeIoFailure> {
            self.events.push(Event::Disconnect(profile_uuid, timeout));
            self.disconnect
        }

        fn wait_profile_released(
            &mut self,
            _expected_identity: DeviceIdentity,
            profile_uuid: &'static str,
            timeout: Duration,
        ) -> Result<bool, AuraWakeIoFailure> {
            self.events
                .push(Event::ConfirmReleased(profile_uuid, timeout));
            self.released
        }
    }

    struct MockObserver<'a> {
        events: &'a mut Vec<Event>,
        observation: Result<Option<FreshFddfObservation>, AuraWakeIoFailure>,
    }

    impl AuraWakeFddfObserver for MockObserver<'_> {
        fn wait_for_fresh_fddf(
            &mut self,
            _expected_identity: DeviceIdentity,
            timeout: Duration,
        ) -> Result<Option<FreshFddfObservation>, AuraWakeIoFailure> {
            self.events.push(Event::Observe(timeout));
            self.observation
        }
    }

    fn identity() -> DeviceIdentity {
        DeviceIdentity::parse("020000000001").expect("synthetic identity")
    }

    const fn valid_stable_state() -> StableAuraState {
        StableAuraState {
            exact_identity: true,
            address_kind: StableAddressKind::Public,
            paired: true,
            trusted: true,
            blocked: false,
        }
    }

    const fn valid_observation() -> FreshFddfObservation {
        FreshFddfObservation {
            fresh: true,
            address_kind: FddfAddressKind::ResolvablePrivate,
            product_id: AURA_STUDIO_5_PRODUCT_ID,
            embedded_identity_matches: true,
        }
    }

    fn run(
        bluez: &mut MockBluez,
        observation: Result<Option<FreshFddfObservation>, AuraWakeIoFailure>,
    ) -> Result<AuraWakeSuccess, AuraWakeError> {
        let mut observer_events = Vec::new();
        let mut observer = MockObserver {
            events: &mut observer_events,
            observation,
        };
        let result =
            acquire_aura_wake(bluez, &mut observer, identity(), AuraWakeTimings::default());
        // Fold the observer event into its actual position between the BlueZ
        // connect and cleanup operations for concise ordering assertions.
        if !observer_events.is_empty() {
            bluez.events.insert(2, observer_events[0]);
        }
        result
    }

    #[test]
    fn exact_happy_path_is_bounded_and_releases_a2dp() {
        let mut bluez = MockBluez::default();
        let result = run(&mut bluez, Ok(Some(valid_observation()))).expect("wake succeeds");

        assert!(result.fresh_fddf_verified);
        assert_eq!(result.cleanup, AuraWakeCleanup::Released);
        assert_eq!(
            bluez.events,
            [
                Event::Inspect,
                Event::Connect(A2DP_SINK_PROFILE_UUID, Duration::from_secs(20)),
                Event::Observe(Duration::from_secs(30)),
                Event::Disconnect(A2DP_SINK_PROFILE_UUID, Duration::from_secs(5)),
                Event::ConfirmReleased(A2DP_SINK_PROFILE_UUID, Duration::from_secs(5)),
            ]
        );
    }

    #[test]
    fn invalid_stable_properties_fail_before_profile_mutation() {
        let cases = [
            (
                StableAuraState {
                    exact_identity: false,
                    ..valid_stable_state()
                },
                AuraWakeFailure::StableIdentityMismatch,
            ),
            (
                StableAuraState {
                    address_kind: StableAddressKind::Other,
                    ..valid_stable_state()
                },
                AuraWakeFailure::StableAddressInvalid,
            ),
            (
                StableAuraState {
                    paired: false,
                    ..valid_stable_state()
                },
                AuraWakeFailure::StableDeviceNotPaired,
            ),
            (
                StableAuraState {
                    trusted: false,
                    ..valid_stable_state()
                },
                AuraWakeFailure::StableDeviceNotTrusted,
            ),
            (
                StableAuraState {
                    blocked: true,
                    ..valid_stable_state()
                },
                AuraWakeFailure::StableDeviceBlocked,
            ),
        ];

        for (state, failure) in cases {
            let mut bluez = MockBluez {
                state: Ok(state),
                ..MockBluez::default()
            };
            let error = run(&mut bluez, Ok(Some(valid_observation()))).unwrap_err();
            assert_eq!(error.failure, failure);
            assert_eq!(error.cleanup, AuraWakeCleanup::NotNeeded);
            assert_eq!(bluez.events, [Event::Inspect]);
        }
    }

    #[test]
    fn ambiguous_connect_failure_still_attempts_and_confirms_release() {
        let mut bluez = MockBluez {
            connect: Err(AuraWakeIoFailure::TimedOut),
            ..MockBluez::default()
        };
        let error = run(&mut bluez, Ok(Some(valid_observation()))).unwrap_err();

        assert_eq!(error.failure, AuraWakeFailure::ProfileConnectFailed);
        assert_eq!(error.cleanup, AuraWakeCleanup::Released);
        assert_eq!(
            bluez.events,
            [
                Event::Inspect,
                Event::Connect(A2DP_SINK_PROFILE_UUID, Duration::from_secs(20)),
                Event::Disconnect(A2DP_SINK_PROFILE_UUID, Duration::from_secs(5)),
                Event::ConfirmReleased(A2DP_SINK_PROFILE_UUID, Duration::from_secs(5)),
            ]
        );
    }

    #[test]
    fn fddf_timeout_is_distinct_and_always_cleans_up() {
        let mut bluez = MockBluez::default();
        let error = run(&mut bluez, Ok(None)).unwrap_err();

        assert_eq!(error.failure, AuraWakeFailure::FreshFddfTimedOut);
        assert_eq!(error.cleanup, AuraWakeCleanup::Released);
        assert!(bluez.events.contains(&Event::Disconnect(
            A2DP_SINK_PROFILE_UUID,
            Duration::from_secs(5)
        )));
    }

    #[test]
    fn fddf_must_be_fresh_random_exact_product_and_identity() {
        let invalid = [
            FreshFddfObservation {
                fresh: false,
                ..valid_observation()
            },
            FreshFddfObservation {
                address_kind: FddfAddressKind::Other,
                ..valid_observation()
            },
            FreshFddfObservation {
                product_id: 0,
                ..valid_observation()
            },
            FreshFddfObservation {
                embedded_identity_matches: false,
                ..valid_observation()
            },
        ];

        for observation in invalid {
            let mut bluez = MockBluez::default();
            let error = run(&mut bluez, Ok(Some(observation))).unwrap_err();
            assert_eq!(error.failure, AuraWakeFailure::FreshFddfInvalid);
            assert_eq!(error.cleanup, AuraWakeCleanup::Released);
        }
    }

    #[test]
    fn observed_release_overrides_lost_disconnect_reply() {
        let mut bluez = MockBluez {
            disconnect: Err(AuraWakeIoFailure::Unavailable),
            released: Ok(true),
            ..MockBluez::default()
        };
        let result = run(&mut bluez, Ok(Some(valid_observation()))).expect("release observed");
        assert_eq!(result.cleanup, AuraWakeCleanup::Released);
    }

    #[test]
    fn unconfirmed_release_is_a_failure_and_preserves_primary_failure() {
        let mut successful_primary = MockBluez {
            released: Ok(false),
            ..MockBluez::default()
        };
        let error = run(&mut successful_primary, Ok(Some(valid_observation()))).unwrap_err();
        assert_eq!(error.failure, AuraWakeFailure::ProfileReleaseFailed);
        assert_eq!(error.cleanup, AuraWakeCleanup::ReleaseFailed);

        let mut failed_primary = MockBluez {
            released: Err(AuraWakeIoFailure::Unavailable),
            ..MockBluez::default()
        };
        let error = run(&mut failed_primary, Ok(None)).unwrap_err();
        assert_eq!(error.failure, AuraWakeFailure::FreshFddfTimedOut);
        assert_eq!(error.cleanup, AuraWakeCleanup::ReleaseFailed);
    }

    #[test]
    fn timing_configuration_cannot_exceed_hardware_evidence_bounds() {
        assert!(AuraWakeTimings::new(
            Duration::from_secs(20),
            Duration::from_secs(30),
            Duration::from_secs(10),
        )
        .is_ok());
        assert!(AuraWakeTimings::new(
            Duration::from_secs(21),
            Duration::from_secs(30),
            Duration::from_secs(5),
        )
        .is_err());
        assert!(AuraWakeTimings::new(
            Duration::from_secs(20),
            Duration::from_secs(31),
            Duration::from_secs(5),
        )
        .is_err());
        assert!(AuraWakeTimings::new(
            Duration::ZERO,
            Duration::from_secs(30),
            Duration::from_secs(5),
        )
        .is_err());
    }

    #[test]
    fn failure_formatting_is_closed_and_non_identifying() {
        let error = AuraWakeError {
            failure: AuraWakeFailure::ProfileConnectFailed,
            cleanup: AuraWakeCleanup::ReleaseFailed,
        };
        assert_eq!(error.to_string(), "profile_connect_failed");
        assert_eq!(error.failure.code(), "profile_connect_failed");
        assert_eq!(
            format!("{error:?}"),
            "AuraWakeError { failure: ProfileConnectFailed, cleanup: ReleaseFailed }"
        );
    }
}
