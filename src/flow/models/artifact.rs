//! Frozen flow artifact: one immutable generation of one flow.
//!
//! Every change makes a new generation that names its parent by digest.

use serde::{Deserialize, Serialize};

use super::{
    AssignmentLayer, DiffOverlay, FactLayer, FlowId, IntentLayer, Sha256Digest, ViewLayer, nullable,
};

/// The flow artifact wire-schema version.
pub const FLOW_ARTIFACT_SCHEMA_VERSION: u32 = 3;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FlowArtifact {
    pub kind: ArtifactKind,
    pub schema_version: u32,
    pub flow_id: FlowId,
    pub status: FrozenStatus,
    pub generation: Generation,
    pub facts: FactLayer,
    pub intent: IntentLayer,
    pub assignment: AssignmentLayer,
    pub view: ViewLayer,
    /// Null for an `initial` generation. Present for a `revision`.
    #[serde(deserialize_with = "nullable")]
    pub diff: Option<DiffOverlay>,
    pub digests: Digests,
    /// Excluded from every digest.
    pub provenance: Provenance,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ArtifactKind {
    #[serde(rename = "tekdek.flow")]
    Flow,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FrozenStatus {
    Frozen,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Generation {
    pub ordinal: u32,
    pub purpose: GenerationPurpose,
    #[serde(deserialize_with = "nullable")]
    pub parent_digest: Option<Sha256Digest>,
}

/// `initial` starts a lineage. `revision` changes the intent on the same graph.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GenerationPurpose {
    Initial,
    Revision,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Digests {
    pub facts: Sha256Digest,
    pub intent: Sha256Digest,
    pub assignment: Sha256Digest,
    pub view: Sha256Digest,
    #[serde(deserialize_with = "nullable")]
    pub diff: Option<Sha256Digest>,
    pub generation: Sha256Digest,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Provenance {
    /// UTC time in the form `YYYY-MM-DDTHH:MM:SSZ`.
    pub frozen_at: String,
    pub cli_version: String,
    pub author: Author,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum Author {
    Agent {
        name: String,
        #[serde(deserialize_with = "nullable")]
        model_id: Option<String>,
    },
    Human {
        name: String,
    },
    System,
}
