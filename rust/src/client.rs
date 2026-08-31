use std::collections::BTreeSet;
use std::error::Error as StdError;
use std::fmt;
use std::io::{self, Read, Write};
use std::net::{IpAddr, SocketAddr, TcpStream};
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::control::{BasicResponse, PlayTogetherCommand, PlayTogetherWriteResult};
use crate::eq::{parse_eq_catalog, parse_eq_feature, EqPresetTarget, EqPresetWriteResult};
use crate::error::JblError;
use crate::inspection::{
    parse_inspection_snapshot, InspectionPayloads, InspectionReadError, InspectionSnapshot,
    MAX_INSPECTION_RESPONSE_BYTES,
};
use crate::media::{
    get_info_ex_request, parse_audio_source_targets, parse_get_info_ex, parse_media_source,
    parse_upnp_action_fault, set_mute_request, set_volume_request, source_mutation_body,
    AudioSourceTarget, AudioSourceWriteResult, MediaSource, MediaStatus, MuteTarget,
    MuteWriteResult, PlaybackStatus, UpnpRequest, VolumeWriteResult,
};
#[cfg(test)]
use crate::media::{
    playback_mutation_request, PlaybackTarget, PlaybackWriteResult, TransportState, TransportStatus,
};
use crate::model::{
    parse_device_info, parse_group_status, DeviceIdentity, GroupStatus, SanitizedStatus,
    SUPPORTED_JBL_MODEL,
};
use crate::oneos::OneOsReadCommand;
#[cfg(target_os = "linux")]
use crate::service_runtime::DirectControlLock;
use crate::tls::{
    build_tls_connector, parse_sha256_fingerprint, PeerPinMismatch, PinnedOpenSslConnector,
};
use ureq::{ReadWrite, TlsConnector};

const MAX_RESPONSE_BYTES: u64 = 1_048_576;
const MAX_SOURCE_RESPONSE_HEADER_BYTES: usize = 16 * 1024;
const PREWRITE_READ_ATTEMPTS: usize = 3;
const PREWRITE_READ_DELAY: Duration = Duration::from_millis(100);
const SOURCE_SETTLE_DELAY: Duration = Duration::from_millis(350);

struct DeadlineTcpStream {
    inner: TcpStream,
    deadline: Instant,
}

impl fmt::Debug for DeadlineTcpStream {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DeadlineTcpStream")
            .field("endpoint", &"redacted")
            .finish()
    }
}

impl DeadlineTcpStream {
    fn remaining(&self) -> io::Result<Duration> {
        self.deadline
            .checked_duration_since(Instant::now())
            .filter(|duration| !duration.is_zero())
            .ok_or_else(|| io::Error::new(io::ErrorKind::TimedOut, "absolute deadline elapsed"))
    }
}

impl Read for DeadlineTcpStream {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        self.inner.set_read_timeout(Some(self.remaining()?))?;
        self.inner.read(buffer)
    }
}

impl Write for DeadlineTcpStream {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        self.inner.set_write_timeout(Some(self.remaining()?))?;
        self.inner.write(buffer)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.set_write_timeout(Some(self.remaining()?))?;
        self.inner.flush()
    }
}

impl ReadWrite for DeadlineTcpStream {
    fn socket(&self) -> Option<&TcpStream> {
        Some(&self.inner)
    }
}

#[derive(Debug, Clone, Copy)]
struct ClientPorts {
    https: u16,
    upnp: u16,
}

const JBL_PORTS: ClientPorts = ClientPorts {
    https: 443,
    upnp: 59_152,
};

pub struct JblLanClient {
    address: IpAddr,
    https_port: u16,
    upnp_port: u16,
    agent: ureq::Agent,
    write_agent: ureq::Agent,
    upnp_agent: ureq::Agent,
    upnp_write_agent: ureq::Agent,
    tls_connector: Arc<PinnedOpenSslConnector>,
    timeout: Duration,
}

pub(crate) struct JblDirectRead {
    pub(crate) media: MediaStatus,
    pub(crate) inspection: InspectionSnapshot,
    pub(crate) source_targets: Vec<AudioSourceTarget>,
    pub(crate) active_eq: Option<EqPresetTarget>,
}

impl JblLanClient {
    pub fn new(
        address: &str,
        certificate_path: &Path,
        private_key_path: &Path,
        tls_sha256: &str,
        timeout: Duration,
    ) -> Result<Self, JblError> {
        Self::new_with_ports(
            address,
            certificate_path,
            private_key_path,
            tls_sha256,
            timeout,
            JBL_PORTS,
        )
    }

    fn new_with_ports(
        address: &str,
        certificate_path: &Path,
        private_key_path: &Path,
        tls_sha256: &str,
        timeout: Duration,
        ports: ClientPorts,
    ) -> Result<Self, JblError> {
        let address = address
            .parse::<IpAddr>()
            .map_err(|_| JblError::InvalidAddress)?;
        let expected_sha256 = parse_sha256_fingerprint(tls_sha256)?;
        let tls_connector =
            build_tls_connector(certificate_path, private_key_path, expected_sha256)?;
        let agent = ureq::AgentBuilder::new()
            .tls_connector(tls_connector.clone())
            .try_proxy_from_env(false)
            .https_only(true)
            .redirects(0)
            .timeout(timeout)
            .timeout_connect(timeout)
            .timeout_read(timeout)
            .timeout_write(timeout)
            .build();
        // State-changing POSTs have a dedicated no-pool Agent. ureq 2.9.1 may
        // transparently retry a failed prelude on a recycled connection, so a
        // read Agent must never be reused for writes.
        let write_agent = ureq::AgentBuilder::new()
            .tls_connector(tls_connector.clone())
            .try_proxy_from_env(false)
            .https_only(true)
            .redirects(0)
            .max_idle_connections(0)
            .max_idle_connections_per_host(0)
            .timeout(timeout)
            .timeout_connect(timeout)
            .timeout_read(timeout)
            .timeout_write(timeout)
            .build();
        let upnp_agent = ureq::AgentBuilder::new()
            .try_proxy_from_env(false)
            .redirects(0)
            .timeout(timeout)
            .timeout_connect(timeout)
            .timeout_read(timeout)
            .timeout_write(timeout)
            .build();
        let upnp_write_agent = ureq::AgentBuilder::new()
            .try_proxy_from_env(false)
            .redirects(0)
            .max_idle_connections(0)
            .max_idle_connections_per_host(0)
            .timeout(timeout)
            .timeout_connect(timeout)
            .timeout_read(timeout)
            .timeout_write(timeout)
            .build();
        Ok(Self {
            address,
            https_port: ports.https,
            upnp_port: ports.upnp,
            agent,
            write_agent,
            upnp_agent,
            upnp_write_agent,
            tls_connector,
            timeout,
        })
    }

