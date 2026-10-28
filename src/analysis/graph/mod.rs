//! Graph Models
//!
//! # Sections
//!
//! `Graph` holds the finished nodes and edges.
//! `Node` gives one entity an identity in the graph.
//! `Edge` gives one relation two endpoints and a weight.
//! `Provenance` records which stage created one graph item.

pub mod build;
#[cfg(test)]
mod fixtures;
mod lookup;
mod semantic;

pub(crate) use lookup::{SyntaxGraphIndex, constructors_by_class};
pub(crate) use semantic::{build_semantic_edges, collect_call_sites, is_call_site};

use crate::analysis::{
    fact::{EntityEvidence, EntityKey, EntityKind, RelationEvidence, RelationKind},
    language::Language,
    source::{ContentHash, SourceRange},
};

// -----------------------------------------------------------------------------
// Graph
// -----------------------------------------------------------------------------

pub struct Graph {
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
}

impl Graph {
    /// Returns each node with its graph-local identity.
    pub fn identified_nodes(&self) -> impl Iterator<Item = (NodeId, &Node)> {
        (0u32..).map(NodeId).zip(&self.nodes)
    }
}

// -----------------------------------------------------------------------------
// Node
// -----------------------------------------------------------------------------

pub struct Node {
    pub language: Language,
    pub key: EntityKey,
    pub name: String,
    pub kind: EntityKind,
    pub content_hash: Option<ContentHash>,
    pub provenance: Provenance<EntityEvidence>,
}

impl Node {
    /// Returns the relative path that contains this node.
    #[must_use]
    pub fn path(&self) -> &str {
        &self.provenance.evidence.path
    }

    /// Returns the declaration ranges for this node.
    #[must_use]
    pub fn declarations(&self) -> &[SourceRange] {
        &self.provenance.evidence.declarations
    }

    /// Returns the lexical scope range for this node.
    #[must_use]
    pub fn scope(&self) -> Option<SourceRange> {
        self.provenance.evidence.scope
    }

    /// Returns the body range that this node owns.
    #[must_use]
    pub fn body(&self) -> Option<SourceRange> {
        self.provenance.evidence.body
    }

    /// Returns the annotation ranges of the types this node accepts.
    #[must_use]
    pub fn type_inputs(&self) -> &[SourceRange] {
        &self.provenance.evidence.type_inputs
    }

    /// Returns the annotation ranges of the types this node returns.
    #[must_use]
    pub fn type_outputs(&self) -> &[SourceRange] {
        &self.provenance.evidence.type_outputs
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct NodeId(pub u32);

impl NodeId {
    #[must_use]
    pub fn index(self) -> usize {
        self.0 as usize
    }
}

// -----------------------------------------------------------------------------
// Edge
// -----------------------------------------------------------------------------

pub struct Edge {
    pub source: NodeId,
    pub target: NodeId,
    pub kind: RelationKind,
    pub weight: u32,
    pub provenance: Provenance<RelationEvidence>,
}

// -----------------------------------------------------------------------------
// Provenance
// -----------------------------------------------------------------------------

pub struct Provenance<E> {
    pub origin: ProvenanceOrigin,
    pub evidence: E,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ProvenanceOrigin {
    Syntax,
    Semantic,
}
