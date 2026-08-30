//! Closed, non-identifying protocol model for the Aura Studio 5 control path.
//!
//! This module performs no Bluetooth I/O.  The Linux transport supplies fresh
//! FDDF service data and writes only the two fixed AA frames observed from the
//! official JBL One runtime for this exact Aura Studio 5.

use crate::model::DeviceIdentity;

pub(crate) const HARMAN_FDDF_UUID: &str = "0000fddf-0000-1000-8000-00805f9b34fb";
pub(crate) const AURA_VENDOR_SERVICE_UUID: &str = "65786365-6c70-6f69-6e74-2e636f6d0000";
pub(crate) const AURA_NOTIFY_UUID: &str = "65786365-6c70-6f69-6e74-2e636f6d0001";
pub(crate) const AURA_WRITE_UUID: &str = "65786365-6c70-6f69-6e74-2e636f6d0002";

const AURA_STUDIO_5_PID: [u8; 2] = [0x2d, 0x21];
const STABLE_IDENTITY_OFFSET: usize = 11;
const STABLE_IDENTITY_END: usize = STABLE_IDENTITY_OFFSET + 6;
const SUCCESS_ACK: [u8; 5] = [0xaa, 0x00, 0x02, 0x13, 0x00];

/// The only two Aura mutations exposed by this product.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AuraPlayTogetherCommand {
    On,
    Off,
}

impl AuraPlayTogetherCommand {
    pub(crate) const fn frame(self) -> [u8; 7] {
        match self {
            Self::On => [0xaa, 0x13, 0x04, 0x00, 0x3c, 0x01, 0x01],
            Self::Off => [0xaa, 0x13, 0x04, 0x00, 0x3c, 0x01, 0x00],
        }
    }
}

/// Positive acknowledgement for one exact AA role command.
///
/// Construction is private so a transport cannot manufacture success from a
/// write acknowledgement alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct AuraCommandAccepted {
    _private: (),
}

pub(crate) fn parse_command_ack(notification: &[u8]) -> Option<AuraCommandAccepted> {
    (notification == SUCCESS_ACK).then_some(AuraCommandAccepted { _private: () })
}

/// Matches a fresh Harman FDDF advertisement to the configured Aura identity.
///
/// The rotating LE address is deliberately not part of this function.  The
/// caller may retain it only after this predicate succeeds for service data
/// received during its own active discovery window.
pub(crate) fn matches_verified_fddf(
    service_uuid: &str,
    payload: &[u8],
    expected_identity: &DeviceIdentity,
) -> bool {
    service_uuid.eq_ignore_ascii_case(HARMAN_FDDF_UUID)
        && payload.len() >= STABLE_IDENTITY_END
        && payload[..2] == AURA_STUDIO_5_PID
        && expected_identity.matches_binary(&payload[STABLE_IDENTITY_OFFSET..STABLE_IDENTITY_END])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity() -> DeviceIdentity {
        DeviceIdentity::parse("02:00:00:00:00:02").expect("fixture identity should be valid")
    }

    fn matching_payload() -> Vec<u8> {
        let mut payload = vec![0_u8; 18];
        payload[..2].copy_from_slice(&AURA_STUDIO_5_PID);
        payload[STABLE_IDENTITY_OFFSET..STABLE_IDENTITY_END]
            .copy_from_slice(&[0x02, 0x00, 0x00, 0x00, 0x00, 0x02]);
        payload
    }

    #[test]
    fn serializes_only_the_closed_on_and_off_frames() {
        assert_eq!(
            AuraPlayTogetherCommand::On.frame(),
            [0xaa, 0x13, 0x04, 0x00, 0x3c, 0x01, 0x01]
        );
        assert_eq!(
            AuraPlayTogetherCommand::Off.frame(),
            [0xaa, 0x13, 0x04, 0x00, 0x3c, 0x01, 0x00]
        );
    }

    #[test]
    fn accepts_only_the_exact_success_notification() {
        assert!(parse_command_ack(&SUCCESS_ACK).is_some());
        for invalid in [
            &[][..],
            &[0xaa, 0x00, 0x02, 0x13][..],
            &[0xaa, 0x00, 0x02, 0x13, 0x01][..],
            &[0xaa, 0x00, 0x02, 0x13, 0x00, 0x00][..],
        ] {
            assert!(parse_command_ack(invalid).is_none());
        }
    }

    #[test]
    fn fddf_requires_uuid_pid_stable_identity_and_minimum_shape() {
        let expected = identity();
        let payload = matching_payload();
        assert!(matches_verified_fddf(HARMAN_FDDF_UUID, &payload, &expected));
        assert!(matches_verified_fddf(
            &HARMAN_FDDF_UUID.to_ascii_uppercase(),
            &payload,
            &expected
        ));

        let mut wrong_pid = payload.clone();
        wrong_pid[0] ^= 1;
        assert!(!matches_verified_fddf(
            HARMAN_FDDF_UUID,
            &wrong_pid,
            &expected
        ));

        let mut wrong_identity = payload.clone();
        wrong_identity[STABLE_IDENTITY_END - 1] ^= 1;
        assert!(!matches_verified_fddf(
            HARMAN_FDDF_UUID,
            &wrong_identity,
            &expected
        ));
        assert!(!matches_verified_fddf(
            "00001852-0000-1000-8000-00805f9b34fb",
            &payload,
            &expected
        ));
        assert!(!matches_verified_fddf(
            HARMAN_FDDF_UUID,
            &payload[..STABLE_IDENTITY_END - 1],
            &expected
        ));
    }
}
