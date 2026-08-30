//! Whole-pair native backend: pinned JBL HTTPS, exact JBL ATT and verified Aura BlueZ.

use std::fmt;
use std::time::Duration;

use crate::aura_bluez::{
    AuraActionResult, AuraBearerTransport, AuraBluezConfig, AuraFailureReason, AuraHealth,
    AuraTransportError, BluezAuraTransport,
};
use crate::backend::{
    AuraAcquisitionRoute, AuraControlTransport, PairActionFailure, PairActionReceipt,
    PairActionResult, PairBackend, PairBackendError, PairBackendEvidence, PairBackendKind,
    PairHealth, PairLifecycle,
};
use crate::broadcast_gena::{GenaAction, GenaBroadcastObserver, GenaFailure};
use crate::client::JblLanClient;
use crate::config::{BroadcastConfirmation, RuntimeConfig};
use crate::control::{BroadcastCommand, PlayTogetherCommand, PlayTogetherWriteResult};
use crate::controller::{PairConfigurationObservation, PairConfigurationProbe, PairProbeError};
use crate::error::JblError;
use crate::jbl_gatt::{JblGattBroadcastTransport, JblGattFailure};
use crate::model::DeviceIdentity;

const MAX_JOIN_DELAY: Duration = Duration::from_secs(10);

trait JblRoleControl {
    fn enter(&self) -> PlayTogetherWriteResult;
    fn exit(&self) -> PlayTogetherWriteResult;
}

impl JblRoleControl for JblLanClient {
    fn enter(&self) -> PlayTogetherWriteResult {
        self.send_play_together(PlayTogetherCommand::Enter)
    }

    fn exit(&self) -> PlayTogetherWriteResult {
        self.send_play_together(PlayTogetherCommand::Exit)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BroadcastAction {
    Start,
    Stop,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BroadcastResultEvidence {
    AckOnly,
    ConfirmedNotification,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BroadcastResultFailure {
    TimedOut,
    Unavailable,
    Rejected,
}

/// Closed coordinator boundary for 7957 mutation plus 7951 business evidence.
trait BroadcastResultObserver {
    fn arm(
        &mut self,
        action: BroadcastAction,
        identity: DeviceIdentity,
    ) -> Result<(), BroadcastResultFailure>;
    fn execute(
        &mut self,
        action: BroadcastAction,
        identity: DeviceIdentity,
    ) -> Result<BroadcastResultEvidence, BroadcastResultFailure>;
    fn cancel(&mut self);
}

trait BroadcastMutationTransport {
    fn arm_mutation(&mut self, identity: DeviceIdentity) -> Result<(), JblGattFailure>;
    fn execute_mutation(&mut self, command: BroadcastCommand) -> Result<(), JblGattFailure>;
    fn cancel_mutation(&mut self);
}

impl BroadcastMutationTransport for JblGattBroadcastTransport {
    fn arm_mutation(&mut self, identity: DeviceIdentity) -> Result<(), JblGattFailure> {
        self.arm(identity)
    }

    fn execute_mutation(&mut self, command: BroadcastCommand) -> Result<(), JblGattFailure> {
        self.execute(command)
    }

    fn cancel_mutation(&mut self) {
        self.cancel();
    }
}

trait BroadcastBusinessObserver {
    fn arm_business_observer(&mut self) -> Result<(), GenaFailure>;
    fn observe_business_result(&mut self, action: GenaAction) -> Result<(), GenaFailure>;
    fn cancel_business_observer(&mut self) -> Result<(), GenaFailure>;
}

impl BroadcastBusinessObserver for GenaBroadcastObserver {
    fn arm_business_observer(&mut self) -> Result<(), GenaFailure> {
        self.arm()
    }

    fn observe_business_result(&mut self, action: GenaAction) -> Result<(), GenaFailure> {
        self.observe(action)
    }

    fn cancel_business_observer(&mut self) -> Result<(), GenaFailure> {
        self.cancel()
    }
}

struct ConfirmedBroadcastCoordinator<B, M> {
    business: B,
    mutation: M,
}

impl<B, M> ConfirmedBroadcastCoordinator<B, M> {
    fn new(business: B, mutation: M) -> Self {
        Self { business, mutation }
    }
}

impl<B: BroadcastBusinessObserver, M: BroadcastMutationTransport>
    ConfirmedBroadcastCoordinator<B, M>
{
    fn cancel_all(&mut self) -> Result<(), GenaFailure> {
        self.mutation.cancel_mutation();
        self.business.cancel_business_observer()
    }
}

impl<B: BroadcastBusinessObserver, M: BroadcastMutationTransport> BroadcastResultObserver
    for ConfirmedBroadcastCoordinator<B, M>
{
    fn arm(
        &mut self,
        _action: BroadcastAction,
        identity: DeviceIdentity,
    ) -> Result<(), BroadcastResultFailure> {
        if let Err(failure) = self.business.arm_business_observer() {
            let _ = self.cancel_all();
            return Err(map_gena_failure(failure));
        }
        if let Err(failure) = self.mutation.arm_mutation(identity) {
            let _ = self.cancel_all();
            return Err(map_jbl_gatt_failure(failure));
        }
        Ok(())
    }

    fn execute(
        &mut self,
        action: BroadcastAction,
        identity: DeviceIdentity,
    ) -> Result<BroadcastResultEvidence, BroadcastResultFailure> {
        let command = match action {
            BroadcastAction::Start => BroadcastCommand::Start(identity),
            BroadcastAction::Stop => BroadcastCommand::Stop,
        };
        if let Err(failure) = self.mutation.execute_mutation(command) {
            let _ = self.cancel_all();
            return Err(map_jbl_gatt_failure(failure));
        }
        let result = self
            .business
            .observe_business_result(match action {
                BroadcastAction::Start => GenaAction::Start,
                BroadcastAction::Stop => GenaAction::Stop,
            })
            .map_err(map_gena_failure);
        let cleanup = self.cancel_all().map_err(map_gena_failure);
        match (result, cleanup) {
            (Ok(()), Ok(())) => Ok(BroadcastResultEvidence::ConfirmedNotification),
            (Ok(()), Err(failure)) | (Err(failure), _) => Err(failure),
        }
    }

    fn cancel(&mut self) {
        let _ = self.cancel_all();
    }
}

enum ProductionBroadcastObserver {
    Ack(JblGattBroadcastTransport),
    Gena(ConfirmedBroadcastCoordinator<GenaBroadcastObserver, JblGattBroadcastTransport>),
}

impl BroadcastResultObserver for ProductionBroadcastObserver {
    fn arm(
        &mut self,
        action: BroadcastAction,
        identity: DeviceIdentity,
    ) -> Result<(), BroadcastResultFailure> {
        match self {
            Self::Ack(mutation) => mutation.arm(identity).map_err(map_jbl_gatt_failure),
            Self::Gena(coordinator) => coordinator.arm(action, identity),
        }
    }

    fn execute(
        &mut self,
        action: BroadcastAction,
        identity: DeviceIdentity,
    ) -> Result<BroadcastResultEvidence, BroadcastResultFailure> {
        match self {
            Self::Ack(mutation) => {
                let command = match action {
                    BroadcastAction::Start => BroadcastCommand::Start(identity),
                    BroadcastAction::Stop => BroadcastCommand::Stop,
                };
                mutation
                    .execute(command)
                    .map(|()| BroadcastResultEvidence::AckOnly)
                    .map_err(map_jbl_gatt_failure)
            }
            Self::Gena(coordinator) => coordinator.execute(action, identity),
        }
    }

    fn cancel(&mut self) {
        match self {
            Self::Ack(mutation) => mutation.cancel(),
            Self::Gena(coordinator) => coordinator.cancel(),
        }
    }
}

#[cfg(test)]
impl BroadcastResultObserver for JblGattBroadcastTransport {
    fn arm(
        &mut self,
        _action: BroadcastAction,
        identity: DeviceIdentity,
    ) -> Result<(), BroadcastResultFailure> {
        JblGattBroadcastTransport::arm(self, identity).map_err(map_jbl_gatt_failure)
    }

    fn execute(
        &mut self,
        action: BroadcastAction,
        identity: DeviceIdentity,
    ) -> Result<BroadcastResultEvidence, BroadcastResultFailure> {
        let command = match action {
            BroadcastAction::Start => BroadcastCommand::Start(identity),
            BroadcastAction::Stop => BroadcastCommand::Stop,
        };
        JblGattBroadcastTransport::execute(self, command)
            .map(|()| BroadcastResultEvidence::AckOnly)
            .map_err(map_jbl_gatt_failure)
    }

    fn cancel(&mut self) {
        JblGattBroadcastTransport::cancel(self);
    }
}

trait AuraRoleControl {
    fn connect_verified(
        &mut self,
        expected_identity: DeviceIdentity,
    ) -> Result<(), AuraTransportError>;
    fn health(&mut self) -> AuraHealth;
    fn transport(&self) -> Option<AuraBearerTransport>;
    fn acquisition_route(&self) -> AuraAcquisitionRoute;
    fn start(&mut self) -> AuraActionResult;
    fn stop(&mut self) -> AuraActionResult;
    fn shutdown(&mut self) -> Result<(), AuraTransportError>;
}

impl AuraRoleControl for BluezAuraTransport {
    fn connect_verified(
        &mut self,
        expected_identity: DeviceIdentity,
    ) -> Result<(), AuraTransportError> {
        BluezAuraTransport::connect_verified(self, expected_identity)
    }

    fn health(&mut self) -> AuraHealth {
        BluezAuraTransport::health(self)
    }

    fn transport(&self) -> Option<AuraBearerTransport> {
        BluezAuraTransport::transport(self)
    }

    fn acquisition_route(&self) -> AuraAcquisitionRoute {
        BluezAuraTransport::acquisition_route(self)
    }

    fn start(&mut self) -> AuraActionResult {
        BluezAuraTransport::start(self)
    }

    fn stop(&mut self) -> AuraActionResult {
        BluezAuraTransport::stop(self)
    }

    fn shutdown(&mut self) -> Result<(), AuraTransportError> {
        BluezAuraTransport::shutdown(self)
    }
}

/// Sanitized native-backend construction failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativePairBuildError {
    InvalidJoinDelay,
    JblClientUnavailable,
    AuraTransportUnavailable,
    JblGattTransportUnavailable,
    JblGenaObserverUnavailable,
}

impl fmt::Display for NativePairBuildError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::InvalidJoinDelay => "native pair join delay is invalid",
            Self::JblClientUnavailable => "native JBL control client could not be initialized",
            Self::AuraTransportUnavailable => "native Aura transport could not be initialized",
            Self::JblGattTransportUnavailable => {
                "native JBL GATT transport could not be initialized"
            }
            Self::JblGenaObserverUnavailable => {
                "native JBL business-result observer could not be initialized"
            }
        };
        formatter.write_str(message)
    }
}

