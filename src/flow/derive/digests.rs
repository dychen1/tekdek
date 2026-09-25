//! Layer digests.
//!
//! This module hashes the layers of one generation and the generation itself.
//! Provenance and the digests are excluded from every digest.

use serde::Serialize;

use crate::flow::{
    canonical::{self, CanonicalError},
    models::{
        ArtifactKind, AssignmentLayer, DiffOverlay, Digests, FactEdge, FactLayer, FactProjection,
        FlowArtifact, FlowId, FrozenStatus, Generation, IntentLayer, NodeKey, Sha256Digest,
        ViewLayer,
    },
};

/// The artifact without digests and provenance: the input of the generation digest.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct GenerationBody<'a> {
    kind: ArtifactKind,
    schema_version: u32,
    flow_id: &'a FlowId,
    status: FrozenStatus,
    generation: &'a Generation,
    facts: &'a FactLayer,
    intent: &'a IntentLayer,
    assignment: &'a AssignmentLayer,
    view: &'a ViewLayer,
    diff: Option<&'a DiffOverlay>,
}

/// Returns the facts digest. The binding and its graph digest are excluded.
///
/// # Errors
/// Returns an error when the facts cannot have a canonical form.
pub fn facts_digest(facts: &FactLayer) -> Result<Sha256Digest, CanonicalError> {
    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    struct NodeRef<'a> {
        key: &'a NodeKey,
        fact_digest: &'a Sha256Digest,
    }
    #[derive(Serialize)]
    struct Input<'a> {
        projection: FactProjection,
        nodes: Vec<NodeRef<'a>>,
        edges: &'a [FactEdge],
    }
    canonical::digest(&Input {
        projection: facts.projection,
        nodes: facts
            .nodes
            .iter()
            .map(|node| NodeRef {
                key: &node.key,
                fact_digest: &node.fact_digest,
            })
            .collect(),
        edges: &facts.edges,
    })
}

/// Computes every digest of one artifact from its layers.
///
/// # Errors
/// Returns an error when a layer cannot have a canonical form.
pub fn digests(artifact: &FlowArtifact) -> Result<Digests, CanonicalError> {
    let body = GenerationBody {
        kind: artifact.kind,
        schema_version: artifact.schema_version,
        flow_id: &artifact.flow_id,
        status: artifact.status,
        generation: &artifact.generation,
        facts: &artifact.facts,
        intent: &artifact.intent,
        assignment: &artifact.assignment,
        view: &artifact.view,
        diff: artifact.diff.as_ref(),
    };
    Ok(Digests {
        facts: facts_digest(&artifact.facts)?,
        intent: canonical::digest(&artifact.intent)?,
        assignment: canonical::digest(&artifact.assignment)?,
        view: canonical::digest(&artifact.view)?,
        diff: artifact.diff.as_ref().map(canonical::digest).transpose()?,
        generation: canonical::digest(&body)?,
    })
}