    fn endpoint(&self, command: OneOsReadCommand) -> String {
        match self.address {
            IpAddr::V4(address) if self.https_port == JBL_PORTS.https => format!(
                "https://{address}/httpapi.asp?command={}",
                command.api_name()
            ),
            IpAddr::V4(address) => format!(
                "https://{address}:{}/httpapi.asp?command={}",
                self.https_port,
                command.api_name()
            ),
            IpAddr::V6(address) if self.https_port == JBL_PORTS.https => format!(
                "https://[{address}]/httpapi.asp?command={}",
                command.api_name()
            ),
            IpAddr::V6(address) => format!(
                "https://[{address}]:{}/httpapi.asp?command={}",
                self.https_port,
                command.api_name()
            ),
        }
    }

    fn get_json(&self, command: OneOsReadCommand) -> Result<Value, JblError> {
        let response = self
            .agent
            .get(&self.endpoint(command))
            .set("Accept", "application/json")
            .set("Accept-Encoding", "identity")
            .call()
            .map_err(|error| match error {
                ureq::Error::Status(status, _) => JblError::HttpStatus(status),
                ureq::Error::Transport(transport) => {
                    if has_peer_pin_mismatch(&transport) {
                        JblError::PeerCertificateMismatch
                    } else {
                        JblError::NetworkUnreachable
                    }
                }
            })?;

        if response.status() != 200 {
            return Err(JblError::HttpStatus(response.status()));
        }
        let mut payload = Vec::new();
        response
            .into_reader()
            .take(MAX_RESPONSE_BYTES + 1)
            .read_to_end(&mut payload)
            .map_err(|_| JblError::NetworkUnreachable)?;
        if payload.len() as u64 > MAX_RESPONSE_BYTES {
            return Err(JblError::ResponseTooLarge);
        }
        serde_json::from_slice(&payload).map_err(|_| JblError::InvalidJson)
    }

    fn get_inspection_payload(&self, command: OneOsReadCommand) -> Result<Vec<u8>, JblError> {
        let response = self
            .agent
            .get(&self.endpoint(command))
            .set("Accept", "application/json")
            .set("Accept-Encoding", "identity")
            .call()
            .map_err(|error| match error {
                ureq::Error::Status(status, _) => JblError::HttpStatus(status),
                ureq::Error::Transport(transport) => {
                    if has_peer_pin_mismatch(&transport) {
                        JblError::PeerCertificateMismatch
                    } else {
                        JblError::NetworkUnreachable
                    }
                }
            })?;

        if response.status() != 200 {
            return Err(JblError::HttpStatus(response.status()));
        }
        let mut payload = Vec::new();
        response
            .into_reader()
            .take(MAX_INSPECTION_RESPONSE_BYTES + 1)
            .read_to_end(&mut payload)
            .map_err(|_| JblError::NetworkUnreachable)?;
        if payload.len() as u64 > MAX_INSPECTION_RESPONSE_BYTES {
            return Err(JblError::ResponseTooLarge);
        }
        Ok(payload)
    }

    fn write_endpoint(&self) -> String {
        match self.address {
            IpAddr::V4(address) if self.https_port == JBL_PORTS.https => {
                format!("https://{address}/httpapi.asp")
            }
            IpAddr::V4(address) => {
                format!("https://{address}:{}/httpapi.asp", self.https_port)
            }
            IpAddr::V6(address) if self.https_port == JBL_PORTS.https => {
                format!("https://[{address}]/httpapi.asp")
            }
            IpAddr::V6(address) => {
                format!("https://[{address}]:{}/httpapi.asp", self.https_port)
            }
        }
    }

    fn send_source_mutation_once(&self, target: AudioSourceTarget) -> Result<(), JblError> {
        let body = source_mutation_body(target);
        self.send_pinned_raw_mutation(&body)
    }

    fn send_pinned_raw_mutation(&self, body: &str) -> Result<(), JblError> {
        let deadline = Instant::now() + self.timeout;
        let peer = SocketAddr::new(self.address, self.https_port);
        let connect_timeout = deadline
            .checked_duration_since(Instant::now())
            .filter(|duration| !duration.is_zero())
            .ok_or(JblError::NetworkUnreachable)?;
        let socket = TcpStream::connect_timeout(&peer, connect_timeout)
            .map_err(|_| JblError::NetworkUnreachable)?;
        let host = match self.address {
            IpAddr::V4(address) if self.https_port == 443 => address.to_string(),
            IpAddr::V4(address) => format!("{address}:{}", self.https_port),
            IpAddr::V6(address) if self.https_port == 443 => format!("[{address}]"),
            IpAddr::V6(address) => format!("[{address}]:{}", self.https_port),
        };
        let mut stream = self
            .tls_connector
            .connect(
                &host,
                Box::new(DeadlineTcpStream {
                    inner: socket,
                    deadline,
                }),
            )
            .map_err(map_direct_tls_error)?;
        let request = format!(
            concat!(
                "POST /httpapi.asp? HTTP/1.1\r\n",
                "Host: {}\r\n",
                "Accept: application/json\r\n",
                "Accept-Encoding: identity\r\n",
                "Content-Length: {}\r\n",
                "Connection: close\r\n\r\n",
                "{}"
            ),
            host,
            body.len(),
            body
        );
        stream
            .write_all(request.as_bytes())
            .and_then(|()| stream.flush())
            .map_err(|_| JblError::NetworkUnreachable)?;
        let mut response = Vec::new();
        let mut chunk = [0_u8; 4096];
        loop {
            match stream.read(&mut chunk) {
                Ok(0) => break,
                Ok(count) => {
                    response.extend_from_slice(&chunk[..count]);
                    if response.len()
                        > MAX_SOURCE_RESPONSE_HEADER_BYTES + 4 + MAX_RESPONSE_BYTES as usize
                    {
                        return Err(JblError::ResponseTooLarge);
                    }
                    if source_response_is_complete(&response)? {
                        break;
                    }
                }
                Err(_) => {
                    if source_response_is_complete(&response)? {
                        break;
                    }
                    return Err(JblError::NetworkUnreachable);
                }
            }
        }
        let payload = parse_source_http_response(&response)?;
        BasicResponse::parse(payload).map(drop)
    }

