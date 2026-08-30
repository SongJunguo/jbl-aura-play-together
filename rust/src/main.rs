use std::env;
use std::fmt;
use std::io::{self, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::process::ExitCode;

use serde::Serialize;

use jbl_aura_link::web::DEFAULT_WEB_PORT;
use jbl_aura_link::{
    build_native_service_actor, ensure_user_service, install_termination_handlers,
    validate_native_runtime, GroupStatus, JblError, JblLanClient, LocalActionEvidence,
    LocalActionName, LocalActionOutcome, LocalActionResult, LocalAuraAcquisitionRoute,
    LocalAuraTransport, LocalBackend, LocalClientError, LocalFailure, LocalHealthLevel,
    LocalLifecycle, LocalManagedState, LocalPairConfiguration, LocalPairMemberChannel,
    LocalPairMemberName, LocalPairMemberVerification, LocalServiceClient, LocalStatus,
    RuntimeConfig, ServiceActor, ServiceRuntimeError, WebServer, WebServerError,
};

fn write_line(arguments: fmt::Arguments<'_>) -> Result<(), CliError> {
    let stdout = io::stdout();
    let mut output = stdout.lock();
    output
        .write_fmt(arguments)
        .and_then(|()| output.write_all(b"\n"))
        .map_err(|_| CliError::OutputFailed)
}

macro_rules! outln {
    ($($argument:tt)*) => {
        write_line(format_args!($($argument)*))
    };
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Command {
    Configure,
    Serve,
    Start,
    Stop,
    RecoverStop,
    Status,
    Doctor,
    Group,
    Help,
}

#[derive(Debug)]
struct Options {
    command: Command,
    config_path: Option<PathBuf>,
    json: bool,
    recovery_confirmed: bool,
    from_environment: bool,
}

#[derive(Debug)]
enum CliError {
    InvalidArguments,
    OutputFailed,
    Configuration(JblError),
    LocalClient(LocalClientError),
    ServiceRuntime(ServiceRuntimeError),
    WebServer(WebServerError),
    ActionNotAccepted,
}

impl fmt::Display for CliError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidArguments => formatter.write_str("invalid command-line arguments"),
            Self::OutputFailed => formatter.write_str("command output could not be written"),
            Self::Configuration(error) => error.fmt(formatter),
            Self::LocalClient(error) => error.fmt(formatter),
            Self::ServiceRuntime(error) => error.fmt(formatter),
            Self::WebServer(error) => error.fmt(formatter),
            Self::ActionNotAccepted => {
                formatter.write_str("the Play Together action was not accepted")
            }
        }
    }
}

impl From<JblError> for CliError {
    fn from(error: JblError) -> Self {
        Self::Configuration(error)
    }
}

impl From<LocalClientError> for CliError {
    fn from(error: LocalClientError) -> Self {
        Self::LocalClient(error)
    }
}

impl From<ServiceRuntimeError> for CliError {
    fn from(error: ServiceRuntimeError) -> Self {
        Self::ServiceRuntime(error)
    }
}

impl From<WebServerError> for CliError {
    fn from(error: WebServerError) -> Self {
        Self::WebServer(error)
    }
}

fn parse_options() -> Result<Options, CliError> {
    parse_options_from(env::args_os().skip(1))
}

