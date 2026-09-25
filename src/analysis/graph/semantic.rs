//! Semantic edge construction.
//!
//! Adds call, type, and reference edges from the semantic facts of one
//! language. The syntax graph and the semantic facts are the input. Semantic
//! edges are the output.
//!
//! # Sections
//!
//! `Semantic Edge Construction` adds semantic edges.
//! `Edge Construction State` collects semantic relations.

use std::collections::{HashMap, HashSet};

use super::{build::SyntaxGraph, lookup::SyntaxGraphIndex};
use crate::analysis::{
    fact::{
        AnalysisError, EntityKind, RelationEvidence, RelationKind, ReportedLocation,
        ResolutionFallback, SemanticFact, SymbolAnchor, TypeKind,
    },
    graph::{Edge, NodeId, Provenance, ProvenanceOrigin},
    language::Language,
    source::SourceSpan,
};

// -----------------------------------------------------------------------------
// Semantic Edge Construction
// -----------------------------------------------------------------------------

/// Builds edges from semantic facts.
///
/// # Errors
///
/// Returns an error if a selected semantic position has no permitted node.
pub(super) fn build_semantic_edges(
    syntax_graph: &SyntaxGraph,
    language: Language,
    semantic_facts: &[SemanticFact],
) -> Result<Vec<Edge>, AnalysisError> {
    let lookup = SyntaxGraphIndex::new(syntax_graph);
    let call_positions = collect_call_positions(semantic_facts);
    let mut edges = SemanticEdgeAccumulator::new();

    add_call_edges(&lookup, language, semantic_facts, &mut edges)?;
    add_type_use_edges(&lookup, language, semantic_facts, &mut edges)?;
    add_reference_edges(
        &lookup,
        language,
        semantic_facts,
        &call_positions,
        &mut edges,
    )?;

    let mut edges = edges.finish()?;
    remove_stated_type_uses(&mut edges);

    Ok(edges)
}

/// Removes type-use edges that duplicate signature edges.
fn remove_stated_type_uses(edges: &mut Vec<Edge>) {
    let stated = edges
        .iter()
        .filter_map(|edge| match edge.kind {
            RelationKind::InputType => Some((edge.target, edge.source)),
            RelationKind::OutputType => Some((edge.source, edge.target)),
            RelationKind::Contains
            | RelationKind::Calls
            | RelationKind::Imports
            | RelationKind::UsesType
            | RelationKind::UsesValue => None,
        })
        .collect::<HashSet<_>>();

    edges.retain(|edge| {
        edge.kind != RelationKind::UsesType || !stated.contains(&(edge.source, edge.target))
    });
}

/// Collects call positions that suppress duplicate references.
fn collect_call_positions(semantic_facts: &[SemanticFact]) -> HashSet<SourcePosition> {
    semantic_facts
        .iter()
        .filter_map(|fact| match fact {
            SemanticFact::Call { call_sites, .. } => Some(call_sites),
            SemanticFact::Reference { .. } | SemanticFact::TypeDefinition { .. } => None,
        })
        .flatten()
        .filter_map(|site| match site {
            ReportedLocation::Selected(span) => Some(source_position(span)),
            ReportedLocation::Unselected => None,
        })
        .collect()
}

/// Adds call edges from call facts.
///
/// # Errors
///
/// Returns an error if a selected anchor or call location cannot match.
fn add_call_edges(
    lookup: &SyntaxGraphIndex<'_>,
    language: Language,
    semantic_facts: &[SemanticFact],
    edges: &mut SemanticEdgeAccumulator,
) -> Result<(), AnalysisError> {
    for fact in semantic_facts {
        let SemanticFact::Call {
            caller,
            callee,
            call_sites,
        } = fact
        else {
            continue;
        };
        let source_anchor = match_symbol(lookup, language, caller)?;
        let target_anchor = match_symbol(lookup, language, callee)?;
        let (Some(_), Some(target_anchor)) = (source_anchor, target_anchor) else {
            validate_body_locations(lookup, language, call_sites, "call")?;
            continue;
        };
        let target = select_call_target(lookup, target_anchor)?;

        for location in call_sites {
            let ReportedLocation::Selected(observation) = location else {
                continue;
            };
            let source = lookup
                .body_owner_at(language, &observation.path, observation.range.start)
                .ok_or_else(|| location_owner_error("call", observation))?;
            if source == target.node && target.used_owner {
                continue;
            }
            edges.observe(
                source,
                target.node,
                RelationKind::Calls,
                observation.clone(),
            );
        }
    }

    Ok(())
}

