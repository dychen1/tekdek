//! Graph test fixtures.
//!
//! Builds the facts, nodes, anchors, and syntax graphs that the `build`,
//! `semantic`, and `lookup` tests share.

use super::{
    Node, Provenance, ProvenanceOrigin,
    build::{SyntaxGraph, build_graph},
};
use crate::analysis::{
    fact::{
        AnalysisError, CallableData, CallableKind, EntityEvidence, EntityFact, EntityKey,
        EntityKind, FactSet, FieldData, RelationEvidence, RelationFact, RelationKind,
        ReportedLocation, ResolutionFallback, SymbolAnchor, SyntaxFacts, TypeData, TypeKind,
    },
    language::Language,
    source::{SourceRange, SourceSpan},
};

/// Builds a syntax graph from syntax facts.
///
/// # Errors
///
/// Returns an error if an entity key repeats or a relation names no entity.
pub(super) fn to_syntax_graph(
    language: Language,
    facts: SyntaxFacts,
) -> Result<SyntaxGraph, AnalysisError> {
    let graph = build_graph(vec![FactSet::new(language, facts, Vec::new())], &[])?;
    Ok(SyntaxGraph {
        nodes: graph.nodes,
        syntax_edges: graph.edges,
    })
}

pub(super) fn fixture_key(id: u32) -> EntityKey {
    EntityKey::module(&format!("e{id}"))
}

pub(super) fn entity(
    id: u32,
    name: &str,
    path: &str,
    kind: EntityKind,
    declaration: Option<(u32, u32)>,
    body: Option<(u32, u32)>,
) -> EntityFact {
    EntityFact {
        key: fixture_key(id),
        name: name.to_owned(),
        kind,
        evidence: EntityEvidence {
            path: path.to_owned(),
            declarations: declaration
                .map(|(start, end)| SourceRange { start, end })
                .into_iter()
                .collect(),
            scope: body
                .or(declaration)
                .map(|(start, end)| SourceRange { start, end }),
            body: body.map(|(start, end)| SourceRange { start, end }),
            type_inputs: Vec::new(),
            type_outputs: Vec::new(),
        },
    }
}

pub(super) fn relation(
    source: u32,
    target: u32,
    kind: RelationKind,
    observation: Option<SourceSpan>,
) -> RelationFact {
    RelationFact {
        source: fixture_key(source),
        target: fixture_key(target),
        kind,
        evidence: RelationEvidence {
            observations: observation.into_iter().collect(),
        },
    }
}

pub(super) fn anchor(
    path: &str,
    offset: u32,
    fallback: ResolutionFallback,
    name: &str,
) -> SymbolAnchor {
    let length = u32::try_from(name.len()).expect("a fixture name must be short");
    SymbolAnchor::Selected {
        path: path.to_owned(),
        range: SourceRange {
            start: offset,
            end: offset + length,
        },
        fallback,
        name: name.to_owned(),
    }
}

pub(super) fn locations(values: &[(&str, u32, u32)]) -> Vec<ReportedLocation> {
    values
        .iter()
        .map(|(path, start, end)| ReportedLocation::Selected(span(path, *start, *end)))
        .collect()
}

pub(super) fn span(path: &str, start: u32, end: u32) -> SourceSpan {
    SourceSpan {
        path: path.to_owned(),
        range: SourceRange { start, end },
    }
}

pub(super) fn class() -> EntityKind {
    EntityKind::Type(TypeData {
        kind: TypeKind::Class,
        documentation: None,
    })
}

pub(super) fn function() -> EntityKind {
    callable(CallableKind::Function)
}

pub(super) fn constructor() -> EntityKind {
    callable(CallableKind::Constructor)
}

fn callable(kind: CallableKind) -> EntityKind {
    EntityKind::Callable(CallableData {
        kind,
        signature: None,
        parameters: Vec::new(),
        return_type: None,
        documentation: None,
    })
}

pub(super) fn field() -> EntityKind {
    EntityKind::Field(FieldData {
        declared_type: None,
    })
}

pub(super) fn node(
    id: u32,
    path: &str,
    kind: EntityKind,
    declarations: Vec<(u32, u32)>,
    body: Option<(u32, u32)>,
) -> Node {
    let declarations = declarations
        .into_iter()
        .map(|(start, end)| SourceRange { start, end })
        .collect::<Vec<_>>();
    let body = body.map(|(start, end)| SourceRange { start, end });
    let scope = match (declarations.first(), body) {
        (Some(declaration), Some(body)) => Some(SourceRange {
            start: declaration.start.min(body.start),
            end: declaration.end.max(body.end),
        }),
        (Some(declaration), None) => Some(*declaration),
        (None, body) => body,
    };
    Node {
        language: Language::Python,
        key: fixture_key(id),
        name: format!("node-{id}"),
        kind,
        content_hash: None,
        provenance: Provenance {
            origin: ProvenanceOrigin::Syntax,
            evidence: EntityEvidence {
                path: path.to_owned(),
                declarations,
                scope,
                body,
                type_inputs: Vec::new(),
                type_outputs: Vec::new(),
            },
        },
    }
}
