use std::fmt;
use std::io::{self, Read, Write};
use std::net::TcpStream;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use openssl::pkey::PKey;
use openssl::ssl::{HandshakeError, SslConnector, SslMethod, SslStream, SslVerifyMode};
use openssl::x509::X509;
use sha2::{Digest, Sha256};
use ureq::{ReadWrite, TlsConnector};
use zeroize::Zeroizing;

use crate::error::JblError;
use crate::private_file::{read_private_file, PrivateFileKind};

#[derive(Debug)]
pub(crate) struct PeerPinMismatch;

impl fmt::Display for PeerPinMismatch {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("peer certificate fingerprint mismatch")
    }
}

impl std::error::Error for PeerPinMismatch {}

const CERTIFICATE_BEGIN: &str = concat!("-----BEGIN CERT", "IFICATE-----");
const CERTIFICATE_END: &str = concat!("-----END CERT", "IFICATE-----");
const PRIVATE_KEY_BEGIN: &str = concat!("-----BEGIN PRIVATE ", "KEY-----");
const PRIVATE_KEY_END: &str = concat!("-----END PRIVATE ", "KEY-----");
const MAX_CREDENTIAL_BYTES: u64 = 262_144;

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn pem_block<'a>(payload: &'a [u8], begin: &[u8], end: &[u8]) -> Result<&'a [u8], JblError> {
    let start = find_bytes(payload, begin).ok_or(JblError::InvalidClientIdentity)?;
    let after_begin = start + begin.len();
    let relative_end =
        find_bytes(&payload[after_begin..], end).ok_or(JblError::InvalidClientIdentity)?;
    let block_end = after_begin + relative_end + end.len();
    Ok(&payload[start..block_end])
}

pub fn parse_sha256_fingerprint(value: &str) -> Result<[u8; 32], JblError> {
    let compact: String = value
        .chars()
        .filter(|character| !matches!(character, ':' | ' ' | '\t' | '\r' | '\n'))
        .collect();
    if compact.len() != 64
        || !compact
            .chars()
            .all(|character| character.is_ascii_hexdigit())
    {
        return Err(JblError::InvalidTlsFingerprint);
    }
    let mut bytes = [0_u8; 32];
    for (index, byte) in bytes.iter_mut().enumerate() {
        let offset = index * 2;
        *byte = u8::from_str_radix(&compact[offset..offset + 2], 16)
            .map_err(|_| JblError::InvalidTlsFingerprint)?;
    }
    Ok(bytes)
}

pub(crate) struct PinnedOpenSslConnector {
    inner: SslConnector,
    expected_sha256: [u8; 32],
}

impl fmt::Debug for PinnedOpenSslConnector {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PinnedOpenSslConnector")
            .finish_non_exhaustive()
    }
}

struct PinnedOpenSslStream {
    inner: SslStream<Box<dyn ReadWrite>>,
}

impl fmt::Debug for PinnedOpenSslStream {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PinnedOpenSslStream")
            .finish_non_exhaustive()
    }
}

impl Read for PinnedOpenSslStream {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        self.inner.read(buffer)
    }
}

impl Write for PinnedOpenSslStream {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        self.inner.write(buffer)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

impl ReadWrite for PinnedOpenSslStream {
    fn socket(&self) -> Option<&TcpStream> {
        self.inner.get_ref().socket()
    }
}

impl TlsConnector for PinnedOpenSslConnector {
    fn connect(
        &self,
        dns_name: &str,
        io: Box<dyn ReadWrite>,
    ) -> Result<Box<dyn ReadWrite>, ureq::Error> {
        let mut configuration = self
            .inner
            .configure()
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "TLS configuration failed"))?;
        configuration.set_verify_hostname(false);
        configuration.set_use_server_name_indication(false);
        let pin_checked = Arc::new(AtomicBool::new(false));
        let pin_matched = Arc::new(AtomicBool::new(false));
        let callback_checked = Arc::clone(&pin_checked);
        let callback_matched = Arc::clone(&pin_matched);
        let expected_sha256 = self.expected_sha256;
        configuration.set_verify_callback(SslVerifyMode::PEER, move |_preverified, context| {
            if context.error_depth() != 0 {
                return true;
            }
            callback_checked.store(true, Ordering::Release);
            let matched = context
                .current_cert()
                .and_then(|certificate| certificate.to_der().ok())
                .map(|der| {
                    let actual: [u8; 32] = Sha256::digest(&der).into();
                    actual == expected_sha256
                })
                .unwrap_or(false);
            callback_matched.store(matched, Ordering::Release);
            matched
        });
        match configuration.connect(dns_name, io) {
            Ok(stream) => {
                if !pin_checked.load(Ordering::Acquire) || !pin_matched.load(Ordering::Acquire) {
                    return Err(
                        io::Error::new(io::ErrorKind::PermissionDenied, PeerPinMismatch).into(),
                    );
                }
                Ok(Box::new(PinnedOpenSslStream { inner: stream }))
            }
            Err(HandshakeError::Failure(_failure)) => {
                let pin_failed =
                    pin_checked.load(Ordering::Acquire) && !pin_matched.load(Ordering::Acquire);
                if pin_failed {
                    Err(io::Error::new(io::ErrorKind::PermissionDenied, PeerPinMismatch).into())
                } else {
                    Err(
                        io::Error::new(io::ErrorKind::ConnectionAborted, "TLS handshake failed")
                            .into(),
                    )
                }
            }
            Err(HandshakeError::WouldBlock(_)) => {
                Err(io::Error::new(io::ErrorKind::WouldBlock, "TLS handshake would block").into())
            }
            Err(HandshakeError::SetupFailure(_)) => Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "TLS handshake setup failed",
            )
            .into()),
        }
    }
}

