use std::env;
use std::fmt;
use std::io::{self, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use serde::Serialize;

use jbl_aura_link::discovery::{CandidateCardinality, DiscoverySummary};
use jbl_aura_link::web::DEFAULT_WEB_PORT;
use jbl_aura_link::{
    acquire_direct_control_lock, authentics_300_capabilities, build_native_service_actor,
    discover_avahi_summary, ensure_user_service, install_termination_handlers,
    validate_native_runtime, AudioSourceTarget, AudioSourceWriteResult, AvahiDiscoveryError,
    CapabilityMaturity, EqPresetTarget, EqPresetWriteResult, GroupStatus, InspectionReadError,
    InspectionSnapshot, JblError, JblLanClient, LocalActionEvidence, LocalActionName,
    LocalActionOutcome, LocalActionResult, LocalAuraAcquisitionRoute, LocalAuraTransport,
    LocalBackend, LocalClientError, LocalFailure, LocalHealthLevel, LocalLifecycle,
    LocalManagedState, LocalPairConfiguration, LocalPairMemberChannel, LocalPairMemberName,
    LocalPairMemberVerification, LocalServiceClient, LocalStatus, MediaSource, MediaSourceActivity,
    MediaStatus, MuteTarget, MuteWriteResult, PersonalListeningState, RuntimeConfig, ServiceActor,
    ServiceRuntimeError, TransportState, TransportStatus, VolumeWriteResult, WebServer,
    WebServerError, MAX_SAFE_DIRECT_VOLUME,
};

// One 3-second live smoke occasionally missed the initial cached ItemNew while
// Avahi itself still resolved the service. Five seconds remains bounded and
// matched the core's original conservative default without adding a retry.
const DISCOVERY_TIMEOUT: Duration = Duration::from_secs(5);

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
    Media,
    Inspect,
    Discover,
    VolumeSet,
    MuteSet,
    SourceSet,
    EqPresetSet,
    Capabilities,
    Help,
}

#[derive(Debug)]
struct Options {
    command: Command,
    config_path: Option<PathBuf>,
    json: bool,
    recovery_confirmed: bool,
    from_environment: bool,
    volume: Option<u8>,
    mute: Option<MuteTarget>,
    source: Option<AudioSourceTarget>,
    eq_preset: Option<EqPresetTarget>,
}

#[derive(Debug)]
enum CliError {
    InvalidArguments,
    OutputFailed,
    Configuration(JblError),
    Inspection(InspectionReadError),
    Discovery(AvahiDiscoveryError),
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
            Self::Inspection(error) => error.fmt(formatter),
            Self::Discovery(error) => error.fmt(formatter),
            Self::LocalClient(error) => error.fmt(formatter),
            Self::ServiceRuntime(error) => error.fmt(formatter),
            Self::WebServer(error) => error.fmt(formatter),
            Self::ActionNotAccepted => {
                formatter.write_str("the requested device action was not accepted")
            }
        }
    }
}

impl From<JblError> for CliError {
    fn from(error: JblError) -> Self {
        Self::Configuration(error)
    }
}

impl From<InspectionReadError> for CliError {
    fn from(error: InspectionReadError) -> Self {
        Self::Inspection(error)
    }
}

