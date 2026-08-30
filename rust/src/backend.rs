use std::fmt;

/// The implementation that owns a complete JBL + Aura lifecycle transaction.
///
/// `LegacyV04WholePair` is deliberately not an Aura-only backend. Its four
/// local socket commands already coordinate both speakers. `NativePair` is the
/// future implementation that may internally combine the verified JBL HTTPS
/// transport with a native BlueZ Aura transport.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PairBackendKind {
    LegacyV04WholePair,
    NativePair,
}

/// Sanitized lifecycle shared by all whole-pair backends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PairLifecycle {
    Offline,
    Initializing,
    Connecting,
    Ready,
    Linking,
    Linked,
    Unlinking,
    Degraded,
    Recovering,
    ShuttingDown,
    Failed,
}

/// Coarse health projection that never carries a raw backend diagnostic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PairHealthLevel {
    Healthy,
    Transitioning,
    Degraded,
    Unavailable,
}

impl PairLifecycle {
    pub const fn health_level(self) -> PairHealthLevel {
        match self {
            Self::Ready | Self::Linked => PairHealthLevel::Healthy,
            Self::Initializing
            | Self::Connecting
            | Self::Linking
            | Self::Unlinking
            | Self::Recovering
            | Self::ShuttingDown => PairHealthLevel::Transitioning,
            Self::Degraded => PairHealthLevel::Degraded,
            Self::Offline | Self::Failed => PairHealthLevel::Unavailable,
        }
    }
}

/// Allowlisted Aura transport projection from the legacy status response.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuraControlTransport {
    Le,
    BrEdr,
    Unresolved,
    Unknown,
}

/// Privacy-safe projection of the successful Aura bearer acquisition path.
///
/// This intentionally records no address, scan count, timestamp, or raw
/// transport diagnostic. Legacy and unavailable bearers remain `Unresolved`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuraAcquisitionRoute {
    StableDirect,
    A2dpWakeThenStable,
    FreshLe,
    Unresolved,
}

/// A sanitized whole-pair health snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PairHealth {
    backend: PairBackendKind,
    lifecycle: PairLifecycle,
    level: PairHealthLevel,
    reported_error: bool,
    aura_transport: AuraControlTransport,
    aura_acquisition_route: AuraAcquisitionRoute,
}

impl PairHealth {
    #[cfg(test)]
    pub(crate) const fn new(
        backend: PairBackendKind,
        lifecycle: PairLifecycle,
        reported_error: bool,
        aura_transport: AuraControlTransport,
    ) -> Self {
        Self::new_with_route(
            backend,
            lifecycle,
            reported_error,
            aura_transport,
            AuraAcquisitionRoute::Unresolved,
        )
    }

    pub(crate) const fn new_with_route(
        backend: PairBackendKind,
        lifecycle: PairLifecycle,
        reported_error: bool,
        aura_transport: AuraControlTransport,
        aura_acquisition_route: AuraAcquisitionRoute,
    ) -> Self {
        let level = if reported_error
            && matches!(lifecycle, PairLifecycle::Ready | PairLifecycle::Linked)
        {
            PairHealthLevel::Degraded
        } else {
            lifecycle.health_level()
        };
        Self {
            backend,
            lifecycle,
            level,
            reported_error,
            aura_transport,
            aura_acquisition_route,
        }
    }

    pub const fn backend(self) -> PairBackendKind {
        self.backend
    }

    pub const fn lifecycle(self) -> PairLifecycle {
        self.lifecycle
    }

    pub const fn level(self) -> PairHealthLevel {
        self.level
    }

    pub const fn has_reported_error(self) -> bool {
        self.reported_error
    }

    pub const fn aura_transport(self) -> AuraControlTransport {
        self.aura_transport
    }

    pub const fn aura_acquisition_route(self) -> AuraAcquisitionRoute {
        self.aura_acquisition_route
    }
}

/// Strongest evidence carried by a lifecycle reply.
///
/// No variant is, by itself, acoustic evidence. In particular, a broadcast
/// acknowledgement proves only the ATT write response, while a business
/// notification proves the matching 7951 application result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PairBackendEvidence {
    LocalSessionState,
    LifecycleAcknowledgement,
    BroadcastAcknowledgementOnly,
    BroadcastBusinessNotification,
}

/// Sanitized result of one whole-pair lifecycle request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PairActionReceipt {
    backend: PairBackendKind,
    lifecycle: PairLifecycle,
    evidence: PairBackendEvidence,
    idempotent: bool,
}

