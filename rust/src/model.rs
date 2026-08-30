use serde::Serialize;
use serde_json::Value;

use crate::error::JblError;

pub const SUPPORTED_JBL_MODEL: &str = "JBL Authentics 300";
const JBL_PUBLIC_ROLE: &str = "JBL Authentics 300";
const AURA_PUBLIC_ROLE: &str = "Aura Studio 5";
const ALLOWED_CHANNELS: &[&str] = &[
    "front_left",
    "front_right",
    "left",
    "right",
    "mono",
    "stereo",
];

/// A private, normalized Bluetooth device identity.
///
/// This type intentionally implements neither `Debug`, `Display` nor
/// `Serialize`, so configuration and diagnostics cannot accidentally print a
/// paired device address. Values are constructed only from private runtime
/// configuration inside this crate.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct DeviceIdentity([u8; 6]);

impl DeviceIdentity {
    pub(crate) fn parse(value: &str) -> Option<Self> {
        let value = value.trim().as_bytes();
        let mut digits = [0_u8; 12];
        match value.len() {
            12 => digits.copy_from_slice(value),
            17 => {
                let separator = value[2];
                if !matches!(separator, b':' | b'-') {
                    return None;
                }
                let mut digit_index = 0_usize;
                for (index, byte) in value.iter().copied().enumerate() {
                    if matches!(index, 2 | 5 | 8 | 11 | 14) {
                        if byte != separator {
                            return None;
                        }
                    } else {
                        digits[digit_index] = byte;
                        digit_index += 1;
                    }
                }
            }
            _ => return None,
        }

        let mut bytes = [0_u8; 6];
        for (index, byte) in bytes.iter_mut().enumerate() {
            let high = hex_nibble(digits[index * 2])?;
            let low = hex_nibble(digits[index * 2 + 1])?;
            *byte = (high << 4) | low;
        }
        if bytes == [0; 6] || bytes[0] & 1 != 0 {
            return None;
        }
        Some(Self(bytes))
    }

    fn from_member_id(value: &str) -> Option<Self> {
        if value.len() != 12 {
            return None;
        }
        Self::parse(value)
    }

    /// Compares a device-originated binary identity without exposing the
    /// configured address to formatting, serialization, or logs.
    pub(crate) fn matches_binary(&self, candidate: &[u8]) -> bool {
        candidate == self.0
    }

    pub(crate) const fn binary(self) -> [u8; 6] {
        self.0
    }

    pub(crate) fn compact_config_value(&self) -> String {
        let mut encoded = String::with_capacity(12);
        for byte in self.0 {
            use std::fmt::Write as _;
            write!(&mut encoded, "{byte:02x}").expect("writing to String cannot fail");
        }
        encoded
    }
}

