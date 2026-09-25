//! Fact projection.
//!
//! This module projects the graph nodes that one flow references into the fact
//! layer, and hashes each node projection. A graph and the step node keys are
//! the input. The fact layer is the output.

use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;

use super::DeriveError;
use crate::{
    flow::{
        canonical::{self, CanonicalError},
        models::{
            ContentHash, FactBinding, FactEdge, FactLayer, FactNode, FactProjection, NodeKey,
            NodeKind, RelationKind, Sha256Digest, from_wire_name, wire_name,
        },
    },
    store::graph::ArtifactGraph,
};

/// Projects the graph nodes with the given keys and the edges between them.
///
/// Nodes sort by key. Edges sort by source, target, and relation name, without duplicates.
///
/// # Errors
/// Returns an error for a graph value that the fact schema cannot hold.
pub fn fact_layer(
    graph: &ArtifactGraph,
    binding: FactBinding,
    keys: &BTreeSet<&str>,
) -> Result<FactLayer, DeriveError> {
    let mut nodes = Vec::new();
    let mut ids = BTreeMap::new();
    for node in graph
        .nodes
        .iter()
        .filter(|node| keys.contains(node.key.as_str()))
    {
        let key = NodeKey::try_from(node.key.as_str()).map_err(DeriveError::Fact)?;
        let kind: NodeKind = from_wire_name(&node.kind)
            .ok_or_else(|| DeriveError::Graph(format!("unknown node kind {}", node.kind)))?;
        let content_hash = node
            .content_hash
            .as_deref()
            .map(TryFrom::try_from)
            .transpose()
            .map_err(DeriveError::Fact)?;
        let mut fact = FactNode {
            key: key.clone(),
            name: node.name.clone(),
            kind,
            language: node.language.clone(),
            path: node.path.clone(),
            content_hash,
            signature: node.signature.clone(),
            documentation: node.documentation.clone(),
            fact_digest: canonical::digest_bytes(b""),
        };
        fact.fact_digest = fact_digest(&fact)?;
        ids.insert(node.id, key);
        nodes.push(fact);
    }
    nodes.sort_by(|left, right| left.key.cmp(&right.key));
    let mut edges = BTreeMap::new();
    for edge in &graph.edges {
        let (Some(source), Some(target)) = (ids.get(&edge.source), ids.get(&edge.target)) else {
            continue;
        };
        let kind: RelationKind = from_wire_name(&edge.kind)
            .ok_or_else(|| DeriveError::Graph(format!("unknown relation kind {}", edge.kind)))?;
        edges.insert((source.clone(), target.clone(), wire_name(&kind)), kind);
    }
    Ok(FactLayer {
        projection: FactProjection::NodeFact1,
        binding,
        nodes,
        edges: edges
            .into_iter()
            .map(|((source, target, _), kind)| FactEdge {
                source,
                target,
                kind,
            })
            .collect(),
    })
}

/// Returns the digest of the `node-fact/1` projection of one node.
///
/// # Errors
/// Returns an error when the projection cannot have a canonical form.
pub fn fact_digest(node: &FactNode) -> Result<Sha256Digest, CanonicalError> {
    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    struct FactInput<'a> {
        projection: FactProjection,
        key: &'a NodeKey,
        name: &'a str,
        kind: NodeKind,
        language: &'a str,
        path: &'a str,
        content_hash: Option<&'a str>,
        signature: Option<&'a str>,
        documentation: Option<&'a str>,
    }
    canonical::digest(&FactInput {
        projection: FactProjection::NodeFact1,
        key: &node.key,
        name: &node.name,
        kind: node.kind,
        language: &node.language,
        path: &node.path,
        content_hash: node.content_hash.as_ref().map(ContentHash::as_str),
        signature: node.signature.as_deref(),
        documentation: node.documentation.as_deref(),
    })
}