impl PairActionReceipt {
    pub(crate) const fn new(
        backend: PairBackendKind,
        lifecycle: PairLifecycle,
        evidence: PairBackendEvidence,
        idempotent: bool,
    ) -> Self {
        Self {
            backend,
            lifecycle,
            evidence,
            idempotent,
        }
    }

    pub const fn backend(self) -> PairBackendKind {
        self.backend
    }

    pub const fn lifecycle(self) -> PairLifecycle {
        self.lifecycle
    }

    pub const fn evidence(self) -> PairBackendEvidence {
        self.evidence
    }

    #[cfg(test)]
    pub const fn is_idempotent(self) -> bool {
        self.idempotent
    }
}

/// Sanitized evidence attached to a lifecycle action that did not produce an
/// accepted receipt.
///
/// `observed_lifecycle` is populated only when a syntactically valid,
/// allowlisted lifecycle was present in a backend reply. It is evidence about
/// the local supervisor reply, not proof of device topology or audible output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PairActionFailure {
    backend: PairBackendKind,
    reason: PairBackendError,
    observed_lifecycle: Option<PairLifecycle>,
}

impl PairActionFailure {
    pub(crate) const fn new(
        backend: PairBackendKind,
        reason: PairBackendError,
        observed_lifecycle: Option<PairLifecycle>,
    ) -> Self {
        Self {
            backend,
            reason,
            observed_lifecycle,
        }
    }

    pub const fn backend(self) -> PairBackendKind {
        self.backend
    }

    pub const fn observed_lifecycle(self) -> Option<PairLifecycle> {
        self.observed_lifecycle
    }

    pub(crate) const fn reason(self) -> PairBackendError {
        self.reason
    }
}

/// Closed result of one whole-pair lifecycle action.
///
/// `RejectedBeforeSend` is reserved for failures known to occur before the
/// backend attempted to write any command bytes. Once a write attempt begins,
/// every timeout, disconnect, malformed reply, negative acknowledgement or
/// post-state mismatch is `OutcomeUnknown`; callers must not retry or switch
/// backends solely because of its reason.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[must_use]
pub enum PairActionResult {
    Accepted(PairActionReceipt),
    RejectedBeforeSend(PairActionFailure),
    OutcomeUnknown(PairActionFailure),
}

impl PairActionResult {
    pub const fn backend(self) -> PairBackendKind {
        match self {
            Self::Accepted(receipt) => receipt.backend(),
            Self::RejectedBeforeSend(failure) | Self::OutcomeUnknown(failure) => failure.backend(),
        }
    }

    #[cfg(test)]
    pub const fn receipt(self) -> Option<PairActionReceipt> {
        match self {
            Self::Accepted(receipt) => Some(receipt),
            Self::RejectedBeforeSend(_) | Self::OutcomeUnknown(_) => None,
        }
    }

    pub const fn observed_lifecycle(self) -> Option<PairLifecycle> {
        match self {
            Self::Accepted(receipt) => Some(receipt.lifecycle()),
            Self::RejectedBeforeSend(failure) | Self::OutcomeUnknown(failure) => {
                failure.observed_lifecycle()
            }
        }
    }
}

/// Safe, non-identifying failures for the local whole-pair backend boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PairBackendError {
    #[cfg(test)]
    InvalidSocketPath,
    #[cfg(test)]
    UntrustedSocket,
    #[cfg(test)]
    InvalidTimeout,
    Unavailable,
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
    AuraWriteUnknown,
    AuraAcknowledgementTimedOut,
    AuraAcknowledgementChannelClosed,
    AuraUnexpectedAcknowledgement,
    JblEnterOutcomeUnknown,
    JblExitOutcomeUnknown,
    JblBroadcastResultTimedOut,
    JblBroadcastResultUnavailable,
    JblBroadcastResultRejected,
    AuraStartOutcomeUnknown,
    #[cfg(test)]
    TimedOut,
    #[cfg(test)]
    ResponseTooLarge,
    #[cfg(test)]
    InvalidResponse,
    BackendReportedFailure,
    #[cfg(test)]
    InvalidLifecycle,
    #[cfg(test)]
    UnexpectedLifecycle,
    RecoveryUnsupported,
    BackendChangedDuringTransaction,
}

