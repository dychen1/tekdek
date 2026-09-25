//! Assignment layer: deterministic step decisions and function membership.
//!
//! Code writes this layer from the intent layer. The only policy copies each
//! agent label (`step-labels` rules assigner) and requires that all steps of one
//! function carry the same label (`unanimous` membership).

use serde::{Deserialize, Serialize};

use super::{ClusterKey, NodeKey, Sha256Digest, StepId};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AssignmentLayer {
    pub policy: AssignmentPolicy,
    pub assigner: AssignerProvenance,
    /// The digest of exactly the inputs that the assigner reads.
    pub input_digest: Sha256Digest,
    /// One decision for each step, in step order.
    pub steps: Vec<StepDecision>,
    /// One decision for each step node, sorted by key.
    pub members: Vec<MemberDecision>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AssignmentPolicy {
    pub label_policy: LabelPolicy,
    pub membership_rule: MembershipRule,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LabelPolicy {
    AgentLabels,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MembershipRule {
    Unanimous,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum AssignerProvenance {
    Rules {
        assigner_id: String,
        version: String,
        input_scope: InputScope,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InputScope {
    LabelsOnly,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "outcome",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum StepDecision {
    Assigned {
        step_id: StepId,
        cluster_key: ClusterKey,
        basis: AssignedBasis,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum AssignedBasis {
    AgentLabel,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "outcome",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum MemberDecision {
    Assigned {
        node_key: NodeKey,
        cluster_key: ClusterKey,
        basis: MembershipBasis,
        /// Step counts for each cluster, sorted by cluster key.
        visits: Vec<ClusterVisitCount>,
    },
}

impl MemberDecision {
    #[must_use]
    pub fn node_key(&self) -> &NodeKey {
        match self {
            Self::Assigned { node_key, .. } => node_key,
        }
    }

    #[must_use]
    pub fn cluster_key(&self) -> &ClusterKey {
        match self {
            Self::Assigned { cluster_key, .. } => cluster_key,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MembershipBasis {
    Unanimous,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClusterVisitCount {
    pub cluster_key: ClusterKey,
    pub steps: u32,
}