/// Adds type-use edges from type-definition facts.
///
/// Each edge records the queried symbol as its evidence.
///
/// # Errors
///
/// Returns an error if a selected source or target cannot match.
fn add_type_use_edges(
    lookup: &SyntaxGraphIndex<'_>,
    language: Language,
    semantic_facts: &[SemanticFact],
    edges: &mut SemanticEdgeAccumulator,
) -> Result<(), AnalysisError> {
    for fact in semantic_facts {
        let SemanticFact::TypeDefinition {
            queried_symbol,
            targets,
        } = fact
        else {
            continue;
        };
        let source = match_symbol(lookup, language, queried_symbol)?;
        let Some(source) = source else {
            validate_type_locations(lookup, language, targets)?;
            continue;
        };
        let Some(observation) = queried_symbol.span() else {
            continue;
        };

        for location in targets {
            let ReportedLocation::Selected(reported) = location else {
                continue;
            };
            let target = match_type_location(lookup, language, reported)?;
            if !is_exact_type(lookup, target)? || source.node == target.node {
                continue;
            }
            edges.observe(
                source.node,
                target.node,
                RelationKind::UsesType,
                observation.clone(),
            );
        }
    }

    Ok(())
}

/// Adds use and signature edges from reference facts.
///
/// # Errors
///
/// Returns an error if a selected target or occurrence cannot match.
fn add_reference_edges(
    lookup: &SyntaxGraphIndex<'_>,
    language: Language,
    semantic_facts: &[SemanticFact],
    call_positions: &HashSet<SourcePosition>,
    edges: &mut SemanticEdgeAccumulator,
) -> Result<(), AnalysisError> {
    // Index type definitions for alias resolution.
    let declared_types = declared_types_by_symbol(semantic_facts);

    for fact in semantic_facts {
        // Select one reference fact.
        let SemanticFact::Reference {
            target: anchor,
            occurrences,
        } = fact
        else {
            continue;
        };
        // Resolve the referenced symbol to a graph node.
        let target = match_symbol(lookup, language, anchor)?;
        let Some(target) = target else {
            validate_body_locations(lookup, language, occurrences, "reference")?;
            continue;
        };
        // Classify the target as a type or value.
        let kind = reference_relation_kind(lookup, target)?;

        for location in occurrences {
            // Select one source occurrence.
            let ReportedLocation::Selected(observation) = location else {
                continue;
            };
            // Drop an occurrence when a call or import edge already states it.
            if call_positions.contains(&source_position(observation))
                || lookup.is_import_position(language, &observation.path, observation.range.start)
            {
                continue;
            }

            // Add input or output edges for callable annotations.
            if let Some((callable, role)) =
                lookup.signature_role_at(language, &observation.path, observation.range.start)
            {
                for stated in annotated_types(lookup, language, anchor, target, &declared_types)? {
                    let (source, target, kind) = role.relation(callable, stated);
                    edges.observe(source, target, kind, observation.clone());
                }
                continue;
            }

            // Drop a symbol without a graph node, such as a parameter or a
            // module-level name. The annotation branch above resolves such a symbol.
            if target.used_owner {
                continue;
            }

            // Find the body that contains this reference.
            let source = lookup
                .body_owner_at(language, &observation.path, observation.range.start)
                .ok_or_else(|| location_owner_error("reference", observation))?;
            // Add the remaining body reference.
            edges.observe(source, target.node, kind, observation.clone());
        }
    }

    Ok(())
}

/// Returns the graph nodes for types stated by an annotation.
///
/// # Errors
///
/// Returns an error if a matched node identity or type location is invalid.
fn annotated_types(
    lookup: &SyntaxGraphIndex<'_>,
    language: Language,
    anchor: &SymbolAnchor,
    target: NodeMatch,
    declared_types: &HashMap<SourcePosition, &[ReportedLocation]>,
) -> Result<Vec<NodeId>, AnalysisError> {
    if is_exact_type(lookup, target)? {
        return Ok(vec![target.node]);
    }

    let Some(locations) =
        anchor_position(anchor).and_then(|position| declared_types.get(&position))
    else {
        return Ok(Vec::new());
    };

    let mut types = Vec::new();
    for location in *locations {
        let ReportedLocation::Selected(observation) = location else {
            continue;
        };
        let declared = match_type_location(lookup, language, observation)?;
        if is_exact_type(lookup, declared)? {
            types.push(declared.node);
        }
    }

    Ok(types)
}

