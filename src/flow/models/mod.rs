//! Flow schema 3 models.
//!
//! This module owns the serde types of the frozen flow artifact and of the agent
//! output contract. Code-owned layers derive from these types; the agent writes
//! only the agent output.
//!
//! Serde rules: structs use camelCase and reject unknown fields. Nullable fields
//! are always present with an explicit `null`. No field uses `flatten`, maps, or
//! floats.
//!
//! # Sections
//! `ids` validated identifiers and digests. `artifact` the artifact root.
//! `facts`, `intent`, `assignment`, `view`, `diff` the layers. `agent` the agent
//! output contract. `issues` validation issues.

mod agent;
mod artifact;
mod assignment;
mod diff;
mod facts;
mod ids;
mod intent;
mod issues;
mod view;

pub use agent::{AgentCluster, AgentFlow, AgentFlowOutput, AgentStep, NoFlowReason};
pub use artifact::{
    ArtifactKind, Author, Digests, FLOW_ARTIFACT_SCHEMA_VERSION, FlowArtifact, FrozenStatus,
    Generation, GenerationPurpose, Provenance,
};
pub use assignment::{
    AssignedBasis, AssignerProvenance, AssignmentLayer, AssignmentPolicy, ClusterVisitCount,
    InputScope, LabelPolicy, MemberDecision, MembershipBasis, MembershipRule, StepDecision,
};
pub use diff::{
    ClusterDiff, ClusterField, DiffBase, DiffOverlay, DiffSummary, InputCategory, MemberDiff,
    MembershipChange, Presence, StepDiff, StepField,
};
pub use facts::{
    FactBinding, FactEdge, FactLayer, FactNode, FactProjection, NodeKind, RelationKind,
    from_wire_name, wire_name,
};
pub use ids::{
    ClusterKey, ContentHash, FlowId, InvalidIdentifier, NodeKey, Sha256Digest, StepId,
    is_cluster_key, is_flow_id, is_node_key, is_step_id,
};
pub use intent::{ClusterIntent, EvidenceBasis, FlowPhase, FlowStep, IntentLayer};
pub use issues::{FlowIssue, IssueCode, Severity};
pub use view::{ClusterColor, ClusterView, Palette, StepView, Transition, ViewLayer, VisitRun};

use serde::{Deserialize, Deserializer};

/// Reads a nullable field whose key must be present.
///
/// Serde treats a missing `Option` field as `None`. This reader makes a missing
/// key an error, so every nullable field keeps an explicit `null`.
pub(crate) fn nullable<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer)
}