fn parse_options_from(
    arguments: impl IntoIterator<Item = impl Into<std::ffi::OsString>>,
) -> Result<Options, CliError> {
    let mut command = None;
    let mut config_path = None;
    let mut json = false;
    let mut recovery_confirmed = false;
    let mut from_environment = false;
    let mut arguments = arguments.into_iter().map(Into::into);
    while let Some(argument) = arguments.next() {
        let argument = argument.to_str().ok_or(CliError::InvalidArguments)?;
        match argument {
            "configure" => set_command(&mut command, Command::Configure)?,
            "serve" => set_command(&mut command, Command::Serve)?,
            "start" => set_command(&mut command, Command::Start)?,
            "stop" => set_command(&mut command, Command::Stop)?,
            "recover-stop" => set_command(&mut command, Command::RecoverStop)?,
            "status" => set_command(&mut command, Command::Status)?,
            "doctor" => set_command(&mut command, Command::Doctor)?,
            "group" => set_command(&mut command, Command::Group)?,
            "help" | "-h" | "--help" => set_command(&mut command, Command::Help)?,
            "--json" if !json => json = true,
            "--confirm" if !recovery_confirmed => recovery_confirmed = true,
            "--from-env" if !from_environment => from_environment = true,
            "--config" if config_path.is_none() => {
                config_path = arguments.next().map(PathBuf::from);
                if config_path.is_none() {
                    return Err(CliError::InvalidArguments);
                }
            }
            _ => return Err(CliError::InvalidArguments),
        }
    }
    let command = command.unwrap_or(Command::Status);
    let confirmation_valid = match command {
        Command::RecoverStop => recovery_confirmed && !from_environment,
        Command::Configure => recovery_confirmed && from_environment,
        _ => !recovery_confirmed && !from_environment,
    };
    if !confirmation_valid
        || (config_path.is_some()
            && !matches!(command, Command::Serve | Command::Doctor | Command::Group))
        || (json && matches!(command, Command::Configure | Command::Serve | Command::Help))
    {
        return Err(CliError::InvalidArguments);
    }
    Ok(Options {
        command,
        config_path,
        json,
        recovery_confirmed,
        from_environment,
    })
}

fn set_command(slot: &mut Option<Command>, value: Command) -> Result<(), CliError> {
    if slot.replace(value).is_some() {
        return Err(CliError::InvalidArguments);
    }
    Ok(())
}

fn build_client(config: &RuntimeConfig) -> Result<JblLanClient, JblError> {
    JblLanClient::new(
        &config.address,
        &config.certificate,
        &config.private_key,
        &config.tls_sha256,
        config.timeout,
    )
}

fn run() -> Result<(), CliError> {
    let options = parse_options()?;
    match options.command {
        Command::Help => usage(),
        Command::Configure => run_configure(options),
        Command::Serve => run_service(options.config_path),
        Command::Start | Command::Stop | Command::RecoverStop | Command::Status => {
            run_local_command(options)
        }
        Command::Doctor => run_doctor(options.config_path, options.json),
        Command::Group => run_group(options.config_path, options.json),
    }
}

fn run_configure(options: Options) -> Result<(), CliError> {
    if !options.recovery_confirmed || !options.from_environment {
        return Err(CliError::InvalidArguments);
    }
    RuntimeConfig::install_private_default_from_environment()?;
    outln!("private_configuration=installed service_started=false")
}

fn run_service(config_path: Option<PathBuf>) -> Result<(), CliError> {
    let config = RuntimeConfig::load(config_path)?;
    let actor = build_native_service_actor(&config)?;
    let shutdown = install_termination_handlers()?;
    let bind = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), DEFAULT_WEB_PORT);
    let server = WebServer::bind(actor, bind, shutdown)?;
    match server.serve() {
        Ok(actor) => finish_service(actor, None),
        Err(failure) => {
            let (listener_error, actor) = failure.into_parts();
            finish_service(actor, Some(listener_error))
        }
    }
}

fn finish_service<A: ServiceActor>(
    mut actor: A,
    listener_error: Option<WebServerError>,
) -> Result<(), CliError> {
    // Device-role and transport safety has priority, but the listener error is
    // preserved whenever graceful shutdown completes successfully.
    match actor.shutdown_for_exit() {
        Err(error) => Err(error.into()),
        Ok(()) => listener_error.map_or(Ok(()), |error| Err(error.into())),
    }
}

fn run_local_command(options: Options) -> Result<(), CliError> {
    let client = LocalServiceClient::default();
    ensure_user_service(&client)?;
    match options.command {
        Command::Status => print_local_status(client.status()?, options.json),
        Command::Start => print_action(client.start()?, options.json),
        Command::Stop => print_action(client.stop()?, options.json),
        Command::RecoverStop => {
            if !options.recovery_confirmed {
                return Err(CliError::InvalidArguments);
            }
            print_action(client.recover_stop()?, options.json)
        }
        _ => unreachable!("only local service commands reach this function"),
    }
}

#[derive(Serialize)]
struct DoctorReport {
    configuration: &'static str,
    tls_pin: &'static str,
    lan: &'static str,
    pair_configuration: &'static str,
    members: usize,
    bluetooth_backend: &'static str,
}