/// Indexes reported type targets by symbol position.
fn declared_types_by_symbol(
    semantic_facts: &[SemanticFact],
) -> HashMap<SourcePosition, &[ReportedLocation]> {
    semantic_facts
        .iter()
        .filter_map(|fact| match fact {
            SemanticFact::TypeDefinition {
                queried_symbol,
                targets,
            } => Some((anchor_position(queried_symbol)?, targets.as_slice())),
            SemanticFact::Call { .. } | SemanticFact::Reference { .. } => None,
        })
        .collect()
}

fn anchor_position(anchor: &SymbolAnchor) -> Option<SourcePosition> {
    match anchor {
        SymbolAnchor::Selected { path, range, .. } => Some(SourcePosition {
            path: path.clone(),
            offset: range.start,
        }),
        SymbolAnchor::Unselected => None,
    }
}

/// Validates selected locations that an ignored endpoint cannot retain.
fn validate_body_locations(
    lookup: &SyntaxGraphIndex<'_>,
    language: Language,
    locations: &[ReportedLocation],
    location_kind: &str,
) -> Result<(), AnalysisError> {
    for location in locations {
        let ReportedLocation::Selected(observation) = location else {
            continue;
        };
        lookup
            .body_owner_at(language, &observation.path, observation.range.start)
            .ok_or_else(|| location_owner_error(location_kind, observation))?;
    }
    Ok(())
}

/// Validates selected type targets that an ignored endpoint cannot retain.
fn validate_type_locations(
    lookup: &SyntaxGraphIndex<'_>,
    language: Language,
    locations: &[ReportedLocation],
) -> Result<(), AnalysisError> {
    for location in locations {
        let ReportedLocation::Selected(observation) = location else {
            continue;
        };
        match_type_location(lookup, language, observation)?;
    }
    Ok(())
}

/// Matches a selected symbol to a graph node.
///
/// # Errors
///
/// Returns an error if the anchor has no permitted syntax graph node.
pub(super) fn match_symbol(
    lookup: &SyntaxGraphIndex<'_>,
    language: Language,
    anchor: &SymbolAnchor,
) -> Result<Option<NodeMatch>, AnalysisError> {
    let SymbolAnchor::Selected {
        path,
        range,
        fallback,
        name,
    } = anchor
    else {
        return Ok(None);
    };

    if let Some(node) = lookup.declaration_at(language, path, range.start) {
        return Ok(Some(NodeMatch {
            node,
            used_owner: false,
        }));
    }

    let node = match fallback {
        ResolutionFallback::None => None,
        ResolutionFallback::Ignore => return Ok(None),
        ResolutionFallback::ScopeOwner => lookup.scope_owner_at(language, path, range.start),
        ResolutionFallback::CallableOrModuleBodyOwner => {
            lookup.callable_or_module_body_owner_at(language, path, range.start)
        }
    };
    node.map(|node| NodeMatch {
        node,
        used_owner: true,
    })
    .map(Some)
    .ok_or_else(|| AnalysisError {
        message: format!("cannot match the {name} symbol anchor"),
    })
}

/// Selects the last direct constructor for an exact class target.
///
/// # Errors
///
/// Returns an error if the matched node identity is invalid.
fn select_call_target(
    lookup: &SyntaxGraphIndex<'_>,
    target: NodeMatch,
) -> Result<NodeMatch, AnalysisError> {
    let node = lookup.node(target.node).ok_or_else(|| AnalysisError {
        message: "a resolved call target has an invalid node identity".to_owned(),
    })?;
    let is_exact_class = !target.used_owner
        && matches!(
            &node.kind,
            EntityKind::Type(data) if matches!(data.kind, TypeKind::Class)
        );
    let Some(constructor) = is_exact_class
        .then(|| lookup.last_direct_constructor(target.node))
        .flatten()
    else {
        return Ok(target);
    };

    Ok(NodeMatch {
        node: constructor,
        used_owner: false,
    })
}

