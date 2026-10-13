//! Fact layer: the indexed source facts that one flow references.
//!
//! Code copies these values from the bound graph. Agents never write them. The
//! node and relation kinds are the wire enums of the graph artifact.

use serde::{Deserialize, Serialize};

use super::{ContentHash, NodeKey, Sha256Digest, nullable};
pub use crate::store::graph::{NodeKind, RelationKind};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FactLayer {
    pub projection: FactProjection,
    pub binding: FactBinding,
    /// The distinct step nodes that the graph contains, sorted by key.
    pub nodes: Vec<FactNode>,
    /// Every graph edge between two fact nodes, sorted by source, target, and kind.
    pub edges: Vec<FactEdge>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum FactProjection {
    #[serde(rename = "node-fact/1")]
    NodeFact1,
}

/// `graph_digest` is the SHA-256 of the exact graph bytes. It is excluded from the facts digest.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FactBinding {
    pub graph_artifact: String,
    pub graph_digest: Sha256Digest,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FactNode {
    pub key: NodeKey,
    pub name: String,
    pub kind: NodeKind,
    pub language: String,
    pub path: String,
    #[serde(deserialize_with = "nullable")]
    pub content_hash: Option<ContentHash>,
    #[serde(deserialize_with = "nullable")]
    pub signature: Option<String>,
    #[serde(deserialize_with = "nullable")]
    pub documentation: Option<String>,
    pub fact_digest: Sha256Digest,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FactEdge {
    pub source: NodeKey,
    pub target: NodeKey,
    pub kind: RelationKind,
}

/// Returns the serde wire name of a unit enum value.
#[must_use]
pub fn wire_name(value: &impl Serialize) -> String {
    match serde_json::to_value(value) {
        Ok(serde_json::Value::String(name)) => name,
        _ => String::new(),
    }
}