impl std::error::Error for NativePairBuildError {}

/// Native Ubuntu implementation that owns both speakers as one transaction.
pub struct NativePairBackend {
    core: NativePairCore<JblLanClient, BluezAuraTransport, ProductionBroadcastObserver>,
}

impl NativePairBackend {
    pub fn new(
        config: &RuntimeConfig,
        aura_config: AuraBluezConfig,
        join_delay: Duration,
    ) -> Result<Self, NativePairBuildError> {
        if join_delay > MAX_JOIN_DELAY {
            return Err(NativePairBuildError::InvalidJoinDelay);
        }
        let jbl =
            build_jbl_client(config).map_err(|_| NativePairBuildError::JblClientUnavailable)?;
        let aura = BluezAuraTransport::new(aura_config)
            .map_err(|_| NativePairBuildError::AuraTransportUnavailable)?;
        let mutation = JblGattBroadcastTransport::new()
            .map_err(|_| NativePairBuildError::JblGattTransportUnavailable)?;
        let broadcast_observer = match config.broadcast_confirmation {
            BroadcastConfirmation::Ack => ProductionBroadcastObserver::Ack(mutation),
            BroadcastConfirmation::Gena => {
                let business = GenaBroadcastObserver::with_callback_port(
                    &config.address,
                    config.gena_callback_port,
                )
                .map_err(|_| NativePairBuildError::JblGenaObserverUnavailable)?;
                ProductionBroadcastObserver::Gena(ConfirmedBroadcastCoordinator::new(
                    business, mutation,
                ))
            }
        };
        Ok(Self {
            core: NativePairCore::new(
                jbl,
                aura,
                broadcast_observer,
                config.jbl_identity,
                config.aura_identity,
                join_delay,
            ),
        })
    }
}

impl fmt::Debug for NativePairBackend {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("NativePairBackend")
            .field("state", &self.core.lifecycle)
            .field("identity", &"redacted")
            .finish()
    }
}

impl PairBackend for NativePairBackend {
    fn kind(&self) -> PairBackendKind {
        PairBackendKind::NativePair
    }

    fn health(&mut self) -> Result<PairHealth, PairBackendError> {
        Ok(self.core.health())
    }

    fn start(&mut self) -> PairActionResult {
        self.core.start()
    }

    fn stop(&mut self) -> PairActionResult {
        self.core.stop()
    }

    fn shutdown(&mut self) -> PairActionResult {
        self.core.shutdown()
    }

    fn recover_stop(&mut self) -> PairActionResult {
        self.core.recover_stop()
    }

    fn teardown_transport(&mut self) -> Result<(), PairBackendError> {
        self.core.teardown_transport()
    }
}

/// Separate read-only membership probe for the controller evidence model.
///
/// It deliberately owns a second client rather than exposing the native
/// backend's mutation client or letting the UI bypass the controller.
pub struct JblPairConfigurationProbe {
    client: JblLanClient,
    expected_model: String,
    jbl_identity: DeviceIdentity,
    aura_identity: DeviceIdentity,
}

impl JblPairConfigurationProbe {
    pub fn new(config: &RuntimeConfig) -> Result<Self, JblError> {
        Ok(Self {
            client: build_jbl_client(config)?,
            expected_model: config.expected_model.clone(),
            jbl_identity: config.jbl_identity,
            aura_identity: config.aura_identity,
        })
    }
}

impl fmt::Debug for JblPairConfigurationProbe {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("JblPairConfigurationProbe")
            .field("identity", &"redacted")
            .finish()
    }
}

impl PairConfigurationProbe for JblPairConfigurationProbe {
    fn pair_configuration(&mut self) -> Result<PairConfigurationObservation, PairProbeError> {
        self.client
            .pair_configuration_status(&self.expected_model, self.jbl_identity, self.aura_identity)
            .map(PairConfigurationObservation::from_group_status)
            .map_err(map_probe_error)
    }
}

fn build_jbl_client(config: &RuntimeConfig) -> Result<JblLanClient, JblError> {
    JblLanClient::new(
        &config.address,
        &config.certificate,
        &config.private_key,
        &config.tls_sha256,
        config.timeout,
    )
}

fn map_probe_error(error: JblError) -> PairProbeError {
    match error {
        JblError::InvalidJson
        | JblError::InvalidXml
        | JblError::BasicResponseNotObject
        | JblError::BasicResponseCodeMissing
        | JblError::BasicResponseCodeInvalid
        | JblError::DeviceInfoMissing
        | JblError::ControlDeviceInfoMissing
        | JblError::UnexpectedDeviceModel
        | JblError::DeviceReportedError
        | JblError::GroupInfoMissing
        | JblError::GroupMembersMissing
        | JblError::GroupDisabledInvalid
        | JblError::GroupMemberInvalid => PairProbeError::InvalidResponse,
        _ => PairProbeError::Unavailable,
    }
}

struct NativePairCore<J, A, O> {
    jbl: J,
    aura: A,
    broadcast_observer: O,
    jbl_identity: DeviceIdentity,
    aura_identity: DeviceIdentity,
    join_delay: Duration,
    lifecycle: PairLifecycle,
    unresolved: Option<PairActionFailure>,
}

impl<J: JblRoleControl, A: AuraRoleControl, O: BroadcastResultObserver> NativePairCore<J, A, O> {
    fn new(
        jbl: J,
        aura: A,
        broadcast_observer: O,
        jbl_identity: DeviceIdentity,
        aura_identity: DeviceIdentity,
        join_delay: Duration,
    ) -> Self {
        Self {
            jbl,
            aura,
            broadcast_observer,
            jbl_identity,
            aura_identity,
            join_delay,
            lifecycle: PairLifecycle::Offline,
            unresolved: None,
        }
    }

