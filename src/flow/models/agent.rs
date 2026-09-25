//! Agent output contract, version 1.
//!
//! This is the only data that a coding agent authors for one flow generation.
//! Text and key fields stay plain strings here: code canonicalizes and checks
//! them after parsing, so every problem becomes one reported issue.

use serde::{Deserialize, Serialize};

use super::{EvidenceBasis, FlowPhase, nullable};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "outcome",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum AgentFlowOutput {
    Flow {
        contract_version: u32,
        flow: AgentFlow,
    },
    NoFlow {
        contract_version: u32,
        reason: NoFlowReason,
        explanation: String,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NoFlowReason {
    BehaviorNotFound,
    BehaviorAmbiguous,
    InsufficientEvidence,
    OutsideIndexedScope,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentFlow {
    pub behavior: String,
    #[serde(deserialize_with = "nullable")]
    pub boundaries: Option<String>,
    pub evidence_basis: EvidenceBasis,
    #[serde(deserialize_with = "nullable")]
    pub revision_summary: Option<String>,
    pub clusters: Vec<AgentCluster>,
    pub steps: Vec<AgentStep>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentCluster {
    pub key: String,
    pub label: String,
    pub description: String,
    pub phase: FlowPhase,
    #[serde(deserialize_with = "nullable")]
    pub phase_note: Option<String>,
}

/// `prior_step_id` and `node_key` are references only; the agent never mints IDs.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentStep {
    #[serde(deserialize_with = "nullable")]
    pub prior_step_id: Option<String>,
    pub node_key: String,
    pub cluster_key: String,
    pub title: String,
    pub annotation: String,
}
