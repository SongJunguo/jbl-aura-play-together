//! Pure Play Together UI/control-state reduction.
//!
//! Command progress, scanner observations, retained configuration and acoustic
//! observations are independent evidence dimensions. No observation in one
//! dimension silently promotes another.

/// Command-owned operation state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PartyOperationState {
    Idle,
    Waiting,
    Entered,
    Quitting,
    Unknown,
}

/// The role-changing command currently represented by command evidence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PartyCommand {
    Enter,
    Quit,
}

/// Evidence produced by the command path alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandEvidence {
    None,
    Pending(PartyCommand),
    Accepted(PartyCommand),
    Rejected(PartyCommand),
    OutcomeUnknown(PartyCommand),
}

/// Scanner/topology evidence, independent of command results.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScannerEvidence {
    Unknown,
    NotObserved,
    ReceiverObserved,
    BroadcasterObserved,
    BroadcasterAndReceiverObserved,
}

/// Device-reported retained membership configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetainedConfigurationEvidence {
    Unknown,
    NotConfigured,
    ExactPairConfigured,
}

/// Human acoustic observation, independent of every control-plane signal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AcousticEvidence {
    Unknown,
    Silent,
    OneSpeakerAudible,
    TwoSpeakersAudible,
}

/// A definitive rejection versus an outcome that became uncertain after send.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandFailure {
    Rejected,
    OutcomeUnknown,
}

/// A closed reducer event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PartyStateEvent {
    BeginEnter,
    EnterAccepted,
    EnterFailed(CommandFailure),
    BeginQuit,
    QuitAccepted,
    QuitFailed(CommandFailure),
    ScannerObserved(ScannerEvidence),
    RetainedConfigurationObserved(RetainedConfigurationEvidence),
    AcousticObserved(AcousticEvidence),
}

/// A rejected state transition contains no raw device diagnostic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PartyStateError {
    InvalidTransition,
}

/// Privacy-safe aggregate of four independent evidence dimensions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PartyState {
    operation: PartyOperationState,
    command: CommandEvidence,
    scanner: ScannerEvidence,
    retained_configuration: RetainedConfigurationEvidence,
    acoustic: AcousticEvidence,
}

impl PartyState {
    pub const fn new() -> Self {
        Self {
            operation: PartyOperationState::Idle,
            command: CommandEvidence::None,
            scanner: ScannerEvidence::Unknown,
            retained_configuration: RetainedConfigurationEvidence::Unknown,
            acoustic: AcousticEvidence::Unknown,
        }
    }

    pub const fn operation(self) -> PartyOperationState {
        self.operation
    }

    pub const fn command_evidence(self) -> CommandEvidence {
        self.command
    }

    pub const fn scanner_evidence(self) -> ScannerEvidence {
        self.scanner
    }

    pub const fn retained_configuration_evidence(self) -> RetainedConfigurationEvidence {
        self.retained_configuration
    }

    pub const fn acoustic_evidence(self) -> AcousticEvidence {
        self.acoustic
    }
}

impl Default for PartyState {
    fn default() -> Self {
        Self::new()
    }
}

