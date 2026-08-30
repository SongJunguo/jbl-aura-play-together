use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JblError {
    ConfigUnavailable,
    ConfigPermissions,
    ConfigTooLarge,
    InvalidConfig,
    MissingSetting(&'static str),
    InvalidTimeout,
    InvalidAddress,
    CertificateUnavailable,
    CertificatePermissions,
    PrivateKeyUnavailable,
    PrivateKeyPermissions,
    InvalidTlsFingerprint,
    InvalidClientIdentity,
    CredentialFileInvalid,
    CredentialTooLarge,
    TlsConfiguration,
    PeerCertificateMismatch,
    NetworkUnreachable,
    HttpStatus(u16),
    ResponseTooLarge,
    InvalidJson,
    InvalidXml,
    InvalidHttpResponse,
    BasicResponseNotObject,
    BasicResponseCodeMissing,
    BasicResponseCodeInvalid,
    ControlCommandRejected,
    DeviceInfoMissing,
    ControlDeviceInfoMissing,
    UnexpectedDeviceModel,
    DeviceReportedError,
    MediaInfoMissing,
    MediaInfoInvalid,
    MediaVolumeMissing,
    MediaMuteMissing,
    MediaSourceMissing,
    MediaSourceInvalid,
    UnsupportedMediaSource,
    PlaybackPreconditionFailed,
    MediaSourceChanged,
    UpnpActionRejected,
    EqPresetInvalid,
    InvalidVolume,
    VolumeSafetyLimitExceeded,
    GroupInfoMissing,
    GroupMembersMissing,
    GroupDisabledInvalid,
    GroupMemberInvalid,
    InvalidArguments,
    OutputFailed,
}

impl fmt::Display for JblError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::ConfigUnavailable => "private configuration is unavailable",
            Self::ConfigPermissions => "private configuration permissions allow another POSIX user",
            Self::ConfigTooLarge => "private configuration exceeded the size limit",
            Self::InvalidConfig => "private configuration is invalid",
            Self::MissingSetting(name) => return write!(formatter, "missing setting: {name}"),
            Self::InvalidTimeout => "JBL local API timeout must be 1 through 30 seconds",
            Self::InvalidAddress => "device address must be an IP literal",
            Self::CertificateUnavailable => "client certificate is unavailable",
            Self::CertificatePermissions => {
                "client certificate permissions allow another POSIX user"
            }
            Self::PrivateKeyUnavailable => "client private key is unavailable",
            Self::PrivateKeyPermissions => {
                "client private key permissions allow another POSIX user"
            }
            Self::InvalidTlsFingerprint => {
                "server TLS fingerprint must contain exactly 64 hexadecimal digits"
            }
            Self::InvalidClientIdentity => "client certificate or private key is invalid",
            Self::CredentialFileInvalid => "client credential must be an owner-only regular file",
            Self::CredentialTooLarge => "client credential exceeded the size limit",
            Self::TlsConfiguration => "TLS configuration failed",
            Self::PeerCertificateMismatch => "server TLS certificate fingerprint did not match",
            Self::NetworkUnreachable => "JBL local API is unreachable",
            Self::HttpStatus(_) => "JBL local API returned an unsuccessful HTTP status",
            Self::ResponseTooLarge => "JBL local API response exceeded the size limit",
            Self::InvalidJson => "JBL local API returned invalid JSON",
            Self::InvalidXml => "JBL UPnP service returned invalid XML",
            Self::InvalidHttpResponse => "JBL local API returned an invalid HTTP response",
            Self::BasicResponseNotObject => "JBL control response was not a JSON object",
            Self::BasicResponseCodeMissing => "JBL control response omitted error_code",
            Self::BasicResponseCodeInvalid => "JBL control response contained invalid error_code",
            Self::ControlCommandRejected => "JBL rejected the control command",
            Self::DeviceInfoMissing => "JBL device information is missing",
            Self::ControlDeviceInfoMissing => "JBL model identity is missing",
            Self::UnexpectedDeviceModel => "the discovered JBL model did not match configuration",
            Self::DeviceReportedError => "JBL reported a command error",
            Self::MediaInfoMissing => "JBL UPnP media information is missing",
            Self::MediaInfoInvalid => "JBL UPnP media information is invalid",
            Self::MediaVolumeMissing => "JBL UPnP media volume is missing",
            Self::MediaMuteMissing => "JBL UPnP media mute state is missing",
            Self::MediaSourceMissing => "JBL media source is missing",
            Self::MediaSourceInvalid => "JBL media source is invalid",
            Self::UnsupportedMediaSource => "the current JBL media source is not supported",
            Self::PlaybackPreconditionFailed => "the JBL playback precondition was not satisfied",
            Self::MediaSourceChanged => "the JBL media source changed during the operation",
            Self::UpnpActionRejected => "JBL rejected the UPnP action",
            Self::EqPresetInvalid => "JBL EQ preset data is invalid",
            Self::InvalidVolume => "volume must be an integer from 0 through 100",
            Self::VolumeSafetyLimitExceeded => {
                "volume exceeds the current direct-control safety limit"
            }
            Self::GroupInfoMissing => "JBL group information is missing",
            Self::GroupMembersMissing => "JBL group members are missing",
            Self::GroupDisabledInvalid => "JBL group disabled state is missing or invalid",
            Self::GroupMemberInvalid => "JBL group member data is invalid",
            Self::InvalidArguments => "invalid command-line arguments",
            Self::OutputFailed => "command output could not be written",
        };
        formatter.write_str(message)
    }
}

impl std::error::Error for JblError {}