fn run_doctor(config_path: Option<PathBuf>, json: bool) -> Result<(), CliError> {
    let config = RuntimeConfig::load(config_path)?;
    // Construction validates the native runtime without discovery or writes.
    validate_native_runtime(&config)?;
    let status = build_client(&config)?.sanitized_status(
        &config.expected_model,
        config.jbl_identity,
        config.aura_identity,
    )?;
    let report = DoctorReport {
        configuration: "ok",
        tls_pin: "present",
        lan: "ok",
        pair_configuration: if status.play_together.expected_pair_configured {
            "ready"
        } else {
            "not_ready"
        },
        members: status.play_together.member_count,
        bluetooth_backend: "native_constructed",
    };
    if json {
        print_json(&report)
    } else {
        outln!(
            "configuration={} tls_pin={} lan={}",
            report.configuration,
            report.tls_pin,
            report.lan
        )?;
        outln!(
            "pair_configuration={} members={}",
            report.pair_configuration,
            report.members
        )?;
        outln!("bluetooth_backend={}", report.bluetooth_backend)
    }
}

fn run_group(config_path: Option<PathBuf>, json: bool) -> Result<(), CliError> {
    let config = RuntimeConfig::load(config_path)?;
    let group = build_client(&config)?.pair_configuration_status(
        &config.expected_model,
        config.jbl_identity,
        config.aura_identity,
    )?;
    if json {
        print_json(&group)
    } else {
        print_group(group)
    }
}

fn print_group(group: GroupStatus) -> Result<(), CliError> {
    outln!(
        "pair_configuration={} members={}",
        if group.expected_pair_configured {
            "ready"
        } else {
            "not_ready"
        },
        group.member_count
    )?;
    for member in group.members {
        let channels = if member.channels.is_empty() {
            "-".to_string()
        } else {
            member.channels.join(",")
        };
        outln!("member={} channels={channels}", member.name)?;
    }
    Ok(())
}

fn print_local_status(status: LocalStatus, json: bool) -> Result<(), CliError> {
    if json {
        return print_json(&status);
    }
    outln!(
        "backend={} pair_configuration={} managed_state={} revision={}",
        backend_name(status.backend),
        pair_configuration_name(status.pair_configuration),
        managed_state_name(status.managed_state),
        status.revision
    )?;
    if let Some(health) = status.backend_health {
        outln!(
            "backend_health={} lifecycle={} reported_error={} aura_transport={} aura_acquisition_route={}",
            health_level_name(health.level),
            lifecycle_name(health.lifecycle),
            health.reported_error,
            aura_transport_name(health.aura_transport),
            acquisition_route_name(health.aura_acquisition_route)
        )?;
    } else {
        outln!("backend_health=unavailable")?;
    }
    for member in &status.members {
        let channels = member
            .channels
            .iter()
            .copied()
            .map(pair_member_channel_name)
            .collect::<Vec<_>>()
            .join(",");
        outln!(
            "member={} verification={} channels={}",
            pair_member_name(member.name),
            pair_member_verification_name(member.verification),
            channels
        )?;
    }
    if let Some(last) = status.last_action {
        outln!(
            "last_action={} outcome={} evidence={} failure={} revision={} age_ms={}",
            action_name(last.action),
            action_outcome_name(last.outcome),
            last.evidence.map(evidence_name).unwrap_or("none"),
            last.failure.map(failure_name).unwrap_or("none"),
            last.revision,
            last.age_ms
        )?;
    } else {
        outln!("last_action=none")?;
    }
    outln!(
        "unresolved_action={} consecutive_failures={}",
        status.unresolved_action,
        status.consecutive_failures
    )
}

fn print_action(result: LocalActionResult, json: bool) -> Result<(), CliError> {
    if json {
        print_json(&result)?;
    } else {
        outln!(
            "action={} outcome={} managed_state={} evidence={} failure={} revision={}",
            action_name(result.action),
            action_outcome_name(result.outcome),
            managed_state_name(result.managed_state),
            result.evidence.map(evidence_name).unwrap_or("none"),
            result.failure.map(failure_name).unwrap_or("none"),
            result.revision
        )?;
    }
    if result.succeeded() {
        Ok(())
    } else {
        Err(CliError::ActionNotAccepted)
    }
}

fn print_json(value: &impl Serialize) -> Result<(), CliError> {
    let serialized = serde_json::to_string(value).map_err(|_| CliError::OutputFailed)?;
    outln!("{serialized}")
}

