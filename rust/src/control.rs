use serde_json::Value;
use std::fmt::Write as _;
use zeroize::Zeroizing;

use crate::error::JblError;
use crate::model::{DeviceIdentity, SUPPORTED_JBL_MODEL};

/// The two Play Together mutations supported by the clean-room protocol.
///
/// There is intentionally no raw command or payload constructor. This keeps
/// callers from turning the fixed-purpose controller into an arbitrary OneOS
/// command sender.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlayTogetherCommand {
    Enter,
    Exit,
}

impl PlayTogetherCommand {
    /// Returns the exact form body accepted by the JBL OneOS HTTPS endpoint.
    pub const fn form_body(self) -> &'static str {
        match self {
            Self::Enter => "command=enterAuracast&payload=null",
            Self::Exit => "command=exitAuracast&payload=null",
        }
    }
}

/// Closed JBL broadcaster actions used by the cross-generation compatibility
/// transaction. There is no arbitrary command or payload constructor.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum BroadcastCommand {
    Start(DeviceIdentity),
    Stop,
}

impl BroadcastCommand {
    const COMMAND_ID: u16 = 7_957;

    /// Builds the exact compact JSON payload used by the official OneGatt path.
    ///
    /// The returned buffer zeroizes on drop because START contains the private
    /// configured JBL Bluetooth identity. This type intentionally implements
    /// neither Debug nor Display.
    fn payload_json(self) -> Zeroizing<String> {
        match self {
            Self::Start(identity) => {
                let address = identity.binary();
                let mut body = Zeroizing::new(String::with_capacity(256));
                write!(
                    &mut *body,
                    concat!(
                        "{{\"action\":1,\"broadcast\":{{\"address\":",
                        "\"{:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}\",",
                        "\"name\":\"{}\",\"need_access_code\":false,",
                        "\"status\":2,\"subgroup\":[{{\"active_status\":1,",
                        "\"index\":0,\"is_support\":true,\"quality\":0}}]}}}}"
                    ),
                    address[0],
                    address[1],
                    address[2],
                    address[3],
                    address[4],
                    address[5],
                    SUPPORTED_JBL_MODEL,
                )
                .expect("writing the closed broadcaster body cannot fail");
                body
            }
            Self::Stop => Zeroizing::new("{\"action\":2}".to_string()),
        }
    }

    /// Serializes one complete PL command. At the negotiated MTU 500 both
    /// supported payloads fit in exactly one ATT Write Request.
    pub(crate) fn pl_frame(self) -> Zeroizing<Vec<u8>> {
        let payload = self.payload_json();
        let payload_length =
            u16::try_from(payload.len()).expect("closed broadcaster payload fits u16");
        let mut frame = Zeroizing::new(Vec::with_capacity(6 + payload.len()));
        frame.extend_from_slice(b"PL");
        frame.extend_from_slice(&Self::COMMAND_ID.to_le_bytes());
        frame.extend_from_slice(&payload_length.to_le_bytes());
        frame.extend_from_slice(payload.as_bytes());
        frame
    }
}

/// A syntactically valid application-level acceptance response.
///
/// Constructing this marker through [`BasicResponse::parse`] proves only that
/// OneOS returned `error_code == 0`. It does not prove that the expected
/// two-member topology formed or that either speaker produced sound.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[must_use]
pub struct BasicResponse {
    _private: (),
}

impl BasicResponse {
    pub fn parse(payload: &[u8]) -> Result<Self, JblError> {
        let response: Value = serde_json::from_slice(payload).map_err(|_| JblError::InvalidJson)?;
        let response = response
            .as_object()
            .ok_or(JblError::BasicResponseNotObject)?;
        let error_code = response
            .get("error_code")
            .ok_or(JblError::BasicResponseCodeMissing)?;

        match error_code {
            Value::Number(code) if code.as_i64() == Some(0) || code.as_u64() == Some(0) => {
                Ok(Self { _private: () })
            }
            Value::String(code) if code == "0" => Ok(Self { _private: () }),
            Value::Number(code) if code.as_i64().is_some() || code.as_u64().is_some() => {
                Err(JblError::ControlCommandRejected)
            }
            Value::String(_) => Err(JblError::ControlCommandRejected),
            _ => Err(JblError::BasicResponseCodeInvalid),
        }
    }
}

/// Conservative state-machine outcomes for a Play Together write.
///
/// `Accepted` is deliberately weaker than topology success: the latter needs
/// a bounded `getAuraCastGroupInfo` postcondition read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[must_use]
pub enum PlayTogetherWriteOutcome {
    Accepted,
    Rejected,
    OutcomeUnknown,
    PostconditionFailed,
    RollbackFailed,
}

/// The typed result of one HTTPS Play Together command attempt.
///
/// `Accepted` means only that an HTTP 200 response carried a valid
/// [`BasicResponse`]. `Rejected` is reserved for a command that was definitely
/// not sent or was explicitly rejected by OneOS. Every failure after the form
/// body may have reached the device is `OutcomeUnknown`.
#[derive(Debug, Clone, PartialEq, Eq)]
#[must_use]
pub enum PlayTogetherWriteResult {
    Accepted(BasicResponse),
    Rejected(JblError),
    OutcomeUnknown(JblError),
}

impl PlayTogetherWriteResult {
    pub const fn outcome(&self) -> PlayTogetherWriteOutcome {
        match self {
            Self::Accepted(_) => PlayTogetherWriteOutcome::Accepted,
            Self::Rejected(_) => PlayTogetherWriteOutcome::Rejected,
            Self::OutcomeUnknown(_) => PlayTogetherWriteOutcome::OutcomeUnknown,
        }
    }

