//! Frozen flows.
//!
//! This module owns schema 3 flows: one frozen, digest-bound artifact for each
//! generation of one flow. A coding agent writes one agent output with clusters
//! and a label on every step. Code derives membership, colors, transitions, and
//! digests, freezes the generation, and verifies it later.
//!
//! # Modules
//! `models` the serde types. `import` freezes one agent output. `verify` checks
//! the current generation. `lineage` owns the store layout and listings.
//! `validate`, `assign`, and `derive` hold the rules; `canonical` and `text` the
//! canonical forms.
//!
//! # Sections
//! `Contracts` embeds the JSON Schemas. `Lookup` resolves a flow name.
//! `Errors` reports failures.

pub mod assign;
pub mod canonical;
pub mod derive;
pub mod import;
pub mod lineage;
pub mod models;
pub mod text;
pub mod validate;
pub mod verify;

#[cfg(test)]
pub(crate) mod fixtures;

use std::{fmt, io, path::Path};

use assign::AssignError;
use canonical::CanonicalError;
use derive::DeriveError;
use lineage::{LineageError, StoredGeneration};
use models::{FlowId, FlowIssue, IssueCode};

// -----------------------------------------------------------------------------
// Contracts
// -----------------------------------------------------------------------------

/// The embedded agent output contract, version 1.
pub const AGENT_OUTPUT_SCHEMA: &str = include_str!("schema/agent-output.schema.json");

/// The embedded frozen artifact schema, version 3.
pub const FLOW_ARTIFACT_SCHEMA: &str = include_str!("schema/flow-artifact.schema.json");

// -----------------------------------------------------------------------------
// Lookup
// -----------------------------------------------------------------------------

/// Reads the current generation of the flow with this name.
///
/// # Errors
/// Returns `Rejected` for an invalid name and `NoLineage` for a flow without a
/// frozen generation. Returns a lineage error when the files cannot be read.
pub fn current_flow(directory: &Path, name: &str) -> Result<StoredGeneration, FlowError> {
    lineage::read_current(directory, &flow_id(name)?)?.ok_or_else(|| FlowError::NoLineage {
        name: name.to_owned(),
    })
}

/// Parses a flow name. An invalid name is a rejected input with an `invalid_key` issue.
fn flow_id(name: &str) -> Result<FlowId, FlowError> {
    FlowId::try_from(name).map_err(|error| {
        FlowError::Rejected(vec![FlowIssue::error(
            IssueCode::InvalidKey,
            error.to_string(),
        )])
    })
}

// -----------------------------------------------------------------------------
// Errors
// -----------------------------------------------------------------------------

#[derive(Debug)]
pub enum FlowError {
    Rejected(Vec<FlowIssue>),
    SelfCheck(Vec<FlowIssue>),
    NoLineage { name: String },
    Lineage(LineageError),
    Derive(DeriveError),
    Assign(AssignError),
    Canonical(CanonicalError),
    Io(io::Error),
}

impl FlowError {
    /// Returns the validation issues of a rejected output.
    #[must_use]
    pub fn issues(&self) -> &[FlowIssue] {
        match self {
            Self::Rejected(issues) | Self::SelfCheck(issues) => issues,
            _ => &[],
        }
    }
}

impl fmt::Display for FlowError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let count = |issues: &[FlowIssue]| issues.iter().filter(|issue| issue.is_error()).count();
        match self {
            // Summarize a rejected output; the issues carry the details.
            Self::Rejected(issues) => write!(
                formatter,
                "validation_failed: the agent output has {} errors, and nothing was written",
                count(issues)
            ),
            // Report an internal consistency fault before any write.
            Self::SelfCheck(issues) => write!(
                formatter,
                "the new generation failed its own verification with {} errors, and nothing was written",
                count(issues)
            ),
            // Name the flow without a lineage.
            Self::NoLineage { name } => write!(
                formatter,
                "unknown_flow: the flow {name} has no frozen generation"
            ),
            // Preserve the lineage error.
            Self::Lineage(error) => write!(formatter, "{error}"),
            // Preserve the derivation error.
            Self::Derive(error) => write!(formatter, "{error}"),
            // Preserve the assignment error.
            Self::Assign(error) => write!(formatter, "{error}"),
            // Preserve the serialization error.
            Self::Canonical(error) => write!(formatter, "{error}"),
            // Preserve the file or graph error.
            Self::Io(error) => write!(formatter, "{error}"),
        }
    }
}