impl From<AvahiDiscoveryError> for CliError {
    fn from(error: AvahiDiscoveryError) -> Self {
        Self::Discovery(error)
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
    let mut volume = None;
    let mut mute = None;
    let mut source = None;
    let mut eq_preset = None;
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
            "media" => set_command(&mut command, Command::Media)?,
            "inspect" => set_command(&mut command, Command::Inspect)?,
            "discover" => set_command(&mut command, Command::Discover)?,
            "volume-set" if volume.is_none() => {
                set_command(&mut command, Command::VolumeSet)?;
                volume = arguments
                    .next()
                    .and_then(|value| value.to_str().and_then(|value| value.parse::<u8>().ok()))
                    .filter(|value| *value <= MAX_SAFE_DIRECT_VOLUME);
                if volume.is_none() {
                    return Err(CliError::InvalidArguments);
                }
            }
            "mute-set" if mute.is_none() => {
                set_command(&mut command, Command::MuteSet)?;
                mute = arguments.next().and_then(|value| match value.to_str()? {
                    "on" => Some(MuteTarget::On),
                    "off" => Some(MuteTarget::Off),
                    _ => None,
                });
                if mute.is_none() {
                    return Err(CliError::InvalidArguments);
                }
            }
            "source-set" if source.is_none() => {
                set_command(&mut command, Command::SourceSet)?;
                source = arguments.next().and_then(|value| match value.to_str()? {
                    "bluetooth" => Some(AudioSourceTarget::Bluetooth),
                    "aux" => Some(AudioSourceTarget::AuxIn),
                    "usb" => Some(AudioSourceTarget::UsbPlayback),
                    _ => None,
                });
                if source.is_none() {
                    return Err(CliError::InvalidArguments);
                }
            }
            "eq-preset-set" if eq_preset.is_none() => {
                set_command(&mut command, Command::EqPresetSet)?;
                eq_preset = arguments.next().and_then(|value| match value.to_str()? {
                    "signature" => Some(EqPresetTarget::Signature),
                    "vocal" => Some(EqPresetTarget::Vocal),
                    "energetic" => Some(EqPresetTarget::Energetic),
                    "chill" => Some(EqPresetTarget::Chill),
                    _ => None,
                });
                if eq_preset.is_none() {
                    return Err(CliError::InvalidArguments);
                }
            }
            "capabilities" => set_command(&mut command, Command::Capabilities)?,
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
        Command::RecoverStop
        | Command::VolumeSet
        | Command::MuteSet
        | Command::SourceSet
        | Command::EqPresetSet => recovery_confirmed && !from_environment,
        Command::Configure => recovery_confirmed && from_environment,
        _ => !recovery_confirmed && !from_environment,
    };
    if !confirmation_valid
        || (config_path.is_some()
            && !matches!(
                command,
                Command::Serve
                    | Command::Doctor
                    | Command::Group
                    | Command::Media
                    | Command::Inspect
                    | Command::VolumeSet
                    | Command::MuteSet
                    | Command::SourceSet
                    | Command::EqPresetSet
            ))
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
        volume,
        mute,
        source,
        eq_preset,
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
        Command::Media => run_media(options.config_path, options.json),
        Command::Inspect => run_inspect(options.config_path, options.json),
        Command::Discover => run_discover(options.json),
        Command::VolumeSet => run_volume_set(options),
        Command::MuteSet => run_mute_set(options),
        Command::SourceSet => run_source_set(options),
        Command::EqPresetSet => run_eq_preset_set(options),
        Command::Capabilities => run_capabilities(options.json),
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

fn run_media(config_path: Option<PathBuf>, json: bool) -> Result<(), CliError> {
    let config = RuntimeConfig::load(config_path)?;
    let media = build_client(&config)?.media_status(&config.expected_model)?;
    if json {
        print_json(&media)
    } else {
        print_media(media)
    }
}

fn run_inspect(config_path: Option<PathBuf>, json: bool) -> Result<(), CliError> {
    let config = RuntimeConfig::load(config_path)?;
    let snapshot = build_client(&config)?.inspection_snapshot(&config.expected_model)?;
    if json {
        print_json(&snapshot)
    } else {
        print_inspection(snapshot)
    }
}

fn run_discover(json: bool) -> Result<(), CliError> {
    let summary = discover_avahi_summary(DISCOVERY_TIMEOUT)?;
    if json {
        print_json(&summary)
    } else {
        outln!("{}", format_discovery_summary(&summary))
    }
}

fn format_discovery_summary(summary: &DiscoverySummary) -> String {
    let mut lines = vec![format!(
        "candidate_count={} cardinality={} timed_out={}",
        summary.candidate_count,
        candidate_cardinality_name(summary.cardinality),
        summary.timed_out
    )];
    for (index, candidate) in summary.candidates.iter().enumerate() {
        lines.push(format!(
            concat!(
                "candidate={} ipv4={} ipv6={} txt_fn={} txt_name={} ",
                "txt_id={} txt_uuid={} txt_md={} txt_model={}"
            ),
            index + 1,
            candidate.has_ipv4,
            candidate.has_ipv6,
            candidate.txt.has_fn,
            candidate.txt.has_name,
            candidate.txt.has_id,
            candidate.txt.has_uuid,
            candidate.txt.has_md,
            candidate.txt.has_model
        ));
    }
    lines.join("\n")
}

fn candidate_cardinality_name(value: CandidateCardinality) -> &'static str {
    match value {
        CandidateCardinality::None => "none",
        CandidateCardinality::One => "one",
        CandidateCardinality::Multiple => "multiple",
    }
}

