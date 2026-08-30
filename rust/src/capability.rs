//! Exact-device capability maturity projected without private device data.

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityMaturity {
    ImplementedReadOnly,
    ImplementedVerifiedWrite,
    ProtocolPortedResearchOnly,
    SerializerOnly,
    EvidenceRequired,
    NotAdvertisedByExactProfile,
    Forbidden,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Capability {
    pub id: &'static str,
    pub maturity: CapabilityMaturity,
}

const AUTHENTICS_300_CAPABILITIES: &[Capability] = &[
    Capability {
        id: "device_info",
        maturity: CapabilityMaturity::ImplementedReadOnly,
    },
    Capability {
        id: "play_together_membership",
        maturity: CapabilityMaturity::ImplementedReadOnly,
    },
    Capability {
        id: "media_status",
        maturity: CapabilityMaturity::ImplementedReadOnly,
    },
    Capability {
        id: "media_source",
        maturity: CapabilityMaturity::ImplementedReadOnly,
    },
    Capability {
        id: "volume_read",
        maturity: CapabilityMaturity::ImplementedReadOnly,
    },
    Capability {
        id: "mute_read",
        maturity: CapabilityMaturity::ImplementedReadOnly,
    },
    Capability {
        id: "eq_read",
        maturity: CapabilityMaturity::ImplementedReadOnly,
    },
    Capability {
        id: "product_settings_read",
        maturity: CapabilityMaturity::EvidenceRequired,
    },
    Capability {
        id: "personal_listening_read",
        maturity: CapabilityMaturity::ImplementedReadOnly,
    },
    Capability {
        id: "audio_sync_read",
        maturity: CapabilityMaturity::ImplementedReadOnly,
    },
    Capability {
        id: "source_list_read",
        maturity: CapabilityMaturity::ImplementedReadOnly,
    },
    Capability {
        id: "volume_set",
        maturity: CapabilityMaturity::ImplementedVerifiedWrite,
    },
    Capability {
        id: "mute_set",
        maturity: CapabilityMaturity::ImplementedVerifiedWrite,
    },
    Capability {
        id: "playback_mutation",
        maturity: CapabilityMaturity::EvidenceRequired,
    },
    Capability {
        id: "source_set",
        maturity: CapabilityMaturity::ImplementedVerifiedWrite,
    },
    Capability {
        id: "eq_set",
        maturity: CapabilityMaturity::ImplementedVerifiedWrite,
    },
    Capability {
        id: "websocket_control",
        maturity: CapabilityMaturity::NotAdvertisedByExactProfile,
    },
    Capability {
        id: "firmware_mutation",
        maturity: CapabilityMaturity::Forbidden,
    },
];

pub const fn authentics_300_capabilities() -> &'static [Capability] {
    AUTHENTICS_300_CAPABILITIES
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn capability_ids_are_unique_and_closed() {
        let mut ids = BTreeSet::new();
        for capability in authentics_300_capabilities() {
            assert!(ids.insert(capability.id));
            assert!(capability
                .id
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte == b'_'));
        }
    }

    #[test]
    fn websocket_presence_is_not_promoted_to_exact_device_support() {
        let websocket = authentics_300_capabilities()
            .iter()
            .find(|capability| capability.id == "websocket_control")
            .expect("websocket capability should be explicit");
        assert_eq!(
            websocket.maturity,
            CapabilityMaturity::NotAdvertisedByExactProfile
        );
    }

    #[test]
    fn firmware_mutation_remains_forbidden() {
        let firmware = authentics_300_capabilities()
            .iter()
            .find(|capability| capability.id == "firmware_mutation")
            .expect("firmware capability should be explicit");
        assert_eq!(firmware.maturity, CapabilityMaturity::Forbidden);
    }

    #[test]
    fn only_hardware_readback_verified_writes_are_promoted() {
        let verified = authentics_300_capabilities()
            .iter()
            .filter(|capability| {
                capability.maturity == CapabilityMaturity::ImplementedVerifiedWrite
            })
            .map(|capability| capability.id)
            .collect::<BTreeSet<_>>();
        assert_eq!(
            verified,
            BTreeSet::from(["eq_set", "mute_set", "source_set", "volume_set"])
        );
    }
}