    fn health(&mut self) -> PairHealth {
        let aura_health = self.aura.health();
        let healthy = self.unresolved.is_none()
            && matches!(self.lifecycle, PairLifecycle::Ready | PairLifecycle::Linked)
            && matches!(aura_health, AuraHealth::Ready | AuraHealth::Offline);
        let offline = self.unresolved.is_none()
            && matches!(aura_health, AuraHealth::Offline)
            && self.lifecycle == PairLifecycle::Offline;
        let lifecycle = if healthy || offline {
            self.lifecycle
        } else {
            PairLifecycle::Degraded
        };
        PairHealth::new_with_route(
            PairBackendKind::NativePair,
            lifecycle,
            !healthy && !offline,
            match self.aura.transport() {
                Some(AuraBearerTransport::Le) => AuraControlTransport::Le,
                Some(AuraBearerTransport::BrEdr) => AuraControlTransport::BrEdr,
                None => AuraControlTransport::Unresolved,
            },
            self.aura.acquisition_route(),
        )
    }

    fn start(&mut self) -> PairActionResult {
        if let Some(failure) = self.unresolved {
            return PairActionResult::OutcomeUnknown(failure);
        }
        if self.lifecycle == PairLifecycle::Linked && self.aura.health() == AuraHealth::Ready {
            return self.accepted(PairLifecycle::Linked, true);
        }
        if let Err(error) = self.acquire_fresh_aura_bearer() {
            self.lifecycle = PairLifecycle::Degraded;
            return self.rejected(error, Some(PairLifecycle::Degraded));
        }
        self.lifecycle = PairLifecycle::Linking;

        if let Err(failure) = self
            .broadcast_observer
            .arm(BroadcastAction::Start, self.jbl_identity)
        {
            self.lifecycle = PairLifecycle::Degraded;
            return self.rejected(
                map_broadcast_result_failure(failure),
                Some(PairLifecycle::Degraded),
            );
        }

        // Deterministic v0.4 compatibility transaction selected for a network
        // source on JBL: enter JBL mode, start its broadcaster, wait the
        // configured join delay, then put Aura into the receiving role. This
        // does not make peer enrolment order a protocol requirement.
        match self.jbl.enter() {
            PlayTogetherWriteResult::Accepted(_) => {}
            PlayTogetherWriteResult::Rejected(JblError::ControlCommandRejected)
            | PlayTogetherWriteResult::OutcomeUnknown(_) => {
                self.broadcast_observer.cancel();
                return self.latch_unknown(
                    PairBackendError::JblEnterOutcomeUnknown,
                    Some(PairLifecycle::Linking),
                );
            }
            PlayTogetherWriteResult::Rejected(_) => {
                self.broadcast_observer.cancel();
                self.lifecycle = PairLifecycle::Degraded;
                return self.rejected(
                    PairBackendError::BackendReportedFailure,
                    Some(PairLifecycle::Degraded),
                );
            }
        }

        let broadcast_evidence = match self
            .broadcast_observer
            .execute(BroadcastAction::Start, self.jbl_identity)
        {
            Ok(evidence) => evidence,
            Err(failure) => {
                return self.latch_unknown(
                    map_broadcast_result_failure(failure),
                    Some(PairLifecycle::Linking),
                );
            }
        };

        if !self.join_delay.is_zero() {
            std::thread::sleep(self.join_delay);
        }
        match self.aura.start() {
            AuraActionResult::Accepted => {
                self.lifecycle = PairLifecycle::Linked;
                self.accepted_with_broadcast_evidence(PairLifecycle::Linked, broadcast_evidence)
            }
            AuraActionResult::RejectedBeforeSend(_) => self.latch_unknown(
                PairBackendError::AuraStartOutcomeUnknown,
                Some(PairLifecycle::Linking),
            ),
            AuraActionResult::OutcomeUnknown(failure) => self.latch_unknown(
                map_aura_action_unknown(failure),
                Some(PairLifecycle::Linking),
            ),
        }
    }

    fn stop(&mut self) -> PairActionResult {
        if let Some(failure) = self.unresolved {
            return PairActionResult::OutcomeUnknown(failure);
        }
        if self.lifecycle == PairLifecycle::Ready && self.aura.health() == AuraHealth::Ready {
            return self.accepted(PairLifecycle::Ready, true);
        }
        if let Err(error) = self.acquire_fresh_aura_bearer() {
            self.lifecycle = PairLifecycle::Degraded;
            return self.rejected(error, Some(PairLifecycle::Degraded));
        }
        self.lifecycle = PairLifecycle::Unlinking;
        if let Err(failure) = self
            .broadcast_observer
            .arm(BroadcastAction::Stop, self.jbl_identity)
        {
            self.lifecycle = PairLifecycle::Degraded;
            return self.rejected(
                map_broadcast_result_failure(failure),
                Some(PairLifecycle::Degraded),
            );
        }
        match self.aura.stop() {
            AuraActionResult::Accepted => {}
            AuraActionResult::RejectedBeforeSend(_) => {
                self.broadcast_observer.cancel();
                self.lifecycle = PairLifecycle::Degraded;
                return self.rejected(
                    PairBackendError::BackendReportedFailure,
                    Some(PairLifecycle::Degraded),
                );
            }
            AuraActionResult::OutcomeUnknown(failure) => {
                self.broadcast_observer.cancel();
                return self.latch_unknown(
                    map_aura_action_unknown(failure),
                    Some(PairLifecycle::Unlinking),
                );
            }
        }

        let broadcast_evidence = match self
            .broadcast_observer
            .execute(BroadcastAction::Stop, self.jbl_identity)
        {
            Ok(evidence) => evidence,
            Err(failure) => {
                return self.latch_unknown(
                    map_broadcast_result_failure(failure),
                    Some(PairLifecycle::Unlinking),
                );
            }
        };

        match self.jbl.exit() {
            PlayTogetherWriteResult::Accepted(_) => {
                self.lifecycle = PairLifecycle::Ready;
                self.accepted_with_broadcast_evidence(PairLifecycle::Ready, broadcast_evidence)
            }
            PlayTogetherWriteResult::Rejected(_) | PlayTogetherWriteResult::OutcomeUnknown(_) => {
                self.latch_unknown(
                    PairBackendError::JblExitOutcomeUnknown,
                    Some(PairLifecycle::Unlinking),
                )
            }
        }
    }

    fn shutdown(&mut self) -> PairActionResult {
        if let Some(failure) = self.unresolved {
            return PairActionResult::OutcomeUnknown(failure);
        }
        if !matches!(
            self.lifecycle,
            PairLifecycle::Ready | PairLifecycle::Offline
        ) {
            let stopped = self.stop();
            if !matches!(stopped, PairActionResult::Accepted(_)) {
                return stopped;
            }
        }
        self.lifecycle = PairLifecycle::ShuttingDown;
        if self.aura.shutdown().is_err() {
            return self.latch_unknown(
                PairBackendError::Unavailable,
                Some(PairLifecycle::ShuttingDown),
            );
        }
        self.lifecycle = PairLifecycle::Offline;
        self.accepted(PairLifecycle::ShuttingDown, false)
    }

    fn recover_stop(&mut self) -> PairActionResult {
        // This is the only method allowed to cross an unresolved-action latch.
        // It first tears down the old notification/device bearer without any
        // role write. Only a successful teardown permits a new verified scan
        // and one receiver-first STOP normalization attempt.
        self.lifecycle = PairLifecycle::Recovering;
        if self.aura.shutdown().is_err() {
            return self.latch_unknown(
                PairBackendError::Unavailable,
                Some(PairLifecycle::Recovering),
            );
        }
        self.unresolved = None;
        self.lifecycle = PairLifecycle::Degraded;
        self.stop()
    }