fn print_inspection(snapshot: InspectionSnapshot) -> Result<(), CliError> {
    outln!(
        "features_known={} features_unknown={}",
        snapshot.feature_support.known.len(),
        snapshot.feature_support.unknown_key_count
    )?;
    for feature in snapshot.feature_support.known {
        outln!(
            "feature={} supported={}",
            feature.key.as_str(),
            if feature.supported { "yes" } else { "no" }
        )?;
    }
    outln!(
        "eq_presets={} eq_active_present={} eq_payload_fs={} eq_payload_gain={} eq_payload_q={} eq_payload_type={}",
        snapshot.eq.preset_count,
        snapshot.eq.active_present,
        snapshot.eq.fs_count,
        snapshot.eq.gain_count,
        snapshot.eq.q_count,
        snapshot.eq.type_count
    )?;
    let supported_sources = snapshot
        .audio_sources
        .support_sources
        .iter()
        .copied()
        .map(media_source_name)
        .collect::<Vec<_>>()
        .join(",");
    outln!(
        "audio_source_active={} audio_source_supported={}",
        media_source_name(snapshot.audio_sources.active),
        supported_sources
    )?;
    outln!(
        "personal_listening={} audio_sync={} media_source={} media_activity={}",
        personal_listening_name(snapshot.personal_listening),
        snapshot.audio_sync,
        media_source_name(snapshot.media_source_activity.source),
        media_source_activity_name(snapshot.media_source_activity.activity)
    )
}

#[derive(Serialize)]
struct VolumeActionReport {
    outcome: &'static str,
    volume: Option<u8>,
    muted: Option<bool>,
    error: Option<VolumeErrorCode>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum VolumeErrorCode {
    InvalidVolume,
    ConfigurationFailed,
    IdentityCheckFailed,
    TransportFailed,
    InvalidResponse,
    DeviceRejected,
    PreconditionFailed,
}

fn volume_error_code(error: &JblError) -> VolumeErrorCode {
    match error {
        JblError::InvalidVolume | JblError::VolumeSafetyLimitExceeded => {
            VolumeErrorCode::InvalidVolume
        }
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
        | JblError::TlsConfiguration => VolumeErrorCode::ConfigurationFailed,
        JblError::PeerCertificateMismatch
        | JblError::UnexpectedDeviceModel
        | JblError::DeviceInfoMissing
        | JblError::ControlDeviceInfoMissing => VolumeErrorCode::IdentityCheckFailed,
        JblError::NetworkUnreachable | JblError::HttpStatus(_) => VolumeErrorCode::TransportFailed,
        JblError::ResponseTooLarge
        | JblError::InvalidJson
        | JblError::InvalidXml
        | JblError::InvalidHttpResponse
        | JblError::BasicResponseNotObject
        | JblError::BasicResponseCodeMissing
        | JblError::BasicResponseCodeInvalid
        | JblError::MediaInfoMissing
        | JblError::MediaInfoInvalid
        | JblError::MediaVolumeMissing
        | JblError::MediaMuteMissing
        | JblError::MediaSourceMissing
        | JblError::MediaSourceInvalid
        | JblError::MediaSourceChanged
        | JblError::EqPresetInvalid
        | JblError::GroupInfoMissing
        | JblError::GroupMembersMissing
        | JblError::GroupDisabledInvalid
        | JblError::GroupMemberInvalid => VolumeErrorCode::InvalidResponse,
        JblError::ControlCommandRejected
        | JblError::DeviceReportedError
        | JblError::UpnpActionRejected => VolumeErrorCode::DeviceRejected,
        JblError::InvalidArguments
        | JblError::OutputFailed
        | JblError::UnsupportedMediaSource
        | JblError::PlaybackPreconditionFailed => VolumeErrorCode::PreconditionFailed,
    }
}

fn volume_error_name(error: VolumeErrorCode) -> &'static str {
    match error {
        VolumeErrorCode::InvalidVolume => "invalid_volume",
        VolumeErrorCode::ConfigurationFailed => "configuration_failed",
        VolumeErrorCode::IdentityCheckFailed => "identity_check_failed",
        VolumeErrorCode::TransportFailed => "transport_failed",
        VolumeErrorCode::InvalidResponse => "invalid_response",
        VolumeErrorCode::DeviceRejected => "device_rejected",
        VolumeErrorCode::PreconditionFailed => "precondition_failed",
    }
}