    /// Sends exactly one closed-set Play Together command attempt.
    ///
    /// An `Accepted` result proves application-level acknowledgement only.
    /// Callers must verify the expected topology separately.
    pub(crate) fn send_play_together(
        &self,
        command: PlayTogetherCommand,
    ) -> PlayTogetherWriteResult {
        self.send_control_form(command.form_body().as_bytes())
    }

    fn send_control_form(&self, form_body: &[u8]) -> PlayTogetherWriteResult {
        let response = self
            .write_agent
            .post(&self.write_endpoint())
            .set("Accept", "application/json")
            .set("Accept-Encoding", "identity")
            .set("Content-Type", "application/x-www-form-urlencoded")
            .send_bytes(form_body);

        let response = match response {
            Ok(response) | Err(ureq::Error::Status(_, response)) => response,
            Err(ureq::Error::Transport(transport)) => {
                return classify_write_transport(transport);
            }
        };
        let status = response.status();
        if status != 200 {
            return PlayTogetherWriteResult::OutcomeUnknown(JblError::HttpStatus(status));
        }
        let mut payload = Vec::new();
        if response
            .into_reader()
            .take(MAX_RESPONSE_BYTES + 1)
            .read_to_end(&mut payload)
            .is_err()
        {
            return PlayTogetherWriteResult::OutcomeUnknown(JblError::NetworkUnreachable);
        }
        if payload.len() as u64 > MAX_RESPONSE_BYTES {
            return PlayTogetherWriteResult::OutcomeUnknown(JblError::ResponseTooLarge);
        }

        match BasicResponse::parse(&payload) {
            Ok(response) => PlayTogetherWriteResult::Accepted(response),
            Err(JblError::ControlCommandRejected) => {
                PlayTogetherWriteResult::Rejected(JblError::ControlCommandRejected)
            }
            Err(error) => PlayTogetherWriteResult::OutcomeUnknown(error),
        }
    }

    fn upnp_endpoint(&self, path: &str) -> String {
        match self.address {
            IpAddr::V4(address) => format!("http://{address}:{}{path}", self.upnp_port),
            IpAddr::V6(address) => format!("http://[{address}]:{}{path}", self.upnp_port),
        }
    }

    fn send_upnp_read(&self, request: &UpnpRequest) -> Result<Vec<u8>, JblError> {
        let response = self
            .upnp_agent
            .post(&self.upnp_endpoint(request.path))
            .set("Content-Type", "text/xml;charset=\"utf-8\"")
            .set("SOAPAction", &request.soap_action)
            .send_string(&request.envelope)
            .map_err(|error| match error {
                ureq::Error::Status(status, _) => JblError::HttpStatus(status),
                ureq::Error::Transport(_) => JblError::NetworkUnreachable,
            })?;
        if response.status() != 200 {
            return Err(JblError::HttpStatus(response.status()));
        }
        let mut payload = Vec::new();
        response
            .into_reader()
            .take(MAX_RESPONSE_BYTES + 1)
            .read_to_end(&mut payload)
            .map_err(|_| JblError::NetworkUnreachable)?;
        if payload.len() as u64 > MAX_RESPONSE_BYTES {
            return Err(JblError::ResponseTooLarge);
        }
        Ok(payload)
    }

    fn send_upnp_mutation_once(&self, request: &UpnpRequest) -> Result<(), JblError> {
        let response = match self
            .upnp_write_agent
            .post(&self.upnp_endpoint(request.path))
            .set("Content-Type", "text/xml;charset=\"utf-8\"")
            .set("SOAPAction", &request.soap_action)
            .send_string(&request.envelope)
        {
            Ok(response) | Err(ureq::Error::Status(_, response)) => response,
            Err(ureq::Error::Transport(_)) => return Err(JblError::NetworkUnreachable),
        };
        let status = response.status();
        let mut payload = Vec::new();
        response
            .into_reader()
            .take(MAX_RESPONSE_BYTES + 1)
            .read_to_end(&mut payload)
            .map_err(|_| JblError::NetworkUnreachable)?;
        if payload.len() as u64 > MAX_RESPONSE_BYTES {
            return Err(JblError::ResponseTooLarge);
        }
        if status == 200 {
            return Ok(());
        }
        if status == 500 {
            return match parse_upnp_action_fault(&payload) {
                Ok(501) => Err(JblError::UpnpActionRejected),
                Ok(_) => Err(JblError::HttpStatus(status)),
                Err(error) => Err(error),
            };
        }
        Err(JblError::HttpStatus(status))
    }

    fn playback_status_upnp(&self) -> Result<PlaybackStatus, JblError> {
        parse_get_info_ex(&self.send_upnp_read(&get_info_ex_request())?)
    }

    fn verify_pinned_device_info(&self) -> Result<(), JblError> {
        parse_device_info(&self.get_json(OneOsReadCommand::DeviceInfo)?).map(drop)
    }

    /// Retries only read-only setup that is provably before a mutation. Once a
    /// write is attempted, callers must never use this helper for readback or
    /// switch transports.
    fn bounded_prewrite_read<T>(
        &self,
        mut operation: impl FnMut() -> Result<T, JblError>,
    ) -> Result<T, JblError> {
        for attempt in 0..PREWRITE_READ_ATTEMPTS {
            match operation() {
                Ok(value) => return Ok(value),
                Err(JblError::NetworkUnreachable) if attempt + 1 < PREWRITE_READ_ATTEMPTS => {
                    std::thread::sleep(PREWRITE_READ_DELAY);
                }
                Err(error) => return Err(error),
            }
        }
        unreachable!("the fixed pre-write attempt loop always returns")
    }

