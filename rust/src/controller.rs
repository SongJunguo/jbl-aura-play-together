//! Backend-neutral, single-owner Play Together state reduction.
//!
//! The service actor is the only intended caller of mutation methods.  This
//! module deliberately keeps retained membership configuration, managed live
//! state, command acknowledgement and acoustic evidence as separate concepts.

use crate::backend::{
    PairActionResult, PairBackend, PairBackendError, PairBackendEvidence, PairBackendKind,
    PairBackendTransaction, PairHealth, PairLifecycle,
};
#[cfg(test)]
use crate::journal::MemoryJournal;
use crate::journal::{JournalAction, UncertaintyJournal};
use crate::model::GroupStatus;
use std::time::Instant;

/// A sanitized read of the retained JBL membership configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PairConfigurationState {
    Ready,
    NotReady,
    Unavailable,
}

/// Fixed public role names for the two supported members. Raw device names
/// and identifiers cannot be represented by this type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PairMemberName {
    JblAuthentics300,
    AuraStudio5,
}

/// Allowlisted channel labels projected from `getAuraCastGroupInfo`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PairMemberChannel {
    FrontLeft,
    FrontRight,
    Left,
    Right,
    Mono,
    Stereo,
    Unknown,
}

/// Whether the expected private identity was observed under the fixed public
/// role name. `Unavailable` means the read failed, not that a member is absent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PairMemberVerification {
    Verified,
    NotVerified,
    Unavailable,
}

/// A privacy-safe member projection. The name and every channel are closed
/// enums, so an address, identifier or device-supplied friendly name cannot
/// escape through controller status.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairMemberStatus {
    name: PairMemberName,
    verification: PairMemberVerification,
    channels: Vec<PairMemberChannel>,
}

impl PairMemberStatus {
    pub const fn name(&self) -> PairMemberName {
        self.name
    }

    pub const fn verification(&self) -> PairMemberVerification {
        self.verification
    }

    pub fn channels(&self) -> &[PairMemberChannel] {
        &self.channels
    }
}

/// One sanitized observation derived from one group-info response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PairConfigurationObservation {
    exact_pair_configured: bool,
    members: [PairMemberStatus; 2],
}

impl PairConfigurationObservation {
    #[cfg(test)]
    pub(crate) fn ready() -> Self {
        Self {
            exact_pair_configured: true,
            members: [
                fixed_member(
                    PairMemberName::JblAuthentics300,
                    PairMemberVerification::Verified,
                    vec![PairMemberChannel::Unknown],
                ),
                fixed_member(
                    PairMemberName::AuraStudio5,
                    PairMemberVerification::Verified,
                    vec![PairMemberChannel::Unknown],
                ),
            ],
        }
    }

    #[cfg(test)]
    fn not_ready() -> Self {
        Self {
            exact_pair_configured: false,
            members: unavailable_members(PairMemberVerification::NotVerified),
        }
    }

    pub(crate) fn from_group_status(group: GroupStatus) -> Self {
        let mut jbl_matches = Vec::new();
        let mut aura_matches = Vec::new();
        for member in group.members {
            match member.name.as_str() {
                "JBL Authentics 300" => jbl_matches.push(member.channels),
                "Aura Studio 5" => aura_matches.push(member.channels),
                _ => {}
            }
        }
        Self {
            exact_pair_configured: group.expected_pair_configured,
            members: [
                observed_member(PairMemberName::JblAuthentics300, jbl_matches),
                observed_member(PairMemberName::AuraStudio5, aura_matches),
            ],
        }
    }
}

fn observed_member(name: PairMemberName, mut matches: Vec<Vec<String>>) -> PairMemberStatus {
    if matches.len() != 1 {
        return fixed_member(
            name,
            PairMemberVerification::NotVerified,
            vec![PairMemberChannel::Unknown],
        );
    }
    let projected = matches
        .pop()
        .expect("one checked member")
        .into_iter()
        .map(|channel| channel_from_allowlist(&channel))
        .collect::<Vec<_>>();
    let channels = if projected.contains(&PairMemberChannel::Unknown) {
        vec![PairMemberChannel::Unknown]
    } else {
        projected
            .into_iter()
            .fold(Vec::new(), |mut unique, channel| {
                if !unique.contains(&channel) {
                    unique.push(channel);
                }
                unique
            })
    };
    fixed_member(
        name,
        PairMemberVerification::Verified,
        if channels.is_empty() {
            vec![PairMemberChannel::Unknown]
        } else {
            channels
        },
    )
}

fn channel_from_allowlist(channel: &str) -> PairMemberChannel {
    match channel {
        "front_left" => PairMemberChannel::FrontLeft,
        "front_right" => PairMemberChannel::FrontRight,
        "left" => PairMemberChannel::Left,
        "right" => PairMemberChannel::Right,
        "mono" => PairMemberChannel::Mono,
        "stereo" => PairMemberChannel::Stereo,
        _ => PairMemberChannel::Unknown,
    }
}

fn fixed_member(
    name: PairMemberName,
    verification: PairMemberVerification,
    channels: Vec<PairMemberChannel>,
) -> PairMemberStatus {
    PairMemberStatus {
        name,
        verification,
        channels,
    }
}

fn unavailable_members(verification: PairMemberVerification) -> [PairMemberStatus; 2] {
    [
        fixed_member(
            PairMemberName::JblAuthentics300,
            verification,
            vec![PairMemberChannel::Unknown],
        ),
        fixed_member(
            PairMemberName::AuraStudio5,
            verification,
            vec![PairMemberChannel::Unknown],
        ),
    ]
}

/// Current live knowledge owned by this controller process.
///
/// `Linked` is the managed state after the last accepted START. Its action
/// evidence may be only an ATT acknowledgement; it is not by itself a 7951,
/// acoustic or BASS/BIG/BIS proof. A restart, lost bearer or external action
/// moves the state back to `Unknown`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ManagedLiveState {
    Unknown,
    Offline,
    Ready,
    Linking,
    Linked,
    Unlinking,
    Recovering,
    Degraded,
    ShuttingDown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControllerAction {
    Start,
    Stop,
    Shutdown,
    RecoverStop,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControllerActionOutcome {
    Accepted,
    AcceptedUnconfirmed,
    Idempotent,
    RejectedBeforeSend,
    OutcomeUnknown,
    PostconditionFailed,
}

/// Fixed, non-identifying reason suitable for CLI and Web UI projection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControllerFailure {
    PairConfigurationUnavailable,
    ExpectedPairNotConfigured,
    BackendRejectedBeforeSend,
    AuraInvalidConfiguration,
    AuraRuntimeUnavailable,
    AuraAdapterUnavailable,
    AuraDiscoveryUnavailable,
    AuraVerifiedAdvertisementNotFound,
    AuraDeviceConnectionFailed,
    AuraWakeProfileConnectFailed,
    AuraWakeFddfTimedOut,
    AuraWakeFddfInvalid,
    AuraWakeFddfUnavailable,
    AuraWakeProfileReleaseFailed,
    AuraGattProfileInvalid,
    AuraNotificationSetupFailed,
    AuraTransportNotReady,
    AuraNotificationQueueInvalid,
    AuraDisconnectFailed,
    AuraWriteUnknown,
    AuraAckTimeout,
    AuraAckChannelClosed,
    AuraUnexpectedAck,
    JblEnterOutcomeUnknown,
    JblExitOutcomeUnknown,
    JblBroadcastResultTimedOut,
    JblBroadcastResultUnavailable,
    JblBroadcastResultRejected,
    AuraStartOutcomeUnknown,
    BackendOutcomeUnknown,
    UnexpectedBackendLifecycle,
    MembershipPostconditionFailed,
    UnresolvedPriorAction,
    RecoveryNotAllowed,
    JournalUnavailable,
    JournalCommitFailed,
}

/// A sanitized controller mutation result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ControllerActionResult {
    action: ControllerAction,
    outcome: ControllerActionOutcome,
    managed_state: ManagedLiveState,
    evidence: Option<PairBackendEvidence>,
    failure: Option<ControllerFailure>,
    revision: u64,
}

impl ControllerActionResult {
    pub const fn action(self) -> ControllerAction {
        self.action
    }

    pub const fn outcome(self) -> ControllerActionOutcome {
        self.outcome
    }

    pub const fn managed_state(self) -> ManagedLiveState {
        self.managed_state
    }

    pub const fn evidence(self) -> Option<PairBackendEvidence> {
        self.evidence
    }

