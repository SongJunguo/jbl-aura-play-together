//! Exact JBL Authentics 300 broadcaster mutation over a public LE ATT bearer.
//!
//! This module is deliberately limited to OneOS command 7957. It binds the
//! configured public identity, negotiates MTU 500, sends one ATT Write Request
//! to value handle 0x002a and requires the ATT Write Response. A separately
//! armed GENA observer will own the 7951 business-result gate; this transport
//! itself exposes ATT acknowledgement only.

use std::fmt;
use std::time::Duration;

use bluer::l2cap::{
    Security, SecurityLevel, SeqPacket, Socket as L2capSocket, SocketAddr as L2capSocketAddr,
};
use bluer::{Address, AddressType, Device, Session};
use tokio::runtime::{Builder, Runtime};
use tokio::time::{self, Instant};
use zeroize::Zeroizing;

use crate::control::BroadcastCommand;
use crate::model::DeviceIdentity;

const DEFAULT_ADAPTER: &str = "hci0";
const ATT_FIXED_CID: u16 = 0x0004;
const JBL_VALUE_HANDLE: u16 = 0x002a;
const ATT_EXCHANGE_MTU_REQUEST: u8 = 0x02;
const ATT_EXCHANGE_MTU_RESPONSE: u8 = 0x03;
const ATT_ERROR_RESPONSE: u8 = 0x01;
const ATT_WRITE_REQUEST: u8 = 0x12;
const ATT_WRITE_RESPONSE: u8 = 0x13;
const ATT_HANDLE_VALUE_NOTIFICATION: u8 = 0x1b;
const REQUESTED_MTU: u16 = 500;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);
const MTU_TIMEOUT: Duration = Duration::from_secs(5);
const WRITE_TIMEOUT: Duration = Duration::from_secs(5);
const RECEIVE_BUFFER_SIZE: usize = 517;
const MAX_PACKETS_PER_ACTION: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WriteResponseKind {
    Accepted,
    Notification,
    Rejected,
    Unexpected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum JblGattFailure {
    RuntimeUnavailable,
    AdapterUnavailable,
    AdapterPoweredOff,
    DeviceConnectionFailed,
    MtuExchangeFailed,
    TransportNotReady,
    FrameTooLarge,
    WriteFailed,
    WriteResponseTimedOut,
    ChannelClosed,
    UnexpectedResponse,
}

impl JblGattFailure {
    const fn label(self) -> &'static str {
        match self {
            Self::RuntimeUnavailable => "runtime_unavailable",
            Self::AdapterUnavailable => "adapter_unavailable",
            Self::AdapterPoweredOff => "adapter_powered_off",
            Self::DeviceConnectionFailed => "device_connection_failed",
            Self::MtuExchangeFailed => "mtu_exchange_failed",
            Self::TransportNotReady => "transport_not_ready",
            Self::FrameTooLarge => "frame_too_large",
            Self::WriteFailed => "write_failed",
            Self::WriteResponseTimedOut => "write_response_timed_out",
            Self::ChannelClosed => "channel_closed",
            Self::UnexpectedResponse => "unexpected_response",
        }
    }
}

impl fmt::Display for JblGattFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.label())
    }
}

impl std::error::Error for JblGattFailure {}

pub(crate) struct JblGattBroadcastTransport {
    runtime: Runtime,
    adapter_name: String,
    bearer: Option<JblBearer>,
}

impl JblGattBroadcastTransport {
    pub(crate) fn new() -> Result<Self, JblGattFailure> {
        let runtime = Builder::new_current_thread()
            .enable_io()
            .enable_time()
            .build()
            .map_err(|_| JblGattFailure::RuntimeUnavailable)?;
        Ok(Self {
            runtime,
            adapter_name: DEFAULT_ADAPTER.to_string(),
            bearer: None,
        })
    }

    pub(crate) fn arm(&mut self, identity: DeviceIdentity) -> Result<(), JblGattFailure> {
        drop(self.bearer.take());
        let adapter_name = self.adapter_name.clone();
        let bearer = self
            .runtime
            .block_on(connect_public_bearer(&adapter_name, identity))?;
        self.bearer = Some(bearer);
        Ok(())
    }