impl std::error::Error for FlowError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Lineage(error) => Some(error),
            Self::Derive(error) => Some(error),
            Self::Assign(error) => Some(error),
            Self::Canonical(error) => Some(error),
            Self::Io(error) => Some(error),
            _ => None,
        }
    }
}

impl From<LineageError> for FlowError {
    fn from(error: LineageError) -> Self {
        Self::Lineage(error)
    }
}

impl From<DeriveError> for FlowError {
    fn from(error: DeriveError) -> Self {
        Self::Derive(error)
    }
}

impl From<AssignError> for FlowError {
    fn from(error: AssignError) -> Self {
        Self::Assign(error)
    }
}

impl From<CanonicalError> for FlowError {
    fn from(error: CanonicalError) -> Self {
        Self::Canonical(error)
    }
}

impl From<io::Error> for FlowError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

// -----------------------------------------------------------------------------
// Tests
// -----------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use serde::Serialize;
    use serde_json::Value;

    use super::{
        AGENT_OUTPUT_SCHEMA, FLOW_ARTIFACT_SCHEMA,
        fixtures::{self, frozen, import, output},
        models::{AgentFlowOutput, ClusterColor, FlowArtifact, wire_name},
    };

    fn schemas() -> (Value, Value) {
        (
            serde_json::from_str(FLOW_ARTIFACT_SCHEMA).expect("artifact schema"),
            serde_json::from_str(AGENT_OUTPUT_SCHEMA).expect("agent schema"),
        )
    }

    fn sorted_keys(value: &Value) -> Vec<String> {
        let mut keys: Vec<String> = value.as_object().expect("object").keys().cloned().collect();
        keys.sort();
        keys
    }

    fn required_keys(schema: &Value) -> Vec<String> {
        let mut keys: Vec<String> = schema["required"]
            .as_array()
            .expect("required")
            .iter()
            .map(|key| key.as_str().expect("key").to_owned())
            .collect();
        keys.sort();
        keys
    }

    fn schema_enum(schema: &Value, definition: &str) -> Vec<String> {
        schema["$defs"][definition]["enum"]
            .as_array()
            .expect(definition)
            .iter()
            .map(|name| name.as_str().expect("name").to_owned())
            .collect()
    }

    fn names<T: Serialize>(values: &[T]) -> Vec<String> {
        values.iter().map(wire_name).collect()
    }

    /// Freezes the fixture output, then one revision, and returns both artifacts as JSON.
    fn generations() -> (Value, Value) {
        let directory = fixtures::store();
        let path = directory.path();
        frozen(import(path, "demo-flow", &output()));
        let initial = fixtures::current(path, "demo-flow");
        let mut revised = output();
        for (step, stored) in revised["flow"]["steps"]
            .as_array_mut()
            .expect("steps")
            .iter_mut()
            .zip(&initial.intent.steps)
        {
            step["priorStepId"] = stored.id.as_str().into();
        }
        revised["flow"]["steps"][5]["title"] = "Hold the store lock".into();
        revised["flow"]["revisionSummary"] = "Rename the last step to name the lock.".into();
        frozen(import(path, "demo-flow", &revised));
        let revision = fixtures::current(path, "demo-flow");
        (
            serde_json::to_value(initial).expect("initial"),
            serde_json::to_value(revision).expect("revision"),
        )
    }

    #[test]
    fn serde_models_write_every_required_schema_key() {
        let (artifact_schema, agent_schema) = schemas();
        let (initial, revision) = generations();
        assert_eq!(sorted_keys(&initial), required_keys(&artifact_schema));
        for (definition, pointer) in [
            ("Generation", "/generation"),
            ("FactLayer", "/facts"),
            ("FactBinding", "/facts/binding"),
            ("FactNode", "/facts/nodes/0"),
            ("FactEdge", "/facts/edges/0"),
            ("IntentLayer", "/intent"),
            ("ClusterIntent", "/intent/clusters/0"),
            ("FlowStep", "/intent/steps/0"),
            ("AssignmentLayer", "/assignment"),
            ("AssignmentPolicy", "/assignment/policy"),
            ("AssignerProvenance", "/assignment/assigner"),
            ("StepDecision", "/assignment/steps/0"),
            ("MemberDecision", "/assignment/members/0"),
            ("ClusterVisitCount", "/assignment/members/0/visits/0"),
            ("ViewLayer", "/view"),
            ("ClusterView", "/view/clusters/0"),
            ("VisitRun", "/view/clusters/0/visits/0"),
            ("StepView", "/view/steps/0"),
            ("Digests", "/digests"),
            ("Provenance", "/provenance"),
        ] {
            let instance = initial.pointer(pointer).expect(pointer);
            assert_eq!(
                sorted_keys(instance),
                required_keys(&artifact_schema["$defs"][definition]),
                "{definition}"
            );
        }
        for (definition, pointer) in [
            ("DiffOverlay", "/diff"),
            ("DiffBase", "/diff/base"),
            ("DiffSummary", "/diff/summary"),
            ("StepDiff", "/diff/steps/0"),
            ("ClusterDiff", "/diff/clusters/0"),
            ("MemberDiff", "/diff/members/0"),
        ] {
            let instance = revision.pointer(pointer).expect(pointer);
            assert_eq!(
                sorted_keys(instance),
                required_keys(&artifact_schema["$defs"][definition]),
                "{definition}"
            );
        }
        let output = serde_json::to_value(
            serde_json::from_value::<AgentFlowOutput>(output()).expect("output"),
        )
        .expect("output");
        for (definition, pointer) in [
            ("FlowOutcome", ""),
            ("AgentFlow", "/flow"),
            ("AgentCluster", "/flow/clusters/0"),
            ("AgentStep", "/flow/steps/0"),
        ] {
            let instance = output.pointer(pointer).expect(pointer);
            assert_eq!(
                sorted_keys(instance),
                required_keys(&agent_schema["$defs"][definition]),
                "{definition}"
            );
        }
        let round_trip: FlowArtifact = serde_json::from_value(revision.clone()).expect("parse");
        assert_eq!(serde_json::to_value(round_trip).expect("value"), revision);
    }

    #[test]
    fn serde_enums_match_the_schema_enums() {
        use super::models::{
            EvidenceBasis as E, FlowPhase as P, GenerationPurpose as G, InputCategory as C,
            InputScope as I, NodeKind as N, Presence as R, RelationKind as K, Transition as T,
        };
        let (artifact_schema, agent_schema) = schemas();
        let phases = [
            P::Entry,
            P::InputValidation,
            P::Guard,
            P::Load,
            P::Transform,
            P::ExternalCall,
            P::Persist,
            P::Respond,
            P::ErrorHandling,
            P::Other,
        ];
        assert_eq!(schema_enum(&artifact_schema, "FlowPhase"), names(&phases));
        assert_eq!(schema_enum(&agent_schema, "FlowPhase"), names(&phases));
        let bases = [
            E::SourceWalkthrough,
            E::IndexedFactsOnly,
            E::RuntimeTrace,
            E::Mixed,
        ];
        assert_eq!(
            schema_enum(&artifact_schema, "EvidenceBasis"),
            names(&bases)
        );
        assert_eq!(schema_enum(&agent_schema, "EvidenceBasis"), names(&bases));
        assert_eq!(
            schema_enum(&artifact_schema, "GenerationPurpose"),
            names(&[G::Initial, G::Revision])
        );
        assert_eq!(
            schema_enum(&artifact_schema, "NodeKind"),
            names(&[N::Module, N::Type, N::Callable, N::Field])
        );
        let relations = [
            K::Contains,
            K::Calls,
            K::Imports,
            K::InputType,
            K::OutputType,
            K::UsesType,
            K::UsesValue,
        ];
        assert_eq!(
            schema_enum(&artifact_schema, "RelationKind"),
            names(&relations)
        );
        assert_eq!(
            schema_enum(&artifact_schema, "InputScope"),
            names(&[I::LabelsOnly])
        );
        assert_eq!(
            schema_enum(&artifact_schema, "InputCategory"),
            names(&[C::Intent])
        );
        assert_eq!(
            schema_enum(&artifact_schema, "Presence"),
            names(&[R::Added, R::Removed, R::Retained])
        );
        assert_eq!(
            schema_enum(&artifact_schema, "ClusterColor"),
            names(&ClusterColor::PALETTE)
        );
        let transitions = [
            T::FlowStart,
            T::SameFunction,
            T::Calls,
            T::ReturnsToCaller,
            T::SiblingCall,
            T::CallsIndirectly,
            T::ReturnsToAncestor,
            T::NoStaticEdge,
        ];
        assert_eq!(
            schema_enum(&artifact_schema, "Transition"),
            names(&transitions)
        );
    }
}
