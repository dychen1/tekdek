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

// -----------------------------------------------------------------------------
// Tests
// -----------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::{build_semantic_edges, match_symbol};
    use crate::analysis::{
        fact::{
            EntityFact, EntityKind, RelationKind, ResolutionFallback, SemanticFact, SyntaxFacts,
        },
        graph::{
            NodeId, ProvenanceOrigin,
            build::SyntaxGraph,
            fixtures::{
                anchor, class, constructor, entity, field, function, locations, relation, span,
                to_syntax_graph,
            },
            lookup::SyntaxGraphIndex,
        },
        language::Language,
        source::SourceRange,
    };

    #[test]
    fn ignores_a_generated_symbol_without_a_declaration() {
        let graph = to_syntax_graph(
            Language::Python,
            SyntaxFacts {
                entities: vec![entity(
                    0,
                    "app",
                    "app.py",
                    EntityKind::Module,
                    None,
                    Some((0, 100)),
                )],
                relations: Vec::new(),
            },
        )
        .expect("the syntax fixture must materialize");
        let lookup = SyntaxGraphIndex::new(&graph);
        let generated = anchor("app.py", 50, ResolutionFallback::Ignore, "(generated)");

        let matched = match_symbol(&lookup, Language::Python, &generated)
            .expect("the ignored symbol should not fail");

        assert!(matched.is_none());
    }

    #[test]
    fn ignored_rust_anchor_adds_no_edge() {
        let graph = to_syntax_graph(
            Language::Rust,
            SyntaxFacts {
                entities: vec![entity(
                    0,
                    "lib",
                    "src/lib.rs",
                    EntityKind::Module,
                    None,
                    Some((0, 100)),
                )],
                relations: Vec::new(),
            },
        )
        .expect("the Rust syntax fixture must materialize");
        let semantic = vec![SemanticFact::Reference {
            target: anchor("src/lib.rs", 20, ResolutionFallback::Ignore, "generated"),
            occurrences: locations(&[("src/lib.rs", 50, 59)]),
        }];

        let edges = build_semantic_edges(&graph, Language::Rust, &semantic)
            .expect("the ignored Rust anchor should not fail");

        assert!(edges.is_empty());
    }

    #[test]
    fn selected_rust_declarations_resolve_exactly() {
        let graph = rust_semantic_validation_graph();
        let semantic = vec![SemanticFact::Reference {
            target: anchor("src/lib.rs", 10, ResolutionFallback::Ignore, "run"),
            occurrences: locations(&[("src/lib.rs", 50, 53)]),
        }];

        let edges = build_semantic_edges(&graph, Language::Rust, &semantic)
            .expect("the exact Rust declaration should resolve");

        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].source, NodeId(0));
        assert_eq!(edges[0].target, NodeId(1));
        assert_eq!(edges[0].kind, RelationKind::UsesValue);
    }

    #[test]
    fn ignored_call_anchor_does_not_hide_an_invalid_secondary_anchor() {
        let graph = rust_semantic_validation_graph();
        let semantic = vec![SemanticFact::Call {
            caller: anchor("src/lib.rs", 40, ResolutionFallback::Ignore, "generated"),
            callee: anchor("missing.rs", 0, ResolutionFallback::None, "missing"),
            call_sites: Vec::new(),
        }];

        let error = build_semantic_edges(&graph, Language::Rust, &semantic)
            .err()
            .expect("the invalid secondary anchor should fail");

        assert!(
            error
                .message
                .contains("cannot match the missing symbol anchor")
        );
    }

    #[test]
    fn ignored_call_anchor_does_not_hide_an_invalid_call_site() {
        let graph = rust_semantic_validation_graph();
        let semantic = vec![SemanticFact::Call {
            caller: anchor("src/lib.rs", 40, ResolutionFallback::Ignore, "generated"),
            callee: anchor("src/lib.rs", 10, ResolutionFallback::Ignore, "run"),
            call_sites: locations(&[("missing.rs", 0, 1)]),
        }];

        let error = build_semantic_edges(&graph, Language::Rust, &semantic)
            .err()
            .expect("the invalid call site should fail");

        assert!(
            error
                .message
                .contains("no syntax graph node owns the call location")
        );
    }

    #[test]
    fn ignored_reference_anchor_does_not_hide_an_invalid_occurrence() {
        let graph = rust_semantic_validation_graph();
        let semantic = vec![SemanticFact::Reference {
            target: anchor("src/lib.rs", 40, ResolutionFallback::Ignore, "generated"),
            occurrences: locations(&[("missing.rs", 0, 1)]),
        }];

        let error = build_semantic_edges(&graph, Language::Rust, &semantic)
            .err()
            .expect("the invalid reference occurrence should fail");

        assert!(
            error
                .message
                .contains("no syntax graph node owns the reference location")
        );
    }

    #[test]
    fn ignored_type_anchor_does_not_hide_an_invalid_type_target() {
        let graph = rust_semantic_validation_graph();
        let semantic = vec![SemanticFact::TypeDefinition {
            queried_symbol: anchor("src/lib.rs", 40, ResolutionFallback::Ignore, "generated"),
            targets: locations(&[("missing.rs", 0, 1)]),
        }];

        let error = build_semantic_edges(&graph, Language::Rust, &semantic)
            .err()
            .expect("the invalid type target should fail");

        assert!(
            error
                .message
                .contains("no syntax graph node owns the type definition location")
        );
    }

    fn rust_semantic_validation_graph() -> SyntaxGraph {
        to_syntax_graph(
            Language::Rust,
            SyntaxFacts {
                entities: vec![
                    entity(
                        0,
                        "lib",
                        "src/lib.rs",
                        EntityKind::Module,
                        None,
                        Some((0, 100)),
                    ),
                    entity(
                        1,
                        "run",
                        "src/lib.rs",
                        function(),
                        Some((10, 13)),
                        Some((14, 30)),
                    ),
                ],
                relations: vec![relation(0, 1, RelationKind::Contains, None)],
            },
        )
        .expect("the Rust syntax fixture must materialize")
    }

    #[test]
    fn ignore_fallback_does_not_hide_an_invalid_occurrence() {
        let graph = to_syntax_graph(
            Language::Python,
            SyntaxFacts {
                entities: vec![entity(0, "value", "app.py", field(), Some((10, 15)), None)],
                relations: Vec::new(),
            },
        )
        .expect("the syntax fixture must materialize");
        let semantic = vec![SemanticFact::Reference {
            target: anchor("app.py", 10, ResolutionFallback::Ignore, "value"),
            occurrences: locations(&[("missing.py", 20, 25)]),
        }];

        let error = build_semantic_edges(&graph, Language::Python, &semantic)
            .err()
            .expect("the invalid occurrence should fail");

        assert!(error.message.contains("no syntax graph node owns"));
    }

    #[test]
    fn builds_direct_semantic_edges_with_deterministic_provenance() {
        let syntax_graph = semantic_edge_syntax_graph();
        let semantic_facts = semantic_edge_facts();

        let edges = build_semantic_edges(&syntax_graph, Language::Python, &semantic_facts)
            .expect("semantic edge construction must succeed");

        assert_eq!(
            edges
                .iter()
                .map(|edge| (
                    edge.source.0,
                    edge_name(edge.kind),
                    edge.target.0,
                    edge.weight,
                ))
                .collect::<Vec<_>>(),
            vec![
                (5, "calls", 4, 2),
                (5, "calls", 5, 1),
                (5, "calls", 7, 1),
                (5, "uses_type", 2, 2),
                (5, "uses_value", 5, 1),
                (5, "uses_value", 6, 1),
                (6, "uses_type", 7, 1),
            ]
        );
        assert!(
            edges
                .iter()
                .all(|edge| { matches!(edge.provenance.origin, ProvenanceOrigin::Semantic) })
        );
        let constructor_call = &edges[0];
        assert_eq!(
            constructor_call
                .provenance
                .evidence
                .observations
                .iter()
                .map(|observation| (observation.range.start, observation.range.end))
                .collect::<Vec<_>>(),
            vec![(50, 55), (60, 65)]
        );
    }

    #[test]
    fn points_a_signature_type_along_its_direction() {
        let syntax_graph = signature_syntax_graph();
        let facts = vec![SemanticFact::Reference {
            target: anchor("lib.py", 10, ResolutionFallback::None, "Model"),
            occurrences: locations(&[
                // Two parameters accept the type.
                ("app.py", 40, 45),
                ("app.py", 50, 55),
                // The return annotation states the type.
                ("app.py", 60, 65),
                // The body names the type as well.
                ("app.py", 250, 255),
            ]),
        }];

        let edges = build_semantic_edges(&syntax_graph, Language::Python, &facts)
            .expect("semantic edge construction must succeed");

        assert_eq!(
            edges
                .iter()
                .map(|edge| (
                    edge.source.0,
                    edge_name(edge.kind),
                    edge.target.0,
                    edge.weight,
                ))
                .collect::<Vec<_>>(),
            vec![(1, "input_type", 2, 2), (2, "output_type", 1, 1)]
        );
    }

    #[test]
    fn resolves_a_signature_type_alias_to_its_reported_type() {
        let syntax_graph = signature_syntax_graph();
        let alias = anchor("app.py", 320, ResolutionFallback::ScopeOwner, "Alias");
        let facts = vec![
            SemanticFact::Reference {
                target: alias.clone(),
                occurrences: locations(&[("app.py", 40, 45), ("app.py", 60, 65)]),
            },
            SemanticFact::TypeDefinition {
                queried_symbol: alias,
                targets: locations(&[("lib.py", 10, 14)]),
            },
        ];

        let edges = build_semantic_edges(&syntax_graph, Language::Python, &facts)
            .expect("semantic edge construction must succeed");

        assert_eq!(
            edges
                .iter()
                .map(|edge| (edge.source.0, edge_name(edge.kind), edge.target.0))
                .collect::<Vec<_>>(),
            vec![
                // The alias itself has no node, so its declared type stays on
                // the module that holds it.
                (0, "uses_type", 1),
                (1, "input_type", 2),
                (2, "output_type", 1),
            ]
        );
    }

    #[test]
    fn drops_a_parameter_type_that_an_annotation_states() {
        let syntax_graph = signature_syntax_graph();
        let facts = vec![
            // The annotation states the input.
            SemanticFact::Reference {
                target: anchor("lib.py", 10, ResolutionFallback::None, "Model"),
                occurrences: locations(&[("app.py", 40, 45)]),
            },
            // The server reports the same type for the parameter.
            SemanticFact::TypeDefinition {
                queried_symbol: anchor("app.py", 35, ResolutionFallback::ScopeOwner, "first"),
                targets: locations(&[("lib.py", 10, 14)]),
            },
        ];

        let edges = build_semantic_edges(&syntax_graph, Language::Python, &facts)
            .expect("semantic edge construction must succeed");

        assert_eq!(
            edges
                .iter()
                .map(|edge| (edge.source.0, edge_name(edge.kind), edge.target.0))
                .collect::<Vec<_>>(),
            vec![(1, "input_type", 2)]
        );
    }

    #[test]
    fn keeps_an_inferred_parameter_type_that_no_annotation_states() {
        let syntax_graph = signature_syntax_graph();
        // An unannotated parameter has no annotation range, so the reported
        // type is the only statement about it.
        let facts = vec![SemanticFact::TypeDefinition {
            queried_symbol: anchor("app.py", 35, ResolutionFallback::ScopeOwner, "first"),
            targets: locations(&[("lib.py", 10, 14)]),
        }];

        let edges = build_semantic_edges(&syntax_graph, Language::Python, &facts)
            .expect("semantic edge construction must succeed");

        assert_eq!(
            edges
                .iter()
                .map(|edge| (edge.source.0, edge_name(edge.kind), edge.target.0))
                .collect::<Vec<_>>(),
            vec![(2, "uses_type", 1)]
        );
    }

    #[test]
    fn ignores_a_type_definition_that_names_a_callable() {
        let syntax_graph = signature_syntax_graph();
        // The reported location is the `transform` declaration, not a type.
        let facts = vec![SemanticFact::TypeDefinition {
            queried_symbol: anchor("app.py", 320, ResolutionFallback::ScopeOwner, "handler"),
            targets: locations(&[("app.py", 20, 29)]),
        }];

        let edges = build_semantic_edges(&syntax_graph, Language::Python, &facts)
            .expect("semantic edge construction must succeed");

        assert!(edges.is_empty());
    }

    #[test]
    fn locates_a_type_use_at_the_queried_symbol() {
        let syntax_graph = signature_syntax_graph();
        let facts = vec![SemanticFact::TypeDefinition {
            queried_symbol: anchor("app.py", 250, ResolutionFallback::ScopeOwner, "config"),
            targets: locations(&[("lib.py", 10, 14)]),
        }];

        let edges = build_semantic_edges(&syntax_graph, Language::Python, &facts)
            .expect("semantic edge construction must succeed");

        assert_eq!(
            edges
                .iter()
                .flat_map(|edge| &edge.provenance.evidence.observations)
                .map(|observation| (
                    observation.path.as_str(),
                    observation.range.start,
                    observation.range.end,
                ))
                .collect::<Vec<_>>(),
            vec![("app.py", 250, 256)]
        );
    }

    #[test]
    fn ignores_a_signature_annotation_without_a_selected_type() {
        let syntax_graph = signature_syntax_graph();
        let facts = vec![SemanticFact::Reference {
            target: anchor("app.py", 320, ResolutionFallback::ScopeOwner, "COUNT"),
            occurrences: locations(&[("app.py", 40, 45), ("app.py", 60, 65)]),
        }];

        let edges = build_semantic_edges(&syntax_graph, Language::Python, &facts)
            .expect("semantic edge construction must succeed");

        assert!(edges.is_empty());
    }

    #[test]
    fn drops_references_to_unmodeled_symbols() {
        let syntax_graph = signature_syntax_graph();
        let facts = vec![
            // A parameter sits in the header and resolves to `transform`.
            SemanticFact::Reference {
                target: anchor("app.py", 35, ResolutionFallback::ScopeOwner, "client"),
                occurrences: locations(&[("app.py", 350, 356)]),
            },
            // A local sits in the body and resolves to `transform`.
            SemanticFact::Reference {
                target: anchor("app.py", 250, ResolutionFallback::ScopeOwner, "result"),
                occurrences: locations(&[("app.py", 360, 366)]),
            },
            // A module-level name resolves to the module.
            SemanticFact::Reference {
                target: anchor("app.py", 320, ResolutionFallback::ScopeOwner, "logger"),
                occurrences: locations(&[("app.py", 250, 256)]),
            },
            // A declared callable resolves to its own node.
            SemanticFact::Reference {
                target: anchor("app.py", 20, ResolutionFallback::None, "transform"),
                occurrences: locations(&[("app.py", 370, 379)]),
            },
        ];

        let edges = build_semantic_edges(&syntax_graph, Language::Python, &facts)
            .expect("semantic edge construction must succeed");

        assert_eq!(
            edges
                .iter()
                .map(|edge| {
                    (
                        edge.source.0,
                        edge_name(edge.kind),
                        edge.target.0,
                        edge.weight,
                    )
                })
                .collect::<Vec<_>>(),
            vec![(0, "uses_value", 2, 1)]
        );
    }

    /// Builds the syntax graph for signature tests.
    fn signature_syntax_graph() -> SyntaxGraph {
        to_syntax_graph(
            Language::Python,
            SyntaxFacts {
                entities: vec![
                    entity(0, "app", "app.py", EntityKind::Module, None, Some((0, 400))),
                    entity(
                        1,
                        "Model",
                        "lib.py",
                        class(),
                        Some((10, 15)),
                        Some((16, 100)),
                    ),
                    scoped(
                        annotated(
                            entity(
                                2,
                                "transform",
                                "app.py",
                                function(),
                                Some((20, 29)),
                                Some((200, 300)),
                            ),
                            &[(40, 45), (50, 55)],
                            &[(60, 65)],
                        ),
                        (20, 300),
                    ),
                ],
                relations: vec![
                    relation(0, 2, RelationKind::Contains, None),
                    relation(1, 1, RelationKind::Contains, None),
                ],
            },
        )
        .expect("the syntax fixture must materialize")
    }

    fn semantic_edge_syntax_graph() -> SyntaxGraph {
        to_syntax_graph(
            Language::Python,
            SyntaxFacts {
                entities: vec![
                    entity(0, "app", "app.py", EntityKind::Module, None, Some((0, 400))),
                    entity(1, "lib", "lib.py", EntityKind::Module, None, Some((0, 200))),
                    entity(
                        2,
                        "User",
                        "lib.py",
                        class(),
                        Some((10, 14)),
                        Some((15, 100)),
                    ),
                    entity(
                        3,
                        "__init__",
                        "lib.py",
                        constructor(),
                        Some((20, 28)),
                        Some((29, 30)),
                    ),
                    entity(
                        4,
                        "__init__",
                        "lib.py",
                        constructor(),
                        Some((30, 38)),
                        Some((39, 40)),
                    ),
                    entity(
                        5,
                        "run",
                        "app.py",
                        function(),
                        Some((10, 13)),
                        Some((20, 300)),
                    ),
                    entity(6, "value", "lib.py", field(), Some((60, 65)), None),
                    entity(
                        7,
                        "Empty",
                        "lib.py",
                        class(),
                        Some((110, 115)),
                        Some((120, 150)),
                    ),
                ],
                relations: vec![
                    relation(0, 5, RelationKind::Contains, None),
                    relation(1, 2, RelationKind::Contains, None),
                    relation(2, 3, RelationKind::Contains, None),
                    relation(2, 4, RelationKind::Contains, None),
                    relation(2, 6, RelationKind::Contains, None),
                    relation(1, 7, RelationKind::Contains, None),
                    relation(0, 1, RelationKind::Imports, Some(span("app.py", 0, 9))),
                ],
            },
        )
        .expect("the syntax fixture must materialize")
    }

    fn semantic_edge_facts() -> Vec<SemanticFact> {
        let run = anchor("app.py", 10, ResolutionFallback::None, "run");
        let user = anchor("lib.py", 10, ResolutionFallback::None, "User");
        vec![
            SemanticFact::Reference {
                target: user.clone(),
                occurrences: locations(&[("app.py", 5, 6), ("app.py", 50, 54), ("app.py", 90, 94)]),
            },
            SemanticFact::Call {
                caller: run.clone(),
                callee: user.clone(),
                call_sites: locations(&[("app.py", 50, 55), ("app.py", 60, 65)]),
            },
            SemanticFact::Call {
                caller: run.clone(),
                callee: user,
                call_sites: locations(&[("app.py", 50, 56), ("app.py", 60, 65)]),
            },
            SemanticFact::Call {
                caller: run.clone(),
                callee: run.clone(),
                call_sites: locations(&[("app.py", 70, 73)]),
            },
            SemanticFact::Call {
                caller: run.clone(),
                callee: anchor(
                    "app.py",
                    80,
                    ResolutionFallback::CallableOrModuleBodyOwner,
                    "(lambda)",
                ),
                call_sites: locations(&[("app.py", 80, 84)]),
            },
            SemanticFact::Call {
                caller: run.clone(),
                callee: anchor("lib.py", 110, ResolutionFallback::None, "Empty"),
                call_sites: locations(&[("app.py", 150, 155)]),
            },
            SemanticFact::Reference {
                target: anchor("lib.py", 60, ResolutionFallback::None, "value"),
                occurrences: locations(&[("app.py", 100, 105)]),
            },
            SemanticFact::Reference {
                target: run.clone(),
                occurrences: locations(&[("app.py", 130, 133)]),
            },
            SemanticFact::TypeDefinition {
                queried_symbol: run,
                targets: locations(&[("lib.py", 10, 14)]),
            },
            SemanticFact::TypeDefinition {
                queried_symbol: anchor("lib.py", 60, ResolutionFallback::None, "value"),
                targets: locations(&[("lib.py", 110, 115)]),
            },
            SemanticFact::TypeDefinition {
                queried_symbol: anchor(
                    "app.py",
                    160,
                    ResolutionFallback::ScopeOwner,
                    "local_value",
                ),
                targets: locations(&[("app.py", 10, 13)]),
            },
        ]
    }

    fn annotated(
        mut fact: EntityFact,
        inputs: &[(u32, u32)],
        outputs: &[(u32, u32)],
    ) -> EntityFact {
        fact.evidence.type_inputs = ranges(inputs);
        fact.evidence.type_outputs = ranges(outputs);
        fact
    }

    fn scoped(mut fact: EntityFact, scope: (u32, u32)) -> EntityFact {
        fact.evidence.scope = Some(SourceRange {
            start: scope.0,
            end: scope.1,
        });
        fact
    }

    fn ranges(values: &[(u32, u32)]) -> Vec<SourceRange> {
        values
            .iter()
            .map(|(start, end)| SourceRange {
                start: *start,
                end: *end,
            })
            .collect()
    }

    fn edge_name(kind: RelationKind) -> &'static str {
        match kind {
            RelationKind::Calls => "calls",
            RelationKind::InputType => "input_type",
            RelationKind::OutputType => "output_type",
            RelationKind::UsesType => "uses_type",
            RelationKind::UsesValue => "uses_value",
            RelationKind::Contains | RelationKind::Imports => "unexpected",
        }
    }
}