/// Matches a type location to a graph node.
///
/// # Errors
///
/// Returns an error if a selected target has no syntax graph node.
fn match_type_location(
    lookup: &SyntaxGraphIndex<'_>,
    language: Language,
    observation: &SourceSpan,
) -> Result<NodeMatch, AnalysisError> {
    if let Some(node) = lookup.declaration_at(language, &observation.path, observation.range.start)
    {
        return Ok(NodeMatch {
            node,
            used_owner: false,
        });
    }

    lookup
        .scope_owner_at(language, &observation.path, observation.range.start)
        .map(|node| NodeMatch {
            node,
            used_owner: true,
        })
        .ok_or_else(|| location_owner_error("type definition", observation))
}

/// Returns the relation kind for a reference target.
///
/// # Errors
///
/// Returns an error if the matched node identity is invalid.
fn reference_relation_kind(
    lookup: &SyntaxGraphIndex<'_>,
    target: NodeMatch,
) -> Result<RelationKind, AnalysisError> {
    if is_exact_type(lookup, target)? {
        Ok(RelationKind::UsesType)
    } else {
        Ok(RelationKind::UsesValue)
    }
}

/// Tests whether a match names a type declaration.
///
/// # Errors
///
/// Returns an error if the matched node identity is invalid.
fn is_exact_type(lookup: &SyntaxGraphIndex<'_>, target: NodeMatch) -> Result<bool, AnalysisError> {
    let node = lookup.node(target.node).ok_or_else(|| AnalysisError {
        message: "a resolved reference target has an invalid node identity".to_owned(),
    })?;

    Ok(!target.used_owner && matches!(&node.kind, EntityKind::Type(_)))
}

fn source_position(span: &SourceSpan) -> SourcePosition {
    SourcePosition {
        path: span.path.clone(),
        offset: span.range.start,
    }
}

/// Creates an error for a source location without an owner.
fn location_owner_error(location_kind: &str, observation: &SourceSpan) -> AnalysisError {
    AnalysisError {
        message: format!(
            "no syntax graph node owns the {location_kind} location in {}",
            observation.path
        ),
    }
}

fn semantic_edge_rank(kind: RelationKind) -> u8 {
    match kind {
        RelationKind::Calls => 0,
        RelationKind::InputType => 1,
        RelationKind::OutputType => 2,
        RelationKind::UsesType => 3,
        RelationKind::UsesValue => 4,
        RelationKind::Contains | RelationKind::Imports => 5,
    }
}

// -----------------------------------------------------------------------------
// Edge Construction State
// -----------------------------------------------------------------------------

#[derive(Clone, Copy)]
pub(super) struct NodeMatch {
    node: NodeId,
    used_owner: bool,
}

#[derive(Eq, Hash, PartialEq)]
struct SourcePosition {
    path: String,
    offset: u32,
}

struct SemanticEdgeAccumulator {
    edges: Vec<Edge>,
    edge_indexes: HashMap<(NodeId, NodeId, RelationKind), usize>,
}

impl SemanticEdgeAccumulator {
    fn new() -> Self {
        Self {
            edges: Vec::new(),
            edge_indexes: HashMap::new(),
        }
    }

    /// Adds an observation to a semantic edge.
    fn observe(
        &mut self,
        source: NodeId,
        target: NodeId,
        kind: RelationKind,
        observation: SourceSpan,
    ) {
        let key = (source, target, kind);
        if let Some(index) = self.edge_indexes.get(&key).copied() {
            self.edges[index]
                .provenance
                .evidence
                .observations
                .push(observation);
            return;
        }

        let index = self.edges.len();
        self.edges.push(Edge {
            source,
            target,
            kind,
            weight: 1,
            provenance: Provenance {
                origin: ProvenanceOrigin::Semantic,
                evidence: RelationEvidence {
                    observations: vec![observation],
                },
            },
        });
        self.edge_indexes.insert(key, index);
    }

    /// Returns normalized semantic edges in a stable order.
    ///
    /// # Errors
    ///
    /// Returns an error if an observation count cannot fit in an edge weight.
    fn finish(mut self) -> Result<Vec<Edge>, AnalysisError> {
        for edge in &mut self.edges {
            edge.provenance.evidence.normalize_observations();
            edge.weight =
                u32::try_from(edge.provenance.evidence.observations.len()).map_err(|_| {
                    AnalysisError {
                        message: "a semantic edge has too many source observations".to_owned(),
                    }
                })?;
        }
        self.edges
            .sort_by_key(|edge| (edge.source.0, semantic_edge_rank(edge.kind), edge.target.0));
        Ok(self.edges)
    }
}
