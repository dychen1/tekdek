//! Diff overlay: the comparison of one revision with its parent.
//!
//! Code writes this layer for `revision` generations. A revision keeps the graph
//! of its parent, so only the intent can change.

use serde::{Deserialize, Serialize};

use super::{ClusterKey, NodeKey, Sha256Digest, StepId};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DiffOverlay {
    pub base: DiffBase,
    pub changed_inputs: Vec<InputCategory>,
    pub summary: DiffSummary,
    /// Current steps in order, then removed base steps in base order.
    pub steps: Vec<StepDiff>,
    /// Current clusters, then removed base clusters.
    pub clusters: Vec<ClusterDiff>,
    /// The union of both sides' step nodes, sorted by key.
    pub members: Vec<MemberDiff>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DiffBase {
    pub generation_digest: Sha256Digest,
    pub ordinal: u32,
}

/// The assignment inputs that differ from the parent.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InputCategory {
    Intent,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DiffSummary {
    pub steps_added: u32,
    pub steps_removed: u32,
    pub steps_changed: u32,
    pub clusters_added: u32,
    pub clusters_removed: u32,
    pub clusters_changed: u32,
    pub members_changed: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Presence {
    Added,
    Removed,
    Retained,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepField {
    Node,
    Cluster,
    Title,
    Annotation,
    Position,
}

/// `changed_fields` is empty unless the step is retained.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StepDiff {
    pub step_id: StepId,
    pub presence: Presence,
    pub changed_fields: Vec<StepField>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClusterField {
    Label,
    Description,
    Phase,
    Members,
    Position,
}

/// `changed_fields` is empty unless the cluster is retained.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClusterDiff {
    pub cluster_key: ClusterKey,
    pub presence: Presence,
    pub changed_fields: Vec<ClusterField>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum MembershipChange {
    Unchanged {
        cluster_key: ClusterKey,
    },
    Joined {
        to_cluster: ClusterKey,
    },
    Left {
        from_cluster: ClusterKey,
    },
    Moved {
        from_cluster: ClusterKey,
        to_cluster: ClusterKey,
    },
}

/// `causes` names the input categories that differ among the inputs of this node's decisions.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MemberDiff {
    pub node_key: NodeKey,
    pub membership: MembershipChange,
    pub causes: Vec<InputCategory>,
}