fn run_volume_set(options: Options) -> Result<(), CliError> {
    if !options.recovery_confirmed {
        return Err(CliError::InvalidArguments);
    }
    let volume = options.volume.ok_or(CliError::InvalidArguments)?;
    let mut lock = acquire_direct_control_lock()?;
    let config = RuntimeConfig::load(options.config_path)?;
    let result = build_client(&config)?.set_volume(&mut lock, &config.expected_model, volume);
    let (report, accepted) = match result {
        VolumeWriteResult::AlreadyAtTarget(playback) => (
            VolumeActionReport {
                outcome: "already_at_target",
                volume: playback.volume,
                muted: playback.muted,
                error: None,
            },
            true,
        ),
        VolumeWriteResult::Applied(playback) => (
            VolumeActionReport {
                outcome: "applied",
                volume: playback.volume,
                muted: playback.muted,
                error: None,
            },
            true,
        ),
        VolumeWriteResult::TargetObservedAfterUnknownWrite(playback) => (
            VolumeActionReport {
                outcome: "target_observed_after_unknown_write",
                volume: playback.volume,
                muted: playback.muted,
                error: Some(VolumeErrorCode::TransportFailed),
            },
            false,
        ),
        VolumeWriteResult::PostconditionFailed(playback) => (
            VolumeActionReport {
                outcome: "postcondition_failed",
                volume: playback.volume,
                muted: playback.muted,
                error: None,
            },
            false,
        ),
        VolumeWriteResult::RejectedBeforeSend(error) => (
            VolumeActionReport {
                outcome: "rejected_before_send",
                volume: None,
                muted: None,
                error: Some(volume_error_code(&error)),
            },
            false,
        ),
        VolumeWriteResult::OutcomeUnknown(error) => (
            VolumeActionReport {
                outcome: "outcome_unknown",
                volume: None,
                muted: None,
                error: Some(volume_error_code(&error)),
            },
            false,
        ),
    };
    if options.json {
        print_json(&report)?;
    } else {
        outln!(
            "outcome={} volume={} muted={} error={}",
            report.outcome,
            report
                .volume
                .map_or_else(|| "unknown".to_string(), |value| value.to_string()),
            report
                .muted
                .map_or("unknown", |value| if value { "yes" } else { "no" }),
            report.error.map(volume_error_name).unwrap_or("none")
        )?;
    }
    if accepted {
        Ok(())
    } else {
        Err(CliError::ActionNotAccepted)
    }
}

#[derive(Serialize)]
struct MuteActionReport {
    outcome: &'static str,
    muted: Option<bool>,
    error: Option<VolumeErrorCode>,
}