    fn verified_model(&self, expected_model: &str) -> Result<String, JblError> {
        if expected_model != SUPPORTED_JBL_MODEL {
            return Err(JblError::UnexpectedDeviceModel);
        }
        let envelope = concat!(
            "<?xml version=\"1.0\" encoding=\"utf-8\"?>",
            "<s:Envelope s:encodingStyle=\"http://schemas.xmlsoap.org/soap/encoding/\" ",
            "xmlns:s=\"http://schemas.xmlsoap.org/soap/envelope/\"><s:Body>",
            "<u:GetControlDeviceInfo xmlns:u=\"urn:schemas-upnp-org:service:RenderingControl:1\">",
            "<InstanceID>0</InstanceID></u:GetControlDeviceInfo></s:Body></s:Envelope>"
        );
        let response = self
            .upnp_agent
            .post(&self.upnp_endpoint("/upnp/control/rendercontrol1"))
            .set("Content-Type", "text/xml;charset=\"utf-8\"")
            .set(
                "SOAPAction",
                "\"urn:schemas-upnp-org:service:RenderingControl:1#GetControlDeviceInfo\"",
            )
            .send_string(envelope)
            .map_err(|error| match error {
                ureq::Error::Status(status, _) => JblError::HttpStatus(status),
                ureq::Error::Transport(_) => JblError::NetworkUnreachable,
            })?;
        let mut payload = Vec::new();
        response
            .into_reader()
            .take(MAX_RESPONSE_BYTES + 1)
            .read_to_end(&mut payload)
            .map_err(|_| JblError::NetworkUnreachable)?;
        if payload.len() as u64 > MAX_RESPONSE_BYTES {
            return Err(JblError::ResponseTooLarge);
        }
        let xml = std::str::from_utf8(&payload).map_err(|_| JblError::InvalidXml)?;
        let document = roxmltree::Document::parse(xml).map_err(|_| JblError::InvalidXml)?;
        let status = document
            .descendants()
            .find(|node| node.is_element() && node.tag_name().name() == "Status")
            .and_then(|node| node.text())
            .ok_or(JblError::ControlDeviceInfoMissing)?;
        let value: Value = serde_json::from_str(status).map_err(|_| JblError::InvalidJson)?;
        let actual_model = value
            .get("hm_product_name")
            .and_then(Value::as_str)
            .map(str::trim)
            .ok_or(JblError::ControlDeviceInfoMissing)?;
        if actual_model != expected_model {
            return Err(JblError::UnexpectedDeviceModel);
        }
        Ok(SUPPORTED_JBL_MODEL.to_string())
    }

    fn group_status(
        &self,
        expected_jbl_identity: DeviceIdentity,
        expected_aura_identity: DeviceIdentity,
    ) -> Result<GroupStatus, JblError> {
        let response = self.get_json(OneOsReadCommand::AuraCastGroupInfo)?;
        parse_group_status(&response, expected_jbl_identity, expected_aura_identity)
    }

    pub fn pair_configuration_status(
        &self,
        expected_model: &str,
        expected_jbl_identity: DeviceIdentity,
        expected_aura_identity: DeviceIdentity,
    ) -> Result<GroupStatus, JblError> {
        // The mTLS read authenticates the IP before the unauthenticated UPnP
        // model cross-check and the subsequent mTLS group query.
        parse_device_info(&self.get_json(OneOsReadCommand::DeviceInfo)?)?;
        self.verified_model(expected_model)?;
        self.group_status(expected_jbl_identity, expected_aura_identity)
    }

    pub fn sanitized_status(
        &self,
        expected_model: &str,
        expected_jbl_identity: DeviceIdentity,
        expected_aura_identity: DeviceIdentity,
    ) -> Result<SanitizedStatus, JblError> {
        let mut device = parse_device_info(&self.get_json(OneOsReadCommand::DeviceInfo)?)?;
        // Bind the unauthenticated UPnP identity read to an IP that has just
        // passed the pinned mTLS request above.
        let verified_model = self.verified_model(expected_model)?;
        device.name = Some(verified_model.clone());
        device.model = Some(verified_model);
        let play_together = self.group_status(expected_jbl_identity, expected_aura_identity)?;
        Ok(SanitizedStatus {
            device,
            play_together,
        })
    }

    /// Reads the exact-device current source and UPnP transport projection.
    ///
    /// The pinned mTLS device-info read and UPnP model cross-check happen
    /// before the unauthenticated UPnP media read. Unknown source/state values
    /// are projected to closed `Unknown` variants and are never echoed.
    pub fn media_status(&self, expected_model: &str) -> Result<MediaStatus, JblError> {
        parse_device_info(&self.get_json(OneOsReadCommand::DeviceInfo)?)?;
        self.verified_model(expected_model)?;
        let source = parse_media_source(&self.get_json(OneOsReadCommand::MediaSource)?)?;
        let playback = self.playback_status_upnp()?;
        Ok(MediaStatus { playback, source })
    }

    /// Reads the closed inspection inventory after pinned-mTLS and exact-model
    /// identity gates. Returned DTOs cannot represent raw device values.
    pub fn inspection_snapshot(
        &self,
        expected_model: &str,
    ) -> Result<InspectionSnapshot, InspectionReadError> {
        parse_device_info(&self.get_json(OneOsReadCommand::DeviceInfo)?)?;
        self.verified_model(expected_model)?;

        let feature_support = self.get_inspection_payload(OneOsReadCommand::FeatureSupport)?;
        let eq_list = self.get_inspection_payload(OneOsReadCommand::EqList)?;
        let eq = self.get_inspection_payload(OneOsReadCommand::Eq)?;
        let audio_sources = self.get_inspection_payload(OneOsReadCommand::DeviceAudioSourceList)?;
        let personal_listening =
            self.get_inspection_payload(OneOsReadCommand::PersonalListeningMode)?;
        let audio_sync = self.get_inspection_payload(OneOsReadCommand::AudioSync)?;
        let media_source_activity =
            self.get_inspection_payload(OneOsReadCommand::MediaSourceStatus)?;

        parse_inspection_snapshot(InspectionPayloads {
            feature_support: &feature_support,
            eq_list: &eq_list,
            eq: &eq,
            audio_sources: &audio_sources,
            personal_listening: &personal_listening,
            audio_sync: &audio_sync,
            media_source_activity: &media_source_activity,
        })
        .map_err(InspectionReadError::from)
    }