/// Reduces one event without transport or device I/O.
pub const fn reduce_party_state(
    mut state: PartyState,
    event: PartyStateEvent,
) -> Result<PartyState, PartyStateError> {
    match event {
        PartyStateEvent::BeginEnter if matches!(state.operation, PartyOperationState::Idle) => {
            state.operation = PartyOperationState::Waiting;
            state.command = CommandEvidence::Pending(PartyCommand::Enter);
        }
        PartyStateEvent::EnterAccepted
            if matches!(state.operation, PartyOperationState::Waiting)
                && matches!(state.command, CommandEvidence::Pending(PartyCommand::Enter)) =>
        {
            state.operation = PartyOperationState::Entered;
            state.command = CommandEvidence::Accepted(PartyCommand::Enter);
        }
        PartyStateEvent::EnterFailed(failure)
            if matches!(state.operation, PartyOperationState::Waiting)
                && matches!(state.command, CommandEvidence::Pending(PartyCommand::Enter)) =>
        {
            state.operation = match failure {
                CommandFailure::Rejected => PartyOperationState::Idle,
                CommandFailure::OutcomeUnknown => PartyOperationState::Unknown,
            };
            state.command = match failure {
                CommandFailure::Rejected => CommandEvidence::Rejected(PartyCommand::Enter),
                CommandFailure::OutcomeUnknown => {
                    CommandEvidence::OutcomeUnknown(PartyCommand::Enter)
                }
            };
        }
        PartyStateEvent::BeginQuit if matches!(state.operation, PartyOperationState::Entered) => {
            state.operation = PartyOperationState::Quitting;
            state.command = CommandEvidence::Pending(PartyCommand::Quit);
        }
        PartyStateEvent::QuitAccepted
            if matches!(state.operation, PartyOperationState::Quitting)
                && matches!(state.command, CommandEvidence::Pending(PartyCommand::Quit)) =>
        {
            state.operation = PartyOperationState::Idle;
            state.command = CommandEvidence::Accepted(PartyCommand::Quit);
        }
        PartyStateEvent::QuitFailed(failure)
            if matches!(state.operation, PartyOperationState::Quitting)
                && matches!(state.command, CommandEvidence::Pending(PartyCommand::Quit)) =>
        {
            state.operation = match failure {
                CommandFailure::Rejected => PartyOperationState::Entered,
                CommandFailure::OutcomeUnknown => PartyOperationState::Unknown,
            };
            state.command = match failure {
                CommandFailure::Rejected => CommandEvidence::Rejected(PartyCommand::Quit),
                CommandFailure::OutcomeUnknown => {
                    CommandEvidence::OutcomeUnknown(PartyCommand::Quit)
                }
            };
        }
        PartyStateEvent::ScannerObserved(evidence) => state.scanner = evidence,
        PartyStateEvent::RetainedConfigurationObserved(evidence) => {
            state.retained_configuration = evidence;
        }
        PartyStateEvent::AcousticObserved(evidence) => state.acoustic = evidence,
        _ => return Err(PartyStateError::InvalidTransition),
    }
    Ok(state)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn waiting() -> PartyState {
        reduce_party_state(PartyState::new(), PartyStateEvent::BeginEnter).unwrap()
    }

    fn entered() -> PartyState {
        reduce_party_state(waiting(), PartyStateEvent::EnterAccepted).unwrap()
    }

    #[test]
    fn enter_moves_through_waiting_only_after_command_success() {
        let waiting = waiting();
        assert_eq!(waiting.operation(), PartyOperationState::Waiting);
        assert_eq!(
            waiting.command_evidence(),
            CommandEvidence::Pending(PartyCommand::Enter)
        );

        let entered = reduce_party_state(waiting, PartyStateEvent::EnterAccepted).unwrap();
        assert_eq!(entered.operation(), PartyOperationState::Entered);
        assert_eq!(
            entered.command_evidence(),
            CommandEvidence::Accepted(PartyCommand::Enter)
        );
    }

    #[test]
    fn rejected_enter_clears_waiting_without_fabricating_entered() {
        let state = reduce_party_state(
            waiting(),
            PartyStateEvent::EnterFailed(CommandFailure::Rejected),
        )
        .unwrap();
        assert_eq!(state.operation(), PartyOperationState::Idle);
        assert_ne!(state.operation(), PartyOperationState::Waiting);
        assert_ne!(state.operation(), PartyOperationState::Entered);
        assert_eq!(
            state.command_evidence(),
            CommandEvidence::Rejected(PartyCommand::Enter)
        );
    }

    #[test]
    fn unknown_enter_clears_waiting_and_remains_explicitly_unknown() {
        let state = reduce_party_state(
            waiting(),
            PartyStateEvent::EnterFailed(CommandFailure::OutcomeUnknown),
        )
        .unwrap();
        assert_eq!(state.operation(), PartyOperationState::Unknown);
        assert_ne!(state.operation(), PartyOperationState::Waiting);
        assert_ne!(state.operation(), PartyOperationState::Entered);
        assert_eq!(
            state.command_evidence(),
            CommandEvidence::OutcomeUnknown(PartyCommand::Enter)
        );
    }

    #[test]
    fn scanner_retained_and_acoustic_observations_never_enter_the_command_state() {
        let events = [
            PartyStateEvent::ScannerObserved(ScannerEvidence::BroadcasterAndReceiverObserved),
            PartyStateEvent::RetainedConfigurationObserved(
                RetainedConfigurationEvidence::ExactPairConfigured,
            ),
            PartyStateEvent::AcousticObserved(AcousticEvidence::TwoSpeakersAudible),
        ];
        let mut state = PartyState::new();
        for event in events {
            state = reduce_party_state(state, event).unwrap();
            assert_eq!(state.operation(), PartyOperationState::Idle);
            assert_eq!(state.command_evidence(), CommandEvidence::None);
        }
        assert_eq!(
            state.scanner_evidence(),
            ScannerEvidence::BroadcasterAndReceiverObserved
        );
        assert_eq!(
            state.retained_configuration_evidence(),
            RetainedConfigurationEvidence::ExactPairConfigured
        );
        assert_eq!(
            state.acoustic_evidence(),
            AcousticEvidence::TwoSpeakersAudible
        );
    }

    #[test]
    fn command_transitions_do_not_overwrite_other_evidence_dimensions() {
        let state = reduce_party_state(
            reduce_party_state(
                reduce_party_state(
                    PartyState::new(),
                    PartyStateEvent::ScannerObserved(ScannerEvidence::ReceiverObserved),
                )
                .unwrap(),
                PartyStateEvent::RetainedConfigurationObserved(
                    RetainedConfigurationEvidence::ExactPairConfigured,
                ),
            )
            .unwrap(),
            PartyStateEvent::AcousticObserved(AcousticEvidence::OneSpeakerAudible),
        )
        .unwrap();
        let state = reduce_party_state(state, PartyStateEvent::BeginEnter).unwrap();
        let state = reduce_party_state(state, PartyStateEvent::EnterAccepted).unwrap();
        assert_eq!(state.scanner_evidence(), ScannerEvidence::ReceiverObserved);
        assert_eq!(
            state.retained_configuration_evidence(),
            RetainedConfigurationEvidence::ExactPairConfigured
        );
        assert_eq!(
            state.acoustic_evidence(),
            AcousticEvidence::OneSpeakerAudible
        );
    }

    #[test]
    fn quit_has_a_distinct_quitting_state() {
        let state = reduce_party_state(entered(), PartyStateEvent::BeginQuit).unwrap();
        assert_eq!(state.operation(), PartyOperationState::Quitting);
        assert_eq!(
            state.command_evidence(),
            CommandEvidence::Pending(PartyCommand::Quit)
        );
        let state = reduce_party_state(state, PartyStateEvent::QuitAccepted).unwrap();
        assert_eq!(state.operation(), PartyOperationState::Idle);
        assert_eq!(
            state.command_evidence(),
            CommandEvidence::Accepted(PartyCommand::Quit)
        );
    }

    #[test]
    fn rejected_quit_restores_entered_but_unknown_quit_does_not_guess() {
        let quitting = reduce_party_state(entered(), PartyStateEvent::BeginQuit).unwrap();
        let rejected = reduce_party_state(
            quitting,
            PartyStateEvent::QuitFailed(CommandFailure::Rejected),
        )
        .unwrap();
        assert_eq!(rejected.operation(), PartyOperationState::Entered);

        let unknown = reduce_party_state(
            quitting,
            PartyStateEvent::QuitFailed(CommandFailure::OutcomeUnknown),
        )
        .unwrap();
        assert_eq!(unknown.operation(), PartyOperationState::Unknown);
    }

    #[test]
    fn invalid_command_transitions_are_rejected() {
        for event in [
            PartyStateEvent::EnterAccepted,
            PartyStateEvent::EnterFailed(CommandFailure::Rejected),
            PartyStateEvent::BeginQuit,
            PartyStateEvent::QuitAccepted,
            PartyStateEvent::QuitFailed(CommandFailure::OutcomeUnknown),
        ] {
            assert_eq!(
                reduce_party_state(PartyState::new(), event),
                Err(PartyStateError::InvalidTransition)
            );
        }
    }
}
