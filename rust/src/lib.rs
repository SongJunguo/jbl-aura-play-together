#[cfg(target_os = "linux")]
mod aura_bluez;
mod aura_protocol;
#[cfg(target_os = "linux")]
pub mod aura_wake;
mod backend;
#[cfg(all(target_os = "linux", test))]
mod backend_legacy;
#[cfg(target_os = "linux")]
mod backend_native;
#[cfg(target_os = "linux")]
mod broadcast_gena;
pub mod capability;
pub mod client;
pub mod config;
pub mod control;
mod controller;
pub mod device_route;
pub mod discovery;
#[cfg(target_os = "linux")]
mod discovery_avahi;
pub mod eq;
pub mod error;
pub mod inspection;
#[cfg(target_os = "linux")]
mod jbl_gatt;
#[cfg(target_os = "linux")]
mod journal;
#[cfg(target_os = "linux")]
mod local_client;
pub mod media;
pub mod model;
pub mod oneos;
pub mod party_flow;
pub mod party_state;
mod private_file;
#[cfg(target_os = "linux")]
mod service_runtime;
mod tls;
pub mod web;
mod web_server;

pub use backend::{
    AuraAcquisitionRoute, AuraControlTransport, PairBackendKind, PairHealth, PairHealthLevel,
    PairLifecycle,
};
pub use capability::{authentics_300_capabilities, Capability, CapabilityMaturity};
pub use client::JblLanClient;
pub use config::RuntimeConfig;
pub use control::{
    BasicResponse, PlayTogetherCommand, PlayTogetherWriteOutcome, PlayTogetherWriteResult,
};
pub use controller::{
    ControllerAction, ControllerActionOutcome, ControllerActionResult, ControllerFailure,
    ControllerStatus, LastActionStatus, ManagedLiveState, PairConfigurationState,
    PairMemberChannel, PairMemberName, PairMemberStatus, PairMemberVerification,
};
#[cfg(target_os = "linux")]
pub use discovery_avahi::{discover_avahi_summary, AvahiDiscoveryError};
pub use eq::{EqPresetTarget, EqPresetWriteResult};
pub use error::JblError;
pub use inspection::{
    AudioSourceSummary, EqSummary, FeatureKey, FeatureSupportEntry, FeatureSupportSummary,
    InspectionError, InspectionReadError, InspectionSnapshot, MediaSourceActivity,
    MediaSourceActivitySummary, PersonalListeningState,
};
#[cfg(target_os = "linux")]
pub use local_client::{
    LocalActionEvidence, LocalActionName, LocalActionOutcome, LocalActionResult,
    LocalAuraAcquisitionRoute, LocalAuraTransport, LocalBackend, LocalClientError, LocalFailure,
    LocalHealth, LocalHealthLevel, LocalLastAction, LocalLifecycle, LocalManagedState,
    LocalPairConfiguration, LocalPairMember, LocalPairMemberChannel, LocalPairMemberName,
    LocalPairMemberVerification, LocalServiceClient, LocalStatus,
};
pub use media::{
    AudioSourceTarget, AudioSourceWriteResult, MediaSource, MediaStatus, MuteTarget,
    MuteWriteResult, PlaybackStatus, TransportState, TransportStatus, VolumeWriteResult,
    MAX_SAFE_DIRECT_VOLUME,
};
pub use model::{GroupStatus, SanitizedStatus};
pub use oneos::OneOsReadCommand;
#[cfg(target_os = "linux")]
pub use service_runtime::{
    acquire_direct_control_lock, build_native_service_actor, ensure_user_service,
    install_termination_handlers, validate_native_runtime, DirectControlLock, ServiceActor,
    ServiceRuntimeError,
};
#[cfg(target_os = "linux")]
pub use web_server::{WebServeError, WebServer, WebServerError, WebServerOptions};