    /// One fixed read plan for the Web card: one pinned identity check, one
    /// exact-model check, one UPnP playback read and the seven inspection
    /// payloads. It deliberately reuses the already-fetched feature, EQ and
    /// source-list payloads instead of repeating identity and network reads.
    pub(crate) fn direct_read(
        &self,
        expected_model: &str,
    ) -> Result<JblDirectRead, InspectionReadError> {
        self.verify_pinned_device_info()?;
        self.verified_model(expected_model)?;
        let playback = self.playback_status_upnp()?;

        let feature_support = self.get_inspection_payload(OneOsReadCommand::FeatureSupport)?;
        let eq_list = self.get_inspection_payload(OneOsReadCommand::EqList)?;
        let eq = self.get_inspection_payload(OneOsReadCommand::Eq)?;
        let audio_sources = self.get_inspection_payload(OneOsReadCommand::DeviceAudioSourceList)?;
        let personal_listening =
            self.get_inspection_payload(OneOsReadCommand::PersonalListeningMode)?;
        let audio_sync = self.get_inspection_payload(OneOsReadCommand::AudioSync)?;
        let media_source_activity =
            self.get_inspection_payload(OneOsReadCommand::MediaSourceStatus)?;

        let inspection = parse_inspection_snapshot(InspectionPayloads {
            feature_support: &feature_support,
            eq_list: &eq_list,
            eq: &eq,
            audio_sources: &audio_sources,
            personal_listening: &personal_listening,
            audio_sync: &audio_sync,
            media_source_activity: &media_source_activity,
        })?;
        let feature_value: Value =
            serde_json::from_slice(&feature_support).map_err(|_| JblError::InvalidJson)?;
        parse_eq_feature(&feature_value)?;
        let eq_list_value: Value =
            serde_json::from_slice(&eq_list).map_err(|_| JblError::InvalidJson)?;
        let active_eq = parse_eq_catalog(&eq_list_value)?.active();
        let source_targets = inspection
            .audio_sources
            .support_sources
            .iter()
            .filter_map(|source| match source {
                MediaSource::Bluetooth => Some(AudioSourceTarget::Bluetooth),
                MediaSource::AuxIn => Some(AudioSourceTarget::AuxIn),
                MediaSource::UsbPlayback => Some(AudioSourceTarget::UsbPlayback),
                _ => None,
            })
            .collect();
        let media = MediaStatus {
            playback,
            source: inspection.media_source_activity.source,
        };
        Ok(JblDirectRead {
            media,
            inspection,
            source_targets,
            active_eq,
        })
    }

    /// Sends one typed UPnP volume mutation and performs an independent
    /// readback. The method never retries and never switches transport.
    #[cfg(target_os = "linux")]
    pub fn set_volume(
        &self,
        _control_lock: &mut DirectControlLock,
        expected_model: &str,
        volume: u8,
    ) -> VolumeWriteResult {
        if volume > crate::media::MAX_SAFE_DIRECT_VOLUME {
            return VolumeWriteResult::RejectedBeforeSend(JblError::VolumeSafetyLimitExceeded);
        }
        let request = match set_volume_request(volume) {
            Ok(request) => request,
            Err(error) => return VolumeWriteResult::RejectedBeforeSend(error),
        };
        if let Err(error) = self.bounded_prewrite_read(|| self.verify_pinned_device_info()) {
            return VolumeWriteResult::RejectedBeforeSend(error);
        }
        if let Err(error) = self.bounded_prewrite_read(|| self.verified_model(expected_model)) {
            return VolumeWriteResult::RejectedBeforeSend(error);
        }
        let before = match self.bounded_prewrite_read(|| self.playback_status_upnp()) {
            Ok(playback) => playback,
            Err(error) => return VolumeWriteResult::RejectedBeforeSend(error),
        };
        if before.volume.is_none() {
            return VolumeWriteResult::RejectedBeforeSend(JblError::MediaVolumeMissing);
        }
        if before.volume == Some(volume) {
            return VolumeWriteResult::AlreadyAtTarget(before);
        }

        let mutation = self.send_upnp_mutation_once(&request);
        let after = match self.playback_status_upnp() {
            Ok(playback) => playback,
            Err(error) => {
                return VolumeWriteResult::OutcomeUnknown(mutation.err().unwrap_or(error));
            }
        };
        // UPnP is plain HTTP. Revalidate the pinned mTLS peer after the
        // mutation/readback window before trusting the observed postcondition.
        if let Err(error) = self.bounded_prewrite_read(|| self.verify_pinned_device_info()) {
            return VolumeWriteResult::OutcomeUnknown(error);
        }

        if after.volume == Some(volume) {
            return match mutation {
                Ok(()) => VolumeWriteResult::Applied(after),
                Err(_) if before.volume.is_some_and(|value| value != volume) => {
                    VolumeWriteResult::TargetObservedAfterUnknownWrite(after)
                }
                Err(error) => VolumeWriteResult::OutcomeUnknown(error),
            };
        }
        match mutation {
            Ok(()) => VolumeWriteResult::PostconditionFailed(after),
            Err(error) => VolumeWriteResult::OutcomeUnknown(error),
        }
    }

