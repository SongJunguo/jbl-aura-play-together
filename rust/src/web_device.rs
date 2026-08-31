//! Privacy-safe direct-device surface shared by the service actor and Web API.

use serde::Serialize;

use crate::capability::Capability;
use crate::eq::EqPresetTarget;
use crate::inspection::InspectionSnapshot;
use crate::media::{AudioSourceTarget, MediaSource, MediaStatus, MuteTarget};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DirectSnapshot {
    pub media: MediaStatus,
    pub inspection: InspectionSnapshot,
    pub capabilities: Vec<Capability>,
    pub source_targets: Vec<AudioSourceTarget>,
    pub active_eq: Option<EqPresetTarget>,
    pub revision: u64,
}

impl DirectSnapshot {
    pub(crate) fn with_revision(mut self, revision: u64) -> Self {
        self.revision = revision;
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DirectMutation {
    Volume(u8),
    Mute(MuteTarget),
    Source(AudioSourceTarget),
    EqPreset(EqPresetTarget),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DirectActionOutcome {
    AlreadyAtTarget,
    Applied,
    RejectedByDevice,
    TargetObservedAfterUnknownWrite,
    PostconditionFailed,
    RejectedBeforeSend,
    OutcomeUnknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DirectFailure {
    Unavailable,
    SafetyGate,
    UnsupportedTarget,
    DeviceRejected,
    InvalidState,
    OutcomeUnknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DirectObservation {
    Volume {
        volume: Option<u8>,
        muted: Option<bool>,
    },
    Mute {
        muted: Option<bool>,
    },
    Source {
        source: MediaSource,
    },
    EqPreset {
        preset: Option<EqPresetTarget>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct DirectActionResult {
    pub outcome: DirectActionOutcome,
    pub observation: Option<DirectObservation>,
    pub failure: Option<DirectFailure>,
    pub revision: u64,
}

impl DirectActionResult {
    pub(crate) const fn new(
        outcome: DirectActionOutcome,
        observation: Option<DirectObservation>,
        failure: Option<DirectFailure>,
    ) -> Self {
        Self {
            outcome,
            observation,
            failure,
            revision: 0,
        }
    }

    pub(crate) const fn with_revision(mut self, revision: u64) -> Self {
        self.revision = revision;
        self
    }
}