    pub const fn failure(self) -> Option<ControllerFailure> {
        self.failure
    }

    pub const fn revision(self) -> u64 {
        self.revision
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PairProbeError {
    Unavailable,
    InvalidResponse,
}

/// Read-only identity gate for the expected retained JBL + Aura pair.
pub(crate) trait PairConfigurationProbe {
    fn pair_configuration(&mut self) -> Result<PairConfigurationObservation, PairProbeError>;
}

/// Closed summary of the latest action in this process. It is intentionally
/// not restored from disk: after process restart status reports `None`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LastActionStatus {
    action: ControllerAction,
    outcome: ControllerActionOutcome,
    evidence: Option<PairBackendEvidence>,
    failure: Option<ControllerFailure>,
    revision: u64,
    age_ms: u64,
}

impl LastActionStatus {
    pub const fn action(self) -> ControllerAction {
        self.action
    }

    pub const fn outcome(self) -> ControllerActionOutcome {
        self.outcome
    }

    pub const fn evidence(self) -> Option<PairBackendEvidence> {
        self.evidence
    }

    pub const fn failure(self) -> Option<ControllerFailure> {
        self.failure
    }

    pub const fn revision(self) -> u64 {
        self.revision
    }

    pub const fn age_ms(self) -> u64 {
        self.age_ms
    }
}

#[derive(Debug, Clone, Copy)]
struct RecordedAction {
    result: ControllerActionResult,
    recorded_at: Instant,
}

/// Sanitized status snapshot.  No device identifier or raw diagnostic is
/// representable in this type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ControllerStatus {
    backend: PairBackendKind,
    pair_configuration: PairConfigurationState,
    members: [PairMemberStatus; 2],
    managed_state: ManagedLiveState,
    backend_health: Option<PairHealth>,
    unresolved_action: bool,
    consecutive_failures: u8,
    revision: u64,
    last_action: Option<LastActionStatus>,
}

impl ControllerStatus {
    pub const fn backend(&self) -> PairBackendKind {
        self.backend
    }

    pub const fn pair_configuration(&self) -> PairConfigurationState {
        self.pair_configuration
    }

    pub fn members(&self) -> &[PairMemberStatus; 2] {
        &self.members
    }

    pub const fn managed_state(&self) -> ManagedLiveState {
        self.managed_state
    }

    pub const fn backend_health(&self) -> Option<PairHealth> {
        self.backend_health
    }

    pub const fn has_unresolved_action(&self) -> bool {
        self.unresolved_action
    }

    pub const fn consecutive_failures(&self) -> u8 {
        self.consecutive_failures
    }

    pub const fn revision(&self) -> u64 {
        self.revision
    }

    pub const fn last_action(&self) -> Option<LastActionStatus> {
        self.last_action
    }
}

/// One in-process owner of an entire JBL + Aura backend.
///
/// Cross-process exclusion is supplied by the service/runtime layer.  This
/// type provides the second line of defence: an uncertain write is latched and
/// no later start/stop/shutdown reaches the backend until an explicit recovery
/// replaces or resets the controller.
pub(crate) struct PairController<B, P, J> {
    backend: B,
    probe: P,
    journal: J,
    managed_state: ManagedLiveState,
    unresolved_action: bool,
    consecutive_failures: u8,
    revision: u64,
    last_action: Option<RecordedAction>,
}

#[cfg(test)]
impl<B: PairBackend, P: PairConfigurationProbe> PairController<B, P, MemoryJournal> {
    pub fn new(backend: B, probe: P) -> Self {
        Self::with_journal(backend, probe, MemoryJournal::clean())
    }
}

impl<B: PairBackend, P: PairConfigurationProbe, J: UncertaintyJournal> PairController<B, P, J> {
    pub(crate) fn with_journal(backend: B, probe: P, journal: J) -> Self {
        let unresolved_action = journal.is_pending();
        Self {
            backend,
            probe,
            journal,
            managed_state: ManagedLiveState::Unknown,
            unresolved_action,
            consecutive_failures: u8::from(unresolved_action),
            revision: 0,
            last_action: None,
        }
    }

    pub fn status(&mut self) -> ControllerStatus {
        let previous_managed_state = self.managed_state;
        let (pair_configuration, members) = self.read_pair_configuration();
        let backend_health = self.backend.health().ok();

        let lost_backend =
            !matches!(self.managed_state, ManagedLiveState::Offline) && backend_health.is_none();
        let membership_not_confirmed = !matches!(pair_configuration, PairConfigurationState::Ready)
            && matches!(self.managed_state, ManagedLiveState::Linked);
        let contradictory_health = !self.health_matches_managed_state(backend_health);
        if lost_backend || membership_not_confirmed || contradictory_health {
            self.managed_state = ManagedLiveState::Unknown;
        }
        if self.managed_state != previous_managed_state {
            self.bump_revision();
        }

        ControllerStatus {
            backend: self.backend.kind(),
            pair_configuration,
            members,
            managed_state: self.managed_state,
            backend_health,
            unresolved_action: self.unresolved_action,
            consecutive_failures: self.consecutive_failures,
            revision: self.revision,
            last_action: self.last_action.map(last_action_status),
        }
    }

    pub fn start(&mut self) -> ControllerActionResult {
        self.mutate(ControllerAction::Start)
    }

    pub fn stop(&mut self) -> ControllerActionResult {
        self.mutate(ControllerAction::Stop)
    }

    pub fn shutdown(&mut self) -> ControllerActionResult {
        self.mutate(ControllerAction::Shutdown)
    }

    /// Explicit bounded normalization after an uncertain action or repeated
    /// pre-send failures. This is never invoked by `start`, `stop`, status, or
    /// backend failover automatically.
    pub fn recover_stop(&mut self) -> ControllerActionResult {
        self.mutate(ControllerAction::RecoverStop)
    }

    #[cfg(test)]
    fn into_parts(self) -> (B, P) {
        (self.backend, self.probe)
    }

    #[cfg(test)]
    fn into_parts_with_journal(self) -> (B, P, J) {
        (self.backend, self.probe, self.journal)
    }

    /// Current compare-and-mutate revision without performing a LAN or
    /// Bluetooth status read.
    pub const fn revision(&self) -> u64 {
        self.revision
    }

    pub(crate) fn teardown_transport_for_exit(&mut self) -> Result<(), PairBackendError> {
        self.backend.teardown_transport()
    }

    fn mutate(&mut self, action: ControllerAction) -> ControllerActionResult {
        let had_pending_uncertainty = self.unresolved_action;
        if self.unresolved_action && action != ControllerAction::RecoverStop {
            return self.finish(
                action,
                ControllerActionOutcome::RejectedBeforeSend,
                Some(ControllerFailure::UnresolvedPriorAction),
            );
        }

        if action == ControllerAction::RecoverStop
            && !self.unresolved_action
            && self.consecutive_failures < 2
        {
            return self.finish(
                action,
                ControllerActionOutcome::RejectedBeforeSend,
                Some(ControllerFailure::RecoveryNotAllowed),
            );
        }

        // Once the controller has already released its backend, shutdown is a
        // local no-op.  It must not depend on LAN availability or try to
        // resurrect the backend merely to shut it down again.
        if action == ControllerAction::Shutdown && self.managed_state == ManagedLiveState::Offline {
            return self.finish(action, ControllerActionOutcome::Idempotent, None);
        }

        match self.read_pair_configuration().0 {
            PairConfigurationState::Ready => {}
            PairConfigurationState::NotReady => {
                return self.reject_preflight(action, ControllerFailure::ExpectedPairNotConfigured);
            }
            PairConfigurationState::Unavailable => {
                return self
                    .reject_preflight(action, ControllerFailure::PairConfigurationUnavailable);
            }
        }

        if self.is_idempotent(action) {
            self.consecutive_failures = 0;
            return self.finish(action, ControllerActionOutcome::Idempotent, None);
        }

        if self.journal.mark_pending(journal_action(action)).is_err() {
            self.consecutive_failures = self.consecutive_failures.saturating_add(1);
            // A file journal can fail after publishing some or all of its
            // fail-closed marker. Mirror that conservative latch in memory so
            // this process cannot bypass what a restart would observe.
            if self.journal.is_pending() {
                self.unresolved_action = true;
                self.managed_state = ManagedLiveState::Unknown;
            }
            return self.finish(
                action,
                ControllerActionOutcome::RejectedBeforeSend,
                Some(ControllerFailure::JournalUnavailable),
            );
        }

        self.managed_state = match action {
            ControllerAction::Start => ManagedLiveState::Linking,
            ControllerAction::Stop => ManagedLiveState::Unlinking,
            ControllerAction::Shutdown => ManagedLiveState::ShuttingDown,
            ControllerAction::RecoverStop => ManagedLiveState::Recovering,
        };
        self.bump_revision();

        let attempted = {
            let mut transaction = PairBackendTransaction::new(&mut self.backend);
            match action {
                ControllerAction::Start => transaction.start(),
                ControllerAction::Stop => transaction.stop(),
                ControllerAction::Shutdown => transaction.shutdown(),
                ControllerAction::RecoverStop => transaction.recover_stop(),
            }
        };
        self.reduce_backend_result(action, attempted, had_pending_uncertainty)
    }