    /// Sends one complete 7957 frame and consumes the temporary bearer.
    /// No command is retried inside this transport.
    pub(crate) fn execute(&mut self, command: BroadcastCommand) -> Result<(), JblGattFailure> {
        let mut bearer = self
            .bearer
            .take()
            .ok_or(JblGattFailure::TransportNotReady)?;
        self.runtime
            .block_on(write_and_wait_for_ack(&mut bearer, command))
    }

    pub(crate) fn cancel(&mut self) {
        drop(self.bearer.take());
    }
}

impl fmt::Debug for JblGattBroadcastTransport {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("JblGattBroadcastTransport")
            .field("connection", &"redacted")
            .finish()
    }
}

struct JblBearer {
    _session: Session,
    _device: Device,
    socket: SeqPacket,
    mtu: u16,
}

async fn connect_public_bearer(
    adapter_name: &str,
    identity: DeviceIdentity,
) -> Result<JblBearer, JblGattFailure> {
    let deadline = Instant::now() + CONNECT_TIMEOUT;
    let session = timeout_at(deadline, Session::new(), JblGattFailure::AdapterUnavailable).await?;
    let adapter = session
        .adapter(adapter_name)
        .map_err(|_| JblGattFailure::AdapterUnavailable)?;
    let powered = timeout_at(
        deadline,
        adapter.is_powered(),
        JblGattFailure::AdapterUnavailable,
    )
    .await?;
    if !powered {
        return Err(JblGattFailure::AdapterPoweredOff);
    }
    let local_address = timeout_at(
        deadline,
        adapter.address(),
        JblGattFailure::AdapterUnavailable,
    )
    .await?;
    let target_address = Address::new(identity.binary());
    let device = adapter
        .device(target_address)
        .map_err(|_| JblGattFailure::DeviceConnectionFailed)?;
    let (local, target) = public_att_socket_addresses(local_address, target_address);
    let socket = connect_att_socket(local, target, deadline).await?;
    let mtu = exchange_mtu(&socket, Instant::now() + MTU_TIMEOUT).await?;
    Ok(JblBearer {
        _session: session,
        _device: device,
        socket,
        mtu,
    })
}

fn public_att_socket_addresses(
    local_address: Address,
    target_address: Address,
) -> (L2capSocketAddr, L2capSocketAddr) {
    (
        L2capSocketAddr {
            addr: local_address,
            addr_type: AddressType::LePublic,
            psm: 0,
            cid: ATT_FIXED_CID,
        },
        L2capSocketAddr {
            addr: target_address,
            addr_type: AddressType::LePublic,
            psm: 0,
            cid: ATT_FIXED_CID,
        },
    )
}

async fn connect_att_socket(
    local: L2capSocketAddr,
    target: L2capSocketAddr,
    deadline: Instant,
) -> Result<SeqPacket, JblGattFailure> {
    let socket = L2capSocket::<SeqPacket>::new_seq_packet()
        .map_err(|_| JblGattFailure::DeviceConnectionFailed)?;
    socket
        .bind(local)
        .map_err(|_| JblGattFailure::DeviceConnectionFailed)?;
    socket
        .set_security(Security {
            level: SecurityLevel::Low,
            key_size: 0,
        })
        .map_err(|_| JblGattFailure::DeviceConnectionFailed)?;
    let socket = timeout_at(
        deadline,
        socket.connect(target),
        JblGattFailure::DeviceConnectionFailed,
    )
    .await?;
    if socket
        .peer_addr()
        .map_err(|_| JblGattFailure::DeviceConnectionFailed)?
        != target
    {
        return Err(JblGattFailure::DeviceConnectionFailed);
    }
    Ok(socket)
}

async fn exchange_mtu(socket: &SeqPacket, deadline: Instant) -> Result<u16, JblGattFailure> {
    let request = [
        ATT_EXCHANGE_MTU_REQUEST,
        REQUESTED_MTU as u8,
        (REQUESTED_MTU >> 8) as u8,
    ];
    send_exact(
        socket,
        &request,
        deadline,
        JblGattFailure::MtuExchangeFailed,
    )
    .await?;
    let response = receive(socket, deadline, JblGattFailure::MtuExchangeFailed).await?;
    parse_mtu_response(&response).ok_or(JblGattFailure::MtuExchangeFailed)
}

