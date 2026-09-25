//! Intent layer: the agent interpretation of one flow.
//!
//! The agent writes the text, the cluster vocabulary, and the step labels. Code
//! adds step IDs and attestations.

use serde::{Deserialize, Serialize};

use super::{ClusterKey, NodeKey, Sha256Digest, StepId, nullable};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct IntentLayer {
    pub behavior: String,
    #[serde(deserialize_with = "nullable")]
    pub boundaries: Option<String>,
    pub evidence_basis: EvidenceBasis,
    #[serde(deserialize_with = "nullable")]
    pub revision_summary: Option<String>,
    /// Narrative order: the order in which the flow first needs each cluster.
    pub clusters: Vec<ClusterIntent>,
    pub steps: Vec<FlowStep>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceBasis {
    SourceWalkthrough,
    IndexedFactsOnly,
    RuntimeTrace,
    Mixed,
}

/// `phase_note` is present if and only if `phase` is `Other`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClusterIntent {
    pub key: ClusterKey,
    pub label: String,
    pub description: String,
    pub phase: FlowPhase,
    #[serde(deserialize_with = "nullable")]
    pub phase_note: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FlowPhase {
    Entry,
    InputValidation,
    Guard,
    Load,
    Transform,
    ExternalCall,
    Persist,
    Respond,
    ErrorHandling,
    Other,
}

/// `attested_fact_digest` is the node fact digest at the last agent write of this step.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FlowStep {
    pub id: StepId,
    pub node_key: NodeKey,
    pub cluster_key: ClusterKey,
    pub title: String,
    pub annotation: String,
    pub attested_fact_digest: Sha256Digest,
}