    fn reduce_backend_result(
        &mut self,
        action: ControllerAction,
        attempted: PairActionResult,
        had_pending_uncertainty: bool,
    ) -> ControllerActionResult {
        match attempted {
            PairActionResult::RejectedBeforeSend(failure) => {
                // A normal rejected-before-send result proves that this action
                // performed no role write, so its just-created pending marker
                // can be cleared. A recovery rejection cannot clear the older
                // uncertainty that authorized recovery in the first place.
                let may_clear = action != ControllerAction::RecoverStop || !had_pending_uncertainty;
                if may_clear && self.journal.clear().is_err() {
                    return self.journal_commit_failed(action);
                }
                if !may_clear {
                    self.unresolved_action = true;
                }
                self.consecutive_failures = self.consecutive_failures.saturating_add(1);
                self.managed_state = ManagedLiveState::Degraded;
                self.finish(
                    action,
                    ControllerActionOutcome::RejectedBeforeSend,
                    Some(controller_failure_from_backend(failure.reason())),
                )
            }
            PairActionResult::OutcomeUnknown(failure) => {
                self.consecutive_failures = self.consecutive_failures.saturating_add(1);
                self.unresolved_action = true;
                self.managed_state = ManagedLiveState::Unknown;
                self.finish(
                    action,
                    ControllerActionOutcome::OutcomeUnknown,
                    Some(controller_failure_from_unknown_backend(failure.reason())),
                )
            }
            PairActionResult::Accepted(receipt) => {
                let expected = match action {
                    ControllerAction::Start => PairLifecycle::Linked,
                    ControllerAction::Stop => PairLifecycle::Ready,
                    ControllerAction::Shutdown => PairLifecycle::ShuttingDown,
                    ControllerAction::RecoverStop => PairLifecycle::Ready,
                };
                if receipt.lifecycle() != expected {
                    self.consecutive_failures = self.consecutive_failures.saturating_add(1);
                    self.unresolved_action = true;
                    self.managed_state = ManagedLiveState::Unknown;
                    return self.finish(
                        action,
                        ControllerActionOutcome::OutcomeUnknown,
                        Some(ControllerFailure::UnexpectedBackendLifecycle),
                    );
                }

                if !matches!(
                    self.read_pair_configuration().0,
                    PairConfigurationState::Ready
                ) {
                    self.consecutive_failures = self.consecutive_failures.saturating_add(1);
                    self.unresolved_action = true;
                    self.managed_state = ManagedLiveState::Unknown;
                    return self.finish(
                        action,
                        ControllerActionOutcome::PostconditionFailed,
                        Some(ControllerFailure::MembershipPostconditionFailed),
                    );
                }

                if self.journal.clear().is_err() {
                    return self.journal_commit_failed(action);
                }

                self.consecutive_failures = 0;
                self.unresolved_action = false;
                self.managed_state = match action {
                    ControllerAction::Start => ManagedLiveState::Linked,
                    ControllerAction::Stop => ManagedLiveState::Ready,
                    ControllerAction::Shutdown => ManagedLiveState::Offline,
                    ControllerAction::RecoverStop => ManagedLiveState::Ready,
                };
                let evidence = receipt.evidence();
                let outcome = if evidence == PairBackendEvidence::BroadcastAcknowledgementOnly {
                    ControllerActionOutcome::AcceptedUnconfirmed
                } else {
                    ControllerActionOutcome::Accepted
                };
                self.finish_with_evidence(action, outcome, Some(evidence), None)
            }
        }
    }

    fn is_idempotent(&mut self, action: ControllerAction) -> bool {
        // Native ACK mode has no business-result or acoustic proof. A user
        // explicitly issuing START/STOP therefore always requests a fresh
        // device transaction, even when this process still projects the same
        // managed lifecycle and a healthy Aura control bearer.
        if self.backend.kind() == PairBackendKind::NativePair
            && matches!(action, ControllerAction::Start | ControllerAction::Stop)
        {
            return false;
        }
        let expected_state = match action {
            ControllerAction::Start => ManagedLiveState::Linked,
            ControllerAction::Stop => ManagedLiveState::Ready,
            ControllerAction::Shutdown => ManagedLiveState::Offline,
            ControllerAction::RecoverStop => return false,
        };
        if self.managed_state != expected_state {
            return false;
        }
        let expected_lifecycle = match action {
            ControllerAction::Start => PairLifecycle::Linked,
            ControllerAction::Stop => PairLifecycle::Ready,
            ControllerAction::Shutdown => PairLifecycle::Offline,
            ControllerAction::RecoverStop => return false,
        };
        self.backend.health().is_ok_and(|health| {
            health.lifecycle() == expected_lifecycle
                && health.level() == crate::backend::PairHealthLevel::Healthy
                && !health.has_reported_error()
        })
    }

    fn health_matches_managed_state(&self, health: Option<PairHealth>) -> bool {
        let healthy_lifecycle = health.filter(|snapshot| {
            snapshot.level() == crate::backend::PairHealthLevel::Healthy
                && !snapshot.has_reported_error()
        });
        match (
            self.managed_state,
            healthy_lifecycle.map(PairHealth::lifecycle),
        ) {
            (ManagedLiveState::Linked, Some(PairLifecycle::Linked))
            | (ManagedLiveState::Ready, Some(PairLifecycle::Ready))
            | (ManagedLiveState::Offline, _)
            | (ManagedLiveState::Unknown, _)
            | (ManagedLiveState::Linking, _)
            | (ManagedLiveState::Unlinking, _)
            | (ManagedLiveState::Recovering, _)
            | (ManagedLiveState::Degraded, _)
            | (ManagedLiveState::ShuttingDown, _) => true,
            (ManagedLiveState::Linked | ManagedLiveState::Ready, _) => false,
        }
    }

    fn reject_preflight(
        &mut self,
        action: ControllerAction,
        failure: ControllerFailure,
    ) -> ControllerActionResult {
        self.consecutive_failures = self.consecutive_failures.saturating_add(1);
        self.finish(
            action,
            ControllerActionOutcome::RejectedBeforeSend,
            Some(failure),
        )
    }

    fn journal_commit_failed(&mut self, action: ControllerAction) -> ControllerActionResult {
        self.consecutive_failures = self.consecutive_failures.saturating_add(1);
        self.unresolved_action = true;
        self.managed_state = ManagedLiveState::Unknown;
        self.finish(
            action,
            ControllerActionOutcome::OutcomeUnknown,
            Some(ControllerFailure::JournalCommitFailed),
        )
    }

    fn finish(
        &mut self,
        action: ControllerAction,
        outcome: ControllerActionOutcome,
        failure: Option<ControllerFailure>,
    ) -> ControllerActionResult {
        self.finish_with_evidence(action, outcome, None, failure)
    }

    fn finish_with_evidence(
        &mut self,
        action: ControllerAction,
        outcome: ControllerActionOutcome,
        evidence: Option<PairBackendEvidence>,
        failure: Option<ControllerFailure>,
    ) -> ControllerActionResult {
        self.bump_revision();
        let result = ControllerActionResult {
            action,
            outcome,
            managed_state: self.managed_state,
            evidence,
            failure,
            revision: self.revision,
        };
        self.last_action = Some(RecordedAction {
            result,
            recorded_at: Instant::now(),
        });
        result
    }

    fn read_pair_configuration(&mut self) -> (PairConfigurationState, [PairMemberStatus; 2]) {
        match self.probe.pair_configuration() {
            Ok(observation) => (
                if observation.exact_pair_configured {
                    PairConfigurationState::Ready
                } else {
                    PairConfigurationState::NotReady
                },
                observation.members,
            ),
            Err(_) => (
                PairConfigurationState::Unavailable,
                unavailable_members(PairMemberVerification::Unavailable),
            ),
        }
    }