    /// Sends one absolute UPnP mute mutation and performs one independent
    /// readback. The lock is a required type-level permit; this method neither
    /// retries nor exposes a toggle operation.
    #[cfg(target_os = "linux")]
    pub fn set_mute(
        &self,
        _control_lock: &mut DirectControlLock,
        expected_model: &str,
        target: MuteTarget,
    ) -> MuteWriteResult {
        if let Err(error) = self.bounded_prewrite_read(|| self.verify_pinned_device_info()) {
            return MuteWriteResult::RejectedBeforeSend(error);
        }
        if let Err(error) = self.bounded_prewrite_read(|| self.verified_model(expected_model)) {
            return MuteWriteResult::RejectedBeforeSend(error);
        }
        let before = match self.bounded_prewrite_read(|| self.playback_status_upnp()) {
            Ok(playback) => playback,
            Err(error) => return MuteWriteResult::RejectedBeforeSend(error),
        };
        let Some(before_muted) = before.muted else {
            return MuteWriteResult::RejectedBeforeSend(JblError::MediaMuteMissing);
        };
        let desired = target.desired();
        if before_muted == desired {
            return MuteWriteResult::AlreadyAtTarget(before);
        }
        if before
            .volume
            .is_none_or(|volume| volume > crate::media::MAX_SAFE_DIRECT_VOLUME)
        {
            return MuteWriteResult::RejectedBeforeSend(
                before.volume.map_or(JblError::MediaVolumeMissing, |_| {
                    JblError::VolumeSafetyLimitExceeded
                }),
            );
        }

        let request = set_mute_request(target);
        let mutation = self.send_upnp_mutation_once(&request);
        let after = self.playback_status_upnp();
        // UPnP is plain HTTP. Always revalidate the pinned mTLS peer after the
        // mutation/readback window, even when the readback itself failed.
        if let Err(error) = self.verify_pinned_device_info() {
            return MuteWriteResult::OutcomeUnknown(error);
        }
        let after = match after {
            Ok(playback) => playback,
            Err(error) => return MuteWriteResult::OutcomeUnknown(mutation.err().unwrap_or(error)),
        };

        if after.muted == Some(desired) {
            return match mutation {
                Ok(()) => MuteWriteResult::Applied(after),
                Err(_) if before_muted != desired => {
                    MuteWriteResult::TargetObservedAfterUnknownWrite(after)
                }
                Err(error) => MuteWriteResult::OutcomeUnknown(error),
            };
        }
        match mutation {
            Ok(()) => MuteWriteResult::PostconditionFailed(after),
            Err(error) => MuteWriteResult::OutcomeUnknown(error),
        }
    }

    /// Sends one source-gated Bluetooth play or pause target. It performs no
    /// write retry, cross-bearer fallback, or unsupported transport action.
    #[cfg(all(target_os = "linux", test))]
    pub(crate) fn set_playback(
        &self,
        _control_lock: &mut DirectControlLock,
        expected_model: &str,
        target: PlaybackTarget,
    ) -> PlaybackWriteResult {
        if let Err(error) = self.bounded_prewrite_read(|| self.verify_pinned_device_info()) {
            return PlaybackWriteResult::RejectedBeforeSend(error);
        }
        if let Err(error) = self.bounded_prewrite_read(|| self.verified_model(expected_model)) {
            return PlaybackWriteResult::RejectedBeforeSend(error);
        }
        let source = match self.bounded_prewrite_read(|| {
            self.get_json(OneOsReadCommand::MediaSource)
                .and_then(|response| parse_media_source(&response))
        }) {
            Ok(source) => source,
            Err(error) => return PlaybackWriteResult::RejectedBeforeSend(error),
        };
        if source != MediaSource::Bluetooth {
            return PlaybackWriteResult::RejectedBeforeSend(JblError::UnsupportedMediaSource);
        }
        let before = match self.bounded_prewrite_read(|| self.playback_status_upnp()) {
            Ok(playback) => playback,
            Err(error) => return PlaybackWriteResult::RejectedBeforeSend(error),
        };
        if before.transport_status != TransportStatus::Ok {
            return PlaybackWriteResult::RejectedBeforeSend(JblError::PlaybackPreconditionFailed);
        }
        if target == PlaybackTarget::Play
            && before
                .volume
                .is_none_or(|volume| volume > crate::media::MAX_SAFE_DIRECT_VOLUME)
        {
            return PlaybackWriteResult::RejectedBeforeSend(
                before.volume.map_or(JblError::MediaVolumeMissing, |_| {
                    JblError::VolumeSafetyLimitExceeded
                }),
            );
        }
        if before.state == target.desired_state() {
            return PlaybackWriteResult::AlreadyAtTarget(MediaStatus {
                playback: before,
                source,
            });
        }
        let valid_predecessor = match target {
            PlaybackTarget::Play => {
                matches!(
                    before.state,
                    TransportState::Paused | TransportState::Stopped
                )
            }
            PlaybackTarget::Pause => before.state == TransportState::Playing,
        };
        if !valid_predecessor {
            return PlaybackWriteResult::RejectedBeforeSend(JblError::PlaybackPreconditionFailed);
        }

        let request = playback_mutation_request(target);
        let mutation = self.send_upnp_mutation_once(&request);
        let after_source = self
            .get_json(OneOsReadCommand::MediaSource)
            .and_then(|response| parse_media_source(&response));
        let after_playback = self.playback_status_upnp();
        if let Err(error) = self.verify_pinned_device_info() {
            return PlaybackWriteResult::OutcomeUnknown(error);
        }
        let after_source = match after_source {
            Ok(source) => source,
            Err(error) => return PlaybackWriteResult::OutcomeUnknown(error),
        };
        let after_playback = match after_playback {
            Ok(playback) => playback,
            Err(error) => {
                return PlaybackWriteResult::OutcomeUnknown(mutation.err().unwrap_or(error));
            }
        };
        let after = MediaStatus {
            playback: after_playback,
            source: after_source,
        };
        if matches!(mutation, Err(JblError::UpnpActionRejected)) {
            return PlaybackWriteResult::RejectedByDevice(after);
        }
        if after.source != MediaSource::Bluetooth {
            return match mutation {
                Ok(()) => PlaybackWriteResult::PostconditionFailed(after),
                Err(_) => PlaybackWriteResult::OutcomeUnknown(JblError::MediaSourceChanged),
            };
        }
        let target_observed = after.playback.state == target.desired_state()
            && after.playback.transport_status == TransportStatus::Ok;
        if target_observed {
            return match mutation {
                Ok(()) => PlaybackWriteResult::Applied(after),
                Err(_) => PlaybackWriteResult::TargetObservedAfterUnknownWrite(after),
            };
        }
        match mutation {
            Ok(()) => PlaybackWriteResult::PostconditionFailed(after),
            Err(error) => PlaybackWriteResult::OutcomeUnknown(error),
        }
    }