impl fmt::Display for PairBackendError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            #[cfg(test)]
            Self::InvalidSocketPath => "legacy backend socket path must be explicit and absolute",
            #[cfg(test)]
            Self::UntrustedSocket => "legacy backend socket failed local trust checks",
            #[cfg(test)]
            Self::InvalidTimeout => "legacy backend timeout must be greater than zero",
            Self::Unavailable => "pair backend is unavailable",
            Self::AuraAdapterUnavailable => "Aura Bluetooth adapter is unavailable",
            Self::AuraDiscoveryUnavailable => "Aura Bluetooth discovery is unavailable",
            Self::AuraVerifiedAdvertisementNotFound => {
                "no verified Aura FDDF advertisement was observed"
            }
            Self::AuraDeviceConnectionFailed => "verified Aura LE connection failed",
            Self::AuraWakeProfileConnectFailed => "Aura wake A2DP profile connection failed",
            Self::AuraWakeFddfTimedOut => "Aura wake FDDF observation timed out",
            Self::AuraWakeFddfInvalid => "Aura wake FDDF observation was invalid",
            Self::AuraWakeFddfUnavailable => "Aura wake FDDF observation was unavailable",
            Self::AuraWakeProfileReleaseFailed => "Aura wake A2DP profile release failed",
            Self::AuraGattProfileInvalid => "Aura vendor GATT profile was unavailable or invalid",
            Self::AuraNotificationSetupFailed => "Aura notification setup failed",
            Self::AuraWriteUnknown => "Aura write outcome is unknown",
            Self::AuraAcknowledgementTimedOut => "Aura acknowledgement timed out",
            Self::AuraAcknowledgementChannelClosed => "Aura acknowledgement channel closed",
            Self::AuraUnexpectedAcknowledgement => "Aura acknowledgement was unexpected",
            Self::JblEnterOutcomeUnknown => "JBL enter outcome is unknown",
            Self::JblExitOutcomeUnknown => "JBL exit outcome is unknown",
            Self::JblBroadcastResultTimedOut => "JBL broadcast result timed out",
            Self::JblBroadcastResultUnavailable => "JBL broadcast result is unavailable",
            Self::JblBroadcastResultRejected => "JBL broadcast result was rejected",
            Self::AuraStartOutcomeUnknown => "Aura start outcome is unknown",
            #[cfg(test)]
            Self::TimedOut => "pair backend request timed out",
            #[cfg(test)]
            Self::ResponseTooLarge => "pair backend response exceeded the size limit",
            #[cfg(test)]
            Self::InvalidResponse => "pair backend returned an invalid response",
            Self::BackendReportedFailure => "pair backend reported a lifecycle command failure",
            #[cfg(test)]
            Self::InvalidLifecycle => "pair backend returned an unknown lifecycle",
            #[cfg(test)]
            Self::UnexpectedLifecycle => "pair backend returned an unexpected post-state",
            Self::RecoveryUnsupported => "pair backend does not support explicit recovery",
            Self::BackendChangedDuringTransaction => {
                "pair backend changed during one lifecycle transaction"
            }
        };
        formatter.write_str(message)
    }
}

impl std::error::Error for PairBackendError {}

/// A complete JBL + Aura backend.
///
/// Every lifecycle method owns the whole pair operation. A caller must not use
/// a legacy implementation for one speaker and a native implementation for the
/// other speaker.
pub trait PairBackend {
    fn kind(&self) -> PairBackendKind;
    fn health(&mut self) -> Result<PairHealth, PairBackendError>;
    fn start(&mut self) -> PairActionResult;
    fn stop(&mut self) -> PairActionResult;
    fn shutdown(&mut self) -> PairActionResult;

    /// Explicitly tears down uncertain local transport state, reconnects and
    /// performs the safe receiver-first STOP normalization.  Normal action
    /// retry/failover code must never call this method automatically.
    fn recover_stop(&mut self) -> PairActionResult {
        PairActionResult::RejectedBeforeSend(PairActionFailure::new(
            self.kind(),
            PairBackendError::RecoveryUnsupported,
            None,
        ))
    }

    /// Releases process-owned transport resources without sending a JBL or
    /// Aura role command. It is safe to call during exit even when a prior
    /// action is outcome-unknown; it must not clear that uncertainty.
    fn teardown_transport(&mut self) -> Result<(), PairBackendError> {
        Ok(())
    }
}

/// Pins one backend implementation for a complete controller transaction.
///
/// The guard checks the backend kind before and after every call. If a future
/// selector wants to fail over, it must end the uncertain transaction first,
/// re-read safe state, and begin a new transaction explicitly.
pub struct PairBackendTransaction<'a, B: PairBackend + ?Sized> {
    expected_kind: PairBackendKind,
    backend: &'a mut B,
    unresolved_action: Option<PairActionFailure>,
}

