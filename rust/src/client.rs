use std::error::Error as StdError;
use std::io::Read;
use std::net::IpAddr;
use std::path::Path;
use std::time::Duration;

use serde_json::Value;

use crate::control::{BasicResponse, PlayTogetherCommand, PlayTogetherWriteResult};
use crate::error::JblError;
use crate::model::{
    parse_device_info, parse_group_status, DeviceIdentity, GroupStatus, SanitizedStatus,
    SUPPORTED_JBL_MODEL,
};
use crate::tls::{build_tls_connector, parse_sha256_fingerprint, PeerPinMismatch};

const MAX_RESPONSE_BYTES: u64 = 1_048_576;

#[derive(Debug, Clone, Copy)]
struct ClientPorts {
    https: u16,
    upnp: u16,
}

const JBL_PORTS: ClientPorts = ClientPorts {
    https: 443,
    upnp: 59_152,
};

#[derive(Debug, Clone, Copy)]
enum Command {
    DeviceInfo,
    AuraCastGroupInfo,
}

impl Command {
    fn api_name(self) -> &'static str {
        match self {
            Self::DeviceInfo => "getDeviceInfo",
            Self::AuraCastGroupInfo => "getAuraCastGroupInfo",
        }
    }
}

pub struct JblLanClient {
    address: IpAddr,
    https_port: u16,
    upnp_port: u16,
    agent: ureq::Agent,
    write_agent: ureq::Agent,
    upnp_agent: ureq::Agent,
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
            .tls_connector(tls_connector)
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
        Ok(Self {
            address,
            https_port: ports.https,
            upnp_port: ports.upnp,
            agent,
            write_agent,
            upnp_agent,
        })
    }

    fn endpoint(&self, command: Command) -> String {
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

    fn get_json(&self, command: Command) -> Result<Value, JblError> {
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

    fn upnp_endpoint(&self) -> String {
        match self.address {
            IpAddr::V4(address) => {
                format!(
                    "http://{address}:{}/upnp/control/rendercontrol1",
                    self.upnp_port
                )
            }
            IpAddr::V6(address) => {
                format!(
                    "http://[{address}]:{}/upnp/control/rendercontrol1",
                    self.upnp_port
                )
            }
        }
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
            .post(&self.upnp_endpoint())
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
        let response = self.get_json(Command::AuraCastGroupInfo)?;
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
        parse_device_info(&self.get_json(Command::DeviceInfo)?)?;
        self.verified_model(expected_model)?;
        self.group_status(expected_jbl_identity, expected_aura_identity)
    }

    pub fn sanitized_status(
        &self,
        expected_model: &str,
        expected_jbl_identity: DeviceIdentity,
        expected_aura_identity: DeviceIdentity,
    ) -> Result<SanitizedStatus, JblError> {
        let mut device = parse_device_info(&self.get_json(Command::DeviceInfo)?)?;
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
