mod id;
pub(crate) mod output;
pub(crate) mod payload;
mod store;

pub(crate) use id::{ArtefactHash, ObjectHash};
pub(crate) use payload::{
    ReviewVerdict, TestOutcome, TypedPayload, artefact_hash_for, encode_plan_decision,
    parse_typed_payload,
};
pub(crate) use store::WorkflowArtefactRepository;

use super::definition::{ArtefactKind, OutputKey, StepKey};
use super::id::{ArtefactId, AttemptId, GateId, RunId};

pub(crate) const MAXIMUM_ARTEFACTS: usize = 256;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ArtefactRecord {
    pub(crate) id: ArtefactId,
    pub(crate) kind: ArtefactKind,
    pub(crate) artefact_hash: ArtefactHash,
    pub(crate) object_hash: ObjectHash,
    pub(crate) payload_bytes: u64,
    pub(crate) created_at_ms: u64,
    pub(crate) provenance: ArtefactProvenance,
    pub(crate) summary: ArtefactSummary,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ArtefactProvenance {
    pub(crate) run_id: RunId,
    pub(crate) producer: ArtefactProducer,
    pub(crate) inputs: Vec<ArtefactReference>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ArtefactProducer {
    StepAttempt {
        attempt_id: AttemptId,
        step: StepKey,
        output: Option<OutputKey>,
        disposition: ProductionDisposition,
    },
    HumanGate {
        gate_id: GateId,
        step: StepKey,
        output: OutputKey,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ProductionDisposition {
    RequiredOutput,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ArtefactReference {
    pub(crate) id: ArtefactId,
    pub(crate) kind: ArtefactKind,
    pub(crate) artefact_hash: ArtefactHash,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ArtefactSummary {
    Plan {
        markdown_bytes: u64,
    },
    Review {
        verdict: ReviewVerdict,
    },
    Test {
        outcome: TestOutcome,
    },
    PlanDecision {
        plan: ArtefactHash,
        decision: crate::workflows::gates::PlanDecisionKind,
    },
}

impl ArtefactRecord {
    pub(crate) fn constraint_label(&self) -> String {
        match &self.summary {
            ArtefactSummary::Review { verdict } => verdict.as_label().to_owned(),
            ArtefactSummary::Test { outcome } => outcome.as_label().to_owned(),
            ArtefactSummary::PlanDecision { plan, decision } => {
                format!("{} · {}", decision.as_label(), plan.short())
            }
            ArtefactSummary::Plan { .. } => String::new(),
        }
    }
}

impl ArtefactProducer {
    pub(crate) fn as_label(&self) -> &'static str {
        match self {
            Self::StepAttempt { .. } => "Step attempt",
            Self::HumanGate { .. } => "Plan review",
        }
    }
}

impl ProductionDisposition {
    pub(crate) fn as_str(self) -> &'static str {
        "required-output"
    }
    pub(crate) fn parse(value: &str) -> Option<Self> {
        (value == "required-output").then_some(Self::RequiredOutput)
    }
}