fn parse_mtu_response(response: &[u8]) -> Option<u16> {
    let [ATT_EXCHANGE_MTU_RESPONSE, low, high] = response else {
        return None;
    };
    let server_mtu = u16::from_le_bytes([*low, *high]);
    (server_mtu >= 23).then_some(server_mtu.min(REQUESTED_MTU))
}

async fn write_and_wait_for_ack(
    bearer: &mut JblBearer,
    command: BroadcastCommand,
) -> Result<(), JblGattFailure> {
    let frame = command.pl_frame();
    let packet = att_write_packet(&frame, bearer.mtu)?;
    let deadline = Instant::now() + WRITE_TIMEOUT;
    send_exact(
        &bearer.socket,
        &packet,
        deadline,
        JblGattFailure::WriteFailed,
    )
    .await?;

    // Exact-firmware dynamic evidence shows no 7951 on this bearer. Ignore
    // unrelated notifications while waiting for the one ATT Write Response;
    // business success remains outside this transport and belongs to the GENA
    // observer once that stronger gate is enabled.
    for _ in 0..MAX_PACKETS_PER_ACTION {
        let response = receive(
            &bearer.socket,
            deadline,
            JblGattFailure::WriteResponseTimedOut,
        )
        .await?;
        match classify_write_response(&response) {
            WriteResponseKind::Accepted => return Ok(()),
            WriteResponseKind::Notification => continue,
            WriteResponseKind::Rejected => return Err(JblGattFailure::WriteFailed),
            WriteResponseKind::Unexpected => return Err(JblGattFailure::UnexpectedResponse),
        }
    }
    Err(JblGattFailure::UnexpectedResponse)
}

fn classify_write_response(response: &[u8]) -> WriteResponseKind {
    match response {
        [ATT_WRITE_RESPONSE] => WriteResponseKind::Accepted,
        [ATT_ERROR_RESPONSE, request_opcode, handle_low, handle_high, _]
            if *request_opcode == ATT_WRITE_REQUEST
                && u16::from_le_bytes([*handle_low, *handle_high]) == JBL_VALUE_HANDLE =>
        {
            WriteResponseKind::Rejected
        }
        [ATT_HANDLE_VALUE_NOTIFICATION, ..] => WriteResponseKind::Notification,
        _ => WriteResponseKind::Unexpected,
    }
}

fn att_write_packet(frame: &[u8], mtu: u16) -> Result<Zeroizing<Vec<u8>>, JblGattFailure> {
    let maximum = usize::from(mtu).saturating_sub(3);
    if frame.len() > maximum {
        return Err(JblGattFailure::FrameTooLarge);
    }
    let mut packet = Zeroizing::new(Vec::with_capacity(3 + frame.len()));
    packet.push(ATT_WRITE_REQUEST);
    packet.extend_from_slice(&JBL_VALUE_HANDLE.to_le_bytes());
    packet.extend_from_slice(frame);
    Ok(packet)
}

async fn send_exact(
    socket: &SeqPacket,
    payload: &[u8],
    deadline: Instant,
    failure: JblGattFailure,
) -> Result<(), JblGattFailure> {
    let written = timeout_at(deadline, socket.send(payload), failure).await?;
    if written == payload.len() {
        Ok(())
    } else {
        Err(failure)
    }
}

async fn receive(
    socket: &SeqPacket,
    deadline: Instant,
    timeout_failure: JblGattFailure,
) -> Result<Zeroizing<Vec<u8>>, JblGattFailure> {
    let mut buffer = Zeroizing::new(vec![0_u8; RECEIVE_BUFFER_SIZE]);
    let received = match time::timeout_at(deadline, socket.recv(&mut buffer)).await {
        Ok(Ok(0)) | Ok(Err(_)) => return Err(JblGattFailure::ChannelClosed),
        Ok(Ok(received)) => received,
        Err(_) => return Err(timeout_failure),
    };
    buffer.truncate(received);
    Ok(buffer)
}

