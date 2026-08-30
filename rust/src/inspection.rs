//! Privacy-safe, read-only summaries for bounded OneOS inspection responses.
//!
//! The wire types remain private. Public DTOs contain only allowlisted enums,
//! booleans, integers and collection sizes; device-provided names, IDs, raw
//! feature keys and numeric EQ values cannot be represented in the output.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::Number;

use crate::error::JblError;
use crate::media::MediaSource;

pub(crate) const MAX_INSPECTION_RESPONSE_BYTES: u64 = 64 * 1024;
const MAX_FEATURE_ENTRIES: usize = 64;
const MAX_EQ_PRESETS: usize = 32;
const MAX_EQ_BANDS: usize = 64;
const MAX_AUDIO_SOURCES: usize = 32;
const MAX_PRIVATE_LABEL_BYTES: usize = 128;
const MAX_SOURCE_TOKEN_BYTES: usize = 40;

/// Closed failures that never contain device-provided text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InspectionError {
    ResponseTooLarge,
    InvalidJson,
    InvalidShape,
    DeviceReportedError,
    LimitExceeded,
    InvalidValue,
}

impl fmt::Display for InspectionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::ResponseTooLarge => "inspection response exceeded the size limit",
            Self::InvalidJson => "inspection response was invalid JSON",
            Self::InvalidShape => "inspection response had an invalid shape",
            Self::DeviceReportedError => "device reported an inspection error",
            Self::LimitExceeded => "inspection response exceeded a structural limit",
            Self::InvalidValue => "inspection response contained an invalid value",
        })
    }
}

impl std::error::Error for InspectionError {}

/// Transport/identity failures and typed response failures remain distinct.
#[derive(Debug)]
pub enum InspectionReadError {
    Device(JblError),
    Response(InspectionError),
}

impl fmt::Display for InspectionReadError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Device(error) => error.fmt(formatter),
            Self::Response(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for InspectionReadError {}

impl From<JblError> for InspectionReadError {
    fn from(error: JblError) -> Self {
        Self::Device(error)
    }
}

impl From<InspectionError> for InspectionReadError {
    fn from(error: InspectionError) -> Self {
        Self::Response(error)
    }
}

#[derive(Deserialize)]
#[serde(untagged)]
enum WireErrorCode {
    Integer(i64),
    String(String),
}

fn ensure_success(code: &WireErrorCode) -> Result<(), InspectionError> {
    match code {
        WireErrorCode::Integer(0) => Ok(()),
        WireErrorCode::String(value) if value == "0" => Ok(()),
        WireErrorCode::Integer(_) | WireErrorCode::String(_) => {
            Err(InspectionError::DeviceReportedError)
        }
    }
}

fn parse_bounded<T>(payload: &[u8]) -> Result<T, InspectionError>
where
    T: for<'de> Deserialize<'de>,
{
    if payload.len() as u64 > MAX_INSPECTION_RESPONSE_BYTES {
        return Err(InspectionError::ResponseTooLarge);
    }
    serde_json::from_slice(payload).map_err(|error| {
        if error.is_syntax() || error.is_eof() {
            InspectionError::InvalidJson
        } else {
            InspectionError::InvalidShape
        }
    })
}

fn bounded_private_label(value: &str) -> bool {
    !value.is_empty() && value.len() <= MAX_PRIVATE_LABEL_BYTES
}

fn bounded_ascii_token(value: &str, maximum: usize) -> bool {
    !value.is_empty() && value.len() <= maximum && value.is_ascii()
}

/// The exact feature keys observed on the current Authentics 300 read surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum FeatureKey {
    #[serde(rename = "OOBE_encryption")]
    OobeEncryption,
    #[serde(rename = "auracast_sq_mode")]
    AuracastSqMode,
    #[serde(rename = "auto_power_off_timer")]
    AutoPowerOffTimer,
    #[serde(rename = "battery_saving_mode")]
    BatterySavingMode,
    #[serde(rename = "diagnosis_report")]
    DiagnosisReport,
    #[serde(rename = "feedback_tone_config")]
    FeedbackToneConfig,
    #[serde(rename = "google_logout")]
    GoogleLogout,
    #[serde(rename = "harman_cast")]
    HarmanCast,
    #[serde(rename = "hiplay")]
    HiPlay,
    #[serde(rename = "hotel_mode_app")]
    HotelModeApp,
    #[serde(rename = "lwa_login")]
    LwaLogin,
    #[serde(rename = "product_usage_time")]
    ProductUsageTime,
    #[serde(rename = "qobuz_connect")]
    QobuzConnect,
    #[serde(rename = "qsymphony")]
    QSymphony,
    #[serde(rename = "remote_controller")]
    RemoteController,
    #[serde(rename = "roon_ready")]
    RoonReady,
    #[serde(rename = "self_tuning_calibration")]
    SelfTuningCalibration,
    #[serde(rename = "soundscape")]
    Soundscape,
    #[serde(rename = "tidal_connect")]
    TidalConnect,
    #[serde(rename = "user_eq")]
    UserEq,
    #[serde(rename = "wifi_authentication")]
    WifiAuthentication,
}