    fn teardown_transport(&mut self) -> Result<(), PairBackendError> {
        // BluezAuraTransport::shutdown performs StartNotify/Device transport
        // teardown only; it emits neither the AA OFF frame nor a JBL command.
        // Preserve both the native uncertainty latch and the persistent
        // controller journal for the next explicit recover-stop.
        self.aura
            .shutdown()
            .map_err(|_| PairBackendError::Unavailable)
    }

    fn acquire_fresh_aura_bearer(&mut self) -> Result<(), PairBackendError> {
        // The official apps release their temporary control bearer after an
        // acknowledged role write. During playback an old socket can still
        // look poll-healthy while no longer delivering the next AA ACK. A
        // non-idempotent transaction therefore begins from an empty bearer
        // slot and performs one fresh, verified acquisition before any role
        // mutation. Shutdown is transport-only and emits no AA/JBL command.
        self.aura
            .shutdown()
            .map_err(|_| PairBackendError::Unavailable)?;
        self.aura
            .connect_verified(self.aura_identity)
            .map_err(map_aura_transport_error)
    }

    fn accepted(&self, lifecycle: PairLifecycle, idempotent: bool) -> PairActionResult {
        PairActionResult::Accepted(PairActionReceipt::new(
            PairBackendKind::NativePair,
            lifecycle,
            if idempotent {
                PairBackendEvidence::LocalSessionState
            } else {
                PairBackendEvidence::LifecycleAcknowledgement
            },
            idempotent,
        ))
    }

    fn accepted_with_broadcast_evidence(
        &self,
        lifecycle: PairLifecycle,
        evidence: BroadcastResultEvidence,
    ) -> PairActionResult {
        PairActionResult::Accepted(PairActionReceipt::new(
            PairBackendKind::NativePair,
            lifecycle,
            match evidence {
                BroadcastResultEvidence::AckOnly => {
                    PairBackendEvidence::BroadcastAcknowledgementOnly
                }
                BroadcastResultEvidence::ConfirmedNotification => {
                    PairBackendEvidence::BroadcastBusinessNotification
                }
            },
            false,
        ))
    }

    fn rejected(
        &self,
        reason: PairBackendError,
        observed: Option<PairLifecycle>,
    ) -> PairActionResult {
        PairActionResult::RejectedBeforeSend(PairActionFailure::new(
            PairBackendKind::NativePair,
            reason,
            observed,
        ))
    }

    fn latch_unknown(
        &mut self,
        reason: PairBackendError,
        observed: Option<PairLifecycle>,
    ) -> PairActionResult {
        let failure = PairActionFailure::new(PairBackendKind::NativePair, reason, observed);
        self.unresolved = Some(failure);
        self.lifecycle = PairLifecycle::Degraded;
        PairActionResult::OutcomeUnknown(failure)
    }
}

fn map_aura_transport_error(error: AuraTransportError) -> PairBackendError {
    match error.reason() {
        AuraFailureReason::AdapterUnavailable | AuraFailureReason::AdapterPoweredOff => {
            PairBackendError::AuraAdapterUnavailable
        }
        AuraFailureReason::DiscoveryUnavailable => PairBackendError::AuraDiscoveryUnavailable,
        AuraFailureReason::VerifiedAdvertisementNotFound => {
            PairBackendError::AuraVerifiedAdvertisementNotFound
        }
        AuraFailureReason::DeviceConnectionFailed => PairBackendError::AuraDeviceConnectionFailed,
        AuraFailureReason::WakeProfileConnectFailed => {
            PairBackendError::AuraWakeProfileConnectFailed
        }
        AuraFailureReason::WakeFddfTimedOut => PairBackendError::AuraWakeFddfTimedOut,
        AuraFailureReason::WakeFddfInvalid => PairBackendError::AuraWakeFddfInvalid,
        AuraFailureReason::WakeFddfUnavailable => PairBackendError::AuraWakeFddfUnavailable,
        AuraFailureReason::WakeProfileReleaseFailed => {
            PairBackendError::AuraWakeProfileReleaseFailed
        }
        AuraFailureReason::GattProfileInvalid => PairBackendError::AuraGattProfileInvalid,
        AuraFailureReason::NotificationSetupFailed => PairBackendError::AuraNotificationSetupFailed,
        AuraFailureReason::InvalidConfiguration
        | AuraFailureReason::RuntimeUnavailable
        | AuraFailureReason::TransportNotReady
        | AuraFailureReason::NotificationQueueInvalid
        | AuraFailureReason::WriteFailed
        | AuraFailureReason::AcknowledgementTimedOut
        | AuraFailureReason::AcknowledgementChannelClosed
        | AuraFailureReason::UnexpectedAcknowledgement
        | AuraFailureReason::DisconnectFailed => PairBackendError::Unavailable,
    }
}

fn map_aura_action_unknown(failure: crate::aura_bluez::AuraActionFailure) -> PairBackendError {
    match failure.reason() {
        AuraFailureReason::WriteFailed => PairBackendError::AuraWriteUnknown,
        AuraFailureReason::AcknowledgementTimedOut => PairBackendError::AuraAcknowledgementTimedOut,
        AuraFailureReason::AcknowledgementChannelClosed => {
            PairBackendError::AuraAcknowledgementChannelClosed
        }
        AuraFailureReason::UnexpectedAcknowledgement => {
            PairBackendError::AuraUnexpectedAcknowledgement
        }
        AuraFailureReason::InvalidConfiguration
        | AuraFailureReason::RuntimeUnavailable
        | AuraFailureReason::AdapterUnavailable
        | AuraFailureReason::AdapterPoweredOff
        | AuraFailureReason::DiscoveryUnavailable
        | AuraFailureReason::VerifiedAdvertisementNotFound
        | AuraFailureReason::DeviceConnectionFailed
        | AuraFailureReason::WakeProfileConnectFailed
        | AuraFailureReason::WakeFddfTimedOut
        | AuraFailureReason::WakeFddfInvalid
        | AuraFailureReason::WakeFddfUnavailable
        | AuraFailureReason::WakeProfileReleaseFailed
        | AuraFailureReason::GattProfileInvalid
        | AuraFailureReason::NotificationSetupFailed
        | AuraFailureReason::TransportNotReady
        | AuraFailureReason::NotificationQueueInvalid
        | AuraFailureReason::DisconnectFailed => PairBackendError::Unavailable,
    }
}

const fn map_broadcast_result_failure(failure: BroadcastResultFailure) -> PairBackendError {
    match failure {
        BroadcastResultFailure::TimedOut => PairBackendError::JblBroadcastResultTimedOut,
        BroadcastResultFailure::Unavailable => PairBackendError::JblBroadcastResultUnavailable,
        BroadcastResultFailure::Rejected => PairBackendError::JblBroadcastResultRejected,
    }
}

const fn map_jbl_gatt_failure(failure: JblGattFailure) -> BroadcastResultFailure {
    match failure {
        JblGattFailure::WriteResponseTimedOut => BroadcastResultFailure::TimedOut,
        JblGattFailure::RuntimeUnavailable
        | JblGattFailure::AdapterUnavailable
        | JblGattFailure::AdapterPoweredOff
        | JblGattFailure::DeviceConnectionFailed
        | JblGattFailure::MtuExchangeFailed
        | JblGattFailure::TransportNotReady
        | JblGattFailure::FrameTooLarge
        | JblGattFailure::WriteFailed
        | JblGattFailure::ChannelClosed
        | JblGattFailure::UnexpectedResponse => BroadcastResultFailure::Unavailable,
    }
}