fn backend_name(value: LocalBackend) -> &'static str {
    match value {
        LocalBackend::LegacyV04WholePair => "legacy_v04_whole_pair",
        LocalBackend::NativePair => "native_pair",
    }
}

fn pair_configuration_name(value: LocalPairConfiguration) -> &'static str {
    match value {
        LocalPairConfiguration::Ready => "ready",
        LocalPairConfiguration::NotReady => "not_ready",
        LocalPairConfiguration::Unavailable => "unavailable",
    }
}

fn managed_state_name(value: LocalManagedState) -> &'static str {
    match value {
        LocalManagedState::Unknown => "unknown",
        LocalManagedState::Offline => "offline",
        LocalManagedState::Ready => "ready",
        LocalManagedState::Linking => "linking",
        LocalManagedState::Linked => "linked",
        LocalManagedState::Unlinking => "unlinking",
        LocalManagedState::Recovering => "recovering",
        LocalManagedState::Degraded => "degraded",
        LocalManagedState::ShuttingDown => "shutting_down",
    }
}

fn lifecycle_name(value: LocalLifecycle) -> &'static str {
    match value {
        LocalLifecycle::Offline => "offline",
        LocalLifecycle::Initializing => "initializing",
        LocalLifecycle::Connecting => "connecting",
        LocalLifecycle::Ready => "ready",
        LocalLifecycle::Linking => "linking",
        LocalLifecycle::Linked => "linked",
        LocalLifecycle::Unlinking => "unlinking",
        LocalLifecycle::Degraded => "degraded",
        LocalLifecycle::Recovering => "recovering",
        LocalLifecycle::ShuttingDown => "shutting_down",
        LocalLifecycle::Failed => "failed",
    }
}

fn health_level_name(value: LocalHealthLevel) -> &'static str {
    match value {
        LocalHealthLevel::Healthy => "healthy",
        LocalHealthLevel::Transitioning => "transitioning",
        LocalHealthLevel::Degraded => "degraded",
        LocalHealthLevel::Unavailable => "unavailable",
    }
}

fn aura_transport_name(value: LocalAuraTransport) -> &'static str {
    match value {
        LocalAuraTransport::Le => "le",
        LocalAuraTransport::BrEdr => "br_edr",
        LocalAuraTransport::Unresolved => "unresolved",
        LocalAuraTransport::Unknown => "unknown",
    }
}

fn acquisition_route_name(value: LocalAuraAcquisitionRoute) -> &'static str {
    match value {
        LocalAuraAcquisitionRoute::StableDirect => "stable_direct",
        LocalAuraAcquisitionRoute::A2dpWakeThenStable => "a2dp_wake_then_stable",
        LocalAuraAcquisitionRoute::FreshLe => "fresh_le",
        LocalAuraAcquisitionRoute::Unresolved => "unresolved",
    }
}

fn action_name(value: LocalActionName) -> &'static str {
    match value {
        LocalActionName::Start => "start",
        LocalActionName::Stop => "stop",
        LocalActionName::Shutdown => "shutdown",
        LocalActionName::RecoverStop => "recover_stop",
    }
}

fn pair_member_name(value: LocalPairMemberName) -> &'static str {
    match value {
        LocalPairMemberName::JblAuthentics300 => "JBL Authentics 300",
        LocalPairMemberName::AuraStudio5 => "Aura Studio 5",
    }
}

fn pair_member_verification_name(value: LocalPairMemberVerification) -> &'static str {
    match value {
        LocalPairMemberVerification::Verified => "verified",
        LocalPairMemberVerification::NotVerified => "not_verified",
        LocalPairMemberVerification::Unavailable => "unavailable",
    }
}

fn pair_member_channel_name(value: LocalPairMemberChannel) -> &'static str {
    match value {
        LocalPairMemberChannel::FrontLeft => "front_left",
        LocalPairMemberChannel::FrontRight => "front_right",
        LocalPairMemberChannel::Left => "left",
        LocalPairMemberChannel::Right => "right",
        LocalPairMemberChannel::Mono => "mono",
        LocalPairMemberChannel::Stereo => "stereo",
        LocalPairMemberChannel::Unknown => "unknown",
    }
}