fn run_mute_set(options: Options) -> Result<(), CliError> {
    if !options.recovery_confirmed {
        return Err(CliError::InvalidArguments);
    }
    let target = options.mute.ok_or(CliError::InvalidArguments)?;
    let mut lock = acquire_direct_control_lock()?;
    let config = RuntimeConfig::load(options.config_path)?;
    let result = build_client(&config)?.set_mute(&mut lock, &config.expected_model, target);
    let (report, accepted) = match result {
        MuteWriteResult::AlreadyAtTarget(playback) => (
            MuteActionReport {
                outcome: "already_at_target",
                muted: playback.muted,
                error: None,
            },
            true,
        ),
        MuteWriteResult::Applied(playback) => (
            MuteActionReport {
                outcome: "applied",
                muted: playback.muted,
                error: None,
            },
            true,
        ),
        MuteWriteResult::TargetObservedAfterUnknownWrite(playback) => (
            MuteActionReport {
                outcome: "target_observed_after_unknown_write",
                muted: playback.muted,
                error: Some(VolumeErrorCode::TransportFailed),
            },
            false,
        ),
        MuteWriteResult::PostconditionFailed(playback) => (
            MuteActionReport {
                outcome: "postcondition_failed",
                muted: playback.muted,
                error: None,
            },
            false,
        ),
        MuteWriteResult::RejectedBeforeSend(error) => (
            MuteActionReport {
                outcome: "rejected_before_send",
                muted: None,
                error: Some(volume_error_code(&error)),
            },
            false,
        ),
        MuteWriteResult::OutcomeUnknown(error) => (
            MuteActionReport {
                outcome: "outcome_unknown",
                muted: None,
                error: Some(volume_error_code(&error)),
            },
            false,
        ),
    };
    if options.json {
        print_json(&report)?;
    } else {
        outln!(
            "outcome={} muted={} error={}",
            report.outcome,
            report
                .muted
                .map_or("unknown", |value| if value { "yes" } else { "no" }),
            report.error.map(volume_error_name).unwrap_or("none")
        )?;
    }
    if accepted {
        Ok(())
    } else {
        Err(CliError::ActionNotAccepted)
    }
}

#[derive(Serialize)]
struct SourceActionReport {
    outcome: &'static str,
    source: Option<&'static str>,
    error: Option<VolumeErrorCode>,
}

fn run_source_set(options: Options) -> Result<(), CliError> {
    let target = options.source.ok_or(CliError::InvalidArguments)?;
    let mut lock = acquire_direct_control_lock()?;
    let config = RuntimeConfig::load(options.config_path)?;
    let result = build_client(&config)?.set_audio_source(&mut lock, &config.expected_model, target);
    let (outcome, source, error, accepted) = match result {
        AudioSourceWriteResult::AlreadyAtTarget(source) => (
            "already_at_target",
            Some(media_source_name(source)),
            None,
            true,
        ),
        AudioSourceWriteResult::Applied(source) => {
            ("applied", Some(media_source_name(source)), None, true)
        }
        AudioSourceWriteResult::RejectedByDevice(source) => (
            "rejected_by_device",
            Some(media_source_name(source)),
            Some(VolumeErrorCode::DeviceRejected),
            false,
        ),
        AudioSourceWriteResult::TargetObservedAfterUnknownWrite(source) => (
            "target_observed_after_unknown_write",
            Some(media_source_name(source)),
            Some(VolumeErrorCode::TransportFailed),
            false,
        ),
        AudioSourceWriteResult::PostconditionFailed(source) => (
            "postcondition_failed",
            Some(media_source_name(source)),
            None,
            false,
        ),
        AudioSourceWriteResult::RejectedBeforeSend(error) => (
            "rejected_before_send",
            None,
            Some(volume_error_code(&error)),
            false,
        ),
        AudioSourceWriteResult::OutcomeUnknown(error) => (
            "outcome_unknown",
            None,
            Some(volume_error_code(&error)),
            false,
        ),
    };
    let report = SourceActionReport {
        outcome,
        source,
        error,
    };
    if options.json {
        print_json(&report)?;
    } else {
        outln!(
            "outcome={} source={} error={}",
            report.outcome,
            report.source.unwrap_or("unknown"),
            report.error.map(volume_error_name).unwrap_or("none")
        )?;
    }
    if accepted {
        Ok(())
    } else {
        Err(CliError::ActionNotAccepted)
    }
}