impl<'a, B: PairBackend + ?Sized> PairBackendTransaction<'a, B> {
    pub fn new(backend: &'a mut B) -> Self {
        Self {
            expected_kind: backend.kind(),
            backend,
            unresolved_action: None,
        }
    }

    #[cfg(test)]
    pub fn health(&mut self) -> Result<PairHealth, PairBackendError> {
        let health = self.checked_call(|backend| backend.health())?;
        if health.backend() != self.expected_kind {
            return Err(PairBackendError::BackendChangedDuringTransaction);
        }
        Ok(health)
    }

    pub fn start(&mut self) -> PairActionResult {
        self.checked_action(|backend| backend.start())
    }

    pub fn stop(&mut self) -> PairActionResult {
        self.checked_action(|backend| backend.stop())
    }

    pub fn shutdown(&mut self) -> PairActionResult {
        self.checked_action(|backend| backend.shutdown())
    }

    pub fn recover_stop(&mut self) -> PairActionResult {
        self.checked_action(|backend| backend.recover_stop())
    }

    fn checked_action(
        &mut self,
        call: impl FnOnce(&mut B) -> PairActionResult,
    ) -> PairActionResult {
        if let Some(failure) = self.unresolved_action {
            // An uncertain action is terminal for this transaction. Returning
            // the same uncertainty makes a second action impossible without
            // an explicit safe-state read and a new transaction.
            return PairActionResult::OutcomeUnknown(failure);
        }

        if let Err(reason) = self.ensure_backend_is_pinned() {
            return PairActionResult::RejectedBeforeSend(PairActionFailure::new(
                self.expected_kind,
                reason,
                None,
            ));
        }

        let attempted = call(self.backend);
        let backend_still_pinned = self.ensure_backend_is_pinned().is_ok();
        let claimed_backend_is_pinned = attempted.backend() == self.expected_kind;
        let result = if backend_still_pinned && claimed_backend_is_pinned {
            attempted
        } else {
            let observed_lifecycle = attempted.observed_lifecycle();
            let failure = PairActionFailure::new(
                self.expected_kind,
                PairBackendError::BackendChangedDuringTransaction,
                observed_lifecycle,
            );
            match attempted {
                PairActionResult::RejectedBeforeSend(_) => {
                    PairActionResult::RejectedBeforeSend(failure)
                }
                PairActionResult::Accepted(_) | PairActionResult::OutcomeUnknown(_) => {
                    PairActionResult::OutcomeUnknown(failure)
                }
            }
        };

        if let PairActionResult::OutcomeUnknown(failure) = result {
            self.unresolved_action = Some(failure);
        }
        result
    }

    #[cfg(test)]
    fn checked_call<T>(
        &mut self,
        call: impl FnOnce(&mut B) -> Result<T, PairBackendError>,
    ) -> Result<T, PairBackendError> {
        self.ensure_backend_is_pinned()?;
        let result = call(self.backend);
        self.ensure_backend_is_pinned()?;
        result
    }