pub(crate) fn build_tls_connector(
    certificate_path: &Path,
    private_key_path: &Path,
    expected_sha256: [u8; 32],
) -> Result<Arc<PinnedOpenSslConnector>, JblError> {
    let certificate_payload = Zeroizing::new(read_private_file(
        certificate_path,
        MAX_CREDENTIAL_BYTES,
        PrivateFileKind::Certificate,
    )?);
    let private_key_payload = Zeroizing::new(read_private_file(
        private_key_path,
        MAX_CREDENTIAL_BYTES,
        PrivateFileKind::PrivateKey,
    )?);
    let certificate_pem = pem_block(
        certificate_payload.as_slice(),
        CERTIFICATE_BEGIN.as_bytes(),
        CERTIFICATE_END.as_bytes(),
    )?;
    let private_key_pem = pem_block(
        private_key_payload.as_slice(),
        PRIVATE_KEY_BEGIN.as_bytes(),
        PRIVATE_KEY_END.as_bytes(),
    )?;
    let certificate =
        X509::from_pem(certificate_pem).map_err(|_| JblError::InvalidClientIdentity)?;
    let private_key =
        PKey::private_key_from_pem(private_key_pem).map_err(|_| JblError::InvalidClientIdentity)?;

    let mut builder =
        SslConnector::builder(SslMethod::tls_client()).map_err(|_| JblError::TlsConfiguration)?;
    builder
        .set_certificate(&certificate)
        .map_err(|_| JblError::InvalidClientIdentity)?;
    builder
        .set_private_key(&private_key)
        .map_err(|_| JblError::InvalidClientIdentity)?;
    builder
        .check_private_key()
        .map_err(|_| JblError::InvalidClientIdentity)?;

    Ok(Arc::new(PinnedOpenSslConnector {
        inner: builder.build(),
        expected_sha256,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_space_separated_fingerprint() {
        let value = "00112233 44556677 8899aabb ccddeeff \
                     00112233 44556677 8899aabb ccddeeff";
        let parsed = parse_sha256_fingerprint(value).expect("fingerprint should parse");
        assert_eq!(parsed[0], 0x00);
        assert_eq!(parsed[31], 0xff);
    }

    #[test]
    fn rejects_short_fingerprint() {
        assert_eq!(
            parse_sha256_fingerprint("0011").unwrap_err(),
            JblError::InvalidTlsFingerprint
        );
    }

    #[test]
    fn extracts_pem_from_openssl_bag_attributes() {
        let payload = concat!(
            "Bag Attributes\nmetadata\n-----BEGIN PRIVATE ",
            "KEY-----\nabc\n-----END PRIVATE ",
            "KEY-----\n"
        );
        let block = pem_block(
            payload.as_bytes(),
            PRIVATE_KEY_BEGIN.as_bytes(),
            PRIVATE_KEY_END.as_bytes(),
        )
        .expect("PEM block should be found");
        assert!(block.starts_with(PRIVATE_KEY_BEGIN.as_bytes()));
        assert!(block.ends_with(PRIVATE_KEY_END.as_bytes()));
        assert!(!block.windows(3).any(|window| window == b"Bag"));
    }
}