    pub const fn error(&self) -> Option<&JblError> {
        match self {
            Self::Accepted(_) => None,
            Self::Rejected(error) | Self::OutcomeUnknown(error) => Some(error),
        }
    }
}

impl From<BasicResponse> for PlayTogetherWriteOutcome {
    fn from(_response: BasicResponse) -> Self {
        Self::Accepted
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_generate_only_the_two_exact_form_bodies() {
        assert_eq!(
            PlayTogetherCommand::Enter.form_body(),
            "command=enterAuracast&payload=null"
        );
        assert_eq!(
            PlayTogetherCommand::Exit.form_body(),
            "command=exitAuracast&payload=null"
        );
    }

    #[test]
    fn broadcaster_commands_generate_only_the_two_exact_pl_frames() {
        let identity =
            DeviceIdentity::parse("02:00:00:00:00:02").expect("fixture identity should parse");
        let start = BroadcastCommand::Start(identity).pl_frame();
        assert_eq!(&start[..6], [0x50, 0x4c, 0x15, 0x1f, 0xc1, 0x00]);
        assert_eq!(
            std::str::from_utf8(&start[6..]).expect("fixture JSON is UTF-8"),
            concat!(
                "{\"action\":1,\"broadcast\":{\"address\":\"02:00:00:00:00:02\",",
                "\"name\":\"JBL Authentics 300\",\"need_access_code\":false,",
                "\"status\":2,\"subgroup\":[{\"active_status\":1,\"index\":0,",
                "\"is_support\":true,\"quality\":0}]}}"
            )
        );
        let stop = BroadcastCommand::Stop.pl_frame();
        assert_eq!(
            stop.as_slice(),
            [
                0x50, 0x4c, 0x15, 0x1f, 0x0c, 0x00, b'{', b'"', b'a', b'c', b't', b'i', b'o', b'n',
                b'"', b':', b'2', b'}'
            ]
        );
    }

    #[test]
    fn accepts_integer_and_string_zero() {
        for payload in [
            br#"{"error_code":0}"#.as_slice(),
            br#"{"error_code":"0"}"#.as_slice(),
            br#"{"error_code":0,"ignored":"field"}"#.as_slice(),
        ] {
            assert_eq!(
                BasicResponse::parse(payload),
                Ok(BasicResponse { _private: () })
            );
        }
    }

    #[test]
    fn rejects_non_object_json_with_a_typed_error() {
        for payload in [
            br#"[]"#.as_slice(),
            br#"null"#.as_slice(),
            br#""response""#.as_slice(),
            br#"0"#.as_slice(),
        ] {
            assert_eq!(
                BasicResponse::parse(payload),
                Err(JblError::BasicResponseNotObject)
            );
        }
    }

    #[test]
    fn rejects_missing_error_code_with_a_typed_error() {
        assert_eq!(
            BasicResponse::parse(br#"{}"#),
            Err(JblError::BasicResponseCodeMissing)
        );
    }

    #[test]
    fn rejects_nonzero_numeric_or_string_codes() {
        for payload in [
            br#"{"error_code":1}"#.as_slice(),
            br#"{"error_code":-1}"#.as_slice(),
            br#"{"error_code":"1"}"#.as_slice(),
            br#"{"error_code":"00"}"#.as_slice(),
            br#"{"error_code":" 0"}"#.as_slice(),
        ] {
            assert_eq!(
                BasicResponse::parse(payload),
                Err(JblError::ControlCommandRejected)
            );
        }
    }

    #[test]
    fn rejects_invalid_error_code_types() {
        for payload in [
            br#"{"error_code":null}"#.as_slice(),
            br#"{"error_code":false}"#.as_slice(),
            br#"{"error_code":[]}"#.as_slice(),
            br#"{"error_code":{}}"#.as_slice(),
            br#"{"error_code":0.0}"#.as_slice(),
        ] {
            assert_eq!(
                BasicResponse::parse(payload),
                Err(JblError::BasicResponseCodeInvalid)
            );
        }
    }

    #[test]
    fn malformed_json_keeps_the_existing_typed_failure() {
        assert_eq!(
            BasicResponse::parse(br#"{"error_code":0"#),
            Err(JblError::InvalidJson)
        );
    }

    #[test]
    fn basic_acceptance_maps_only_to_the_conservative_accepted_outcome() {
        let response =
            BasicResponse::parse(br#"{"error_code":0}"#).expect("zero response should be accepted");
        assert_eq!(
            PlayTogetherWriteOutcome::from(response),
            PlayTogetherWriteOutcome::Accepted
        );
        assert_ne!(
            PlayTogetherWriteOutcome::Accepted,
            PlayTogetherWriteOutcome::PostconditionFailed
        );
    }

    #[test]
    fn write_result_preserves_outcome_and_typed_cause() {
        let rejected = PlayTogetherWriteResult::Rejected(JblError::ControlCommandRejected);
        assert_eq!(rejected.outcome(), PlayTogetherWriteOutcome::Rejected);
        assert_eq!(rejected.error(), Some(&JblError::ControlCommandRejected));

        let unknown = PlayTogetherWriteResult::OutcomeUnknown(JblError::ResponseTooLarge);
        assert_eq!(unknown.outcome(), PlayTogetherWriteOutcome::OutcomeUnknown);
        assert_eq!(unknown.error(), Some(&JblError::ResponseTooLarge));
    }
}