    fn bump_revision(&mut self) {
        self.revision = self.revision.saturating_add(1);
    }
}

fn last_action_status(recorded: RecordedAction) -> LastActionStatus {
    LastActionStatus {
        action: recorded.result.action(),
        outcome: recorded.result.outcome(),
        evidence: recorded.result.evidence(),
        failure: recorded.result.failure(),
        revision: recorded.result.revision(),
        age_ms: u64::try_from(recorded.recorded_at.elapsed().as_millis()).unwrap_or(u64::MAX),
    }
}

const fn journal_action(action: ControllerAction) -> JournalAction {
    match action {
        ControllerAction::Start => JournalAction::Start,
        ControllerAction::Stop => JournalAction::Stop,
        ControllerAction::Shutdown => JournalAction::Shutdown,
        ControllerAction::RecoverStop => JournalAction::RecoverStop,
    }
}

const fn controller_failure_from_backend(error: PairBackendError) -> ControllerFailure {
    match error {
        PairBackendError::AuraInvalidConfiguration => ControllerFailure::AuraInvalidConfiguration,
        PairBackendError::AuraRuntimeUnavailable => ControllerFailure::AuraRuntimeUnavailable,
        PairBackendError::AuraAdapterUnavailable => ControllerFailure::AuraAdapterUnavailable,
        PairBackendError::AuraDiscoveryUnavailable => ControllerFailure::AuraDiscoveryUnavailable,
        PairBackendError::AuraVerifiedAdvertisementNotFound => {
            ControllerFailure::AuraVerifiedAdvertisementNotFound
        }
        PairBackendError::AuraDeviceConnectionFailed => {
            ControllerFailure::AuraDeviceConnectionFailed
        }
        PairBackendError::AuraWakeProfileConnectFailed => {
            ControllerFailure::AuraWakeProfileConnectFailed
        }
        PairBackendError::AuraWakeFddfTimedOut => ControllerFailure::AuraWakeFddfTimedOut,
        PairBackendError::AuraWakeFddfInvalid => ControllerFailure::AuraWakeFddfInvalid,
        PairBackendError::AuraWakeFddfUnavailable => ControllerFailure::AuraWakeFddfUnavailable,
        PairBackendError::AuraWakeProfileReleaseFailed => {
            ControllerFailure::AuraWakeProfileReleaseFailed
        }
        PairBackendError::AuraGattProfileInvalid => ControllerFailure::AuraGattProfileInvalid,
        PairBackendError::AuraNotificationSetupFailed => {
            ControllerFailure::AuraNotificationSetupFailed
        }
        PairBackendError::AuraTransportNotReady => ControllerFailure::AuraTransportNotReady,
        PairBackendError::AuraNotificationQueueInvalid => {
            ControllerFailure::AuraNotificationQueueInvalid
        }
        PairBackendError::AuraDisconnectFailed => ControllerFailure::AuraDisconnectFailed,
        PairBackendError::AuraWriteUnknown => ControllerFailure::AuraWriteUnknown,
        PairBackendError::AuraAcknowledgementTimedOut => ControllerFailure::AuraAckTimeout,
        PairBackendError::AuraAcknowledgementChannelClosed => {
            ControllerFailure::AuraAckChannelClosed
        }
        PairBackendError::AuraUnexpectedAcknowledgement => ControllerFailure::AuraUnexpectedAck,
        PairBackendError::JblEnterOutcomeUnknown => ControllerFailure::JblEnterOutcomeUnknown,
        PairBackendError::JblExitOutcomeUnknown => ControllerFailure::JblExitOutcomeUnknown,
        PairBackendError::JblBroadcastResultTimedOut => {
            ControllerFailure::JblBroadcastResultTimedOut
        }
        PairBackendError::JblBroadcastResultUnavailable => {
            ControllerFailure::JblBroadcastResultUnavailable
        }
        PairBackendError::JblBroadcastResultRejected => {
            ControllerFailure::JblBroadcastResultRejected
        }
        PairBackendError::AuraStartOutcomeUnknown => ControllerFailure::AuraStartOutcomeUnknown,
        #[cfg(test)]
        PairBackendError::InvalidSocketPath
        | PairBackendError::UntrustedSocket
        | PairBackendError::InvalidTimeout
        | PairBackendError::TimedOut
        | PairBackendError::ResponseTooLarge
        | PairBackendError::InvalidResponse
        | PairBackendError::InvalidLifecycle
        | PairBackendError::UnexpectedLifecycle => ControllerFailure::BackendRejectedBeforeSend,
        PairBackendError::Unavailable
        | PairBackendError::BackendReportedFailure
        | PairBackendError::RecoveryUnsupported
        | PairBackendError::BackendChangedDuringTransaction => {
            ControllerFailure::BackendRejectedBeforeSend
        }
    }
}