fn hex_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DeviceInfo {
    pub name: Option<String>,
    pub model: Option<String>,
    pub firmware: Option<String>,
    pub one_os_version: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GroupMember {
    pub name: String,
    pub channels: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GroupStatus {
    pub expected_pair_configured: bool,
    pub disabled: Option<bool>,
    pub member_count: usize,
    pub members: Vec<GroupMember>,
    pub error: Option<&'static str>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SanitizedStatus {
    pub device: DeviceInfo,
    pub play_together: GroupStatus,
}

fn safe_version(value: Option<&Value>) -> Option<String> {
    let raw = value?.as_str()?.trim();
    if raw.is_empty()
        || raw.len() > 40
        || !raw
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || ".-_".contains(character))
    {
        return None;
    }
    Some(raw.to_string())
}

fn error_code_is_zero(value: Option<&Value>) -> bool {
    matches!(value, Some(Value::String(code)) if code == "0")
        || matches!(value, Some(Value::Number(code)) if code.as_i64() == Some(0))
}

pub fn parse_device_info(response: &Value) -> Result<DeviceInfo, JblError> {
    if !error_code_is_zero(response.get("error_code")) {
        return Err(JblError::DeviceReportedError);
    }
    let info = response
        .get("device_info")
        .and_then(Value::as_object)
        .ok_or(JblError::DeviceInfoMissing)?;
    Ok(DeviceInfo {
        // Friendly names and identifiers are intentionally not projected.
        // sanitized_status fills these two fields only after independent model
        // verification through the UPnP device identity endpoint.
        name: None,
        model: None,
        firmware: safe_version(info.get("firmware")),
        one_os_version: safe_version(info.get("one_os_ver")),
    })
}

pub fn parse_group_status(
    response: &Value,
    expected_jbl_identity: DeviceIdentity,
    expected_aura_identity: DeviceIdentity,
) -> Result<GroupStatus, JblError> {
    if expected_jbl_identity == expected_aura_identity {
        return Err(JblError::InvalidConfig);
    }
    if !error_code_is_zero(response.get("error_code")) {
        return Err(JblError::DeviceReportedError);
    }
    let group = response
        .get("group_info")
        .and_then(Value::as_object)
        .ok_or(JblError::GroupInfoMissing)?;
    let raw_members = group
        .get("members")
        .and_then(Value::as_array)
        .ok_or(JblError::GroupMembersMissing)?;
    if raw_members.len() != 2 {
        return Err(JblError::GroupMemberInvalid);
    }
    let disabled = group
        .get("disabled")
        .and_then(Value::as_bool)
        .ok_or(JblError::GroupDisabledInvalid)?;

    let mut jbl_count = 0_usize;
    let mut aura_count = 0_usize;
    let members: Vec<GroupMember> = raw_members
        .iter()
        .map(|raw_member| {
            let member = raw_member.as_object().ok_or(JblError::GroupMemberInvalid)?;
            let raw_name = member
                .get("device_name")
                .and_then(Value::as_str)
                .filter(|name| !name.is_empty())
                .ok_or(JblError::GroupMemberInvalid)?;
            let raw_identity = member
                .get("id")
                .and_then(Value::as_str)
                .and_then(DeviceIdentity::from_member_id)
                .ok_or(JblError::GroupMemberInvalid)?;
            let name = if raw_name == JBL_PUBLIC_ROLE && raw_identity == expected_jbl_identity {
                jbl_count += 1;
                JBL_PUBLIC_ROLE.to_string()
            } else if raw_name == AURA_PUBLIC_ROLE && raw_identity == expected_aura_identity {
                aura_count += 1;
                AURA_PUBLIC_ROLE.to_string()
            } else {
                "unknown".to_string()
            };
            let raw_channels = member
                .get("channel")
                .and_then(Value::as_array)
                .ok_or(JblError::GroupMemberInvalid)?;
            let channels = raw_channels
                .iter()
                .map(|raw_channel| {
                    let channel = raw_channel
                        .as_str()
                        .ok_or(JblError::GroupMemberInvalid)?
                        .trim()
                        .to_ascii_lowercase();
                    Ok(if ALLOWED_CHANNELS.contains(&channel.as_str()) {
                        channel
                    } else {
                        "unknown".to_string()
                    })
                })
                .collect::<Result<Vec<_>, JblError>>()?;
            Ok(GroupMember { name, channels })
        })
        .collect::<Result<Vec<_>, JblError>>()?;

    let expected_pair_configured = jbl_count == 1 && aura_count == 1 && !disabled;

    Ok(GroupStatus {
        expected_pair_configured,
        disabled: Some(disabled),
        member_count: members.len(),
        members,
        error: (!expected_pair_configured).then_some("expected_pair_not_configured"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const JBL_MEMBER_ID: &str = "020000000001";
    const AURA_MEMBER_ID: &str = "020000000002";

    fn expected_identities() -> (DeviceIdentity, DeviceIdentity) {
        (
            DeviceIdentity::parse("02:00:00:00:00:01").expect("JBL placeholder should parse"),
            DeviceIdentity::parse("02:00:00:00:00:02").expect("Aura placeholder should parse"),
        )
    }

    fn parse(response: &Value) -> Result<GroupStatus, JblError> {
        let (jbl, aura) = expected_identities();
        parse_group_status(response, jbl, aura)
    }

    #[test]
    fn group_parser_configures_only_exact_names_and_private_identities() {
        let response = json!({
            "error_code": "0",
            "group_info": {
                "disabled": false,
                "group": {"id": "must-not-escape"},
                "members": [
                    {
                        "device_name": "JBL Authentics 300",
                        "channel": ["front_left"],
                        "id": JBL_MEMBER_ID
                    },
                    {
                        "device_name": "Aura Studio 5",
                        "channel": ["front_right"],
                        "id": AURA_MEMBER_ID
                    }
                ]
            }
        });
        let status = parse(&response).expect("synthetic response should parse");
        assert!(status.expected_pair_configured);
        assert_eq!(status.member_count, 2);
        assert_eq!(status.members[1].channels, vec!["front_right"]);
        let serialized = serde_json::to_string(&status).expect("status should serialize");
        assert!(!serialized.contains("\"id\""));
        assert!(!serialized.contains("must-not-escape"));
        assert!(!serialized.contains(JBL_MEMBER_ID));
        assert!(!serialized.contains(AURA_MEMBER_ID));
    }

    #[test]
    fn group_parser_refuses_identical_expected_identities() {
        let (jbl, _) = expected_identities();
        let response = json!({"error_code": "0"});
        assert_eq!(
            parse_group_status(&response, jbl, jbl),
            Err(JblError::InvalidConfig)
        );
    }

    #[test]
    fn malicious_member_rename_cannot_substitute_for_canonical_role() {
        let response = json!({
            "error_code": "0",
            "group_info": {
                "disabled": false,
                "members": [
                    {
                        "device_name": "attacker-controlled-jbl-name",
                        "id": JBL_MEMBER_ID,
                        "channel": []
                    },
                    {
                        "device_name": "Aura Studio 5",
                        "id": AURA_MEMBER_ID,
                        "channel": []
                    }
                ]
            }
        });
        let status = parse(&response).expect("synthetic response should parse");
        assert!(!status.expected_pair_configured);
        assert_eq!(status.members[0].name, "unknown");
        let serialized = serde_json::to_string(&status).expect("status should serialize");
        assert!(!serialized.contains("attacker-controlled"));
        assert!(!serialized.contains(JBL_MEMBER_ID));
    }

    #[test]
    fn swapped_or_unknown_member_id_cannot_verify_and_is_not_echoed() {
        for (jbl_id, aura_id) in [
            (AURA_MEMBER_ID, JBL_MEMBER_ID),
            ("020000000003", AURA_MEMBER_ID),
        ] {
            let response = json!({
                "error_code": "0",
                "group_info": {
                    "disabled": false,
                    "members": [
                        {
                            "device_name": "JBL Authentics 300",
                            "id": jbl_id,
                            "channel": []
                        },
                        {
                            "device_name": "Aura Studio 5",
                            "id": aura_id,
                            "channel": []
                        }
                    ]
                }
            });
            let status = parse(&response).expect("well-formed unknown identity should parse");
            assert!(!status.expected_pair_configured);
            let serialized = serde_json::to_string(&status).expect("status should serialize");
            assert!(!serialized.contains(jbl_id));
            assert!(!serialized.contains(aura_id));
        }
    }

    #[test]
    fn missing_or_malformed_member_id_is_a_typed_failure() {
        for invalid_aura in [
            json!({"device_name": "Aura Studio 5", "channel": []}),
            json!({"device_name": "Aura Studio 5", "id": null, "channel": []}),
            json!({"device_name": "Aura Studio 5", "id": "02:00:00:00:00:02", "channel": []}),
            json!({"device_name": "Aura Studio 5", "id": "not-12-hex", "channel": []}),
        ] {
            let response = json!({
                "error_code": "0",
                "group_info": {
                    "disabled": false,
                    "members": [
                        {
                            "device_name": "JBL Authentics 300",
                            "id": JBL_MEMBER_ID,
                            "channel": []
                        },
                        invalid_aura
                    ]
                }
            });
            assert_eq!(parse(&response), Err(JblError::GroupMemberInvalid));
        }
    }

    #[test]
    fn group_parser_rejects_missing_aura() {
        let response = json!({
            "error_code": 0,
            "group_info": {
                "disabled": false,
                "members": [
                    {
                        "device_name": "JBL Authentics 300",
                        "id": JBL_MEMBER_ID,
                        "channel": []
                    },
                    {
                        "device_name": "JBL Authentics 300",
                        "id": JBL_MEMBER_ID,
                        "channel": []
                    }
                ]
            }
        });
        let status = parse(&response).expect("synthetic response should parse");
        assert!(!status.expected_pair_configured);
        assert_eq!(status.error, Some("expected_pair_not_configured"));
    }

    #[test]
    fn unknown_channel_is_not_echoed() {
        let response = json!({
            "error_code": "0",
            "group_info": {
                "disabled": false,
                "members": [
                    {
                        "device_name": "JBL Authentics 300",
                        "id": JBL_MEMBER_ID,
                        "channel": ["private-value"]
                    },
                    {
                        "device_name": "Aura Studio 5",
                        "id": AURA_MEMBER_ID,
                        "channel": []
                    }
                ]
            }
        });
        let status = parse(&response).expect("synthetic response should parse");
        assert_eq!(status.members[0].channels, vec!["unknown"]);
    }

    #[test]
    fn missing_disabled_is_a_typed_failure() {
        let response = json!({
            "error_code": "0",
            "group_info": {
                "members": [
                    {"device_name": "JBL Authentics 300", "id": JBL_MEMBER_ID, "channel": []},
                    {"device_name": "Aura Studio 5", "id": AURA_MEMBER_ID, "channel": []}
                ]
            }
        });
        assert_eq!(parse(&response), Err(JblError::GroupDisabledInvalid));
    }

    #[test]
    fn non_boolean_disabled_is_a_typed_failure() {
        let response = json!({
            "error_code": "0",
            "group_info": {
                "disabled": "false",
                "members": [
                    {"device_name": "JBL Authentics 300", "id": JBL_MEMBER_ID, "channel": []},
                    {"device_name": "Aura Studio 5", "id": AURA_MEMBER_ID, "channel": []}
                ]
            }
        });
        assert_eq!(parse(&response), Err(JblError::GroupDisabledInvalid));
    }

    #[test]
    fn disabled_flag_is_preserved_without_claiming_ready_configuration() {
        let response = json!({
            "error_code": "0",
            "group_info": {
                "disabled": true,
                "members": [
                    {"device_name": "JBL Authentics 300", "id": JBL_MEMBER_ID, "channel": []},
                    {"device_name": "Aura Studio 5", "id": AURA_MEMBER_ID, "channel": []}
                ]
            }
        });
        let status = parse(&response).expect("well-formed disabled state should parse");
        assert!(!status.expected_pair_configured);
        assert_eq!(status.disabled, Some(true));
    }

    #[test]
    fn valid_pair_plus_malformed_third_member_is_rejected() {
        let response = json!({
            "error_code": "0",
            "group_info": {
                "disabled": false,
                "members": [
                    {"device_name": "JBL Authentics 300", "id": JBL_MEMBER_ID, "channel": []},
                    {"device_name": "Aura Studio 5", "id": AURA_MEMBER_ID, "channel": []},
                    "malformed-private-value"
                ]
            }
        });
        assert_eq!(parse(&response), Err(JblError::GroupMemberInvalid));
    }

    #[test]
    fn every_member_must_be_an_object_with_complete_fields() {
        let incomplete_members = [
            json!(null),
            json!({"id": AURA_MEMBER_ID, "channel": []}),
            json!({"device_name": "Aura Studio 5", "id": AURA_MEMBER_ID}),
            json!({"device_name": "Aura Studio 5", "id": AURA_MEMBER_ID, "channel": [7]}),
        ];

        for incomplete_member in incomplete_members {
            let response = json!({
                "error_code": "0",
                "group_info": {
                    "disabled": false,
                    "members": [
                        {"device_name": "JBL Authentics 300", "id": JBL_MEMBER_ID, "channel": []},
                        incomplete_member
                    ]
                }
            });
            assert_eq!(parse(&response), Err(JblError::GroupMemberInvalid));
        }
    }
}