impl FeatureKey {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::OobeEncryption => "OOBE_encryption",
            Self::AuracastSqMode => "auracast_sq_mode",
            Self::AutoPowerOffTimer => "auto_power_off_timer",
            Self::BatterySavingMode => "battery_saving_mode",
            Self::DiagnosisReport => "diagnosis_report",
            Self::FeedbackToneConfig => "feedback_tone_config",
            Self::GoogleLogout => "google_logout",
            Self::HarmanCast => "harman_cast",
            Self::HiPlay => "hiplay",
            Self::HotelModeApp => "hotel_mode_app",
            Self::LwaLogin => "lwa_login",
            Self::ProductUsageTime => "product_usage_time",
            Self::QobuzConnect => "qobuz_connect",
            Self::QSymphony => "qsymphony",
            Self::RemoteController => "remote_controller",
            Self::RoonReady => "roon_ready",
            Self::SelfTuningCalibration => "self_tuning_calibration",
            Self::Soundscape => "soundscape",
            Self::TidalConnect => "tidal_connect",
            Self::UserEq => "user_eq",
            Self::WifiAuthentication => "wifi_authentication",
        }
    }
}

const FEATURE_ALLOWLIST: [FeatureKey; 21] = [
    FeatureKey::OobeEncryption,
    FeatureKey::AuracastSqMode,
    FeatureKey::AutoPowerOffTimer,
    FeatureKey::BatterySavingMode,
    FeatureKey::DiagnosisReport,
    FeatureKey::FeedbackToneConfig,
    FeatureKey::GoogleLogout,
    FeatureKey::HarmanCast,
    FeatureKey::HiPlay,
    FeatureKey::HotelModeApp,
    FeatureKey::LwaLogin,
    FeatureKey::ProductUsageTime,
    FeatureKey::QobuzConnect,
    FeatureKey::QSymphony,
    FeatureKey::RemoteController,
    FeatureKey::RoonReady,
    FeatureKey::SelfTuningCalibration,
    FeatureKey::Soundscape,
    FeatureKey::TidalConnect,
    FeatureKey::UserEq,
    FeatureKey::WifiAuthentication,
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct FeatureSupportEntry {
    pub key: FeatureKey,
    pub supported: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FeatureSupportSummary {
    pub known: Vec<FeatureSupportEntry>,
    pub unknown_key_count: usize,
}

#[derive(Deserialize)]
struct FeatureSupportResponseWire {
    error_code: WireErrorCode,
    feature_support: BTreeMap<String, FeatureEntryWire>,
}

#[derive(Deserialize)]
struct FeatureEntryWire {
    support: String,
}

pub(crate) fn parse_feature_support(
    payload: &[u8],
) -> Result<FeatureSupportSummary, InspectionError> {
    let response: FeatureSupportResponseWire = parse_bounded(payload)?;
    ensure_success(&response.error_code)?;
    if response.feature_support.len() > MAX_FEATURE_ENTRIES {
        return Err(InspectionError::LimitExceeded);
    }

    for entry in response.feature_support.values() {
        if !matches!(entry.support.as_str(), "true" | "false") {
            return Err(InspectionError::InvalidValue);
        }
    }

    let known = FEATURE_ALLOWLIST
        .iter()
        .filter_map(|key| {
            response
                .feature_support
                .get(key.as_str())
                .map(|entry| FeatureSupportEntry {
                    key: *key,
                    supported: entry.support == "true",
                })
        })
        .collect::<Vec<_>>();
    Ok(FeatureSupportSummary {
        unknown_key_count: response.feature_support.len() - known.len(),
        known,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct EqSummary {
    pub preset_count: usize,
    pub active_present: bool,
    pub fs_count: usize,
    pub gain_count: usize,
    pub q_count: usize,
    pub type_count: usize,
}

#[derive(Deserialize)]
struct EqListResponseWire {
    error_code: WireErrorCode,
    active_eq_id: String,
    eq_list: Vec<EqListItemWire>,
}

#[derive(Deserialize)]
struct EqListItemWire {
    band: usize,
    eq_id: String,
    eq_name: String,
    eq_payload: EqPresetPayloadWire,
}

#[derive(Deserialize)]
struct EqPresetPayloadWire {
    fs: Vec<Number>,
    gain: Vec<Number>,
}

#[derive(Deserialize)]
struct EqResponseWire {
    error_code: WireErrorCode,
    eq_setting: EqSettingWire,
}

#[derive(Deserialize)]
struct EqSettingWire {
    eq_id: String,
    eq_name: String,
    eq_status: String,
    eq_payload: EqPayloadWire,
}

#[derive(Deserialize)]
struct EqPayloadWire {
    fs: Vec<Number>,
    gain: Vec<Number>,
    q: Vec<Number>,
    #[serde(rename = "type")]
    types: Vec<Number>,
}

pub(crate) fn parse_eq_summary(
    eq_list_payload: &[u8],
    eq_payload: &[u8],
) -> Result<EqSummary, InspectionError> {
    let list: EqListResponseWire = parse_bounded(eq_list_payload)?;
    let current: EqResponseWire = parse_bounded(eq_payload)?;
    ensure_success(&list.error_code)?;
    ensure_success(&current.error_code)?;
    if list.eq_list.len() > MAX_EQ_PRESETS {
        return Err(InspectionError::LimitExceeded);
    }
    if !bounded_private_label(&list.active_eq_id) {
        return Err(InspectionError::InvalidValue);
    }

    let mut ids = BTreeSet::new();
    for preset in &list.eq_list {
        if !bounded_private_label(&preset.eq_id)
            || !bounded_private_label(&preset.eq_name)
            || preset.band > MAX_EQ_BANDS
            || preset.eq_payload.fs.len() > MAX_EQ_BANDS
            || preset.eq_payload.gain.len() > MAX_EQ_BANDS
            || preset.band != preset.eq_payload.fs.len()
            || preset.band != preset.eq_payload.gain.len()
            || !ids.insert(preset.eq_id.as_str())
        {
            return Err(InspectionError::InvalidValue);
        }
    }

    let setting = current.eq_setting;
    if !bounded_private_label(&setting.eq_id)
        || !bounded_private_label(&setting.eq_name)
        || !bounded_ascii_token(&setting.eq_status, MAX_SOURCE_TOKEN_BYTES)
    {
        return Err(InspectionError::InvalidValue);
    }
    for count in [
        setting.eq_payload.fs.len(),
        setting.eq_payload.gain.len(),
        setting.eq_payload.q.len(),
        setting.eq_payload.types.len(),
    ] {
        if count > MAX_EQ_BANDS {
            return Err(InspectionError::LimitExceeded);
        }
    }

    Ok(EqSummary {
        preset_count: list.eq_list.len(),
        active_present: ids.contains(list.active_eq_id.as_str()),
        fs_count: setting.eq_payload.fs.len(),
        gain_count: setting.eq_payload.gain.len(),
        q_count: setting.eq_payload.q.len(),
        type_count: setting.eq_payload.types.len(),
    })
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AudioSourceSummary {
    pub active: MediaSource,
    pub support_sources: Vec<MediaSource>,
}

#[derive(Deserialize)]
struct AudioSourceResponseWire {
    error_code: WireErrorCode,
    audiosource_info: AudioSourceInfoWire,
}

#[derive(Deserialize)]
struct AudioSourceInfoWire {
    active_source: String,
    support_sources: Vec<AudioSourceWire>,
}

#[derive(Deserialize)]
struct AudioSourceWire {
    source: String,
    #[serde(rename = "type")]
    _kind: i64,
}

pub(crate) fn parse_audio_sources(payload: &[u8]) -> Result<AudioSourceSummary, InspectionError> {
    let response: AudioSourceResponseWire = parse_bounded(payload)?;
    ensure_success(&response.error_code)?;
    let info = response.audiosource_info;
    if info.support_sources.len() > MAX_AUDIO_SOURCES {
        return Err(InspectionError::LimitExceeded);
    }
    if !bounded_ascii_token(&info.active_source, MAX_SOURCE_TOKEN_BYTES) {
        return Err(InspectionError::InvalidValue);
    }

    let mut exact_tokens = BTreeSet::new();
    let mut support_sources = Vec::with_capacity(info.support_sources.len());
    for source in info.support_sources {
        if !bounded_ascii_token(&source.source, MAX_SOURCE_TOKEN_BYTES)
            || !exact_tokens.insert(source.source.clone())
        {
            return Err(InspectionError::InvalidValue);
        }
        support_sources.push(MediaSource::from_device_token(&source.source));
    }

    Ok(AudioSourceSummary {
        active: MediaSource::from_device_token(&info.active_source),
        support_sources,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PersonalListeningState {
    On,
    Off,
    Unknown,
}

#[derive(Deserialize)]
struct PersonalListeningResponseWire {
    error_code: WireErrorCode,
    status: String,
}

pub(crate) fn parse_personal_listening(
    payload: &[u8],
) -> Result<PersonalListeningState, InspectionError> {
    let response: PersonalListeningResponseWire = parse_bounded(payload)?;
    ensure_success(&response.error_code)?;
    if !bounded_ascii_token(&response.status, 16) {
        return Err(InspectionError::InvalidValue);
    }
    Ok(match response.status.as_str() {
        "on" => PersonalListeningState::On,
        "off" => PersonalListeningState::Off,
        _ => PersonalListeningState::Unknown,
    })
}

#[derive(Deserialize)]
struct AudioSyncResponseWire {
    error_code: WireErrorCode,
    audio_sync: String,
}

pub(crate) fn parse_audio_sync(payload: &[u8]) -> Result<i32, InspectionError> {
    let response: AudioSyncResponseWire = parse_bounded(payload)?;
    ensure_success(&response.error_code)?;
    let value = response.audio_sync.as_bytes();
    let digits = if value.first() == Some(&b'-') {
        &value[1..]
    } else {
        value
    };
    if value.len() > 12 || digits.is_empty() || !digits.iter().all(u8::is_ascii_digit) {
        return Err(InspectionError::InvalidValue);
    }
    response
        .audio_sync
        .parse::<i32>()
        .map_err(|_| InspectionError::InvalidValue)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MediaSourceActivity {
    Playing,
    Paused,
    Stopped,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct MediaSourceActivitySummary {
    pub source: MediaSource,
    pub activity: MediaSourceActivity,
}

#[derive(Deserialize)]
struct MediaSourceActivityResponseWire {
    error_code: WireErrorCode,
    media_source: String,
    media_status: String,
}

pub(crate) fn parse_media_source_activity(
    payload: &[u8],
) -> Result<MediaSourceActivitySummary, InspectionError> {
    let response: MediaSourceActivityResponseWire = parse_bounded(payload)?;
    ensure_success(&response.error_code)?;
    if !bounded_ascii_token(&response.media_source, MAX_SOURCE_TOKEN_BYTES)
        || !bounded_ascii_token(&response.media_status, 16)
    {
        return Err(InspectionError::InvalidValue);
    }
    Ok(MediaSourceActivitySummary {
        source: MediaSource::from_device_token(&response.media_source),
        activity: match response.media_status.as_str() {
            "playing" => MediaSourceActivity::Playing,
            "paused" => MediaSourceActivity::Paused,
            "stopped" => MediaSourceActivity::Stopped,
            _ => MediaSourceActivity::Unknown,
        },
    })
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct InspectionSnapshot {
    pub feature_support: FeatureSupportSummary,
    pub eq: EqSummary,
    pub audio_sources: AudioSourceSummary,
    pub personal_listening: PersonalListeningState,
    pub audio_sync: i32,
    pub media_source_activity: MediaSourceActivitySummary,
}

/// The seven independent payloads required for one complete snapshot.
pub(crate) struct InspectionPayloads<'a> {
    pub(crate) feature_support: &'a [u8],
    pub(crate) eq_list: &'a [u8],
    pub(crate) eq: &'a [u8],
    pub(crate) audio_sources: &'a [u8],
    pub(crate) personal_listening: &'a [u8],
    pub(crate) audio_sync: &'a [u8],
    pub(crate) media_source_activity: &'a [u8],
}

pub(crate) fn parse_inspection_snapshot(
    payloads: InspectionPayloads<'_>,
) -> Result<InspectionSnapshot, InspectionError> {
    Ok(InspectionSnapshot {
        feature_support: parse_feature_support(payloads.feature_support)?,
        eq: parse_eq_summary(payloads.eq_list, payloads.eq)?,
        audio_sources: parse_audio_sources(payloads.audio_sources)?,
        personal_listening: parse_personal_listening(payloads.personal_listening)?,
        audio_sync: parse_audio_sync(payloads.audio_sync)?,
        media_source_activity: parse_media_source_activity(payloads.media_source_activity)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const EQ_LIST: &[u8] = br#"{
      "error_code":"0","active_eq_id":"private-active-id","eq_list":[
        {"band":2,"eq_id":"private-other-id","eq_name":"private-name-a",
         "eq_payload":{"fs":[100,200],"gain":[0,1]}},
        {"band":2,"eq_id":"private-active-id","eq_name":"private-name-b",
         "eq_payload":{"fs":[100,200],"gain":[1,2]}}
      ]}"#;
    const EQ: &[u8] = br#"{
      "error_code":0,"eq_setting":{"eq_id":"private-active-id",
      "eq_name":"private-name-b","eq_status":"on",
      "eq_payload":{"fs":[100,200,300],"gain":[0,1,2],
      "q":[1.0,1.1,1.2],"type":[0,1,2]}}}"#;
    const AUDIO_SOURCES: &[u8] = br#"{
      "error_code":"0","audiosource_info":{"active_source":"BT",
      "support_sources":[{"source":"BT","type":1},
      {"source":"AP2","type":2},{"source":"PRIVATE_SOURCE","type":9}]}}"#;

    fn feature_payload(error_code: serde_json::Value) -> Vec<u8> {
        serde_json::to_vec(&json!({
            "error_code": error_code,
            "feature_support": {
                "OOBE_encryption": {"support": "true"},
                "user_eq": {"support": "false"},
                "private_future_feature": {"support": "true"}
            }
        }))
        .unwrap()
    }

    #[test]
    fn feature_allowlist_is_exactly_the_current_twenty_one_unique_keys() {
        assert_eq!(FEATURE_ALLOWLIST.len(), 21);
        let keys = FEATURE_ALLOWLIST
            .iter()
            .map(|key| key.as_str())
            .collect::<BTreeSet<_>>();
        assert_eq!(keys.len(), 21);
    }

    #[test]
    fn feature_summary_projects_known_values_and_only_counts_unknown_keys() {
        let summary = parse_feature_support(&feature_payload(json!(0))).unwrap();
        assert_eq!(summary.unknown_key_count, 1);
        assert_eq!(
            summary.known,
            vec![
                FeatureSupportEntry {
                    key: FeatureKey::OobeEncryption,
                    supported: true,
                },
                FeatureSupportEntry {
                    key: FeatureKey::UserEq,
                    supported: false,
                },
            ]
        );
        let serialized = serde_json::to_string(&summary).unwrap();
        assert!(!serialized.contains("private_future_feature"));
    }

    #[test]
    fn feature_support_requires_exact_string_booleans_even_for_unknown_keys() {
        for support in [json!(true), json!("TRUE"), json!("1"), json!(1)] {
            let payload = serde_json::to_vec(&json!({
                "error_code": 0,
                "feature_support": {"private_key": {"support": support}}
            }))
            .unwrap();
            assert!(matches!(
                parse_feature_support(&payload),
                Err(InspectionError::InvalidShape | InspectionError::InvalidValue)
            ));
        }
    }

    #[test]
    fn eq_summary_exposes_only_counts_and_active_presence() {
        let summary = parse_eq_summary(EQ_LIST, EQ).unwrap();
        assert_eq!(
            summary,
            EqSummary {
                preset_count: 2,
                active_present: true,
                fs_count: 3,
                gain_count: 3,
                q_count: 3,
                type_count: 3,
            }
        );
        let serialized = serde_json::to_string(&summary).unwrap();
        for private_value in ["private-active-id", "private-other-id", "private-name"] {
            assert!(!serialized.contains(private_value));
        }
    }

    #[test]
    fn eq_rejects_duplicate_ids_mismatched_preset_bands_and_non_numeric_arrays() {
        for payload in [
            br#"{"error_code":0,"active_eq_id":"same","eq_list":[
                {"band":1,"eq_id":"same","eq_name":"a","eq_payload":{"fs":[1],"gain":[1]}},
                {"band":1,"eq_id":"same","eq_name":"b","eq_payload":{"fs":[1],"gain":[1]}}]}"#
                .as_slice(),
            br#"{"error_code":0,"active_eq_id":"a","eq_list":[
                {"band":2,"eq_id":"a","eq_name":"a","eq_payload":{"fs":[1],"gain":[1]}}]}"#
                .as_slice(),
        ] {
            assert_eq!(
                parse_eq_summary(payload, EQ),
                Err(InspectionError::InvalidValue)
            );
        }
        let invalid_eq = br#"{"error_code":0,"eq_setting":{"eq_id":"a","eq_name":"b",
            "eq_status":"on","eq_payload":{"fs":["secret"],"gain":[],"q":[],"type":[]}}}"#;
        assert_eq!(
            parse_eq_summary(EQ_LIST, invalid_eq),
            Err(InspectionError::InvalidShape)
        );
    }

    #[test]
    fn audio_sources_use_the_shared_media_mapping_without_echoing_unknown_tokens() {
        let summary = parse_audio_sources(AUDIO_SOURCES).unwrap();
        assert_eq!(summary.active, MediaSource::Bluetooth);
        assert_eq!(
            summary.support_sources,
            vec![
                MediaSource::Bluetooth,
                MediaSource::AirPlay2,
                MediaSource::Unknown,
            ]
        );
        assert!(!serde_json::to_string(&summary)
            .unwrap()
            .contains("PRIVATE_SOURCE"));
    }

    #[test]
    fn personal_listening_maps_on_off_and_sanitizes_other_values() {
        for (status, expected) in [
            ("on", PersonalListeningState::On),
            ("off", PersonalListeningState::Off),
            ("future", PersonalListeningState::Unknown),
        ] {
            let payload = serde_json::to_vec(&json!({"error_code":"0","status":status})).unwrap();
            assert_eq!(parse_personal_listening(&payload), Ok(expected));
        }
    }

    #[test]
    fn audio_sync_requires_a_bounded_canonical_integer_string() {
        for (value, expected) in [("0", 0), ("125", 125), ("-25", -25)] {
            let payload = serde_json::to_vec(&json!({"error_code":0,"audio_sync":value})).unwrap();
            assert_eq!(parse_audio_sync(&payload), Ok(expected));
        }
        for value in ["", "+1", " 1", "1.5", "9999999999999"] {
            let payload = serde_json::to_vec(&json!({"error_code":0,"audio_sync":value})).unwrap();
            assert_eq!(
                parse_audio_sync(&payload),
                Err(InspectionError::InvalidValue)
            );
        }
    }

    #[test]
    fn media_source_activity_accepts_only_exact_known_lowercase_tokens() {
        for (status, expected) in [
            ("playing", MediaSourceActivity::Playing),
            ("paused", MediaSourceActivity::Paused),
            ("stopped", MediaSourceActivity::Stopped),
            ("PLAYING", MediaSourceActivity::Unknown),
            ("private", MediaSourceActivity::Unknown),
        ] {
            let payload = serde_json::to_vec(&json!({
                "error_code":"0","media_source":"C4A","media_status":status
            }))
            .unwrap();
            assert_eq!(
                parse_media_source_activity(&payload),
                Ok(MediaSourceActivitySummary {
                    source: MediaSource::Chromecast,
                    activity: expected,
                })
            );
        }
    }

    #[test]
    fn every_parser_accepts_string_or_integer_zero_error_codes() {
        for error_code in [json!(0), json!("0")] {
            assert!(parse_feature_support(&feature_payload(error_code.clone())).is_ok());
            let personal =
                serde_json::to_vec(&json!({"error_code":error_code.clone(),"status":"off"}))
                    .unwrap();
            assert!(parse_personal_listening(&personal).is_ok());
            let sync =
                serde_json::to_vec(&json!({"error_code":error_code.clone(),"audio_sync":"0"}))
                    .unwrap();
            assert!(parse_audio_sync(&sync).is_ok());
            let media = serde_json::to_vec(&json!({
                "error_code":error_code,"media_source":"BT","media_status":"stopped"
            }))
            .unwrap();
            assert!(parse_media_source_activity(&media).is_ok());
        }
    }

    #[test]
    fn nonzero_or_invalid_error_codes_fail_closed() {
        for error_code in [json!(1), json!("1"), json!(-1)] {
            assert_eq!(
                parse_feature_support(&feature_payload(error_code)),
                Err(InspectionError::DeviceReportedError)
            );
        }
        for error_code in [json!(false), json!(0.0), json!(null)] {
            assert_eq!(
                parse_feature_support(&feature_payload(error_code)),
                Err(InspectionError::InvalidShape)
            );
        }
    }

    #[test]
    fn response_size_is_checked_before_json_parsing() {
        let oversized = vec![b' '; MAX_INSPECTION_RESPONSE_BYTES as usize + 1];
        assert_eq!(
            parse_feature_support(&oversized),
            Err(InspectionError::ResponseTooLarge)
        );
    }

    #[test]
    fn feature_source_and_eq_collection_limits_are_enforced() {
        let features = (0..=MAX_FEATURE_ENTRIES)
            .map(|index| (format!("future_{index}"), json!({"support":"true"})))
            .collect::<serde_json::Map<_, _>>();
        let payload = serde_json::to_vec(&json!({
            "error_code": 0,
            "feature_support": features
        }))
        .unwrap();
        assert_eq!(
            parse_feature_support(&payload),
            Err(InspectionError::LimitExceeded)
        );

        let sources = (0..=MAX_AUDIO_SOURCES)
            .map(|index| json!({"source":format!("SOURCE_{index}"),"type":index}))
            .collect::<Vec<_>>();
        let payload = serde_json::to_vec(&json!({
            "error_code": 0,
            "audiosource_info": {"active_source":"BT","support_sources":sources}
        }))
        .unwrap();
        assert_eq!(
            parse_audio_sources(&payload),
            Err(InspectionError::LimitExceeded)
        );

        let bands = vec![json!(0); MAX_EQ_BANDS + 1];
        let payload = serde_json::to_vec(&json!({
            "error_code": 0,
            "eq_setting": {
                "eq_id":"id","eq_name":"name","eq_status":"on",
                "eq_payload":{"fs":bands,"gain":[],"q":[],"type":[]}
            }
        }))
        .unwrap();
        assert_eq!(
            parse_eq_summary(EQ_LIST, &payload),
            Err(InspectionError::LimitExceeded)
        );
    }

    #[test]
    fn complete_snapshot_is_composed_only_from_typed_summaries() {
        let personal = br#"{"error_code":0,"status":"off"}"#;
        let sync = br#"{"error_code":"0","audio_sync":"25"}"#;
        let activity = br#"{"error_code":0,"media_source":"BT","media_status":"playing"}"#;
        let snapshot = parse_inspection_snapshot(InspectionPayloads {
            feature_support: &feature_payload(json!(0)),
            eq_list: EQ_LIST,
            eq: EQ,
            audio_sources: AUDIO_SOURCES,
            personal_listening: personal,
            audio_sync: sync,
            media_source_activity: activity,
        })
        .unwrap();
        assert_eq!(snapshot.eq.preset_count, 2);
        assert_eq!(snapshot.personal_listening, PersonalListeningState::Off);
        assert_eq!(snapshot.audio_sync, 25);
        assert_eq!(
            snapshot.media_source_activity.activity,
            MediaSourceActivity::Playing
        );
    }
}
