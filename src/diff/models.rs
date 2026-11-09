//! Diff models: the agent diff document and the frozen diff artifact.
//!
//! The agent writes the pairs and the cluster notes. Code adds the commits, the
//! two frozen flows, and one row of derived facts for each step.

use serde::{Deserialize, Serialize};

use crate::flow::models::{ClusterKey, FlowArtifact, FlowId, Provenance, Sha256Digest, StepId};

/// The diff artifact wire-schema version.
pub const DIFF_ARTIFACT_SCHEMA_VERSION: u32 = 1;

/// The agent diff contract version.
pub const DIFF_CONTRACT_VERSION: u32 = 1;

// -----------------------------------------------------------------------------
// Agent document
// -----------------------------------------------------------------------------

/// The only data that a coding agent writes for one diff.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentDiff {
    pub contract_version: u32,
    pub pairs: Vec<AgentPair>,
    #[serde(default)]
    pub notes: Vec<AgentNote>,
}

/// One Before step and one After step that are the same step. Numbers count from 1 in flow order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentPair {
    pub before: usize,
    pub after: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentNote {
    pub side: Side,
    pub cluster_key: String,
    pub text: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Side {
    Before,
    After,
}

// -----------------------------------------------------------------------------
// Frozen artifact
// -----------------------------------------------------------------------------

/// One frozen diff. `digest` covers every field except `digest` and `provenance`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DiffArtifact {
    pub kind: DiffKind,
    pub schema_version: u32,
    pub name: FlowId,
    pub base: DiffSide,
    pub head: DiffSide,
    pub pairs: Vec<StepPair>,
    pub notes: Vec<ClusterNote>,
    /// After steps in flow order, then the removed Before steps in flow order.
    pub rows: Vec<DiffRow>,
    pub digest: Sha256Digest,
    pub provenance: Provenance,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum DiffKind {
    #[serde(rename = "tekdek.diff")]
    Diff,
}

/// One side of the diff: a full commit ID and the frozen flow on that commit.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DiffSide {
    pub commit: String,
    pub flow: FlowArtifact,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StepPair {
    pub before: StepId,
    pub after: StepId,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClusterNote {
    pub side: Side,
    pub cluster_key: ClusterKey,
    pub text: String,
}

/// The derived facts of one step, or of one pair of steps.
///
/// `base_span` and `head_span` locate the function on each commit. A new step
/// compares with the function of the same node key on the base, when it exists;
/// a removed step compares with the function on the head.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DiffRow {
    pub kind: ChangeKind,
    pub before_step: Option<StepId>,
    pub after_step: Option<StepId>,
    pub base_span: Option<LineSpan>,
    pub head_span: Option<LineSpan>,
    pub added: u32,
    pub removed: u32,
    /// The function as one unified diff, context lines included.
    pub lines: Vec<DiffLine>,
}

/// `moved` means that the function is in another file. A cluster change is never a move.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeKind {
    New,
    Removed,
    Changed,
    Moved,
    Unchanged,
}

/// Lines `start` to `end` of one file, counted from 1, both included.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LineSpan {
    pub path: String,
    pub start: u32,
    pub end: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DiffLine {
    pub op: LineOp,
    pub text: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LineOp {
    Context,
    Added,
    Removed,
}