fn run_eq_preset_set(options: Options) -> Result<(), CliError> {
    let target = options.eq_preset.ok_or(CliError::InvalidArguments)?;
    let mut lock = acquire_direct_control_lock()?;
    let config = RuntimeConfig::load(options.config_path)?;
    let result = build_client(&config)?.set_eq_preset(&mut lock, &config.expected_model, target);
    let (outcome, error, accepted) = match result {
        EqPresetWriteResult::AlreadyAtTarget(_) => ("already_at_target", None, true),
        EqPresetWriteResult::Applied(_) => ("applied", None, true),
        EqPresetWriteResult::RejectedByDevice(_) => (
            "rejected_by_device",
            Some(VolumeErrorCode::DeviceRejected),
            false,
        ),
        EqPresetWriteResult::TargetObservedAfterUnknownWrite(_) => (
            "target_observed_after_unknown_write",
            Some(VolumeErrorCode::TransportFailed),
            false,
        ),
        EqPresetWriteResult::PostconditionFailed(_) => ("postcondition_failed", None, false),
        EqPresetWriteResult::RejectedBeforeSend(error) => (
            "rejected_before_send",
            Some(volume_error_code(&error)),
            false,
        ),
        EqPresetWriteResult::OutcomeUnknown(error) => {
            ("outcome_unknown", Some(volume_error_code(&error)), false)
        }
    };
    if options.json {
        print_json(&serde_json::json!({"outcome":outcome,"error":error.map(volume_error_name)}))?;
    } else {
        outln!(
            "outcome={} error={}",
            outcome,
            error.map(volume_error_name).unwrap_or("none")
        )?;
    }
    if accepted {
        Ok(())
    } else {
        Err(CliError::ActionNotAccepted)
    }
}

fn print_media(media: MediaStatus) -> Result<(), CliError> {
    outln!(
        "source={} playback={} transport_status={} volume={} muted={}",
        media_source_name(media.source),
        transport_state_name(media.playback.state),
        transport_status_name(media.playback.transport_status),
        media
            .playback
            .volume
            .map_or_else(|| "unknown".to_string(), |value| value.to_string()),
        media
            .playback
            .muted
            .map_or("unknown", |value| if value { "yes" } else { "no" })
    )
}

fn run_capabilities(json: bool) -> Result<(), CliError> {
    let capabilities = authentics_300_capabilities();
    if json {
        return print_json(&capabilities);
    }
    for capability in capabilities {
        outln!(
            "capability={} maturity={}",
            capability.id,
            capability_maturity_name(capability.maturity)
        )?;
    }
    Ok(())
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

fn media_source_name(value: MediaSource) -> &'static str {
    match value {
        MediaSource::Bluetooth => "bluetooth",
        MediaSource::Tv => "tv",
        MediaSource::Hdmi => "hdmi",
        MediaSource::Optical => "optical",
        MediaSource::Coaxial => "coaxial",
        MediaSource::AuxIn => "aux_in",
        MediaSource::UsbPlayback => "usb_playback",
        MediaSource::Multiroom => "multiroom",
        MediaSource::AirPlay2 => "airplay_2",
        MediaSource::Alexa => "alexa",
        MediaSource::Chromecast => "chromecast",
        MediaSource::HuaweiVoice => "huawei_voice",
        MediaSource::HuaweiMusic => "huawei_music",
        MediaSource::Unknown => "unknown",
    }
}

fn personal_listening_name(value: PersonalListeningState) -> &'static str {
    match value {
        PersonalListeningState::On => "on",
        PersonalListeningState::Off => "off",
        PersonalListeningState::Unknown => "unknown",
    }
}

fn media_source_activity_name(value: MediaSourceActivity) -> &'static str {
    match value {
        MediaSourceActivity::Playing => "playing",
        MediaSourceActivity::Paused => "paused",
        MediaSourceActivity::Stopped => "stopped",
        MediaSourceActivity::Unknown => "unknown",
    }
}

fn transport_state_name(value: TransportState) -> &'static str {
    match value {
        TransportState::Playing => "playing",
        TransportState::Paused => "paused",
        TransportState::Stopped => "stopped",
        TransportState::Transitioning => "transitioning",
        TransportState::NoMedia => "no_media",
        TransportState::Unknown => "unknown",
    }
}

fn transport_status_name(value: TransportStatus) -> &'static str {
    match value {
        TransportStatus::Ok => "ok",
        TransportStatus::ErrorOccurred => "error_occurred",
        TransportStatus::Unknown => "unknown",
    }
}