const fn controller_failure_from_unknown_backend(error: PairBackendError) -> ControllerFailure {
    match error {
        PairBackendError::AuraInvalidConfiguration => ControllerFailure::AuraInvalidConfiguration,
        PairBackendError::AuraRuntimeUnavailable => ControllerFailure::AuraRuntimeUnavailable,
        PairBackendError::AuraTransportNotReady => ControllerFailure::AuraTransportNotReady,
        PairBackendError::AuraNotificationQueueInvalid => {
            ControllerFailure::AuraNotificationQueueInvalid
        }
        PairBackendError::AuraDisconnectFailed => ControllerFailure::AuraDisconnectFailed,
        PairBackendError::AuraWriteUnknown => ControllerFailure::AuraWriteUnknown,
        PairBackendError::AuraAcknowledgementTimedOut => ControllerFailure::AuraAckTimeout,
        PairBackendError::AuraAcknowledgementChannelClosed => {
            ControllerFailure::AuraAckChannelClosed
        }
        PairBackendError::AuraUnexpectedAcknowledgement => ControllerFailure::AuraUnexpectedAck,
        PairBackendError::JblEnterOutcomeUnknown => ControllerFailure::JblEnterOutcomeUnknown,
        PairBackendError::JblExitOutcomeUnknown => ControllerFailure::JblExitOutcomeUnknown,
        PairBackendError::JblBroadcastResultTimedOut => {
            ControllerFailure::JblBroadcastResultTimedOut
        }
        PairBackendError::JblBroadcastResultUnavailable => {
            ControllerFailure::JblBroadcastResultUnavailable
        }
        PairBackendError::JblBroadcastResultRejected => {
            ControllerFailure::JblBroadcastResultRejected
        }
        PairBackendError::AuraStartOutcomeUnknown => ControllerFailure::AuraStartOutcomeUnknown,
        #[cfg(test)]
        PairBackendError::InvalidSocketPath
        | PairBackendError::UntrustedSocket
        | PairBackendError::InvalidTimeout
        | PairBackendError::TimedOut
        | PairBackendError::ResponseTooLarge
        | PairBackendError::InvalidResponse
        | PairBackendError::InvalidLifecycle
        | PairBackendError::UnexpectedLifecycle => ControllerFailure::BackendOutcomeUnknown,
        PairBackendError::Unavailable
        | PairBackendError::AuraAdapterUnavailable
        | PairBackendError::AuraDiscoveryUnavailable
        | PairBackendError::AuraVerifiedAdvertisementNotFound
        | PairBackendError::AuraDeviceConnectionFailed
        | PairBackendError::AuraWakeProfileConnectFailed
        | PairBackendError::AuraWakeFddfTimedOut
        | PairBackendError::AuraWakeFddfInvalid
        | PairBackendError::AuraWakeFddfUnavailable
        | PairBackendError::AuraWakeProfileReleaseFailed
        | PairBackendError::AuraGattProfileInvalid
        | PairBackendError::AuraNotificationSetupFailed
        | PairBackendError::BackendReportedFailure
        | PairBackendError::RecoveryUnsupported
        | PairBackendError::BackendChangedDuringTransaction => {
            ControllerFailure::BackendOutcomeUnknown
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::panic::AssertUnwindSafe;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    use crate::backend::{
        AuraControlTransport, PairActionFailure, PairActionReceipt, PairBackendError,
        PairBackendEvidence, PairHealth,
    };

    use super::*;
    use crate::journal::FileUncertaintyJournal;
    use crate::model::GroupMember;

    static JOURNAL_TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn temporary_journal_root(label: &str) -> std::path::PathBuf {
        let unique = JOURNAL_TEST_COUNTER.fetch_add(1, Ordering::Relaxed);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "jbl-aura-controller-journal-{label}-{}-{nanos}-{unique}",
            std::process::id()
        ));
        std::fs::create_dir(&root).expect("journal test root");
        root
    }

    struct Probe {
        replies: VecDeque<Result<PairConfigurationObservation, PairProbeError>>,
    }

    impl Probe {
        fn ready_reads(count: usize) -> Self {
            Self {
                replies: std::iter::repeat_n(Ok(PairConfigurationObservation::ready()), count)
                    .collect(),
            }
        }
    }

    impl PairConfigurationProbe for Probe {
        fn pair_configuration(&mut self) -> Result<PairConfigurationObservation, PairProbeError> {
            self.replies
                .pop_front()
                .unwrap_or(Err(PairProbeError::Unavailable))
        }
    }

    struct Backend {
        health: Result<PairHealth, PairBackendError>,
        actions: VecDeque<PairActionResult>,
        action_calls: usize,
    }

    impl Backend {
        fn healthy(lifecycle: PairLifecycle, actions: Vec<PairActionResult>) -> Self {
            Self {
                health: Ok(PairHealth::new(
                    PairBackendKind::LegacyV04WholePair,
                    lifecycle,
                    false,
                    AuraControlTransport::Le,
                )),
                actions: actions.into(),
                action_calls: 0,
            }
        }

        fn accepted(lifecycle: PairLifecycle) -> PairActionResult {
            PairActionResult::Accepted(PairActionReceipt::new(
                PairBackendKind::LegacyV04WholePair,
                lifecycle,
                PairBackendEvidence::LifecycleAcknowledgement,
                false,
            ))
        }

        fn rejected_before_send() -> PairActionResult {
            PairActionResult::RejectedBeforeSend(PairActionFailure::new(
                PairBackendKind::LegacyV04WholePair,
                PairBackendError::Unavailable,
                None,
            ))
        }

        fn reported_error(lifecycle: PairLifecycle, actions: Vec<PairActionResult>) -> Self {
            Self {
                health: Ok(PairHealth::new(
                    PairBackendKind::LegacyV04WholePair,
                    lifecycle,
                    true,
                    AuraControlTransport::Le,
                )),
                actions: actions.into(),
                action_calls: 0,
            }
        }
    }

    impl PairBackend for Backend {
        fn kind(&self) -> PairBackendKind {
            PairBackendKind::LegacyV04WholePair
        }

        fn health(&mut self) -> Result<PairHealth, PairBackendError> {
            self.health
        }

        fn start(&mut self) -> PairActionResult {
            self.action_calls += 1;
            self.actions.pop_front().expect("start result fixture")
        }

        fn stop(&mut self) -> PairActionResult {
            self.action_calls += 1;
            self.actions.pop_front().expect("stop result fixture")
        }

        fn shutdown(&mut self) -> PairActionResult {
            self.action_calls += 1;
            self.actions.pop_front().expect("shutdown result fixture")
        }

        fn recover_stop(&mut self) -> PairActionResult {
            self.action_calls += 1;
            self.actions
                .pop_front()
                .expect("recover-stop result fixture")
        }
    }

    struct NativeUnresolvedBackend {
        action_calls: usize,
    }

    impl PairBackend for NativeUnresolvedBackend {
        fn kind(&self) -> PairBackendKind {
            PairBackendKind::NativePair
        }

        fn health(&mut self) -> Result<PairHealth, PairBackendError> {
            Ok(PairHealth::new(
                PairBackendKind::NativePair,
                PairLifecycle::Linked,
                false,
                AuraControlTransport::Unresolved,
            ))
        }

        fn start(&mut self) -> PairActionResult {
            self.action_calls += 1;
            PairActionResult::Accepted(PairActionReceipt::new(
                PairBackendKind::NativePair,
                PairLifecycle::Linked,
                PairBackendEvidence::BroadcastAcknowledgementOnly,
                false,
            ))
        }

        fn stop(&mut self) -> PairActionResult {
            unreachable!("not used")
        }

        fn shutdown(&mut self) -> PairActionResult {
            unreachable!("not used")
        }
    }

    struct NativeHealthyBackend {
        lifecycle: PairLifecycle,
        action_calls: usize,
    }

    impl NativeHealthyBackend {
        fn accepted(lifecycle: PairLifecycle) -> PairActionResult {
            PairActionResult::Accepted(PairActionReceipt::new(
                PairBackendKind::NativePair,
                lifecycle,
                PairBackendEvidence::BroadcastAcknowledgementOnly,
                false,
            ))
        }
    }

    impl PairBackend for NativeHealthyBackend {
        fn kind(&self) -> PairBackendKind {
            PairBackendKind::NativePair
        }

        fn health(&mut self) -> Result<PairHealth, PairBackendError> {
            Ok(PairHealth::new(
                PairBackendKind::NativePair,
                self.lifecycle,
                false,
                AuraControlTransport::Le,
            ))
        }

        fn start(&mut self) -> PairActionResult {
            self.action_calls += 1;
            self.lifecycle = PairLifecycle::Linked;
            Self::accepted(PairLifecycle::Linked)
        }

        fn stop(&mut self) -> PairActionResult {
            self.action_calls += 1;
            self.lifecycle = PairLifecycle::Ready;
            Self::accepted(PairLifecycle::Ready)
        }

        fn shutdown(&mut self) -> PairActionResult {
            unreachable!("not used")
        }
    }

    struct CountingJournal {
        pending: bool,
        marks: usize,
        clears: usize,
    }

    impl UncertaintyJournal for CountingJournal {
        fn is_pending(&self) -> bool {
            self.pending
        }

        fn mark_pending(
            &mut self,
            _action: JournalAction,
        ) -> Result<(), crate::journal::JournalError> {
            self.pending = true;
            self.marks += 1;
            Ok(())
        }

        fn clear(&mut self) -> Result<(), crate::journal::JournalError> {
            self.pending = false;
            self.clears += 1;
            Ok(())
        }
    }

    #[test]
    fn start_requires_membership_before_touching_backend() {
        let backend = Backend::healthy(PairLifecycle::Ready, Vec::new());
        let probe = Probe {
            replies: VecDeque::from([Ok(PairConfigurationObservation::not_ready())]),
        };
        let mut controller = PairController::new(backend, probe);
        let result = controller.start();
        assert_eq!(
            result.failure(),
            Some(ControllerFailure::ExpectedPairNotConfigured)
        );
        let (backend, _) = controller.into_parts();
        assert_eq!(backend.action_calls, 0);
    }

    #[test]
    fn accepted_start_needs_a_fresh_membership_postcondition() {
        let backend = Backend::healthy(
            PairLifecycle::Ready,
            vec![Backend::accepted(PairLifecycle::Linked)],
        );
        let probe = Probe {
            replies: VecDeque::from([
                Ok(PairConfigurationObservation::ready()),
                Ok(PairConfigurationObservation::not_ready()),
            ]),
        };
        let mut controller = PairController::new(backend, probe);
        let result = controller.start();
        assert_eq!(
            result.outcome(),
            ControllerActionOutcome::PostconditionFailed
        );
        assert_eq!(result.managed_state(), ManagedLiveState::Unknown);
        assert_eq!(result.evidence(), None);
        assert_eq!(
            result.failure(),
            Some(ControllerFailure::MembershipPostconditionFailed)
        );
    }

    #[test]
    fn retained_membership_after_stop_is_the_expected_postcondition() {
        let backend = Backend::healthy(
            PairLifecycle::Linked,
            vec![Backend::accepted(PairLifecycle::Ready)],
        );
        let probe = Probe::ready_reads(2);
        let mut controller = PairController::new(backend, probe);
        let result = controller.stop();
        assert_eq!(result.outcome(), ControllerActionOutcome::Accepted);
        assert_eq!(result.managed_state(), ManagedLiveState::Ready);
    }

    #[test]
    fn uncertain_action_latches_and_blocks_every_later_write() {
        let unknown = PairActionResult::OutcomeUnknown(PairActionFailure::new(
            PairBackendKind::LegacyV04WholePair,
            PairBackendError::TimedOut,
            Some(PairLifecycle::Linking),
        ));
        let backend = Backend::healthy(PairLifecycle::Ready, vec![unknown]);
        let probe = Probe::ready_reads(4);
        let mut controller = PairController::new(backend, probe);

        assert_eq!(
            controller.start().outcome(),
            ControllerActionOutcome::OutcomeUnknown
        );
        let second = controller.stop();
        assert_eq!(
            second.failure(),
            Some(ControllerFailure::UnresolvedPriorAction)
        );
        let third = controller.shutdown();
        assert_eq!(
            third.failure(),
            Some(ControllerFailure::UnresolvedPriorAction)
        );
        let (backend, _) = controller.into_parts();
        assert_eq!(backend.action_calls, 1);
    }

    #[test]
    fn closed_unknown_reasons_survive_controller_projection() {
        for (backend_reason, controller_reason) in [
            (
                PairBackendError::AuraWriteUnknown,
                ControllerFailure::AuraWriteUnknown,
            ),
            (
                PairBackendError::AuraAcknowledgementTimedOut,
                ControllerFailure::AuraAckTimeout,
            ),
            (
                PairBackendError::AuraAcknowledgementChannelClosed,
                ControllerFailure::AuraAckChannelClosed,
            ),
            (
                PairBackendError::AuraUnexpectedAcknowledgement,
                ControllerFailure::AuraUnexpectedAck,
            ),
            (
                PairBackendError::JblEnterOutcomeUnknown,
                ControllerFailure::JblEnterOutcomeUnknown,
            ),
            (
                PairBackendError::JblExitOutcomeUnknown,
                ControllerFailure::JblExitOutcomeUnknown,
            ),
        ] {
            let unknown = PairActionResult::OutcomeUnknown(PairActionFailure::new(
                PairBackendKind::LegacyV04WholePair,
                backend_reason,
                Some(PairLifecycle::Linking),
            ));
            let backend = Backend::healthy(PairLifecycle::Ready, vec![unknown]);
            let probe = Probe::ready_reads(1);
            let mut controller = PairController::new(backend, probe);

            let result = controller.start();
            assert_eq!(result.outcome(), ControllerActionOutcome::OutcomeUnknown);
            assert_eq!(result.failure(), Some(controller_reason));
            assert_eq!(result.managed_state(), ManagedLiveState::Unknown);
        }
    }

    #[test]
    fn closed_rejected_reasons_survive_controller_projection() {
        for (backend_reason, controller_reason) in [
            (
                PairBackendError::AuraInvalidConfiguration,
                ControllerFailure::AuraInvalidConfiguration,
            ),
            (
                PairBackendError::AuraRuntimeUnavailable,
                ControllerFailure::AuraRuntimeUnavailable,
            ),
            (
                PairBackendError::AuraTransportNotReady,
                ControllerFailure::AuraTransportNotReady,
            ),
            (
                PairBackendError::AuraNotificationQueueInvalid,
                ControllerFailure::AuraNotificationQueueInvalid,
            ),
            (
                PairBackendError::AuraDisconnectFailed,
                ControllerFailure::AuraDisconnectFailed,
            ),
            (
                PairBackendError::AuraWriteUnknown,
                ControllerFailure::AuraWriteUnknown,
            ),
            (
                PairBackendError::AuraAcknowledgementTimedOut,
                ControllerFailure::AuraAckTimeout,
            ),
            (
                PairBackendError::AuraAcknowledgementChannelClosed,
                ControllerFailure::AuraAckChannelClosed,
            ),
            (
                PairBackendError::AuraUnexpectedAcknowledgement,
                ControllerFailure::AuraUnexpectedAck,
            ),
            (
                PairBackendError::JblBroadcastResultTimedOut,
                ControllerFailure::JblBroadcastResultTimedOut,
            ),
            (
                PairBackendError::JblBroadcastResultUnavailable,
                ControllerFailure::JblBroadcastResultUnavailable,
            ),
            (
                PairBackendError::JblBroadcastResultRejected,
                ControllerFailure::JblBroadcastResultRejected,
            ),
            (
                PairBackendError::AuraStartOutcomeUnknown,
                ControllerFailure::AuraStartOutcomeUnknown,
            ),
        ] {
            let rejected = PairActionResult::RejectedBeforeSend(PairActionFailure::new(
                PairBackendKind::LegacyV04WholePair,
                backend_reason,
                None,
            ));
            let backend = Backend::healthy(PairLifecycle::Ready, vec![rejected]);
            let probe = Probe::ready_reads(1);
            let mut controller = PairController::new(backend, probe);

            let result = controller.start();
            assert_eq!(
                result.outcome(),
                ControllerActionOutcome::RejectedBeforeSend
            );
            assert_eq!(result.failure(), Some(controller_reason));
        }
    }

    #[test]
    fn wake_stage_rejections_survive_controller_projection() {
        for (backend_reason, controller_reason) in [
            (
                PairBackendError::AuraWakeProfileConnectFailed,
                ControllerFailure::AuraWakeProfileConnectFailed,
            ),
            (
                PairBackendError::AuraWakeFddfTimedOut,
                ControllerFailure::AuraWakeFddfTimedOut,
            ),
            (
                PairBackendError::AuraWakeFddfInvalid,
                ControllerFailure::AuraWakeFddfInvalid,
            ),
            (
                PairBackendError::AuraWakeFddfUnavailable,
                ControllerFailure::AuraWakeFddfUnavailable,
            ),
            (
                PairBackendError::AuraWakeProfileReleaseFailed,
                ControllerFailure::AuraWakeProfileReleaseFailed,
            ),
        ] {
            assert_eq!(
                controller_failure_from_backend(backend_reason),
                controller_reason
            );
        }
    }

    #[test]
    fn legacy_same_session_managed_state_can_be_adopted_as_idempotent() {
        let backend = Backend::healthy(
            PairLifecycle::Linked,
            vec![Backend::accepted(PairLifecycle::Linked)],
        );
        let probe = Probe::ready_reads(4);
        let mut controller = PairController::new(backend, probe);

        // Backend health alone is insufficient because the process starts at
        // managed Unknown; one acknowledged action is still required.
        let accepted = controller.start();
        assert_eq!(accepted.outcome(), ControllerActionOutcome::Accepted);
        assert_eq!(
            accepted.evidence(),
            Some(PairBackendEvidence::LifecycleAcknowledgement)
        );
        assert_eq!(
            controller.start().outcome(),
            ControllerActionOutcome::Idempotent
        );
        let (backend, _) = controller.into_parts();
        assert_eq!(backend.action_calls, 1);
    }

    #[test]
    fn native_explicit_start_and_stop_never_use_managed_health_shortcuts() {
        for (managed_state, lifecycle, action, expected_state, initial_revision) in [
            (
                ManagedLiveState::Linked,
                PairLifecycle::Linked,
                ControllerAction::Start,
                ManagedLiveState::Linked,
                4,
            ),
            (
                ManagedLiveState::Ready,
                PairLifecycle::Ready,
                ControllerAction::Stop,
                ManagedLiveState::Ready,
                9,
            ),
        ] {
            let mut controller = PairController {
                backend: NativeHealthyBackend {
                    lifecycle,
                    action_calls: 0,
                },
                probe: Probe::ready_reads(2),
                journal: CountingJournal {
                    pending: false,
                    marks: 0,
                    clears: 0,
                },
                managed_state,
                unresolved_action: false,
                consecutive_failures: 0,
                revision: initial_revision,
                last_action: None,
            };

            let result = match action {
                ControllerAction::Start => controller.start(),
                ControllerAction::Stop => controller.stop(),
                ControllerAction::Shutdown | ControllerAction::RecoverStop => unreachable!(),
            };
            assert_eq!(controller.backend.action_calls, 1);
            assert_eq!(controller.journal.marks, 1);
            assert_eq!(controller.journal.clears, 1);
            assert!(!controller.journal.pending);
            assert_eq!(
                result.outcome(),
                ControllerActionOutcome::AcceptedUnconfirmed
            );
            assert_eq!(result.managed_state(), expected_state);
            assert_eq!(
                result.evidence(),
                Some(PairBackendEvidence::BroadcastAcknowledgementOnly)
            );
            assert_eq!(result.revision(), initial_revision + 2);
            assert_eq!(controller.revision(), initial_revision + 2);
            let last = controller
                .last_action
                .map(last_action_status)
                .expect("explicit action must be recorded");
            assert_eq!(last.action(), action);
            assert_eq!(last.outcome(), ControllerActionOutcome::AcceptedUnconfirmed);
            assert_eq!(
                last.evidence(),
                Some(PairBackendEvidence::BroadcastAcknowledgementOnly)
            );
            assert_eq!(last.failure(), None);
            assert_eq!(last.revision(), initial_revision + 2);
            assert!(last.age_ms() < 1_000);
        }
    }

    #[test]
    fn native_linked_status_survives_released_bearer_but_start_revalidates() {
        let backend = NativeUnresolvedBackend { action_calls: 0 };
        let probe = Probe::ready_reads(3);
        let mut controller = PairController {
            backend,
            probe,
            journal: MemoryJournal::clean(),
            managed_state: ManagedLiveState::Linked,
            unresolved_action: false,
            consecutive_failures: 0,
            revision: 4,
            last_action: None,
        };

        let status = controller.status();
        assert_eq!(status.managed_state(), ManagedLiveState::Linked);
        assert_eq!(controller.backend.action_calls, 0, "status is read-only");

        let result = controller.start();
        assert_eq!(
            result.outcome(),
            ControllerActionOutcome::AcceptedUnconfirmed
        );
        assert_eq!(
            result.evidence(),
            Some(PairBackendEvidence::BroadcastAcknowledgementOnly)
        );
        assert_eq!(result.managed_state(), ManagedLiveState::Linked);
        assert_eq!(controller.backend.action_calls, 1);
    }

    #[test]
    fn status_downgrades_lost_bearer_without_claiming_membership_disappeared() {
        let mut backend = Backend::healthy(
            PairLifecycle::Ready,
            vec![Backend::accepted(PairLifecycle::Linked)],
        );
        let probe = Probe::ready_reads(4);
        let mut controller = PairController::new(backend, probe);
        assert_eq!(controller.start().managed_state(), ManagedLiveState::Linked);

        backend = controller.into_parts().0;
        backend.health = Err(PairBackendError::Unavailable);
        let probe = Probe::ready_reads(1);
        let mut controller = PairController {
            backend,
            probe,
            journal: MemoryJournal::clean(),
            managed_state: ManagedLiveState::Linked,
            unresolved_action: false,
            consecutive_failures: 0,
            revision: 1,
            last_action: None,
        };
        let status = controller.status();
        assert_eq!(status.pair_configuration(), PairConfigurationState::Ready);
        assert_eq!(status.managed_state(), ManagedLiveState::Unknown);
    }

    #[test]
    fn status_downgrades_a_backend_lifecycle_that_contradicts_managed_linked() {
        let backend = Backend::healthy(PairLifecycle::Ready, Vec::new());
        let probe = Probe::ready_reads(1);
        let mut controller = PairController {
            backend,
            probe,
            journal: MemoryJournal::clean(),
            managed_state: ManagedLiveState::Linked,
            unresolved_action: false,
            consecutive_failures: 0,
            revision: 7,
            last_action: None,
        };
        let status = controller.status();
        assert_eq!(status.managed_state(), ManagedLiveState::Unknown);
        assert_eq!(status.revision(), 8);
    }

    #[test]
    fn reported_backend_error_cannot_preserve_or_idempotently_adopt_linked() {
        let backend = Backend::reported_error(
            PairLifecycle::Linked,
            vec![
                Backend::accepted(PairLifecycle::Linked),
                Backend::accepted(PairLifecycle::Linked),
            ],
        );
        let probe = Probe::ready_reads(5);
        let mut controller = PairController::new(backend, probe);

        assert_eq!(controller.start().managed_state(), ManagedLiveState::Linked);
        assert_eq!(
            controller.status().managed_state(),
            ManagedLiveState::Unknown
        );
        assert_eq!(
            controller.start().outcome(),
            ControllerActionOutcome::Accepted,
            "degraded health must not return a local idempotent success"
        );
        let (backend, _) = controller.into_parts();
        assert_eq!(backend.action_calls, 2);
    }

    #[test]
    fn unavailable_membership_probe_downgrades_managed_linked() {
        let backend = Backend::healthy(
            PairLifecycle::Linked,
            vec![Backend::accepted(PairLifecycle::Linked)],
        );
        let probe = Probe {
            replies: VecDeque::from([
                Ok(PairConfigurationObservation::ready()),
                Ok(PairConfigurationObservation::ready()),
                Err(PairProbeError::Unavailable),
            ]),
        };
        let mut controller = PairController::new(backend, probe);
        assert_eq!(controller.start().managed_state(), ManagedLiveState::Linked);
        let status = controller.status();
        assert_eq!(
            status.pair_configuration(),
            PairConfigurationState::Unavailable
        );
        assert_eq!(status.managed_state(), ManagedLiveState::Unknown);
    }

    #[test]
    fn repeated_shutdown_after_offline_is_local_and_network_independent() {
        let backend = Backend::healthy(PairLifecycle::Ready, Vec::new());
        let probe = Probe {
            replies: VecDeque::new(),
        };
        let mut controller = PairController {
            backend,
            probe,
            journal: MemoryJournal::clean(),
            managed_state: ManagedLiveState::Offline,
            unresolved_action: false,
            consecutive_failures: 0,
            revision: 1,
            last_action: None,
        };
        let result = controller.shutdown();
        assert_eq!(result.outcome(), ControllerActionOutcome::Idempotent);
        let (backend, _) = controller.into_parts();
        assert_eq!(backend.action_calls, 0);
    }

    #[test]
    fn explicit_recover_stop_crosses_unknown_latch_once_and_clears_it_on_ack() {
        let unknown = PairActionResult::OutcomeUnknown(PairActionFailure::new(
            PairBackendKind::LegacyV04WholePair,
            PairBackendError::TimedOut,
            Some(PairLifecycle::Linking),
        ));
        let backend = Backend::healthy(
            PairLifecycle::Ready,
            vec![unknown, Backend::accepted(PairLifecycle::Ready)],
        );
        let probe = Probe::ready_reads(5);
        let mut controller = PairController::new(backend, probe);

        assert_eq!(
            controller.start().outcome(),
            ControllerActionOutcome::OutcomeUnknown
        );
        let recovered = controller.recover_stop();
        assert_eq!(recovered.outcome(), ControllerActionOutcome::Accepted);
        assert_eq!(recovered.action(), ControllerAction::RecoverStop);
        assert_eq!(recovered.managed_state(), ManagedLiveState::Ready);
        assert!(!controller.status().has_unresolved_action());
        let (backend, _) = controller.into_parts();
        assert_eq!(backend.action_calls, 2);
    }

    #[test]
    fn recovery_is_not_an_ordinary_healthy_state_action() {
        let backend = Backend::healthy(PairLifecycle::Ready, Vec::new());
        let probe = Probe::ready_reads(1);
        let mut controller = PairController::new(backend, probe);
        let result = controller.recover_stop();
        assert_eq!(
            result.failure(),
            Some(ControllerFailure::RecoveryNotAllowed)
        );
        let (backend, _) = controller.into_parts();
        assert_eq!(backend.action_calls, 0);
    }

    #[test]
    fn one_group_observation_projects_only_fixed_members_and_allowlisted_channels() {
        let observation = PairConfigurationObservation::from_group_status(GroupStatus {
            expected_pair_configured: false,
            disabled: Some(false),
            member_count: 2,
            members: vec![
                GroupMember {
                    name: "JBL Authentics 300".to_string(),
                    channels: vec!["stereo".to_string(), "stereo".to_string()],
                },
                GroupMember {
                    name: "attacker supplied name and identifier".to_string(),
                    channels: vec!["private-channel".to_string()],
                },
            ],
            error: Some("expected_pair_not_configured"),
        });
        assert!(!observation.exact_pair_configured);
        assert_eq!(
            observation.members[0],
            fixed_member(
                PairMemberName::JblAuthentics300,
                PairMemberVerification::Verified,
                vec![PairMemberChannel::Stereo],
            )
        );
        assert_eq!(
            observation.members[1],
            fixed_member(
                PairMemberName::AuraStudio5,
                PairMemberVerification::NotVerified,
                vec![PairMemberChannel::Unknown],
            )
        );
    }

    #[test]
    fn latest_action_is_process_local_closed_and_monotonic() {
        let backend = Backend::healthy(
            PairLifecycle::Linked,
            vec![Backend::accepted(PairLifecycle::Linked)],
        );
        let probe = Probe::ready_reads(5);
        let mut controller = PairController::new(backend, probe);
        assert!(controller.status().last_action().is_none());

        let result = controller.start();
        let first = controller.status().last_action().expect("latest action");
        std::thread::sleep(std::time::Duration::from_millis(2));
        let second = controller.status().last_action().expect("latest action");
        assert_eq!(first.action(), result.action());
        assert_eq!(first.outcome(), result.outcome());
        assert_eq!(first.evidence(), result.evidence());
        assert_eq!(first.failure(), result.failure());
        assert_eq!(first.revision(), result.revision());
        assert!(second.age_ms() >= first.age_ms());

        let restarted = PairController::new(
            Backend::healthy(PairLifecycle::Ready, Vec::new()),
            Probe::ready_reads(1),
        );
        assert!(restarted.last_action.is_none());
    }

    #[test]
    fn clean_threshold_recovery_rejection_clears_its_new_marker() {
        let backend = Backend::healthy(PairLifecycle::Ready, vec![Backend::rejected_before_send()]);
        let probe = Probe {
            replies: VecDeque::from([
                Ok(PairConfigurationObservation::not_ready()),
                Ok(PairConfigurationObservation::not_ready()),
                Ok(PairConfigurationObservation::ready()),
            ]),
        };
        let journal = MemoryJournal::clean();
        let mut controller = PairController::with_journal(backend, probe, journal);
        assert_eq!(
            controller.start().outcome(),
            ControllerActionOutcome::RejectedBeforeSend
        );
        assert_eq!(
            controller.start().outcome(),
            ControllerActionOutcome::RejectedBeforeSend
        );
        let recovery = controller.recover_stop();
        assert_eq!(
            recovery.outcome(),
            ControllerActionOutcome::RejectedBeforeSend
        );
        assert!(!controller.unresolved_action);
        let (_, _, journal) = controller.into_parts_with_journal();
        assert!(!journal.is_pending());
    }

    #[test]
    fn pending_restart_recovery_rejection_preserves_disk_and_memory_latch() {
        let backend = Backend::healthy(PairLifecycle::Ready, vec![Backend::rejected_before_send()]);
        let probe = Probe::ready_reads(1);
        let journal = MemoryJournal::pending(JournalAction::Start);
        let mut controller = PairController::with_journal(backend, probe, journal);
        assert!(controller.unresolved_action);
        assert_eq!(
            controller.start().failure(),
            Some(ControllerFailure::UnresolvedPriorAction)
        );
        let recovery = controller.recover_stop();
        assert_eq!(
            recovery.outcome(),
            ControllerActionOutcome::RejectedBeforeSend
        );
        assert!(controller.unresolved_action);
        let (_, _, journal) = controller.into_parts_with_journal();
        assert!(journal.is_pending());
    }

    #[test]
    fn clean_directory_sync_failure_maps_unknown_and_restart_blocks_writes() {
        let root = temporary_journal_root("clean-directory-sync");
        let journal = FileUncertaintyJournal::open_under(&root).expect("clean journal");
        // mark_pending commits the independent marker and compatibility
        // snapshot first; the third directory sync is the clean snapshot.
        journal.fail_directory_sync_on_nth_for_test(3);
        let backend = Backend::healthy(
            PairLifecycle::Ready,
            vec![Backend::accepted(PairLifecycle::Linked)],
        );
        let probe = Probe::ready_reads(2);
        let mut controller = PairController::with_journal(backend, probe, journal);

        let result = controller.start();
        assert_eq!(result.outcome(), ControllerActionOutcome::OutcomeUnknown);
        assert_eq!(
            result.failure(),
            Some(ControllerFailure::JournalCommitFailed)
        );
        assert_eq!(result.managed_state(), ManagedLiveState::Unknown);
        assert!(controller.unresolved_action);
        let (backend, _, journal) = controller.into_parts_with_journal();
        assert_eq!(backend.action_calls, 1);
        assert!(journal.is_pending());
        drop(journal);

        let reopened = FileUncertaintyJournal::open_under(&root).expect("reopen pending");
        assert!(reopened.is_pending());
        let backend = Backend::healthy(
            PairLifecycle::Ready,
            vec![Backend::accepted(PairLifecycle::Linked)],
        );
        let probe = Probe::ready_reads(1);
        let mut restarted = PairController::with_journal(backend, probe, reopened);
        let rejected = restarted.start();
        assert_eq!(
            rejected.outcome(),
            ControllerActionOutcome::RejectedBeforeSend
        );
        assert_eq!(
            rejected.failure(),
            Some(ControllerFailure::UnresolvedPriorAction)
        );
        let (backend, _, journal) = restarted.into_parts_with_journal();
        assert_eq!(backend.action_calls, 0);
        assert!(journal.is_pending());
        drop(journal);
        std::fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn pending_directory_sync_failure_stays_pre_send_but_latches_controller() {
        let root = temporary_journal_root("pending-directory-sync");
        let journal = FileUncertaintyJournal::open_under(&root).expect("clean journal");
        journal.fail_directory_sync_on_nth_for_test(1);
        let backend = Backend::healthy(
            PairLifecycle::Ready,
            vec![Backend::accepted(PairLifecycle::Linked)],
        );
        let probe = Probe::ready_reads(1);
        let mut controller = PairController::with_journal(backend, probe, journal);

        let result = controller.start();
        assert_eq!(
            result.outcome(),
            ControllerActionOutcome::RejectedBeforeSend
        );
        assert_eq!(
            result.failure(),
            Some(ControllerFailure::JournalUnavailable)
        );
        assert_eq!(result.managed_state(), ManagedLiveState::Unknown);
        assert!(controller.unresolved_action);
        let (backend, _, journal) = controller.into_parts_with_journal();
        assert_eq!(backend.action_calls, 0);
        assert!(journal.is_pending());
        drop(journal);

        let reopened = FileUncertaintyJournal::open_under(&root).expect("reopen pending");
        assert!(reopened.is_pending());
        drop(reopened);
        std::fs::remove_dir_all(root).expect("cleanup");
    }

    struct CrashBackend {
        action_calls: usize,
    }

    impl PairBackend for CrashBackend {
        fn kind(&self) -> PairBackendKind {
            PairBackendKind::NativePair
        }

        fn health(&mut self) -> Result<PairHealth, PairBackendError> {
            Ok(PairHealth::new(
                self.kind(),
                PairLifecycle::Ready,
                false,
                AuraControlTransport::Le,
            ))
        }

        fn start(&mut self) -> PairActionResult {
            self.action_calls += 1;
            panic!("synthetic crash after a START phase write")
        }

        fn stop(&mut self) -> PairActionResult {
            self.action_calls += 1;
            panic!("synthetic crash after a STOP phase write")
        }

        fn shutdown(&mut self) -> PairActionResult {
            unreachable!("not used")
        }
    }

    #[test]
    fn crashes_at_each_device_write_boundary_block_ordinary_writes_after_restart() {
        for (phase, action) in [
            ("jbl-enter", ControllerAction::Start),
            ("jbl-broadcast-start", ControllerAction::Start),
            ("aura-on", ControllerAction::Start),
            ("aura-off", ControllerAction::Stop),
            ("jbl-broadcast-stop", ControllerAction::Stop),
            ("jbl-exit", ControllerAction::Stop),
        ] {
            let root = temporary_journal_root(phase);
            let journal = FileUncertaintyJournal::open_under(&root).expect("clean journal");
            let backend = CrashBackend { action_calls: 0 };
            let probe = Probe::ready_reads(1);
            let mut controller = PairController::with_journal(backend, probe, journal);
            let crashed = std::panic::catch_unwind(AssertUnwindSafe(|| match action {
                ControllerAction::Start => {
                    let _ = controller.start();
                }
                ControllerAction::Stop => {
                    let _ = controller.stop();
                }
                ControllerAction::Shutdown | ControllerAction::RecoverStop => unreachable!(),
            }));
            assert!(crashed.is_err(), "{phase} fixture must terminate abruptly");
            drop(controller);

            let reopened =
                FileUncertaintyJournal::open_under(&root).expect("pending journal must reopen");
            assert!(reopened.is_pending(), "{phase} must remain pending");
            let backend = Backend::healthy(
                PairLifecycle::Ready,
                vec![Backend::accepted(PairLifecycle::Linked)],
            );
            let probe = Probe::ready_reads(1);
            let mut restarted = PairController::with_journal(backend, probe, reopened);
            assert_eq!(
                restarted.start().failure(),
                Some(ControllerFailure::UnresolvedPriorAction),
                "{phase} restart must not issue an ordinary write"
            );
            let (backend, _, journal) = restarted.into_parts_with_journal();
            assert_eq!(backend.action_calls, 0);
            assert!(journal.is_pending());
            drop(journal);
            std::fs::remove_dir_all(root).expect("cleanup");
        }
    }
}