    fn ensure_backend_is_pinned(&self) -> Result<(), PairBackendError> {
        if self.backend.kind() == self.expected_kind {
            Ok(())
        } else {
            Err(PairBackendError::BackendChangedDuringTransaction)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct SwitchingBackend {
        kind: PairBackendKind,
    }

    impl PairBackend for SwitchingBackend {
        fn kind(&self) -> PairBackendKind {
            self.kind
        }

        fn health(&mut self) -> Result<PairHealth, PairBackendError> {
            self.kind = PairBackendKind::NativePair;
            Ok(PairHealth::new(
                PairBackendKind::LegacyV04WholePair,
                PairLifecycle::Ready,
                false,
                AuraControlTransport::Le,
            ))
        }

        fn start(&mut self) -> PairActionResult {
            unreachable!("not used by this test")
        }

        fn stop(&mut self) -> PairActionResult {
            unreachable!("not used by this test")
        }

        fn shutdown(&mut self) -> PairActionResult {
            unreachable!("not used by this test")
        }
    }

    struct ActionSwitchingBackend {
        kind: PairBackendKind,
    }

    impl PairBackend for ActionSwitchingBackend {
        fn kind(&self) -> PairBackendKind {
            self.kind
        }

        fn health(&mut self) -> Result<PairHealth, PairBackendError> {
            unreachable!("not used by this test")
        }

        fn start(&mut self) -> PairActionResult {
            self.kind = PairBackendKind::NativePair;
            PairActionResult::Accepted(PairActionReceipt::new(
                PairBackendKind::LegacyV04WholePair,
                PairLifecycle::Linked,
                PairBackendEvidence::LifecycleAcknowledgement,
                false,
            ))
        }

        fn stop(&mut self) -> PairActionResult {
            unreachable!("not used by this test")
        }

        fn shutdown(&mut self) -> PairActionResult {
            unreachable!("not used by this test")
        }
    }

    struct UncertainBackend {
        action_calls: usize,
    }

    impl PairBackend for UncertainBackend {
        fn kind(&self) -> PairBackendKind {
            PairBackendKind::LegacyV04WholePair
        }

        fn health(&mut self) -> Result<PairHealth, PairBackendError> {
            unreachable!("not used by this test")
        }

        fn start(&mut self) -> PairActionResult {
            self.action_calls += 1;
            PairActionResult::OutcomeUnknown(PairActionFailure::new(
                self.kind(),
                PairBackendError::TimedOut,
                Some(PairLifecycle::Linking),
            ))
        }

        fn stop(&mut self) -> PairActionResult {
            self.action_calls += 1;
            PairActionResult::Accepted(PairActionReceipt::new(
                self.kind(),
                PairLifecycle::Ready,
                PairBackendEvidence::LifecycleAcknowledgement,
                false,
            ))
        }

        fn shutdown(&mut self) -> PairActionResult {
            unreachable!("not used by this test")
        }
    }

    #[test]
    fn transaction_rejects_a_backend_switch_even_after_a_successful_call() {
        let mut backend = SwitchingBackend {
            kind: PairBackendKind::LegacyV04WholePair,
        };
        let mut transaction = PairBackendTransaction::new(&mut backend);

        assert_eq!(
            transaction.health(),
            Err(PairBackendError::BackendChangedDuringTransaction)
        );
        assert_eq!(
            transaction.start(),
            PairActionResult::RejectedBeforeSend(PairActionFailure::new(
                PairBackendKind::LegacyV04WholePair,
                PairBackendError::BackendChangedDuringTransaction,
                None,
            ))
        );
    }

    #[test]
    fn backend_switch_after_an_action_attempt_is_outcome_unknown() {
        let mut backend = ActionSwitchingBackend {
            kind: PairBackendKind::LegacyV04WholePair,
        };
        let mut transaction = PairBackendTransaction::new(&mut backend);

        let result = transaction.start();
        assert_eq!(
            result,
            PairActionResult::OutcomeUnknown(PairActionFailure::new(
                PairBackendKind::LegacyV04WholePair,
                PairBackendError::BackendChangedDuringTransaction,
                Some(PairLifecycle::Linked),
            ))
        );
    }

    #[test]
    fn outcome_unknown_latches_transaction_and_prevents_a_second_action() {
        let mut backend = UncertainBackend { action_calls: 0 };
        let mut transaction = PairBackendTransaction::new(&mut backend);

        let first = transaction.start();
        let second = transaction.stop();
        assert_eq!(second, first);
        assert_eq!(backend.action_calls, 1, "stop must not reach the backend");
    }

    #[test]
    fn lifecycle_health_projection_is_conservative() {
        assert_eq!(
            PairLifecycle::Linked.health_level(),
            PairHealthLevel::Healthy
        );
        assert_eq!(
            PairLifecycle::Linking.health_level(),
            PairHealthLevel::Transitioning
        );
        assert_eq!(
            PairLifecycle::Degraded.health_level(),
            PairHealthLevel::Degraded
        );
        assert_eq!(
            PairLifecycle::Failed.health_level(),
            PairHealthLevel::Unavailable
        );
    }

    #[test]
    fn reported_error_downgrades_an_otherwise_healthy_state() {
        let health = PairHealth::new(
            PairBackendKind::LegacyV04WholePair,
            PairLifecycle::Ready,
            true,
            AuraControlTransport::Unknown,
        );
        assert_eq!(health.level(), PairHealthLevel::Degraded);
        assert!(health.has_reported_error());
        assert_eq!(
            health.aura_acquisition_route(),
            AuraAcquisitionRoute::Unresolved
        );

        let routed = PairHealth::new_with_route(
            PairBackendKind::NativePair,
            PairLifecycle::Ready,
            false,
            AuraControlTransport::BrEdr,
            AuraAcquisitionRoute::A2dpWakeThenStable,
        );
        assert_eq!(
            routed.aura_acquisition_route(),
            AuraAcquisitionRoute::A2dpWakeThenStable
        );
    }
}