    #[cfg(target_os = "linux")]
    pub fn set_audio_source(
        &self,
        _control_lock: &mut DirectControlLock,
        expected_model: &str,
        target: AudioSourceTarget,
    ) -> AudioSourceWriteResult {
        if let Err(error) = self.bounded_prewrite_read(|| self.verify_pinned_device_info()) {
            return AudioSourceWriteResult::RejectedBeforeSend(error);
        }
        if let Err(error) = self.bounded_prewrite_read(|| self.verified_model(expected_model)) {
            return AudioSourceWriteResult::RejectedBeforeSend(error);
        }
        let current = match self.bounded_prewrite_read(|| {
            self.get_json(OneOsReadCommand::MediaSource)
                .and_then(|response| parse_media_source(&response))
        }) {
            Ok(source) => source,
            Err(error) => return AudioSourceWriteResult::RejectedBeforeSend(error),
        };
        let supported = match self.bounded_prewrite_read(|| {
            self.get_json(OneOsReadCommand::DeviceAudioSourceList)
                .and_then(|response| parse_audio_source_targets(&response))
        }) {
            Ok(supported) => supported,
            Err(error) => return AudioSourceWriteResult::RejectedBeforeSend(error),
        };
        if !supported.contains(&target) {
            return AudioSourceWriteResult::RejectedBeforeSend(JblError::UnsupportedMediaSource);
        }
        let playback = match self.bounded_prewrite_read(|| self.playback_status_upnp()) {
            Ok(playback) => playback,
            Err(error) => return AudioSourceWriteResult::RejectedBeforeSend(error),
        };
        if playback
            .volume
            .is_none_or(|volume| volume > crate::media::MAX_SAFE_DIRECT_VOLUME)
        {
            return AudioSourceWriteResult::RejectedBeforeSend(
                playback.volume.map_or(JblError::MediaVolumeMissing, |_| {
                    JblError::VolumeSafetyLimitExceeded
                }),
            );
        }
        if current == target.source() {
            return AudioSourceWriteResult::AlreadyAtTarget(current);
        }

        let mutation = self.send_source_mutation_once(target);
        if !matches!(mutation, Err(JblError::ControlCommandRejected)) {
            // Exact-device observation: A300 publishes the new source roughly
            // 300 ms after a successful setMediaSource response. Wait once,
            // then perform exactly one readback; never resend or poll.
            std::thread::sleep(SOURCE_SETTLE_DELAY);
        }
        let after = self
            .get_json(OneOsReadCommand::MediaSource)
            .and_then(|response| parse_media_source(&response));
        if let Err(error) = self.verify_pinned_device_info() {
            return AudioSourceWriteResult::OutcomeUnknown(error);
        }
        let after = match after {
            Ok(source) => source,
            Err(error) => {
                return AudioSourceWriteResult::OutcomeUnknown(mutation.err().unwrap_or(error));
            }
        };
        if matches!(mutation, Err(JblError::ControlCommandRejected)) {
            return AudioSourceWriteResult::RejectedByDevice(after);
        }
        if after == target.source() {
            return match mutation {
                Ok(()) => AudioSourceWriteResult::Applied(after),
                Err(_) => AudioSourceWriteResult::TargetObservedAfterUnknownWrite(after),
            };
        }
        match mutation {
            Ok(()) => AudioSourceWriteResult::PostconditionFailed(after),
            Err(error) => AudioSourceWriteResult::OutcomeUnknown(error),
        }
    }

    #[cfg(target_os = "linux")]
    pub fn set_eq_preset(
        &self,
        _control_lock: &mut DirectControlLock,
        expected_model: &str,
        target: EqPresetTarget,
    ) -> EqPresetWriteResult {
        if let Err(error) = self.bounded_prewrite_read(|| self.verify_pinned_device_info()) {
            return EqPresetWriteResult::RejectedBeforeSend(error);
        }
        if let Err(error) = self.bounded_prewrite_read(|| self.verified_model(expected_model)) {
            return EqPresetWriteResult::RejectedBeforeSend(error);
        }
        if let Err(error) = self.bounded_prewrite_read(|| {
            self.get_json(OneOsReadCommand::FeatureSupport)
                .and_then(|response| parse_eq_feature(&response))
        }) {
            return EqPresetWriteResult::RejectedBeforeSend(error);
        }
        let catalog = match self.bounded_prewrite_read(|| {
            self.get_json(OneOsReadCommand::EqList)
                .and_then(|response| parse_eq_catalog(&response))
        }) {
            Ok(catalog) => catalog,
            Err(error) => return EqPresetWriteResult::RejectedBeforeSend(error),
        };
        let playback = match self.bounded_prewrite_read(|| self.playback_status_upnp()) {
            Ok(playback) => playback,
            Err(error) => return EqPresetWriteResult::RejectedBeforeSend(error),
        };
        if playback
            .volume
            .is_none_or(|volume| volume > crate::media::MAX_SAFE_DIRECT_VOLUME)
        {
            return EqPresetWriteResult::RejectedBeforeSend(
                playback.volume.map_or(JblError::MediaVolumeMissing, |_| {
                    JblError::VolumeSafetyLimitExceeded
                }),
            );
        }
        if catalog.active().is_none() {
            return EqPresetWriteResult::RejectedBeforeSend(JblError::EqPresetInvalid);
        }
        if catalog.active() == Some(target) {
            return EqPresetWriteResult::AlreadyAtTarget(target);
        }
        let body = match catalog.mutation_body(target) {
            Ok(body) => body,
            Err(error) => return EqPresetWriteResult::RejectedBeforeSend(error),
        };
        let mutation = self.send_pinned_raw_mutation(&body);
        std::thread::sleep(Duration::from_millis(350));
        let after = self
            .get_json(OneOsReadCommand::EqList)
            .and_then(|response| parse_eq_catalog(&response));
        if let Err(error) = self.verify_pinned_device_info() {
            return EqPresetWriteResult::OutcomeUnknown(error);
        }
        let after = match after {
            Ok(catalog) => catalog.active(),
            Err(error) => {
                return EqPresetWriteResult::OutcomeUnknown(mutation.err().unwrap_or(error))
            }
        };
        if matches!(mutation, Err(JblError::ControlCommandRejected)) {
            return EqPresetWriteResult::RejectedByDevice(target);
        }
        if after == Some(target) {
            return match mutation {
                Ok(()) => EqPresetWriteResult::Applied(target),
                Err(_) => EqPresetWriteResult::TargetObservedAfterUnknownWrite(target),
            };
        }
        match mutation {
            Ok(()) => EqPresetWriteResult::PostconditionFailed(after),
            Err(error) => EqPresetWriteResult::OutcomeUnknown(error),
        }
    }
}