const fn map_gena_failure(failure: GenaFailure) -> BroadcastResultFailure {
    match failure {
        GenaFailure::CallbackTimedOut => BroadcastResultFailure::TimedOut,
        GenaFailure::BusinessRejected => BroadcastResultFailure::Rejected,
        GenaFailure::InvalidConfiguration
        | GenaFailure::RouteUnavailable
        | GenaFailure::ListenerUnavailable
        | GenaFailure::SubscriptionUnavailable
        | GenaFailure::InvalidSubscription
        | GenaFailure::InvalidCallback
        | GenaFailure::CleanupFailed => BroadcastResultFailure::Unavailable,
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::VecDeque;
    use std::rc::Rc;

    use crate::aura_bluez::{AuraActionFailure, AuraFailureReason};
    use crate::control::BasicResponse;

    use super::*;

    type SharedTrace = Rc<RefCell<Vec<&'static str>>>;

    fn jbl_identity() -> DeviceIdentity {
        DeviceIdentity::parse("02:00:00:00:00:01").expect("fixture JBL identity")
    }

    fn aura_identity() -> DeviceIdentity {
        DeviceIdentity::parse("02:00:00:00:00:02").expect("fixture Aura identity")
    }

    fn accepted() -> PlayTogetherWriteResult {
        PlayTogetherWriteResult::Accepted(
            BasicResponse::parse(br#"{"error_code":0}"#).expect("fixture BasicResponse"),
        )
    }

    fn aura_unknown(reason: AuraFailureReason) -> AuraActionResult {
        AuraActionResult::OutcomeUnknown(AuraActionFailure::from_reason_for_test(reason))
    }

    fn unknown_reason(result: PairActionResult) -> PairBackendError {
        match result {
            PairActionResult::OutcomeUnknown(failure) => failure.reason(),
            PairActionResult::Accepted(_) | PairActionResult::RejectedBeforeSend(_) => {
                panic!("fixture must be outcome-unknown")
            }
        }
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum JblCommand {
        Enter,
        Exit,
    }

    struct RecordingJbl {
        results: RefCell<VecDeque<PlayTogetherWriteResult>>,
        commands: RefCell<Vec<JblCommand>>,
        trace: SharedTrace,
    }

    impl RecordingJbl {
        fn new(results: Vec<PlayTogetherWriteResult>, trace: SharedTrace) -> Self {
            Self {
                results: RefCell::new(results.into()),
                commands: RefCell::new(Vec::new()),
                trace,
            }
        }

        fn record(
            &self,
            command: JblCommand,
            trace_label: &'static str,
        ) -> PlayTogetherWriteResult {
            self.commands.borrow_mut().push(command);
            self.trace.borrow_mut().push(trace_label);
            self.results
                .borrow_mut()
                .pop_front()
                .expect("JBL result fixture")
        }
    }

    impl JblRoleControl for RecordingJbl {
        fn enter(&self) -> PlayTogetherWriteResult {
            self.record(JblCommand::Enter, "jbl_enter")
        }

        fn exit(&self) -> PlayTogetherWriteResult {
            self.record(JblCommand::Exit, "jbl_exit")
        }
    }

    struct MockAura {
        health: AuraHealth,
        actions: VecDeque<AuraActionResult>,
        commands: Vec<&'static str>,
        shutdown_calls: usize,
        next_connect_failure: Option<AuraFailureReason>,
        next_shutdown_failure: Option<AuraFailureReason>,
        trace: SharedTrace,
    }

    impl MockAura {
        fn new(health: AuraHealth, actions: Vec<AuraActionResult>, trace: SharedTrace) -> Self {
            Self {
                health,
                actions: actions.into(),
                commands: Vec::new(),
                shutdown_calls: 0,
                next_connect_failure: None,
                next_shutdown_failure: None,
                trace,
            }
        }
    }

    impl AuraRoleControl for MockAura {
        fn connect_verified(
            &mut self,
            expected_identity: DeviceIdentity,
        ) -> Result<(), AuraTransportError> {
            assert!(expected_identity == aura_identity());
            self.trace.borrow_mut().push("aura_connect");
            if let Some(reason) = self.next_connect_failure.take() {
                return Err(AuraTransportError::from_reason_for_test(reason));
            }
            self.health = AuraHealth::Ready;
            Ok(())
        }

        fn health(&mut self) -> AuraHealth {
            self.health
        }

        fn transport(&self) -> Option<AuraBearerTransport> {
            (self.health == AuraHealth::Ready).then_some(AuraBearerTransport::Le)
        }

        fn acquisition_route(&self) -> AuraAcquisitionRoute {
            if self.health == AuraHealth::Ready {
                AuraAcquisitionRoute::FreshLe
            } else {
                AuraAcquisitionRoute::Unresolved
            }
        }

        fn start(&mut self) -> AuraActionResult {
            self.commands.push("on");
            self.trace.borrow_mut().push("aura_on");
            self.actions.pop_front().expect("Aura start fixture")
        }

        fn stop(&mut self) -> AuraActionResult {
            self.commands.push("off");
            self.trace.borrow_mut().push("aura_off");
            self.actions.pop_front().expect("Aura stop fixture")
        }

        fn shutdown(&mut self) -> Result<(), AuraTransportError> {
            self.shutdown_calls += 1;
            self.trace.borrow_mut().push("transport_teardown");
            if let Some(reason) = self.next_shutdown_failure.take() {
                return Err(AuraTransportError::from_reason_for_test(reason));
            }
            self.health = AuraHealth::Offline;
            Ok(())
        }
    }

    struct MockObserver {
        arm_results: VecDeque<Result<(), BroadcastResultFailure>>,
        execute_results: VecDeque<Result<BroadcastResultEvidence, BroadcastResultFailure>>,
        trace: SharedTrace,
    }

    impl MockObserver {
        fn ack_only(trace: SharedTrace) -> Self {
            Self {
                arm_results: VecDeque::from([Ok(())]),
                execute_results: VecDeque::from([Ok(BroadcastResultEvidence::AckOnly)]),
                trace,
            }
        }

        fn with(
            arm_results: Vec<Result<(), BroadcastResultFailure>>,
            execute_results: Vec<Result<BroadcastResultEvidence, BroadcastResultFailure>>,
            trace: SharedTrace,
        ) -> Self {
            Self {
                arm_results: arm_results.into(),
                execute_results: execute_results.into(),
                trace,
            }
        }
    }

    impl BroadcastResultObserver for MockObserver {
        fn arm(
            &mut self,
            action: BroadcastAction,
            identity: DeviceIdentity,
        ) -> Result<(), BroadcastResultFailure> {
            assert!(identity == jbl_identity());
            self.trace.borrow_mut().push(match action {
                BroadcastAction::Start => "observer_arm_start",
                BroadcastAction::Stop => "observer_arm_stop",
            });
            self.arm_results.pop_front().expect("observer arm fixture")
        }

        fn execute(
            &mut self,
            action: BroadcastAction,
            identity: DeviceIdentity,
        ) -> Result<BroadcastResultEvidence, BroadcastResultFailure> {
            assert!(identity == jbl_identity());
            self.trace.borrow_mut().push(match action {
                BroadcastAction::Start => "gatt_7957_start",
                BroadcastAction::Stop => "gatt_7957_stop",
            });
            self.execute_results
                .pop_front()
                .expect("observer result fixture")
        }

        fn cancel(&mut self) {
            self.trace.borrow_mut().push("gatt_cancel");
        }
    }

    struct MockBusinessObserver {
        arm_results: VecDeque<Result<(), GenaFailure>>,
        observe_results: VecDeque<Result<(), GenaFailure>>,
        cancel_results: VecDeque<Result<(), GenaFailure>>,
        trace: SharedTrace,
    }

    impl BroadcastBusinessObserver for MockBusinessObserver {
        fn arm_business_observer(&mut self) -> Result<(), GenaFailure> {
            self.trace.borrow_mut().push("gena_arm");
            self.arm_results.pop_front().expect("GENA arm fixture")
        }

        fn observe_business_result(&mut self, action: GenaAction) -> Result<(), GenaFailure> {
            self.trace.borrow_mut().push(match action {
                GenaAction::Start => "gena_observe_start",
                GenaAction::Stop => "gena_observe_stop",
            });
            self.observe_results
                .pop_front()
                .expect("GENA observation fixture")
        }

        fn cancel_business_observer(&mut self) -> Result<(), GenaFailure> {
            self.trace.borrow_mut().push("gena_cancel");
            self.cancel_results.pop_front().unwrap_or(Ok(()))
        }
    }

    struct MockMutationTransport {
        arm_results: VecDeque<Result<(), JblGattFailure>>,
        execute_results: VecDeque<Result<(), JblGattFailure>>,
        trace: SharedTrace,
    }

    impl BroadcastMutationTransport for MockMutationTransport {
        fn arm_mutation(&mut self, identity: DeviceIdentity) -> Result<(), JblGattFailure> {
            assert!(identity == jbl_identity());
            self.trace.borrow_mut().push("gatt_arm");
            self.arm_results.pop_front().expect("GATT arm fixture")
        }

        fn execute_mutation(&mut self, command: BroadcastCommand) -> Result<(), JblGattFailure> {
            self.trace.borrow_mut().push(match command {
                BroadcastCommand::Start(identity) => {
                    assert!(identity == jbl_identity());
                    "gatt_execute_start"
                }
                BroadcastCommand::Stop => "gatt_execute_stop",
            });
            self.execute_results
                .pop_front()
                .expect("GATT execute fixture")
        }

        fn cancel_mutation(&mut self) {
            self.trace.borrow_mut().push("gatt_cancel");
        }
    }

    fn confirmed_coordinator(
        arm_business: Result<(), GenaFailure>,
        observe_business: Vec<Result<(), GenaFailure>>,
        arm_mutation: Result<(), JblGattFailure>,
        execute_mutation: Vec<Result<(), JblGattFailure>>,
        trace: SharedTrace,
    ) -> ConfirmedBroadcastCoordinator<MockBusinessObserver, MockMutationTransport> {
        ConfirmedBroadcastCoordinator::new(
            MockBusinessObserver {
                arm_results: VecDeque::from([arm_business]),
                observe_results: observe_business.into(),
                cancel_results: VecDeque::new(),
                trace: Rc::clone(&trace),
            },
            MockMutationTransport {
                arm_results: VecDeque::from([arm_mutation]),
                execute_results: execute_mutation.into(),
                trace,
            },
        )
    }

    #[test]
    fn confirmed_coordinator_arms_gena_before_gatt_then_waits_after_one_write() {
        let trace = Rc::new(RefCell::new(Vec::new()));
        let mut coordinator = confirmed_coordinator(
            Ok(()),
            vec![Ok(())],
            Ok(()),
            vec![Ok(())],
            Rc::clone(&trace),
        );

        coordinator
            .arm(BroadcastAction::Start, jbl_identity())
            .expect("coordinator arm");
        assert_eq!(
            coordinator.execute(BroadcastAction::Start, jbl_identity()),
            Ok(BroadcastResultEvidence::ConfirmedNotification)
        );
        assert_eq!(
            trace.borrow().as_slice(),
            [
                "gena_arm",
                "gatt_arm",
                "gatt_execute_start",
                "gena_observe_start",
                "gatt_cancel",
                "gena_cancel",
            ]
        );
    }

    #[test]
    fn coordinator_gena_arm_failure_never_arms_or_writes_gatt() {
        let trace = Rc::new(RefCell::new(Vec::new()));
        let mut coordinator = confirmed_coordinator(
            Err(GenaFailure::SubscriptionUnavailable),
            Vec::new(),
            Ok(()),
            Vec::new(),
            Rc::clone(&trace),
        );

        assert_eq!(
            coordinator.arm(BroadcastAction::Start, jbl_identity()),
            Err(BroadcastResultFailure::Unavailable)
        );
        assert_eq!(
            trace.borrow().as_slice(),
            ["gena_arm", "gatt_cancel", "gena_cancel"]
        );
    }

    #[test]
    fn coordinator_gatt_arm_failure_cleans_both_without_a_write() {
        let trace = Rc::new(RefCell::new(Vec::new()));
        let mut coordinator = confirmed_coordinator(
            Ok(()),
            Vec::new(),
            Err(JblGattFailure::DeviceConnectionFailed),
            Vec::new(),
            Rc::clone(&trace),
        );

        assert_eq!(
            coordinator.arm(BroadcastAction::Start, jbl_identity()),
            Err(BroadcastResultFailure::Unavailable)
        );
        assert_eq!(
            trace.borrow().as_slice(),
            ["gena_arm", "gatt_arm", "gatt_cancel", "gena_cancel"]
        );
    }

    #[test]
    fn coordinator_gatt_unknown_cleans_both_and_never_waits_for_gena() {
        let trace = Rc::new(RefCell::new(Vec::new()));
        let mut coordinator = confirmed_coordinator(
            Ok(()),
            Vec::new(),
            Ok(()),
            vec![Err(JblGattFailure::WriteResponseTimedOut)],
            Rc::clone(&trace),
        );
        coordinator
            .arm(BroadcastAction::Start, jbl_identity())
            .expect("coordinator arm");

        assert_eq!(
            coordinator.execute(BroadcastAction::Start, jbl_identity()),
            Err(BroadcastResultFailure::TimedOut)
        );
        assert_eq!(
            trace.borrow().as_slice(),
            [
                "gena_arm",
                "gatt_arm",
                "gatt_execute_start",
                "gatt_cancel",
                "gena_cancel",
            ]
        );
    }

    #[test]
    fn coordinator_maps_post_write_gena_results_and_always_cleans_both() {
        for (failure, expected) in [
            (
                GenaFailure::CallbackTimedOut,
                BroadcastResultFailure::TimedOut,
            ),
            (
                GenaFailure::BusinessRejected,
                BroadcastResultFailure::Rejected,
            ),
            (
                GenaFailure::ListenerUnavailable,
                BroadcastResultFailure::Unavailable,
            ),
        ] {
            let trace = Rc::new(RefCell::new(Vec::new()));
            let mut coordinator = confirmed_coordinator(
                Ok(()),
                vec![Err(failure)],
                Ok(()),
                vec![Ok(())],
                Rc::clone(&trace),
            );
            coordinator
                .arm(BroadcastAction::Stop, jbl_identity())
                .expect("coordinator arm");

            assert_eq!(
                coordinator.execute(BroadcastAction::Stop, jbl_identity()),
                Err(expected)
            );
            assert_eq!(
                trace.borrow().as_slice(),
                [
                    "gena_arm",
                    "gatt_arm",
                    "gatt_execute_stop",
                    "gena_observe_stop",
                    "gatt_cancel",
                    "gena_cancel",
                ]
            );
        }
    }

    fn core(
        jbl_results: Vec<PlayTogetherWriteResult>,
        aura_actions: Vec<AuraActionResult>,
        observer: MockObserver,
        trace: SharedTrace,
    ) -> NativePairCore<RecordingJbl, MockAura, MockObserver> {
        NativePairCore::new(
            RecordingJbl::new(jbl_results, Rc::clone(&trace)),
            MockAura::new(AuraHealth::Offline, aura_actions, trace),
            observer,
            jbl_identity(),
            aura_identity(),
            Duration::ZERO,
        )
    }

    #[test]
    fn start_uses_selected_v04_transaction_order_and_ack_only_evidence() {
        let trace = Rc::new(RefCell::new(Vec::new()));
        let observer = MockObserver::ack_only(Rc::clone(&trace));
        let mut core = core(
            vec![accepted()],
            vec![AuraActionResult::Accepted],
            observer,
            Rc::clone(&trace),
        );

        let receipt = core.start().receipt().expect("START should succeed");
        assert_eq!(receipt.lifecycle(), PairLifecycle::Linked);
        assert_eq!(
            receipt.evidence(),
            PairBackendEvidence::BroadcastAcknowledgementOnly
        );
        assert_eq!(
            trace.borrow().as_slice(),
            [
                "transport_teardown",
                "aura_connect",
                "observer_arm_start",
                "jbl_enter",
                "gatt_7957_start",
                "aura_on",
            ]
        );
    }

    #[test]
    fn stop_uses_receiver_off_broadcast_stop_then_exit_order() {
        let trace = Rc::new(RefCell::new(Vec::new()));
        let observer = MockObserver::ack_only(Rc::clone(&trace));
        let mut core = core(
            vec![accepted()],
            vec![AuraActionResult::Accepted],
            observer,
            Rc::clone(&trace),
        );
        core.lifecycle = PairLifecycle::Linked;

        let receipt = core.stop().receipt().expect("STOP should succeed");
        assert_eq!(receipt.lifecycle(), PairLifecycle::Ready);
        assert_eq!(
            receipt.evidence(),
            PairBackendEvidence::BroadcastAcknowledgementOnly
        );
        assert_eq!(
            trace.borrow().as_slice(),
            [
                "transport_teardown",
                "aura_connect",
                "observer_arm_stop",
                "aura_off",
                "gatt_7957_stop",
                "jbl_exit",
            ]
        );
    }

    #[test]
    fn confirmed_business_notification_has_a_distinct_evidence_level() {
        let trace = Rc::new(RefCell::new(Vec::new()));
        let observer = MockObserver::with(
            vec![Ok(())],
            vec![Ok(BroadcastResultEvidence::ConfirmedNotification)],
            Rc::clone(&trace),
        );
        let mut core = core(
            vec![accepted()],
            vec![AuraActionResult::Accepted],
            observer,
            trace,
        );

        let receipt = core.start().receipt().expect("START should succeed");
        assert_eq!(
            receipt.evidence(),
            PairBackendEvidence::BroadcastBusinessNotification
        );
    }

    #[test]
    fn gatt_failures_map_to_a_closed_privacy_safe_result_vocabulary() {
        assert_eq!(
            map_jbl_gatt_failure(JblGattFailure::WriteResponseTimedOut),
            BroadcastResultFailure::TimedOut
        );
        for failure in [
            JblGattFailure::RuntimeUnavailable,
            JblGattFailure::AdapterUnavailable,
            JblGattFailure::AdapterPoweredOff,
            JblGattFailure::DeviceConnectionFailed,
            JblGattFailure::MtuExchangeFailed,
            JblGattFailure::TransportNotReady,
            JblGattFailure::FrameTooLarge,
            JblGattFailure::WriteFailed,
            JblGattFailure::ChannelClosed,
            JblGattFailure::UnexpectedResponse,
        ] {
            assert_eq!(
                map_jbl_gatt_failure(failure),
                BroadcastResultFailure::Unavailable
            );
        }
    }

    #[test]
    fn wake_failures_map_to_distinct_closed_backend_stages() {
        for (reason, expected) in [
            (
                AuraFailureReason::WakeProfileConnectFailed,
                PairBackendError::AuraWakeProfileConnectFailed,
            ),
            (
                AuraFailureReason::WakeFddfTimedOut,
                PairBackendError::AuraWakeFddfTimedOut,
            ),
            (
                AuraFailureReason::WakeFddfInvalid,
                PairBackendError::AuraWakeFddfInvalid,
            ),
            (
                AuraFailureReason::WakeFddfUnavailable,
                PairBackendError::AuraWakeFddfUnavailable,
            ),
            (
                AuraFailureReason::WakeProfileReleaseFailed,
                PairBackendError::AuraWakeProfileReleaseFailed,
            ),
        ] {
            assert_eq!(
                map_aura_transport_error(AuraTransportError::from_reason_for_test(reason)),
                expected
            );
            assert!(!expected.to_string().contains(':'));
        }
    }

    #[test]
    fn gena_failures_map_to_a_closed_privacy_safe_result_vocabulary() {
        assert_eq!(
            map_gena_failure(GenaFailure::CallbackTimedOut),
            BroadcastResultFailure::TimedOut
        );
        assert_eq!(
            map_gena_failure(GenaFailure::BusinessRejected),
            BroadcastResultFailure::Rejected
        );
        for failure in [
            GenaFailure::InvalidConfiguration,
            GenaFailure::RouteUnavailable,
            GenaFailure::ListenerUnavailable,
            GenaFailure::SubscriptionUnavailable,
            GenaFailure::InvalidSubscription,
            GenaFailure::InvalidCallback,
            GenaFailure::CleanupFailed,
        ] {
            assert_eq!(
                map_gena_failure(failure),
                BroadcastResultFailure::Unavailable
            );
        }
    }

    #[test]
    fn observer_arm_failure_is_rejected_before_any_role_write() {
        let trace = Rc::new(RefCell::new(Vec::new()));
        let observer = MockObserver::with(
            vec![Err(BroadcastResultFailure::Unavailable)],
            Vec::new(),
            Rc::clone(&trace),
        );
        let mut core = core(Vec::new(), Vec::new(), observer, Rc::clone(&trace));

        assert!(matches!(
            core.start(),
            PairActionResult::RejectedBeforeSend(_)
        ));
        assert!(core.jbl.commands.borrow().is_empty());
        assert!(core.aura.commands.is_empty());
        assert_eq!(
            trace.borrow().as_slice(),
            ["transport_teardown", "aura_connect", "observer_arm_start"]
        );
    }

    #[test]
    fn fresh_teardown_failure_rejects_start_before_any_role_write() {
        let trace = Rc::new(RefCell::new(Vec::new()));
        let observer = MockObserver::ack_only(Rc::clone(&trace));
        let mut core = core(Vec::new(), Vec::new(), observer, Rc::clone(&trace));
        core.aura.next_shutdown_failure = Some(AuraFailureReason::DisconnectFailed);

        assert!(matches!(
            core.start(),
            PairActionResult::RejectedBeforeSend(_)
        ));
        assert_eq!(trace.borrow().as_slice(), ["transport_teardown"]);
        assert!(core.jbl.commands.borrow().is_empty());
        assert!(core.aura.commands.is_empty());
        assert_eq!(core.broadcast_observer.arm_results.len(), 1);
    }

    #[test]
    fn fresh_connect_failure_rejects_stop_before_any_role_write() {
        let trace = Rc::new(RefCell::new(Vec::new()));
        let observer = MockObserver::ack_only(Rc::clone(&trace));
        let mut core = core(Vec::new(), Vec::new(), observer, Rc::clone(&trace));
        core.lifecycle = PairLifecycle::Linked;
        core.aura.next_connect_failure = Some(AuraFailureReason::DeviceConnectionFailed);

        assert!(matches!(
            core.stop(),
            PairActionResult::RejectedBeforeSend(_)
        ));
        assert_eq!(
            trace.borrow().as_slice(),
            ["transport_teardown", "aura_connect"]
        );
        assert!(core.jbl.commands.borrow().is_empty());
        assert!(core.aura.commands.is_empty());
        assert_eq!(core.broadcast_observer.arm_results.len(), 1);
    }

    #[test]
    fn idempotent_fast_path_does_not_replace_a_ready_bearer() {
        let trace = Rc::new(RefCell::new(Vec::new()));
        let observer = MockObserver::ack_only(Rc::clone(&trace));
        let mut core = core(Vec::new(), Vec::new(), observer, Rc::clone(&trace));
        core.lifecycle = PairLifecycle::Linked;
        core.aura.health = AuraHealth::Ready;

        let receipt = core.start().receipt().expect("START should be idempotent");
        assert!(receipt.is_idempotent());
        assert!(trace.borrow().is_empty());
        assert_eq!(core.aura.shutdown_calls, 0);
    }

    #[test]
    fn enter_unknown_latches_without_broadcast_aura_or_retry() {
        let trace = Rc::new(RefCell::new(Vec::new()));
        let observer = MockObserver::ack_only(Rc::clone(&trace));
        let mut core = core(
            vec![PlayTogetherWriteResult::OutcomeUnknown(
                JblError::NetworkUnreachable,
            )],
            Vec::new(),
            observer,
            trace,
        );

        let first = core.start();
        assert_eq!(
            unknown_reason(first),
            PairBackendError::JblEnterOutcomeUnknown
        );
        assert_eq!(core.start(), first);
        assert_eq!(core.jbl.commands.borrow().as_slice(), [JblCommand::Enter]);
        assert!(core.aura.commands.is_empty());
    }

    #[test]
    fn gatt_start_unknown_latches_after_one_enter_without_retry() {
        let trace = Rc::new(RefCell::new(Vec::new()));
        let observer = MockObserver::with(
            vec![Ok(())],
            vec![Err(BroadcastResultFailure::Unavailable)],
            Rc::clone(&trace),
        );
        let mut core = core(vec![accepted()], Vec::new(), observer, trace);

        let first = core.start();
        assert_eq!(
            unknown_reason(first),
            PairBackendError::JblBroadcastResultUnavailable
        );
        assert_eq!(core.stop(), first);
        assert_eq!(core.jbl.commands.borrow().as_slice(), [JblCommand::Enter]);
        assert!(core.aura.commands.is_empty());
    }

    #[test]
    fn broadcast_result_failure_latches_after_start_ack_without_aura() {
        for failure in [
            BroadcastResultFailure::TimedOut,
            BroadcastResultFailure::Unavailable,
            BroadcastResultFailure::Rejected,
        ] {
            let trace = Rc::new(RefCell::new(Vec::new()));
            let observer = MockObserver::with(vec![Ok(())], vec![Err(failure)], Rc::clone(&trace));
            let mut core = core(vec![accepted()], Vec::new(), observer, trace);

            let first = core.start();
            assert_eq!(unknown_reason(first), map_broadcast_result_failure(failure));
            assert_eq!(core.start(), first);
            assert!(core.aura.commands.is_empty());
            assert_eq!(core.jbl.commands.borrow().len(), 1);
        }
    }

    #[test]
    fn aura_failure_after_broadcaster_ack_latches_without_repeating_jbl() {
        for (aura_result, expected) in [
            (
                AuraActionResult::RejectedBeforeSend(AuraActionFailure::from_reason_for_test(
                    AuraFailureReason::TransportNotReady,
                )),
                PairBackendError::AuraStartOutcomeUnknown,
            ),
            (
                aura_unknown(AuraFailureReason::AcknowledgementTimedOut),
                PairBackendError::AuraAcknowledgementTimedOut,
            ),
        ] {
            let trace = Rc::new(RefCell::new(Vec::new()));
            let observer = MockObserver::ack_only(Rc::clone(&trace));
            let mut core = core(vec![accepted()], vec![aura_result], observer, trace);

            let first = core.start();
            assert_eq!(unknown_reason(first), expected);
            assert_eq!(core.start(), first);
            assert_eq!(core.jbl.commands.borrow().len(), 1);
            assert_eq!(core.aura.commands, ["on"]);
        }
    }

    #[test]
    fn stop_aura_unknown_latches_before_broadcast_stop_and_exit() {
        let trace = Rc::new(RefCell::new(Vec::new()));
        let observer = MockObserver::ack_only(Rc::clone(&trace));
        let mut core = core(
            Vec::new(),
            vec![aura_unknown(
                AuraFailureReason::AcknowledgementChannelClosed,
            )],
            observer,
            trace,
        );
        core.lifecycle = PairLifecycle::Linked;

        let first = core.stop();
        assert_eq!(
            unknown_reason(first),
            PairBackendError::AuraAcknowledgementChannelClosed
        );
        assert_eq!(core.stop(), first);
        assert!(core.jbl.commands.borrow().is_empty());
        assert_eq!(core.aura.commands, ["off"]);
    }

    #[test]
    fn gatt_stop_unknown_latches_before_exit() {
        let trace = Rc::new(RefCell::new(Vec::new()));
        let observer = MockObserver::with(
            vec![Ok(())],
            vec![Err(BroadcastResultFailure::TimedOut)],
            Rc::clone(&trace),
        );
        let mut core = core(
            Vec::new(),
            vec![AuraActionResult::Accepted],
            observer,
            trace,
        );
        core.lifecycle = PairLifecycle::Linked;

        let first = core.stop();
        assert_eq!(
            unknown_reason(first),
            PairBackendError::JblBroadcastResultTimedOut
        );
        assert_eq!(core.start(), first);
        assert!(core.jbl.commands.borrow().is_empty());
    }

    #[test]
    fn exit_unknown_latches_without_repeating_stop_sequence() {
        let trace = Rc::new(RefCell::new(Vec::new()));
        let observer = MockObserver::ack_only(Rc::clone(&trace));
        let mut core = core(
            vec![PlayTogetherWriteResult::OutcomeUnknown(
                JblError::NetworkUnreachable,
            )],
            vec![AuraActionResult::Accepted],
            observer,
            trace,
        );
        core.lifecycle = PairLifecycle::Linked;

        let first = core.stop();
        assert_eq!(
            unknown_reason(first),
            PairBackendError::JblExitOutcomeUnknown
        );
        assert_eq!(core.stop(), first);
        assert_eq!(core.jbl.commands.borrow().as_slice(), [JblCommand::Exit]);
        assert_eq!(core.aura.commands, ["off"]);
    }

    #[test]
    fn degraded_shutdown_runs_full_stop_then_tears_down_transport() {
        let trace = Rc::new(RefCell::new(Vec::new()));
        let observer = MockObserver::ack_only(Rc::clone(&trace));
        let mut core = core(
            vec![accepted()],
            vec![AuraActionResult::Accepted],
            observer,
            Rc::clone(&trace),
        );
        core.lifecycle = PairLifecycle::Degraded;

        let receipt = core.shutdown().receipt().expect("shutdown should succeed");
        assert_eq!(receipt.lifecycle(), PairLifecycle::ShuttingDown);
        assert_eq!(
            trace.borrow().as_slice(),
            [
                "transport_teardown",
                "aura_connect",
                "observer_arm_stop",
                "aura_off",
                "gatt_7957_stop",
                "jbl_exit",
                "transport_teardown",
            ]
        );
        assert_eq!(core.lifecycle, PairLifecycle::Offline);
    }

    #[test]
    fn recover_stop_tolerates_its_intentional_double_teardown() {
        let trace = Rc::new(RefCell::new(Vec::new()));
        let observer = MockObserver::ack_only(Rc::clone(&trace));
        let mut core = core(
            vec![accepted()],
            vec![AuraActionResult::Accepted],
            observer,
            Rc::clone(&trace),
        );
        core.lifecycle = PairLifecycle::Degraded;

        let receipt = core
            .recover_stop()
            .receipt()
            .expect("recover-stop should succeed");
        assert_eq!(receipt.lifecycle(), PairLifecycle::Ready);
        assert_eq!(core.aura.shutdown_calls, 2);
        assert_eq!(
            trace.borrow().as_slice(),
            [
                "transport_teardown",
                "transport_teardown",
                "aura_connect",
                "observer_arm_stop",
                "aura_off",
                "gatt_7957_stop",
                "jbl_exit",
            ]
        );
    }

    #[test]
    fn transport_teardown_preserves_uncertainty_latch() {
        let trace = Rc::new(RefCell::new(Vec::new()));
        let observer = MockObserver::ack_only(Rc::clone(&trace));
        let mut core = core(
            vec![accepted()],
            vec![aura_unknown(AuraFailureReason::AcknowledgementTimedOut)],
            observer,
            trace,
        );

        let unresolved = core.start();
        assert!(matches!(unresolved, PairActionResult::OutcomeUnknown(_)));
        core.teardown_transport()
            .expect("transport-only teardown should succeed");

        assert_eq!(core.aura.shutdown_calls, 2);
        assert_eq!(core.start(), unresolved);
        assert_eq!(core.stop(), unresolved);
        assert_eq!(core.jbl.commands.borrow().len(), 1);
        assert_eq!(core.aura.commands, ["on"]);
        assert!(core.unresolved.is_some());
    }

    #[test]
    fn linked_health_survives_an_intentionally_released_control_bearer() {
        let trace = Rc::new(RefCell::new(Vec::new()));
        let observer = MockObserver::ack_only(Rc::clone(&trace));
        let mut core = core(Vec::new(), Vec::new(), observer, trace);
        core.lifecycle = PairLifecycle::Linked;
        core.aura.health = AuraHealth::Offline;

        let health = core.health();
        assert_eq!(health.lifecycle(), PairLifecycle::Linked);
        assert_eq!(health.level(), crate::backend::PairHealthLevel::Healthy);
        assert_eq!(health.aura_transport(), AuraControlTransport::Unresolved);
        assert_eq!(
            health.aura_acquisition_route(),
            AuraAcquisitionRoute::Unresolved
        );
        assert!(!health.has_reported_error());
    }

    #[test]
    fn native_health_projects_the_sanitized_acquisition_route() {
        let trace = Rc::new(RefCell::new(Vec::new()));
        let observer = MockObserver::ack_only(Rc::clone(&trace));
        let mut core = core(Vec::new(), Vec::new(), observer, trace);
        core.lifecycle = PairLifecycle::Ready;
        core.aura.health = AuraHealth::Ready;

        let health = core.health();
        assert_eq!(health.aura_transport(), AuraControlTransport::Le);
        assert_eq!(
            health.aura_acquisition_route(),
            AuraAcquisitionRoute::FreshLe
        );
    }
}
