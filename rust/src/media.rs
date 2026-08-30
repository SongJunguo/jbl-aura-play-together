//! Sanitized OneOS/UPnP media models and closed request serializers.

use serde::Serialize;
use serde_json::Value;

use crate::error::JblError;

const AV_TRANSPORT_SERVICE: &str = "urn:schemas-upnp-org:service:AVTransport:1";
const RENDERING_CONTROL_SERVICE: &str = "urn:schemas-upnp-org:service:RenderingControl:1";
const SOAP_ENVELOPE_SERVICE: &str = "http://schemas.xmlsoap.org/soap/envelope/";
const UPNP_CONTROL_SERVICE: &str = "urn:schemas-upnp-org:control-1-0";

/// Current direct-device safety ceiling. A future louder path must use a
/// separate, explicit capability instead of weakening `--confirm`.
pub const MAX_SAFE_DIRECT_VOLUME: u8 = 9;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TransportState {
    Playing,
    Paused,
    Stopped,
    Transitioning,
    NoMedia,
    Unknown,
}

impl TransportState {
    fn parse(value: Option<&str>) -> Self {
        match value.map(str::trim) {
            Some("PLAYING") => Self::Playing,
            Some("PAUSED_PLAYBACK") | Some("PAUSED_RECORDING") => Self::Paused,
            Some("STOPPED") => Self::Stopped,
            Some("TRANSITIONING") => Self::Transitioning,
            Some("NO_MEDIA_PRESENT") => Self::NoMedia,
            _ => Self::Unknown,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TransportStatus {
    Ok,
    ErrorOccurred,
    Unknown,
}

impl TransportStatus {
    fn parse(value: Option<&str>) -> Self {
        match value.map(str::trim) {
            Some("OK") => Self::Ok,
            Some("ERROR_OCCURRED") => Self::ErrorOccurred,
            _ => Self::Unknown,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MediaSource {
    Bluetooth,
    Tv,
    Hdmi,
    Optical,
    Coaxial,
    AuxIn,
    UsbPlayback,
    Multiroom,
    AirPlay2,
    Alexa,
    Chromecast,
    HuaweiVoice,
    HuaweiMusic,
    Unknown,
}

impl MediaSource {
    pub(crate) fn from_device_token(value: &str) -> Self {
        match value.trim() {
            "BT" => Self::Bluetooth,
            "TV" => Self::Tv,
            "HDMI" => Self::Hdmi,
            "Optical" => Self::Optical,
            "Coaxial" => Self::Coaxial,
            "AUX" | "Aux In" => Self::AuxIn,
            "USB" | "USB_Playback" => Self::UsbPlayback,
            "MRM" => Self::Multiroom,
            "AP2" => Self::AirPlay2,
            "AVS" => Self::Alexa,
            "C4A" => Self::Chromecast,
            "HVA" => Self::HuaweiVoice,
            "HMS" => Self::HuaweiMusic,
            _ => Self::Unknown,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AudioSourceTarget {
    Bluetooth,
    AuxIn,
    UsbPlayback,
}

impl AudioSourceTarget {
    pub(crate) const fn token(self) -> &'static str {
        match self {
            Self::Bluetooth => "BT",
            Self::AuxIn => "AUX",
            Self::UsbPlayback => "USB",
        }
    }

    pub(crate) const fn source(self) -> MediaSource {
        match self {
            Self::Bluetooth => MediaSource::Bluetooth,
            Self::AuxIn => MediaSource::AuxIn,
            Self::UsbPlayback => MediaSource::UsbPlayback,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PlaybackStatus {
    pub state: TransportState,
    pub transport_status: TransportStatus,
    pub volume: Option<u8>,
    pub muted: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MediaStatus {
    pub playback: PlaybackStatus,
    pub source: MediaSource,
}

/// Absolute mute target. There is intentionally no toggle operation because a
/// lost reply must never make a retry invert an unknown device state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MuteTarget {
    On,
    Off,
}

/// The only playback mutations exposed by the first production control stage.
/// Stop, next and previous remain unrepresentable until their source-specific
/// behavior is independently verified.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
#[cfg(test)]
pub(crate) enum PlaybackTarget {
    Play,
    Pause,
}

#[cfg(test)]
impl PlaybackTarget {
    pub(crate) const fn desired_state(self) -> TransportState {
        match self {
            Self::Play => TransportState::Playing,
            Self::Pause => TransportState::Paused,
        }
    }
}

impl MuteTarget {
    pub(crate) const fn desired(self) -> bool {
        matches!(self, Self::On)
    }
}

/// Conservative result of one explicit UPnP volume mutation.
#[derive(Debug, Clone, PartialEq, Eq)]
#[must_use]
pub enum VolumeWriteResult {
    /// The authenticated/preverified snapshot was already at the target, so
    /// no mutation was sent.
    AlreadyAtTarget(PlaybackStatus),
    /// The device returned HTTP success and the independent readback matched.
    Applied(PlaybackStatus),
    /// A changed readback observed the target after the write response was
    /// lost. This is useful evidence but is not promoted to success because
    /// another controller could have changed the unauthenticated UPnP state.
    TargetObservedAfterUnknownWrite(PlaybackStatus),
    /// The write returned, but the independent readback contradicted it.
    PostconditionFailed(PlaybackStatus),
    /// Identity, serializer, or connection setup failed before the write.
    RejectedBeforeSend(JblError),
    /// A write might have reached the device and readback could not prove it.
    OutcomeUnknown(JblError),
}

impl VolumeWriteResult {
    pub const fn accepted(&self) -> bool {
        matches!(self, Self::AlreadyAtTarget(_) | Self::Applied(_))
    }

    pub const fn playback(&self) -> Option<&PlaybackStatus> {
        match self {
            Self::AlreadyAtTarget(playback)
            | Self::Applied(playback)
            | Self::TargetObservedAfterUnknownWrite(playback)
            | Self::PostconditionFailed(playback) => Some(playback),
            Self::RejectedBeforeSend(_) | Self::OutcomeUnknown(_) => None,
        }
    }

    pub const fn error(&self) -> Option<&JblError> {
        match self {
            Self::RejectedBeforeSend(error) | Self::OutcomeUnknown(error) => Some(error),
            Self::AlreadyAtTarget(_)
            | Self::Applied(_)
            | Self::TargetObservedAfterUnknownWrite(_)
            | Self::PostconditionFailed(_) => None,
        }
    }
}

/// Conservative result of one explicit, absolute UPnP mute mutation.
#[derive(Debug, Clone, PartialEq, Eq)]
#[must_use]
pub enum MuteWriteResult {
    /// The verified pre-write snapshot was already at the requested target, so
    /// no mutation was sent.
    AlreadyAtTarget(PlaybackStatus),
    /// The one mutation returned HTTP success and the independent readback
    /// matched the requested target.
    Applied(PlaybackStatus),
    /// The mutation response was lost, while a changed readback observed the
    /// target. This remains non-success because UPnP cannot exclude an external
    /// controller as the cause.
    TargetObservedAfterUnknownWrite(PlaybackStatus),
    /// The mutation returned, but the independent readback contradicted it.
    PostconditionFailed(PlaybackStatus),
    /// Identity or the required pre-write mute state failed before mutation.
    RejectedBeforeSend(JblError),
    /// A mutation might have reached the device and its result is unresolved.
    OutcomeUnknown(JblError),
}

impl MuteWriteResult {
    pub const fn accepted(&self) -> bool {
        matches!(self, Self::AlreadyAtTarget(_) | Self::Applied(_))
    }

    pub const fn playback(&self) -> Option<&PlaybackStatus> {
        match self {
            Self::AlreadyAtTarget(playback)
            | Self::Applied(playback)
            | Self::TargetObservedAfterUnknownWrite(playback)
            | Self::PostconditionFailed(playback) => Some(playback),
            Self::RejectedBeforeSend(_) | Self::OutcomeUnknown(_) => None,
        }
    }

    pub const fn error(&self) -> Option<&JblError> {
        match self {
            Self::RejectedBeforeSend(error) | Self::OutcomeUnknown(error) => Some(error),
            Self::AlreadyAtTarget(_)
            | Self::Applied(_)
            | Self::TargetObservedAfterUnknownWrite(_)
            | Self::PostconditionFailed(_) => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[must_use]
#[cfg(test)]
pub(crate) enum PlaybackWriteResult {
    AlreadyAtTarget(MediaStatus),
    Applied(MediaStatus),
    RejectedByDevice(MediaStatus),
    TargetObservedAfterUnknownWrite(MediaStatus),
    PostconditionFailed(MediaStatus),
    RejectedBeforeSend(JblError),
    OutcomeUnknown(JblError),
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[must_use]
pub enum AudioSourceWriteResult {
    AlreadyAtTarget(MediaSource),
    Applied(MediaSource),
    RejectedByDevice(MediaSource),
    TargetObservedAfterUnknownWrite(MediaSource),
    PostconditionFailed(MediaSource),
    RejectedBeforeSend(JblError),
    OutcomeUnknown(JblError),
}

pub fn parse_get_info_ex(payload: &[u8]) -> Result<PlaybackStatus, JblError> {
    let xml = std::str::from_utf8(payload).map_err(|_| JblError::InvalidXml)?;
    let document = roxmltree::Document::parse(xml).map_err(|_| JblError::InvalidXml)?;
    let envelope = document.root_element();
    if !matches_element(envelope, "Envelope", Some(SOAP_ENVELOPE_SERVICE))
        || document
            .descendants()
            .filter(|node| node.is_element() && node.tag_name().name() == "Envelope")
            .count()
            != 1
    {
        return Err(JblError::MediaInfoInvalid);
    }

    let bodies = document
        .descendants()
        .filter(|node| node.is_element() && node.tag_name().name() == "Body")
        .collect::<Vec<_>>();
    let [body] = bodies.as_slice() else {
        return Err(JblError::MediaInfoInvalid);
    };
    if !matches_element(*body, "Body", Some(SOAP_ENVELOPE_SERVICE))
        || body.parent_element() != Some(envelope)
        || envelope
            .children()
            .filter(roxmltree::Node::is_element)
            .any(|node| {
                node != *body && !matches_element(node, "Header", Some(SOAP_ENVELOPE_SERVICE))
            })
        || document
            .descendants()
            .any(|node| node.is_element() && node.tag_name().name() == "Fault")
    {
        return Err(JblError::MediaInfoInvalid);
    }

    let responses = document
        .descendants()
        .filter(|node| node.is_element() && node.tag_name().name() == "GetInfoExResponse")
        .collect::<Vec<_>>();
    let [response] = responses.as_slice() else {
        return if responses.is_empty() {
            Err(JblError::MediaInfoMissing)
        } else {
            Err(JblError::MediaInfoInvalid)
        };
    };
    if !matches_element(*response, "GetInfoExResponse", Some(AV_TRANSPORT_SERVICE))
        || response.parent_element() != Some(*body)
        || body.children().filter(roxmltree::Node::is_element).count() != 1
    {
        return Err(JblError::MediaInfoInvalid);
    }

    let transport_state = unique_response_text(*response, "CurrentTransportState")?;
    let transport_status = unique_response_text(*response, "CurrentTransportStatus")?;
    let current_volume = unique_response_text(*response, "CurrentVolume")?;
    let current_mute = unique_response_text(*response, "CurrentMute")?;

    let volume = match current_volume.map(str::trim) {
        None | Some("") => None,
        Some(value) => {
            if !value.bytes().all(|byte| byte.is_ascii_digit()) {
                return Err(JblError::MediaInfoInvalid);
            }
            let volume = value
                .parse::<u8>()
                .map_err(|_| JblError::MediaInfoInvalid)?;
            if volume > 100 {
                return Err(JblError::MediaInfoInvalid);
            }
            Some(volume)
        }
    };
    let muted = match current_mute.map(str::trim) {
        None | Some("") => None,
        Some("0") => Some(false),
        Some("1") => Some(true),
        Some(_) => return Err(JblError::MediaInfoInvalid),
    };

    Ok(PlaybackStatus {
        state: TransportState::parse(transport_state),
        transport_status: TransportStatus::parse(transport_status),
        volume,
        muted,
    })
}

/// Parses only a standards-shaped SOAP 1.1 UPnP action fault. Descriptions are
/// deliberately ignored so device-supplied text can never reach diagnostics.
pub(crate) fn parse_upnp_action_fault(payload: &[u8]) -> Result<u16, JblError> {
    let xml = std::str::from_utf8(payload).map_err(|_| JblError::InvalidXml)?;
    let document = roxmltree::Document::parse(xml).map_err(|_| JblError::InvalidXml)?;
    let envelope = document.root_element();
    if !matches_element(envelope, "Envelope", Some(SOAP_ENVELOPE_SERVICE))
        || document
            .descendants()
            .filter(|node| node.is_element() && node.tag_name().name() == "Envelope")
            .count()
            != 1
    {
        return Err(JblError::MediaInfoInvalid);
    }
    let bodies = document
        .descendants()
        .filter(|node| node.is_element() && node.tag_name().name() == "Body")
        .collect::<Vec<_>>();
    let [body] = bodies.as_slice() else {
        return Err(JblError::MediaInfoInvalid);
    };
    if !matches_element(*body, "Body", Some(SOAP_ENVELOPE_SERVICE))
        || body.parent_element() != Some(envelope)
    {
        return Err(JblError::MediaInfoInvalid);
    }
    let faults = document
        .descendants()
        .filter(|node| node.is_element() && node.tag_name().name() == "Fault")
        .collect::<Vec<_>>();
    let [fault] = faults.as_slice() else {
        return Err(JblError::MediaInfoInvalid);
    };
    if !matches_element(*fault, "Fault", Some(SOAP_ENVELOPE_SERVICE))
        || fault.parent_element() != Some(*body)
        || body.children().filter(roxmltree::Node::is_element).count() != 1
    {
        return Err(JblError::MediaInfoInvalid);
    }
    let details = fault
        .descendants()
        .filter(|node| node.is_element() && node.tag_name().name() == "detail")
        .collect::<Vec<_>>();
    let [detail] = details.as_slice() else {
        return Err(JblError::MediaInfoInvalid);
    };
    if detail.parent_element() != Some(*fault) || detail.tag_name().namespace().is_some() {
        return Err(JblError::MediaInfoInvalid);
    }
    let upnp_errors = detail
        .descendants()
        .filter(|node| node.is_element() && node.tag_name().name() == "UPnPError")
        .collect::<Vec<_>>();
    let [upnp_error] = upnp_errors.as_slice() else {
        return Err(JblError::MediaInfoInvalid);
    };
    if !matches_element(*upnp_error, "UPnPError", Some(UPNP_CONTROL_SERVICE))
        || upnp_error.parent_element() != Some(*detail)
        || detail
            .children()
            .filter(roxmltree::Node::is_element)
            .count()
            != 1
    {
        return Err(JblError::MediaInfoInvalid);
    }
    let codes = upnp_error
        .descendants()
        .filter(|node| node.is_element() && node.tag_name().name() == "errorCode")
        .collect::<Vec<_>>();
    let [code] = codes.as_slice() else {
        return Err(JblError::MediaInfoInvalid);
    };
    if !matches_element(*code, "errorCode", Some(UPNP_CONTROL_SERVICE))
        || code.parent_element() != Some(*upnp_error)
        || code.children().any(|node| !node.is_text())
    {
        return Err(JblError::MediaInfoInvalid);
    }
    let value = code
        .text()
        .map(str::trim)
        .filter(|value| !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit()))
        .ok_or(JblError::MediaInfoInvalid)?;
    value.parse::<u16>().map_err(|_| JblError::MediaInfoInvalid)
}

fn matches_element(
    node: roxmltree::Node<'_, '_>,
    local_name: &str,
    namespace: Option<&str>,
) -> bool {
    node.is_element()
        && node.tag_name().name() == local_name
        && node.tag_name().namespace() == namespace
}

fn unique_response_text<'a, 'input>(
    response: roxmltree::Node<'a, 'input>,
    local_name: &str,
) -> Result<Option<&'a str>, JblError> {
    let fields = response
        .descendants()
        .filter(|node| node.is_element() && node.tag_name().name() == local_name)
        .collect::<Vec<_>>();
    let field = match fields.as_slice() {
        [] => return Ok(None),
        [field] => *field,
        _ => return Err(JblError::MediaInfoInvalid),
    };
    if field.parent_element() != Some(response)
        || !matches!(
            field.tag_name().namespace(),
            None | Some(AV_TRANSPORT_SERVICE)
        )
        || field.children().any(|node| !node.is_text())
    {
        return Err(JblError::MediaInfoInvalid);
    }
    Ok(field.text())
}

pub fn parse_media_source(response: &Value) -> Result<MediaSource, JblError> {
    let error_code = response
        .get("error_code")
        .ok_or(JblError::BasicResponseCodeMissing)?;
    let accepted = matches!(error_code, Value::Number(code) if code.as_i64() == Some(0))
        || matches!(error_code, Value::String(code) if code == "0");
    if !accepted {
        return Err(JblError::DeviceReportedError);
    }
    let source = response
        .get("media_source")
        .and_then(Value::as_str)
        .ok_or(JblError::MediaSourceMissing)?;
    if source.len() > 40 || !source.is_ascii() {
        return Err(JblError::MediaSourceInvalid);
    }
    Ok(MediaSource::from_device_token(source))
}

pub(crate) fn parse_audio_source_targets(
    response: &Value,
) -> Result<Vec<AudioSourceTarget>, JblError> {
    let code = response
        .get("error_code")
        .ok_or(JblError::BasicResponseCodeMissing)?;
    if !matches!(code, Value::Number(value) if value.as_i64() == Some(0))
        && !matches!(code, Value::String(value) if value == "0")
    {
        return Err(JblError::DeviceReportedError);
    }
    let sources = response
        .get("audiosource_info")
        .and_then(|value| value.get("support_sources"))
        .and_then(Value::as_array)
        .ok_or(JblError::MediaSourceMissing)?;
    if sources.len() > 32 {
        return Err(JblError::MediaSourceInvalid);
    }
    let mut targets = Vec::new();
    for entry in sources {
        if entry.get("type").and_then(Value::as_i64).is_none() {
            return Err(JblError::MediaSourceInvalid);
        }
        let token = entry
            .get("source")
            .and_then(Value::as_str)
            .filter(|token| !token.is_empty() && token.len() <= 40 && token.is_ascii())
            .ok_or(JblError::MediaSourceInvalid)?;
        let target = match token {
            "BT" => Some(AudioSourceTarget::Bluetooth),
            "AUX" => Some(AudioSourceTarget::AuxIn),
            "USB" => Some(AudioSourceTarget::UsbPlayback),
            _ => None,
        };
        if let Some(target) = target {
            if targets.contains(&target) {
                return Err(JblError::MediaSourceInvalid);
            }
            targets.push(target);
        }
    }
    Ok(targets)
}

pub(crate) fn source_mutation_body(target: AudioSourceTarget) -> String {
    format!(
        "command=setMediaSource&payload={{\"media_source\":\"{}\"}}",
        target.token()
    )
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct UpnpRequest {
    pub(crate) path: &'static str,
    pub(crate) soap_action: String,
    pub(crate) envelope: String,
}

fn envelope(service: &str, action: &str, arguments: &str) -> String {
    format!(
        concat!(
            "<?xml version=\"1.0\" encoding=\"utf-8\"?>",
            "<s:Envelope s:encodingStyle=\"http://schemas.xmlsoap.org/soap/encoding/\" ",
            "xmlns:s=\"http://schemas.xmlsoap.org/soap/envelope/\"><s:Body>",
            "<u:{0} xmlns:u=\"{1}\">{2}</u:{0}>",
            "</s:Body></s:Envelope>"
        ),
        action, service, arguments
    )
}

fn request(
    path: &'static str,
    service: &'static str,
    action: &'static str,
    arguments: &str,
) -> UpnpRequest {
    UpnpRequest {
        path,
        soap_action: format!("\"{service}#{action}\""),
        envelope: envelope(service, action, arguments),
    }
}

pub(crate) fn get_info_ex_request() -> UpnpRequest {
    request(
        "/upnp/control/rendertransport1",
        AV_TRANSPORT_SERVICE,
        "GetInfoEx",
        "<InstanceID>0</InstanceID>",
    )
}

#[allow(dead_code)] // Kept behind the P1 serializer-only maturity gate.
pub(crate) fn set_volume_request(volume: u8) -> Result<UpnpRequest, JblError> {
    if volume > 100 {
        return Err(JblError::InvalidVolume);
    }
    Ok(request(
        "/upnp/control/rendercontrol1",
        RENDERING_CONTROL_SERVICE,
        "SetVolume",
        &format!(
            "<InstanceID>0</InstanceID><Channel>Single</Channel><DesiredVolume>{volume}</DesiredVolume>"
        ),
    ))
}

pub(crate) fn set_mute_request(target: MuteTarget) -> UpnpRequest {
    request(
        "/upnp/control/rendercontrol1",
        RENDERING_CONTROL_SERVICE,
        "SetMute",
        &format!(
            "<InstanceID>0</InstanceID><Channel>Master</Channel><DesiredMute>{}</DesiredMute>",
            u8::from(target.desired())
        ),
    )
}

#[cfg(test)]
pub(crate) fn playback_mutation_request(action: PlaybackTarget) -> UpnpRequest {
    let action_name = match action {
        PlaybackTarget::Play => "Play",
        PlaybackTarget::Pause => "Pause",
    };
    let arguments = if action == PlaybackTarget::Play {
        "<InstanceID>0</InstanceID><Speed>1</Speed>"
    } else {
        "<InstanceID>0</InstanceID>"
    };
    request(
        "/upnp/control/rendertransport1",
        AV_TRANSPORT_SERVICE,
        action_name,
        arguments,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const INFO: &[u8] = br#"<?xml version="1.0"?>
      <s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/">
        <s:Body>
          <u:GetInfoExResponse xmlns:u="urn:schemas-upnp-org:service:AVTransport:1">
            <CurrentTransportState>PLAYING</CurrentTransportState>
            <CurrentTransportStatus>OK</CurrentTransportStatus>
            <CurrentVolume>9</CurrentVolume><CurrentMute>0</CurrentMute>
          </u:GetInfoExResponse>
        </s:Body>
      </s:Envelope>"#;

    fn response_with_fields(fields: &str) -> Vec<u8> {
        format!(
            concat!(
                "<s:Envelope xmlns:s=\"http://schemas.xmlsoap.org/soap/envelope/\">",
                "<s:Body><u:GetInfoExResponse ",
                "xmlns:u=\"urn:schemas-upnp-org:service:AVTransport:1\">",
                "{}",
                "</u:GetInfoExResponse></s:Body></s:Envelope>"
            ),
            fields
        )
        .into_bytes()
    }

    #[test]
    fn get_info_ex_projects_only_allowlisted_media_state() {
        assert_eq!(
            parse_get_info_ex(INFO),
            Ok(PlaybackStatus {
                state: TransportState::Playing,
                transport_status: TransportStatus::Ok,
                volume: Some(9),
                muted: Some(false),
            })
        );
    }

    #[test]
    fn invalid_or_out_of_range_volume_is_rejected() {
        for value in ["101", "-1", "loud"] {
            let payload = response_with_fields(&format!("<CurrentVolume>{value}</CurrentVolume>"));
            assert_eq!(parse_get_info_ex(&payload), Err(JblError::MediaInfoInvalid));
        }
    }

    #[test]
    fn unknown_transport_and_source_are_sanitized_not_echoed() {
        let payload =
            response_with_fields("<CurrentTransportState>PRIVATE_STATE</CurrentTransportState>");
        let parsed = parse_get_info_ex(&payload).expect("synthetic XML should parse");
        assert_eq!(parsed.state, TransportState::Unknown);
        assert_eq!(
            parse_media_source(&json!({"error_code": 0, "media_source": "PRIVATE"})),
            Ok(MediaSource::Unknown)
        );
    }

    #[test]
    fn soap_envelope_body_response_and_fault_shape_are_strict() {
        let fixtures = [
            concat!(
                "<s:Envelope xmlns:s=\"urn:wrong-soap\"><s:Body>",
                "<u:GetInfoExResponse xmlns:u=\"urn:schemas-upnp-org:service:AVTransport:1\"/>",
                "</s:Body></s:Envelope>"
            ),
            concat!(
                "<s:Envelope xmlns:s=\"http://schemas.xmlsoap.org/soap/envelope/\">",
                "<Body><u:GetInfoExResponse ",
                "xmlns:u=\"urn:schemas-upnp-org:service:AVTransport:1\"/></Body>",
                "</s:Envelope>"
            ),
            concat!(
                "<s:Envelope xmlns:s=\"http://schemas.xmlsoap.org/soap/envelope/\">",
                "<s:Body><u:GetInfoExResponse xmlns:u=\"urn:wrong-service\"/>",
                "</s:Body></s:Envelope>"
            ),
            concat!(
                "<s:Envelope xmlns:s=\"http://schemas.xmlsoap.org/soap/envelope/\">",
                "<s:Body><s:Fault><faultcode>s:Client</faultcode></s:Fault></s:Body>",
                "</s:Envelope>"
            ),
            concat!(
                "<s:Envelope xmlns:s=\"http://schemas.xmlsoap.org/soap/envelope/\">",
                "<s:Body>",
                "<u:GetInfoExResponse xmlns:u=\"urn:schemas-upnp-org:service:AVTransport:1\"/>",
                "<u:GetInfoExResponse xmlns:u=\"urn:schemas-upnp-org:service:AVTransport:1\"/>",
                "</s:Body></s:Envelope>"
            ),
            concat!(
                "<s:Envelope xmlns:s=\"http://schemas.xmlsoap.org/soap/envelope/\">",
                "<s:Body><Wrapper>",
                "<u:GetInfoExResponse xmlns:u=\"urn:schemas-upnp-org:service:AVTransport:1\"/>",
                "</Wrapper></s:Body></s:Envelope>"
            ),
        ];
        for fixture in fixtures {
            assert_eq!(
                parse_get_info_ex(fixture.as_bytes()),
                Err(JblError::MediaInfoInvalid)
            );
        }

        let missing = concat!(
            "<s:Envelope xmlns:s=\"http://schemas.xmlsoap.org/soap/envelope/\">",
            "<s:Body><OtherResponse/></s:Body></s:Envelope>"
        );
        assert_eq!(
            parse_get_info_ex(missing.as_bytes()),
            Err(JblError::MediaInfoMissing)
        );
    }

    #[test]
    fn media_fields_are_unique_direct_and_correctly_namespaced() {
        for fields in [
            concat!(
                "<CurrentVolume>9</CurrentVolume>",
                "<CurrentVolume>10</CurrentVolume>"
            ),
            "<Wrapper><CurrentVolume>9</CurrentVolume></Wrapper>",
            "<x:CurrentVolume xmlns:x=\"urn:wrong-field\">9</x:CurrentVolume>",
            "<CurrentVolume><Value>9</Value></CurrentVolume>",
            "<CurrentVolume>9<!--decoy-->10</CurrentVolume>",
            concat!(
                "<CurrentTransportState>PLAYING</CurrentTransportState>",
                "<CurrentTransportState>STOPPED</CurrentTransportState>"
            ),
        ] {
            assert_eq!(
                parse_get_info_ex(&response_with_fields(fields)),
                Err(JblError::MediaInfoInvalid)
            );
        }
    }

    #[test]
    fn current_mute_accepts_only_exact_zero_or_one() {
        for (value, expected) in [("0", Some(false)), ("1", Some(true))] {
            let parsed = parse_get_info_ex(&response_with_fields(&format!(
                "<CurrentMute>{value}</CurrentMute>"
            )))
            .expect("exact numeric mute should parse");
            assert_eq!(parsed.muted, expected);
        }
        for invalid in ["true", "false", "True", "2", "-1"] {
            assert_eq!(
                parse_get_info_ex(&response_with_fields(&format!(
                    "<CurrentMute>{invalid}</CurrentMute>"
                ))),
                Err(JblError::MediaInfoInvalid)
            );
        }
    }

    #[test]
    fn strict_upnp_fault_extracts_only_the_numeric_code() {
        let fault = concat!(
            "<s:Envelope xmlns:s=\"http://schemas.xmlsoap.org/soap/envelope/\">",
            "<s:Body><s:Fault><faultcode>s:Client</faultcode>",
            "<faultstring>UPnPError</faultstring><detail>",
            "<UPnPError xmlns=\"urn:schemas-upnp-org:control-1-0\">",
            "<errorCode>501</errorCode><errorDescription>private text</errorDescription>",
            "</UPnPError></detail></s:Fault></s:Body></s:Envelope>"
        );
        assert_eq!(parse_upnp_action_fault(fault.as_bytes()), Ok(501));
    }

    #[test]
    fn upnp_fault_rejects_wrong_namespace_duplicates_and_nested_decoys() {
        for fault in [
            concat!(
                "<s:Envelope xmlns:s=\"http://schemas.xmlsoap.org/soap/envelope/\">",
                "<s:Body><s:Fault><detail><UPnPError xmlns=\"urn:wrong\">",
                "<errorCode>501</errorCode></UPnPError></detail></s:Fault></s:Body></s:Envelope>"
            ),
            concat!(
                "<s:Envelope xmlns:s=\"http://schemas.xmlsoap.org/soap/envelope/\">",
                "<s:Body><s:Fault><detail><UPnPError xmlns=\"urn:schemas-upnp-org:control-1-0\">",
                "<errorCode>501</errorCode><errorCode>501</errorCode>",
                "</UPnPError></detail></s:Fault></s:Body></s:Envelope>"
            ),
            concat!(
                "<s:Envelope xmlns:s=\"http://schemas.xmlsoap.org/soap/envelope/\">",
                "<s:Body><s:Fault><detail><UPnPError xmlns=\"urn:schemas-upnp-org:control-1-0\">",
                "<Wrapper><errorCode>501</errorCode></Wrapper>",
                "</UPnPError></detail></s:Fault></s:Body></s:Envelope>"
            ),
        ] {
            assert_eq!(
                parse_upnp_action_fault(fault.as_bytes()),
                Err(JblError::MediaInfoInvalid)
            );
        }
    }

    #[test]
    fn exact_media_source_tokens_are_closed() {
        assert_eq!(
            parse_media_source(&json!({"error_code": "0", "media_source": "BT"})),
            Ok(MediaSource::Bluetooth)
        );
        assert_eq!(
            parse_media_source(&json!({"error_code": 0, "media_source": "AP2"})),
            Ok(MediaSource::AirPlay2)
        );
        assert_eq!(
            parse_media_source(&json!({"error_code": 0, "media_source": "AUX"})),
            Ok(MediaSource::AuxIn)
        );
        assert_eq!(
            parse_media_source(&json!({"error_code": 0, "media_source": "USB"})),
            Ok(MediaSource::UsbPlayback)
        );
        assert_eq!(
            parse_media_source(&json!({"error_code": 0, "media_source": "aux"})),
            Ok(MediaSource::Unknown)
        );
    }

    #[test]
    fn upnp_serializers_are_typed_and_bounded() {
        let read = get_info_ex_request();
        assert_eq!(read.path, "/upnp/control/rendertransport1");
        assert!(read.soap_action.ends_with("#GetInfoEx\""));

        let volume = set_volume_request(9).expect("safe volume should serialize");
        assert!(volume.envelope.contains("<Channel>Single</Channel>"));
        assert!(volume.envelope.contains("<DesiredVolume>9</DesiredVolume>"));
        assert_eq!(set_volume_request(101), Err(JblError::InvalidVolume));

        let mute_on = set_mute_request(MuteTarget::On);
        assert!(mute_on.envelope.contains("<Channel>Master</Channel>"));
        assert!(mute_on.envelope.contains("<DesiredMute>1</DesiredMute>"));
        let mute_off = set_mute_request(MuteTarget::Off);
        assert!(mute_off.envelope.contains("<Channel>Master</Channel>"));
        assert!(mute_off.envelope.contains("<DesiredMute>0</DesiredMute>"));
        let play = playback_mutation_request(PlaybackTarget::Play);
        assert!(play.envelope.contains("<Speed>1</Speed>"));
        let pause = playback_mutation_request(PlaybackTarget::Pause);
        assert!(pause.envelope.contains("<InstanceID>0</InstanceID>"));
        assert!(!pause.envelope.contains("<Speed>"));
    }

    #[test]
    fn volume_result_never_promotes_a_postcondition_conflict() {
        let playback = PlaybackStatus {
            state: TransportState::Stopped,
            transport_status: TransportStatus::Ok,
            volume: Some(8),
            muted: Some(false),
        };
        assert!(VolumeWriteResult::AlreadyAtTarget(playback.clone()).accepted());
        assert!(VolumeWriteResult::Applied(playback.clone()).accepted());
        assert!(!VolumeWriteResult::TargetObservedAfterUnknownWrite(playback.clone()).accepted());
        assert!(!VolumeWriteResult::PostconditionFailed(playback).accepted());
        assert!(!VolumeWriteResult::OutcomeUnknown(JblError::NetworkUnreachable).accepted());
    }

    #[test]
    fn mute_result_promotes_only_already_or_applied() {
        let playback = PlaybackStatus {
            state: TransportState::Paused,
            transport_status: TransportStatus::Ok,
            volume: Some(9),
            muted: Some(true),
        };
        assert!(MuteWriteResult::AlreadyAtTarget(playback.clone()).accepted());
        assert!(MuteWriteResult::Applied(playback.clone()).accepted());
        assert!(!MuteWriteResult::TargetObservedAfterUnknownWrite(playback.clone()).accepted());
        assert!(!MuteWriteResult::PostconditionFailed(playback).accepted());
        assert!(!MuteWriteResult::OutcomeUnknown(JblError::NetworkUnreachable).accepted());
    }
}
