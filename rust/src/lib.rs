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
pub mod client;
pub mod config;
pub mod control;
mod controller;
pub mod error;
#[cfg(target_os = "linux")]
mod jbl_gatt;
#[cfg(target_os = "linux")]
mod journal;
#[cfg(target_os = "linux")]
mod local_client;
pub mod model;
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
pub use error::JblError;
#[cfg(target_os = "linux")]
pub use local_client::{
    LocalActionEvidence, LocalActionName, LocalActionOutcome, LocalActionResult,
    LocalAuraAcquisitionRoute, LocalAuraTransport, LocalBackend, LocalClientError, LocalFailure,
    LocalHealth, LocalHealthLevel, LocalLastAction, LocalLifecycle, LocalManagedState,
    LocalPairConfiguration, LocalPairMember, LocalPairMemberChannel, LocalPairMemberName,
    LocalPairMemberVerification, LocalServiceClient, LocalStatus,
};
pub use model::{GroupStatus, SanitizedStatus};
#[cfg(target_os = "linux")]
pub use service_runtime::{
    build_native_service_actor, ensure_user_service, install_termination_handlers,
    validate_native_runtime, ServiceActor, ServiceRuntimeError,
};
#[cfg(target_os = "linux")]
pub use web_server::{WebServeError, WebServer, WebServerError, WebServerOptions};