fn action_outcome_name(value: LocalActionOutcome) -> &'static str {
    match value {
        LocalActionOutcome::Accepted => "accepted",
        LocalActionOutcome::AcceptedUnconfirmed => "accepted_unconfirmed",
        LocalActionOutcome::Idempotent => "idempotent",
        LocalActionOutcome::RejectedBeforeSend => "rejected_before_send",
        LocalActionOutcome::OutcomeUnknown => "outcome_unknown",
        LocalActionOutcome::PostconditionFailed => "postcondition_failed",
    }
}

fn evidence_name(value: LocalActionEvidence) -> &'static str {
    match value {
        LocalActionEvidence::LocalSessionState => "local_session_state",
        LocalActionEvidence::LifecycleAcknowledgement => "lifecycle_acknowledgement",
        LocalActionEvidence::BroadcastAcknowledgementOnly => "broadcast_acknowledgement_only",
        LocalActionEvidence::BroadcastBusinessNotification => "broadcast_business_notification",
    }
}

fn failure_name(value: LocalFailure) -> &'static str {
    match value {
        LocalFailure::PairConfigurationUnavailable => "pair_configuration_unavailable",
        LocalFailure::ExpectedPairNotConfigured => "expected_pair_not_configured",
        LocalFailure::BackendRejectedBeforeSend => "backend_rejected_before_send",
        LocalFailure::AuraInvalidConfiguration => "aura_invalid_configuration",
        LocalFailure::AuraRuntimeUnavailable => "aura_runtime_unavailable",
        LocalFailure::AuraAdapterUnavailable => "aura_adapter_unavailable",
        LocalFailure::AuraDiscoveryUnavailable => "aura_discovery_unavailable",
        LocalFailure::AuraVerifiedAdvertisementNotFound => "aura_verified_advertisement_not_found",
        LocalFailure::AuraDeviceConnectionFailed => "aura_device_connection_failed",
        LocalFailure::WakeProfileConnectFailed => "wake_profile_connect_failed",
        LocalFailure::WakeFddfTimedOut => "wake_fddf_timed_out",
        LocalFailure::WakeFddfInvalid => "wake_fddf_invalid",
        LocalFailure::WakeFddfUnavailable => "wake_fddf_unavailable",
        LocalFailure::WakeProfileReleaseFailed => "wake_profile_release_failed",
        LocalFailure::AuraGattProfileInvalid => "aura_gatt_profile_invalid",
        LocalFailure::AuraNotificationSetupFailed => "aura_notification_setup_failed",
        LocalFailure::AuraTransportNotReady => "aura_transport_not_ready",
        LocalFailure::AuraNotificationQueueInvalid => "aura_notification_queue_invalid",
        LocalFailure::AuraDisconnectFailed => "aura_disconnect_failed",
        LocalFailure::AuraWriteUnknown => "aura_write_unknown",
        LocalFailure::AuraAckTimeout => "aura_ack_timeout",
        LocalFailure::AuraAckChannelClosed => "aura_ack_channel_closed",
        LocalFailure::AuraUnexpectedAck => "aura_unexpected_ack",
        LocalFailure::JblEnterOutcomeUnknown => "jbl_enter_outcome_unknown",
        LocalFailure::JblExitOutcomeUnknown => "jbl_exit_outcome_unknown",
        LocalFailure::JblBroadcastResultTimedOut => "jbl_broadcast_result_timed_out",
        LocalFailure::JblBroadcastResultUnavailable => "jbl_broadcast_result_unavailable",
        LocalFailure::JblBroadcastResultRejected => "jbl_broadcast_result_rejected",
        LocalFailure::AuraStartOutcomeUnknown => "aura_start_outcome_unknown",
        LocalFailure::BackendOutcomeUnknown => "backend_outcome_unknown",
        LocalFailure::UnexpectedBackendLifecycle => "unexpected_backend_lifecycle",
        LocalFailure::MembershipPostconditionFailed => "membership_postcondition_failed",
        LocalFailure::UnresolvedPriorAction => "unresolved_prior_action",
        LocalFailure::RecoveryNotAllowed => "recovery_not_allowed",
        LocalFailure::JournalUnavailable => "journal_unavailable",
        LocalFailure::JournalCommitFailed => "journal_commit_failed",
    }
}