async fn timeout_at<T, E, F>(
    deadline: Instant,
    future: F,
    timeout_failure: JblGattFailure,
) -> Result<T, JblGattFailure>
where
    F: std::future::Future<Output = Result<T, E>>,
{
    match time::timeout_at(deadline, future).await {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(_)) | Err(_) => Err(timeout_failure),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity() -> DeviceIdentity {
        DeviceIdentity::parse("02:00:00:00:00:02").expect("fixture identity")
    }

    #[test]
    fn public_socket_addresses_bind_exact_fixed_att_cid() {
        let local = Address::new([0x02, 0, 0, 0, 0, 1]);
        let target = Address::new(identity().binary());
        let (source, peer) = public_att_socket_addresses(local, target);
        assert_eq!(source.addr, local);
        assert_eq!(source.addr_type, AddressType::LePublic);
        assert_eq!(source.cid, ATT_FIXED_CID);
        assert_eq!(peer.addr, target);
        assert_eq!(peer.addr_type, AddressType::LePublic);
        assert_eq!(peer.cid, ATT_FIXED_CID);
    }

    #[test]
    fn mtu_parser_requires_exact_valid_exchange_response() {
        assert_eq!(parse_mtu_response(&[0x03, 0xf4, 0x01]), Some(500));
        assert_eq!(parse_mtu_response(&[0x03, 0x05, 0x02]), Some(500));
        assert_eq!(parse_mtu_response(&[0x03, 0x16, 0x00]), None);
        assert_eq!(parse_mtu_response(&[0x03, 0xf4]), None);
        assert_eq!(parse_mtu_response(&[0x13, 0xf4, 0x01]), None);
    }

    #[test]
    fn broadcaster_frame_is_one_exact_write_request_at_mtu_500() {
        let frame = BroadcastCommand::Start(identity()).pl_frame();
        let packet = att_write_packet(&frame, 500).expect("frame must fit");
        assert_eq!(&packet[..3], [0x12, 0x2a, 0x00]);
        assert_eq!(&packet[3..], frame.as_slice());
        assert_eq!(packet.len(), 202);
        assert_eq!(
            att_write_packet(&frame, u16::try_from(frame.len() + 2).unwrap()),
            Err(JblGattFailure::FrameTooLarge)
        );

        let stop_frame = BroadcastCommand::Stop.pl_frame();
        let stop_packet = att_write_packet(&stop_frame, 500).expect("STOP frame must fit");
        assert_eq!(&stop_packet[..3], [0x12, 0x2a, 0x00]);
        assert_eq!(&stop_packet[3..], stop_frame.as_slice());
        assert_eq!(stop_packet.len(), 21);
    }

    #[test]
    fn write_response_classifier_accepts_only_exact_att_ack() {
        assert_eq!(
            classify_write_response(&[ATT_WRITE_RESPONSE]),
            WriteResponseKind::Accepted
        );
        assert_eq!(
            classify_write_response(&[ATT_ERROR_RESPONSE, ATT_WRITE_REQUEST, 0x2a, 0x00, 0x03,]),
            WriteResponseKind::Rejected
        );
        assert_eq!(
            classify_write_response(&[ATT_HANDLE_VALUE_NOTIFICATION, 0x2a, 0x00]),
            WriteResponseKind::Notification
        );
        assert_eq!(
            classify_write_response(&[ATT_WRITE_RESPONSE, 0x00]),
            WriteResponseKind::Unexpected
        );
    }

    #[test]
    fn sanitized_failures_and_debug_contain_no_transport_material() {
        assert_eq!(
            format!(
                "{:?} {}",
                JblGattFailure::WriteFailed,
                JblGattFailure::WriteFailed
            ),
            "WriteFailed write_failed"
        );
        let transport = JblGattBroadcastTransport::new().expect("offline construction");
        assert_eq!(
            format!("{transport:?}"),
            "JblGattBroadcastTransport { connection: \"redacted\" }"
        );
    }
}
