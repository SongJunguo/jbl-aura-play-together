//! Closed, clean-room OneOS network command inventory.
//!
//! These are command names traced through the pinned JBL One build.  This
//! module deliberately exposes no raw string constructor: adding a command
//! requires an explicit evidence review and a typed response parser.

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OneOsReadCommand {
    DeviceInfo,
    FeatureSupport,
    EqList,
    Eq,
    BatteryStatus,
    DeviceName,
    ProductSettings,
    AutoPowerOffTimer,
    BluetoothConfig,
    FeedbackToneConfig,
    GeneralConfig,
    BatterySavingMode,
    StreamingStatus,
    SleepTimer,
    PersonalListeningMode,
    MediaSourceStatus,
    RearSpeakerStatus,
    AudioSync,
    SmartMode,
    LightInfo,
    AuraCastGroupInfo,
    AuraCastGroupParameter,
    MediaSource,
    DeviceAudioSourceList,
    QSymphonyInfo,
    MultiroomInfo,
    RemoteControllerStyle,
    DeviceAuracastBroadcastInfo,
    ScanningAuracastBroadcast,
    SpotifyTapInfo,
}

impl OneOsReadCommand {
    pub const fn api_name(self) -> &'static str {
        match self {
            Self::DeviceInfo => "getDeviceInfo",
            Self::FeatureSupport => "getFeatureSupport",
            Self::EqList => "getEQList",
            Self::Eq => "getEQ",
            Self::BatteryStatus => "getBatteryStatus",
            Self::DeviceName => "getDeviceName",
            Self::ProductSettings => "getProdSetting",
            Self::AutoPowerOffTimer => "getAutoPowerOffTimer",
            Self::BluetoothConfig => "getBluetoothConfig",
            Self::FeedbackToneConfig => "getFeedbackToneConfig",
            Self::GeneralConfig => "getGernalConfig",
            Self::BatterySavingMode => "getBatterySavingMode",
            Self::StreamingStatus => "getStreamingStatus",
            Self::SleepTimer => "getSleepTimer",
            Self::PersonalListeningMode => "getPersonalListeningMode",
            Self::MediaSourceStatus => "getMediaSourceStatus",
            Self::RearSpeakerStatus => "getRearSpeakerStatus",
            Self::AudioSync => "getAudioSync",
            Self::SmartMode => "getSmartMode",
            Self::LightInfo => "getLightInfo",
            Self::AuraCastGroupInfo => "getAuraCastGroupInfo",
            Self::AuraCastGroupParameter => "getAuraCastGroupParameter",
            Self::MediaSource => "getMediaSource",
            Self::DeviceAudioSourceList => "getDeviceAudioSourceList",
            Self::QSymphonyInfo => "getQSymphonyInfo",
            Self::MultiroomInfo => "getMultiroomInfo",
            Self::RemoteControllerStyle => "getRemoteControllerStyle",
            Self::DeviceAuracastBroadcastInfo => "getDeviceAuracastBroadcastInfo",
            Self::ScanningAuracastBroadcast => "getScanningAuracastBroadcast",
            Self::SpotifyTapInfo => "getSpotifyTapInfo",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn high_value_exact_device_reads_have_fixed_names() {
        assert_eq!(OneOsReadCommand::DeviceInfo.api_name(), "getDeviceInfo");
        assert_eq!(OneOsReadCommand::EqList.api_name(), "getEQList");
        assert_eq!(OneOsReadCommand::Eq.api_name(), "getEQ");
        assert_eq!(OneOsReadCommand::MediaSource.api_name(), "getMediaSource");
        assert_eq!(
            OneOsReadCommand::AuraCastGroupInfo.api_name(),
            "getAuraCastGroupInfo"
        );
    }

    #[test]
    fn inventory_has_no_raw_command_escape_hatch() {
        let command = OneOsReadCommand::ProductSettings;
        assert_eq!(command.api_name(), "getProdSetting");
        assert!(!command.api_name().contains('&'));
        assert!(!command.api_name().contains('?'));
        assert!(!command.api_name().contains('='));
    }
}