fn usage() -> Result<(), CliError> {
    outln!(concat!(
        "Usage: jbl-aura-link-rust [--json] <start|stop|status>\n",
        "       jbl-aura-link-rust recover-stop --confirm [--json]\n",
        "       jbl-aura-link-rust configure --from-env --confirm\n",
        "       jbl-aura-link-rust [--config PATH] <serve|doctor|group> [--json]\n",
        "\n",
        "Commands:\n",
        "  configure     Atomically install private Rust config from environment\n",
        "  serve         Run the loopback-only single-owner service\n",
        "  start         Ask the local service to start Play Together\n",
        "  stop          Ask the local service to stop Play Together\n",
        "  recover-stop  Explicit one-shot safe STOP recovery; requires --confirm\n",
        "  status        Read sanitized managed state from the local service (default)\n",
        "  doctor        Direct read-only configuration and device check\n",
        "  group         Direct read-only retained membership check\n",
        "  help          Show this text\n",
        "\n",
        "start/stop/status/recover-stop never open a device backend. If needed,\n",
        "they start jbl-aura-link-rust.service once and use only loopback HTTP."
    ))
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            let stderr = io::stderr();
            let mut output = stderr.lock();
            let _ = writeln!(output, "jbl-aura-link-rust: {error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;

    use super::*;

    struct ExitActor {
        shutdown_calls: Arc<AtomicU64>,
        result: Result<(), ServiceRuntimeError>,
    }

    impl jbl_aura_link::web::WebActor for ExitActor {
        fn status(&mut self) -> jbl_aura_link::ControllerStatus {
            panic!("exit fixture must not read status")
        }

        fn mutate_if_revision(
            &mut self,
            _expected_revision: u64,
            _mutation: jbl_aura_link::web::WebMutation,
        ) -> Result<jbl_aura_link::ControllerActionResult, jbl_aura_link::web::RevisionConflict>
        {
            panic!("exit fixture must not mutate")
        }
    }

    impl ServiceActor for ExitActor {
        fn shutdown_for_exit(&mut self) -> Result<(), ServiceRuntimeError> {
            self.shutdown_calls.fetch_add(1, Ordering::Relaxed);
            self.result
        }
    }

    fn exit_actor(result: Result<(), ServiceRuntimeError>) -> (ExitActor, Arc<AtomicU64>) {
        let shutdown_calls = Arc::new(AtomicU64::new(0));
        (
            ExitActor {
                shutdown_calls: Arc::clone(&shutdown_calls),
                result,
            },
            shutdown_calls,
        )
    }

    #[test]
    fn command_surface_and_default_status_are_stable() {
        assert_eq!(
            parse_options_from(Vec::<&str>::new()).unwrap().command,
            Command::Status
        );
        for (name, expected) in [
            ("serve", Command::Serve),
            ("start", Command::Start),
            ("stop", Command::Stop),
            ("status", Command::Status),
            ("doctor", Command::Doctor),
            ("group", Command::Group),
            ("help", Command::Help),
        ] {
            assert_eq!(parse_options_from([name]).unwrap().command, expected);
        }
    }

    #[test]
    fn recovery_requires_exact_standalone_confirmation() {
        assert!(parse_options_from(["recover-stop"]).is_err());
        assert!(parse_options_from(["recover-stop", "--confirm=true"]).is_err());
        let options = parse_options_from(["recover-stop", "--confirm"]).unwrap();
        assert_eq!(options.command, Command::RecoverStop);
        assert!(options.recovery_confirmed);
        assert!(parse_options_from(["stop", "--confirm"]).is_err());
    }

    #[test]
    fn configure_requires_environment_mode_and_confirmation() {
        assert!(parse_options_from(["configure"]).is_err());
        assert!(parse_options_from(["configure", "--from-env"]).is_err());
        assert!(parse_options_from(["configure", "--confirm"]).is_err());
        let options =
            parse_options_from(["configure", "--from-env", "--confirm"]).expect("configure");
        assert_eq!(options.command, Command::Configure);
        assert!(options.from_environment && options.recovery_confirmed);
        assert!(parse_options_from(["configure", "--from-env", "--confirm", "--json"]).is_err());
    }

    #[test]
    fn local_commands_cannot_accept_a_private_config_path() {
        for command in ["start", "stop", "status", "recover-stop"] {
            let mut arguments = vec![command, "--config", "/private/path"];
            if command == "recover-stop" {
                arguments.push("--confirm");
            }
            assert!(parse_options_from(arguments).is_err());
        }
        assert!(parse_options_from(["serve", "--config", "/private/path"]).is_ok());
    }

    #[test]
    fn diagnostic_failure_labels_are_fixed_and_non_identifying() {
        for (failure, expected) in [
            (
                LocalFailure::AuraInvalidConfiguration,
                "aura_invalid_configuration",
            ),
            (
                LocalFailure::AuraRuntimeUnavailable,
                "aura_runtime_unavailable",
            ),
            (
                LocalFailure::AuraTransportNotReady,
                "aura_transport_not_ready",
            ),
            (
                LocalFailure::AuraNotificationQueueInvalid,
                "aura_notification_queue_invalid",
            ),
            (LocalFailure::AuraDisconnectFailed, "aura_disconnect_failed"),
            (
                LocalFailure::WakeProfileConnectFailed,
                "wake_profile_connect_failed",
            ),
            (LocalFailure::WakeFddfTimedOut, "wake_fddf_timed_out"),
            (LocalFailure::WakeFddfInvalid, "wake_fddf_invalid"),
            (LocalFailure::WakeFddfUnavailable, "wake_fddf_unavailable"),
            (
                LocalFailure::WakeProfileReleaseFailed,
                "wake_profile_release_failed",
            ),
            (LocalFailure::AuraWriteUnknown, "aura_write_unknown"),
            (LocalFailure::AuraAckTimeout, "aura_ack_timeout"),
            (
                LocalFailure::AuraAckChannelClosed,
                "aura_ack_channel_closed",
            ),
            (LocalFailure::AuraUnexpectedAck, "aura_unexpected_ack"),
            (
                LocalFailure::JblEnterOutcomeUnknown,
                "jbl_enter_outcome_unknown",
            ),
            (
                LocalFailure::JblExitOutcomeUnknown,
                "jbl_exit_outcome_unknown",
            ),
            (
                LocalFailure::JblBroadcastResultTimedOut,
                "jbl_broadcast_result_timed_out",
            ),
            (
                LocalFailure::JblBroadcastResultUnavailable,
                "jbl_broadcast_result_unavailable",
            ),
            (
                LocalFailure::JblBroadcastResultRejected,
                "jbl_broadcast_result_rejected",
            ),
            (
                LocalFailure::AuraStartOutcomeUnknown,
                "aura_start_outcome_unknown",
            ),
        ] {
            assert_eq!(failure_name(failure), expected);
            assert!(expected
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte == b'_'));
        }
    }

    #[test]
    fn unconfirmed_success_is_explicit_and_uses_fixed_evidence_text() {
        assert_eq!(
            action_outcome_name(LocalActionOutcome::AcceptedUnconfirmed),
            "accepted_unconfirmed"
        );
        assert_eq!(
            evidence_name(LocalActionEvidence::BroadcastAcknowledgementOnly),
            "broadcast_acknowledgement_only"
        );
        assert_eq!(
            evidence_name(LocalActionEvidence::BroadcastBusinessNotification),
            "broadcast_business_notification"
        );
    }

    #[test]
    fn service_exit_always_shuts_down_and_preserves_listener_error_after_success() {
        let (actor, calls) = exit_actor(Ok(()));
        assert!(finish_service(actor, None).is_ok());
        assert_eq!(calls.load(Ordering::Relaxed), 1);

        let (actor, calls) = exit_actor(Ok(()));
        assert!(matches!(
            finish_service(actor, Some(WebServerError::AcceptFailed)),
            Err(CliError::WebServer(WebServerError::AcceptFailed))
        ));
        assert_eq!(calls.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn shutdown_safety_error_has_priority_over_simultaneous_listener_error() {
        let (actor, calls) = exit_actor(Err(ServiceRuntimeError::GracefulShutdownOutcomeUnknown));
        assert!(matches!(
            finish_service(actor, Some(WebServerError::AcceptFailed)),
            Err(CliError::ServiceRuntime(
                ServiceRuntimeError::GracefulShutdownOutcomeUnknown
            ))
        ));
        assert_eq!(calls.load(Ordering::Relaxed), 1);
    }
}
