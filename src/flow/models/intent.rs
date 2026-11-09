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
///
/// `input` and `output` name the data models that enter and leave the group. An
/// empty list means that no data model enters or leaves it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClusterIntent {
    pub key: ClusterKey,
    pub label: String,
    pub input: Vec<String>,
    pub output: Vec<String>,
    pub rules: Vec<String>,
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
    pub ledger: StepLedger,
    pub attested_fact_digest: Sha256Digest,
}

/// The step text: one row for each slot that the step uses.
///
/// A null slot is not used. A used slot holds one or more one-line items, and at
/// least one slot is used. Items name identifiers with marks such as
/// `` `fn:read_graph` ``.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StepLedger {
    #[serde(deserialize_with = "nullable")]
    pub takes: Option<Vec<String>>,
    #[serde(deserialize_with = "nullable")]
    pub requires: Option<Vec<String>>,
    #[serde(deserialize_with = "nullable")]
    pub does: Option<Vec<String>>,
    #[serde(deserialize_with = "nullable")]
    pub on_fail: Option<Vec<String>>,
    #[serde(deserialize_with = "nullable")]
    pub returns: Option<Vec<String>>,
    #[serde(deserialize_with = "nullable")]
    pub writes: Option<Vec<String>>,
}

impl StepLedger {
    /// The slots in display order, with their wire names.
    #[must_use]
    pub fn slots(&self) -> [(&'static str, Option<&[String]>); 6] {
        [
            ("takes", self.takes.as_deref()),
            ("requires", self.requires.as_deref()),
            ("does", self.does.as_deref()),
            ("onFail", self.on_fail.as_deref()),
            ("returns", self.returns.as_deref()),
            ("writes", self.writes.as_deref()),
        ]
    }

    /// Returns a ledger with each item replaced by `item(slot, index, text)`.
    #[must_use]
    pub fn map_items(&self, mut item: impl FnMut(&str, usize, &str) -> String) -> Self {
        let mut slot = |name: &str, items: &Option<Vec<String>>| {
            items.as_ref().map(|items| {
                items
                    .iter()
                    .enumerate()
                    .map(|(index, text)| item(name, index, text))
                    .collect()
            })
        };
        Self {
            takes: slot("takes", &self.takes),
            requires: slot("requires", &self.requires),
            does: slot("does", &self.does),
            on_fail: slot("onFail", &self.on_fail),
            returns: slot("returns", &self.returns),
            writes: slot("writes", &self.writes),
        }
    }
}