fn has_peer_pin_mismatch(transport: &ureq::Transport) -> bool {
    let mut source = transport.source();
    while let Some(error) = source {
        if error_is_peer_pin_mismatch(error) {
            return true;
        }
        source = error.source();
    }
    false
}

fn map_direct_tls_error(error: ureq::Error) -> JblError {
    match error {
        ureq::Error::Transport(transport) if has_peer_pin_mismatch(&transport) => {
            JblError::PeerCertificateMismatch
        }
        _ => JblError::NetworkUnreachable,
    }
}

fn parse_source_http_response(response: &[u8]) -> Result<&[u8], JblError> {
    let header_end = response
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .ok_or(JblError::InvalidHttpResponse)?;
    if header_end > MAX_SOURCE_RESPONSE_HEADER_BYTES
        || response.len() > MAX_SOURCE_RESPONSE_HEADER_BYTES + 4 + MAX_RESPONSE_BYTES as usize
    {
        return Err(JblError::ResponseTooLarge);
    }
    let head =
        std::str::from_utf8(&response[..header_end]).map_err(|_| JblError::InvalidHttpResponse)?;
    if !head
        .bytes()
        .all(|byte| matches!(byte, b'\t' | b'\r' | b'\n' | 0x20..=0x7e))
    {
        return Err(JblError::InvalidHttpResponse);
    }
    let mut lines = head.split("\r\n");
    let status_line = lines.next().ok_or(JblError::InvalidHttpResponse)?;
    let mut status_parts = status_line.splitn(3, ' ');
    if status_parts.next() != Some("HTTP/1.1") {
        return Err(JblError::InvalidHttpResponse);
    }
    let status = status_parts
        .next()
        .filter(|value| value.len() == 3 && value.bytes().all(|byte| byte.is_ascii_digit()))
        .and_then(|value| value.parse::<u16>().ok())
        .ok_or(JblError::InvalidHttpResponse)?;
    if status_parts.next().is_none() {
        return Err(JblError::InvalidHttpResponse);
    }
    let mut names = BTreeSet::new();
    let mut content_length = None;
    for line in lines {
        if line.is_empty() || line.starts_with([' ', '\t']) {
            return Err(JblError::InvalidHttpResponse);
        }
        let (name, value) = line.split_once(':').ok_or(JblError::InvalidHttpResponse)?;
        let name = name.to_ascii_lowercase();
        if !names.insert(name.clone()) || name == "transfer-encoding" {
            return Err(JblError::InvalidHttpResponse);
        }
        if name == "content-length" {
            content_length = Some(
                value
                    .trim()
                    .parse::<usize>()
                    .ok()
                    .filter(|length| *length <= MAX_RESPONSE_BYTES as usize)
                    .ok_or(JblError::InvalidHttpResponse)?,
            );
        }
    }
    let content_length = content_length.ok_or(JblError::InvalidHttpResponse)?;
    let payload = &response[header_end + 4..];
    if payload.len() != content_length {
        return Err(JblError::InvalidHttpResponse);
    }
    if status != 200 {
        return Err(JblError::HttpStatus(status));
    }
    Ok(payload)
}

fn source_response_is_complete(response: &[u8]) -> Result<bool, JblError> {
    let Some(header_end) = response.windows(4).position(|window| window == b"\r\n\r\n") else {
        if response.len() > MAX_SOURCE_RESPONSE_HEADER_BYTES + 4 {
            return Err(JblError::ResponseTooLarge);
        }
        return Ok(false);
    };
    if header_end > MAX_SOURCE_RESPONSE_HEADER_BYTES {
        return Err(JblError::ResponseTooLarge);
    }
    let head =
        std::str::from_utf8(&response[..header_end]).map_err(|_| JblError::InvalidHttpResponse)?;
    let mut content_length = None;
    for line in head.split("\r\n").skip(1) {
        let Some((name, value)) = line.split_once(':') else {
            return Err(JblError::InvalidHttpResponse);
        };
        if name.eq_ignore_ascii_case("transfer-encoding") {
            return Err(JblError::InvalidHttpResponse);
        }
        if name.eq_ignore_ascii_case("content-length") {
            if content_length.is_some() {
                return Err(JblError::InvalidHttpResponse);
            }
            content_length = Some(
                value
                    .trim()
                    .parse::<usize>()
                    .ok()
                    .filter(|length| *length <= MAX_RESPONSE_BYTES as usize)
                    .ok_or(JblError::InvalidHttpResponse)?,
            );
        }
    }
    let length = content_length.ok_or(JblError::InvalidHttpResponse)?;
    let expected = header_end + 4 + length;
    if response.len() > expected {
        return Err(JblError::InvalidHttpResponse);
    }
    Ok(response.len() == expected)
}

fn error_is_peer_pin_mismatch(error: &(dyn StdError + 'static)) -> bool {
    if error.downcast_ref::<PeerPinMismatch>().is_some() {
        return true;
    }
    // std::io::Error does not consistently expose its custom boxed error
    // through Error::source, so inspect that box by concrete type while still
    // requiring the exact PeerPinMismatch marker.
    error
        .downcast_ref::<std::io::Error>()
        .and_then(std::io::Error::get_ref)
        .and_then(|inner| inner.downcast_ref::<PeerPinMismatch>())
        .is_some()
}

fn classify_write_transport(transport: ureq::Transport) -> PlayTogetherWriteResult {
    if has_peer_pin_mismatch(&transport) {
        return PlayTogetherWriteResult::Rejected(JblError::PeerCertificateMismatch);
    }
    let definitely_not_sent = matches!(
        transport.kind(),
        ureq::ErrorKind::InvalidUrl
            | ureq::ErrorKind::UnknownScheme
            | ureq::ErrorKind::Dns
            | ureq::ErrorKind::InsecureRequestHttpsOnly
            | ureq::ErrorKind::ConnectionFailed
            | ureq::ErrorKind::InvalidProxyUrl
            | ureq::ErrorKind::ProxyConnect
            | ureq::ErrorKind::ProxyUnauthorized
    );
    if definitely_not_sent {
        PlayTogetherWriteResult::Rejected(JblError::NetworkUnreachable)
    } else {
        PlayTogetherWriteResult::OutcomeUnknown(JblError::NetworkUnreachable)
    }
}

#[cfg(all(test, unix))]
#[path = "../tests/support/network_negative.rs"]
mod network_negative_tests;