fn capability_maturity_name(value: CapabilityMaturity) -> &'static str {
    match value {
        CapabilityMaturity::ImplementedReadOnly => "implemented_read_only",
        CapabilityMaturity::ImplementedVerifiedWrite => "implemented_verified_write",
        CapabilityMaturity::ProtocolPortedResearchOnly => "protocol_ported_research_only",
        CapabilityMaturity::SerializerOnly => "serializer_only",
        CapabilityMaturity::EvidenceRequired => "evidence_required",
        CapabilityMaturity::NotAdvertisedByExactProfile => "not_advertised_by_exact_profile",
        CapabilityMaturity::Forbidden => "forbidden",
    }
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
        "       jbl-aura-link-rust discover [--json]\n",
        "       jbl-aura-link-rust [--config PATH] <serve|doctor|group|media|inspect> [--json]\n",
        "       jbl-aura-link-rust [--config PATH] volume-set VALUE --confirm [--json]\n",
        "       jbl-aura-link-rust [--config PATH] mute-set <on|off> --confirm [--json]\n",
        "       jbl-aura-link-rust [--config PATH] source-set <bluetooth|aux|usb> --confirm [--json]\n",
        "       jbl-aura-link-rust [--config PATH] eq-preset-set <signature|vocal|energetic|chill> --confirm [--json]\n",
        "       jbl-aura-link-rust capabilities [--json]\n",
        "\n",
        "Commands:\n",
        "  configure     Atomically install private Rust config from environment\n",
        "  serve         Run the loopback-only single-owner service\n",
        "  start         Ask the local service to start Play Together\n",
        "  stop          Ask the local service to stop Play Together\n",
        "  recover-stop  Explicit one-shot safe STOP recovery; requires --confirm\n",
        "  status        Read sanitized managed state from the local service (default)\n",
        "  discover      Run one fixed 5-second sanitized JBL mDNS scan\n",
        "  doctor        Direct read-only configuration and device check\n",
        "  group         Direct read-only retained membership check\n",
        "  media         Direct read-only source, transport, volume and mute check\n",
        "  inspect       Direct typed read-only OneOS inspection without raw values\n",
        "  volume-set    One bounded 0..9 UPnP write with exact-model readback\n",
        "  mute-set      One absolute UPnP mute write with exact-model readback\n",
        "  source-set    One dynamic-list-gated source write with bounded readback\n",
        "  eq-preset-set One non-custom seven-band preset write with bounded readback\n",
        "  capabilities  Show exact-model feature maturity without device I/O\n",
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
            ("media", Command::Media),
            ("inspect", Command::Inspect),
            ("discover", Command::Discover),
            ("capabilities", Command::Capabilities),
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
        assert!(parse_options_from(["volume-set", "9"]).is_err());
        let volume = parse_options_from(["volume-set", "9", "--confirm"]).unwrap();
        assert_eq!(volume.command, Command::VolumeSet);
        assert_eq!(volume.volume, Some(9));
        for rejected in ["-1", "10", "51", "100"] {
            assert!(parse_options_from(["volume-set", rejected, "--confirm"]).is_err());
        }
        assert!(parse_options_from(["volume-set", "9", "--confirm", "--confirm"]).is_err());
        assert!(parse_options_from(["mute-set", "on"]).is_err());
        let mute_on = parse_options_from(["mute-set", "on", "--confirm"]).unwrap();
        assert_eq!(mute_on.command, Command::MuteSet);
        assert_eq!(mute_on.mute, Some(MuteTarget::On));
        let mute_off = parse_options_from(["mute-set", "off", "--confirm"]).unwrap();
        assert_eq!(mute_off.command, Command::MuteSet);
        assert_eq!(mute_off.mute, Some(MuteTarget::Off));
        for rejected in ["toggle", "true", "false", "1", "0"] {
            assert!(parse_options_from(["mute-set", rejected, "--confirm"]).is_err());
        }
        assert!(parse_options_from(["mute-set", "on", "--confirm", "--confirm"]).is_err());
        for target in ["play", "pause", "stop", "next", "previous", "toggle"] {
            assert!(parse_options_from(["playback-set", target, "--confirm"]).is_err());
        }
        assert!(parse_options_from(["source-set", "aux"]).is_err());
        let aux = parse_options_from(["source-set", "aux", "--confirm"]).unwrap();
        assert_eq!(aux.command, Command::SourceSet);
        assert_eq!(aux.source, Some(AudioSourceTarget::AuxIn));
        let bluetooth = parse_options_from(["source-set", "bluetooth", "--confirm"]).unwrap();
        assert_eq!(bluetooth.source, Some(AudioSourceTarget::Bluetooth));
        let usb = parse_options_from(["source-set", "usb", "--confirm"]).unwrap();
        assert_eq!(usb.source, Some(AudioSourceTarget::UsbPlayback));
        for rejected in ["stop", "AUX", "Bluetooth", "USB", "aux ", "hdmi", "toggle"] {
            assert!(parse_options_from(["source-set", rejected, "--confirm"]).is_err());
        }
        assert!(parse_options_from(["source-set", "aux", "--confirm", "--confirm"]).is_err());
        assert!(parse_options_from(["eq-preset-set", "vocal"]).is_err());
        for (token, expected) in [
            ("signature", EqPresetTarget::Signature),
            ("vocal", EqPresetTarget::Vocal),
            ("energetic", EqPresetTarget::Energetic),
            ("chill", EqPresetTarget::Chill),
        ] {
            let options = parse_options_from(["eq-preset-set", token, "--confirm"]).unwrap();
            assert_eq!(options.command, Command::EqPresetSet);
            assert_eq!(options.eq_preset, Some(expected));
        }
        for rejected in [
            "custom", "0", "1", "VOCAL", "Vocal", "vocal ", "bass", "private",
        ] {
            assert!(parse_options_from(["eq-preset-set", rejected, "--confirm"]).is_err());
        }
        assert!(parse_options_from(["eq-preset-set", "vocal", "--confirm", "--confirm"]).is_err());
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
        assert!(parse_options_from(["inspect", "--config", "/private/path"]).is_ok());
    }

    #[test]
    fn discover_accepts_only_the_optional_json_flag() {
        let plain = parse_options_from(["discover"]).expect("discover should parse");
        assert_eq!(plain.command, Command::Discover);
        assert!(!plain.json);
        let json = parse_options_from(["discover", "--json"]).expect("JSON discover");
        assert_eq!(json.command, Command::Discover);
        assert!(json.json);
        assert!(parse_options_from(["discover", "--config", "/private/path"]).is_err());
        assert!(parse_options_from(["discover", "--confirm"]).is_err());
        assert!(parse_options_from(["discover", "--from-env"]).is_err());
        assert!(parse_options_from(["discover", "--json", "--json"]).is_err());
    }

    #[test]
    fn discovery_human_and_json_output_cannot_represent_private_record_values() {
        use jbl_aura_link::discovery::{
            CandidateCollector, ResolvedMdnsRecord, JBL_MDNS_SERVICE_TYPE,
        };

        let private_instance = "private-instance-marker";
        let private_txt = "private-txt-marker";
        let private_address = IpAddr::V4(Ipv4Addr::new(192, 0, 2, 77));
        let record = ResolvedMdnsRecord::new(
            JBL_MDNS_SERVICE_TYPE,
            private_instance,
            [private_address],
            [
                ("uuid", private_txt.as_bytes()),
                ("model", private_txt.as_bytes()),
            ],
        )
        .expect("synthetic discovery record should validate");
        let mut collector = CandidateCollector::new();
        collector.observe(record).expect("candidate should collect");
        let summary = collector.finish(false);
        let human = format_discovery_summary(&summary);
        let json = serde_json::to_string(&summary).expect("summary should serialize");

        for output in [&human, &json] {
            assert!(!output.contains(private_instance));
            assert!(!output.contains(private_txt));
            assert!(!output.contains(&private_address.to_string()));
        }
        assert!(human.contains("candidate_count=1 cardinality=one timed_out=false"));
        assert!(human.contains(
            "candidate=1 ipv4=true ipv6=false txt_fn=false txt_name=false txt_id=false txt_uuid=true txt_md=false txt_model=true"
        ));
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["candidate_count"], 1);
        assert_eq!(value["cardinality"], "one");
        assert_eq!(value["candidates"][0]["has_ipv4"], true);
        assert_eq!(value["candidates"][0]["txt"]["has_uuid"], true);
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
