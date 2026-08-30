//! Closed, clean-room start plans for the two separately evidenced flows.
//!
//! These plans are protocol facts only. They do not execute a transport and
//! do not merge the official Home flow with the JBL-source compatibility flow.

/// A supported Play Together flow profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PartyFlow {
    /// Official Home UI semantics: add the Aura receiver, then enter the JBL.
    OfficialHome,
    /// Directional compatibility semantics for a network source on the JBL.
    JblSourceCompatibility,
}

/// One closed step in a Play Together start plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PartyFlowStep {
    AuraReceiverOn,
    JblEnterAuracast,
    /// The distinct JBL broadcaster-assistant command; never implicit.
    JblBroadcastStart7957,
}

const OFFICIAL_HOME_START: [PartyFlowStep; 2] = [
    PartyFlowStep::AuraReceiverOn,
    PartyFlowStep::JblEnterAuracast,
];

const JBL_SOURCE_COMPATIBILITY_START: [PartyFlowStep; 3] = [
    PartyFlowStep::JblEnterAuracast,
    PartyFlowStep::JblBroadcastStart7957,
    PartyFlowStep::AuraReceiverOn,
];

impl PartyFlow {
    /// Returns the immutable start plan for this exact flow profile.
    pub const fn start_steps(self) -> &'static [PartyFlowStep] {
        match self {
            Self::OfficialHome => &OFFICIAL_HOME_START,
            Self::JblSourceCompatibility => &JBL_SOURCE_COMPATIBILITY_START,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn official_home_adds_aura_before_entering_jbl() {
        assert_eq!(
            PartyFlow::OfficialHome.start_steps(),
            [
                PartyFlowStep::AuraReceiverOn,
                PartyFlowStep::JblEnterAuracast,
            ]
        );
    }

    #[test]
    fn official_home_never_contains_7957() {
        assert!(!PartyFlow::OfficialHome
            .start_steps()
            .contains(&PartyFlowStep::JblBroadcastStart7957));
    }

    #[test]
    fn jbl_source_compatibility_keeps_7957_explicit() {
        assert_eq!(
            PartyFlow::JblSourceCompatibility.start_steps(),
            [
                PartyFlowStep::JblEnterAuracast,
                PartyFlowStep::JblBroadcastStart7957,
                PartyFlowStep::AuraReceiverOn,
            ]
        );
        assert_eq!(
            PartyFlow::JblSourceCompatibility
                .start_steps()
                .iter()
                .filter(|step| **step == PartyFlowStep::JblBroadcastStart7957)
                .count(),
            1
        );
    }

    #[test]
    fn the_two_profiles_are_not_collapsed_into_one_plan() {
        assert_ne!(
            PartyFlow::OfficialHome.start_steps(),
            PartyFlow::JblSourceCompatibility.start_steps()
        );
    }
}
