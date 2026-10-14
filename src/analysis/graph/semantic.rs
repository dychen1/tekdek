//! Semantic edge construction.
//!
//! Adds call, type, reference, and import edges from the semantic facts of one
//! language. The graph with its syntax nodes, the semantic facts, and the
//! import statements are the input. Semantic edges are the output.
//!
//! # Sections
//!
//! `Semantic Edge Construction` adds semantic edges.
//! `Edge Construction State` holds node matches and source positions.

use std::collections::{HashMap, HashSet};

use super::{build::EdgeAccumulator, lookup::SyntaxGraphIndex};
use crate::analysis::{
    fact::{
        AnalysisError, EntityKind, ImportSite, RelationKind, ReportedLocation, ResolutionFallback,
        SemanticFact, SymbolAnchor, TypeKind,
    },
    graph::{Edge, Graph, NodeId, ProvenanceOrigin},
    language::Language,
    source::{SourceRange, SourceSpan},
};

// -----------------------------------------------------------------------------
// Semantic Edge Construction
// -----------------------------------------------------------------------------

/// Builds edges from semantic facts and the import statements with their
/// owner nodes.
///
/// # Errors
///
/// Returns an error if a selected semantic position has no permitted node.
pub(crate) fn build_semantic_edges(
    graph: &Graph,
    language: Language,
    semantic_facts: &[SemanticFact],
    imports: &[(NodeId, ImportSite)],
) -> Result<Vec<Edge>, AnalysisError> {
    let lookup = SyntaxGraphIndex::new(graph, imports);
    let call_sites = collect_call_sites(semantic_facts);
    let mut edges = EdgeAccumulator::new(ProvenanceOrigin::Semantic);

    add_import_edges(&lookup, language, semantic_facts, imports, &mut edges);
    add_call_edges(&lookup, language, semantic_facts, &mut edges)?;
    add_type_use_edges(&lookup, language, semantic_facts, &mut edges)?;
    add_reference_edges(&lookup, language, semantic_facts, &call_sites, &mut edges)?;

    let mut edges = edges.finish()?;
    edges.sort_by_key(|edge| (edge.source.0, semantic_edge_rank(edge.kind), edge.target.0));
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

/// Collects the call sites that suppress duplicate references, by path.
pub(crate) fn collect_call_sites(semantic_facts: &[SemanticFact]) -> CallSites<'_> {
    let mut sites = CallSites::new();
    for site in semantic_facts
        .iter()
        .filter_map(|fact| match fact {
            SemanticFact::Call { call_sites, .. } => Some(call_sites),
            SemanticFact::Reference { .. }
            | SemanticFact::TypeDefinition { .. }
            | SemanticFact::Definition { .. } => None,
        })
        .flatten()
    {
        if let ReportedLocation::Selected(span) = site {
            sites
                .entry(span.path.as_str())
                .or_default()
                .push(span.range);
        }
    }
    sites
}

/// Tests whether one occurrence is the callee name of a call site.
///
/// A call site can start before its name, for example at the path of a
/// qualified Rust call: the name then ends the call site. Another name inside
/// the path, such as a type or a generic argument, stays a reference.
pub(crate) fn is_call_site(call_sites: &CallSites<'_>, occurrence: &SourceSpan) -> bool {
    let name = occurrence.range;
    call_sites
        .get(occurrence.path.as_str())
        .is_some_and(|ranges| {
            ranges.iter().any(|range| {
                range.start == name.start || (range.start < name.start && name.end == range.end)
            })
        })
}

/// Adds import edges from the definitions of imported names.
///
/// The target is the node declared at a definition, else the smallest scope
/// that holds it, else its file module. A definition at offset 0 names its
/// file module: servers report a module there, and a node that starts there
/// is not the module. A definition outside the selected sources and an import
/// of the owner itself add no edge.
fn add_import_edges(
    lookup: &SyntaxGraphIndex<'_>,
    language: Language,
    semantic_facts: &[SemanticFact],
    imports: &[(NodeId, ImportSite)],
    edges: &mut EdgeAccumulator,
) {
    let imports_by_name = imports
        .iter()
        .flat_map(|import| {
            import
                .1
                .names
                .iter()
                .map(move |name| ((import.1.statement.path.as_str(), name.start), import))
        })
        .collect::<HashMap<_, _>>();

    for fact in semantic_facts {
        let SemanticFact::Definition { query, targets } = fact else {
            continue;
        };
        let Some((owner, site)) = imports_by_name.get(&(query.path.as_str(), query.range.start))
        else {
            continue;
        };
        for location in targets {
            let ReportedLocation::Selected(location) = location else {
                continue;
            };
            let offset = location.range.start;
            let target = (offset > 0)
                .then(|| {
                    lookup
                        .declaration_at(language, &location.path, offset)
                        .or_else(|| lookup.scope_owner_at(language, &location.path, offset))
                })
                .flatten()
                .or_else(|| lookup.file_module(language, &location.path));
            if let Some(target) = target.filter(|target| target != owner) {
                edges.observe(
                    *owner,
                    target,
                    RelationKind::Imports,
                    vec![site.statement.clone()],
                );
            }
        }
    }
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
    edges: &mut EdgeAccumulator,
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
        let target = select_call_target(lookup, target_anchor);

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
                vec![observation.clone()],
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
    edges: &mut EdgeAccumulator,
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
            if !is_exact_type(lookup, target) || source.node == target.node {
                continue;
            }
            edges.observe(
                source.node,
                target.node,
                RelationKind::UsesType,
                vec![observation.clone()],
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
    call_sites: &CallSites<'_>,
    edges: &mut EdgeAccumulator,
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
        let kind = reference_relation_kind(lookup, target);

        for location in occurrences {
            // Select one source occurrence.
            let ReportedLocation::Selected(observation) = location else {
                continue;
            };
            // Drop an occurrence when a call or import edge already states it.
            if is_call_site(call_sites, observation)
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
                    edges.observe(source, target, kind, vec![observation.clone()]);
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
            edges.observe(source, target.node, kind, vec![observation.clone()]);
        }
    }

    Ok(())
}

/// Returns the graph nodes for types stated by an annotation.
///
/// # Errors
///
/// Returns an error if a selected type location has no node.
fn annotated_types(
    lookup: &SyntaxGraphIndex<'_>,
    language: Language,
    anchor: &SymbolAnchor,
    target: NodeMatch,
    declared_types: &HashMap<SourcePosition, &[ReportedLocation]>,
) -> Result<Vec<NodeId>, AnalysisError> {
    if is_exact_type(lookup, target) {
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
        if is_exact_type(lookup, declared) {
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
            SemanticFact::Call { .. }
            | SemanticFact::Reference { .. }
            | SemanticFact::Definition { .. } => None,
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
fn select_call_target(lookup: &SyntaxGraphIndex<'_>, target: NodeMatch) -> NodeMatch {
    let is_exact_class = !target.used_owner
        && matches!(
            &lookup.node(target.node).kind,
            EntityKind::Type(data) if matches!(data.kind, TypeKind::Class)
        );
    let Some(constructor) = is_exact_class
        .then(|| lookup.last_direct_constructor(target.node))
        .flatten()
    else {
        return target;
    };

    NodeMatch {
        node: constructor,
        used_owner: false,
    }
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
fn reference_relation_kind(lookup: &SyntaxGraphIndex<'_>, target: NodeMatch) -> RelationKind {
    if is_exact_type(lookup, target) {
        RelationKind::UsesType
    } else {
        RelationKind::UsesValue
    }
}

/// Tests whether a match names a type declaration.
fn is_exact_type(lookup: &SyntaxGraphIndex<'_>, target: NodeMatch) -> bool {
    !target.used_owner && matches!(&lookup.node(target.node).kind, EntityKind::Type(_))
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

/// The call site ranges of each source path.
pub(crate) type CallSites<'a> = HashMap<&'a str, Vec<SourceRange>>;

#[derive(Eq, Hash, PartialEq)]
struct SourcePosition {
    path: String,
    offset: u32,
}

// -----------------------------------------------------------------------------
// Tests
// -----------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::{build_semantic_edges, match_symbol};
    use crate::analysis::{
        fact::{
            EntityFact, EntityKind, ImportSite, RelationKind, ReportedLocation, ResolutionFallback,
            SemanticFact, SyntaxFacts,
        },
        graph::{
            Graph, NodeId, ProvenanceOrigin,
            fixtures::{
                anchor, class, constructor, entity, field, fixture_key, function, locations,
                relation, span, to_syntax_graph,
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
                import_sites: Vec::new(),
            },
        )
        .expect("the syntax fixture must materialize");
        let lookup = SyntaxGraphIndex::new(&graph, &[]);
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
                import_sites: Vec::new(),
            },
        )
        .expect("the Rust syntax fixture must materialize");
        let semantic = vec![SemanticFact::Reference {
            target: anchor("src/lib.rs", 20, ResolutionFallback::Ignore, "generated"),
            occurrences: locations(&[("src/lib.rs", 50, 59)]),
        }];

        let edges = build_semantic_edges(&graph, Language::Rust, &semantic, &[])
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

        let edges = build_semantic_edges(&graph, Language::Rust, &semantic, &[])
            .expect("the exact Rust declaration should resolve");

        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].source, NodeId(0));
        assert_eq!(edges[0].target, NodeId(1));
        assert_eq!(edges[0].kind, RelationKind::UsesValue);
    }

    #[test]
    fn drops_only_the_callee_name_of_a_call_site_that_starts_before_it() {
        let graph = rust_semantic_validation_graph();
        let run = || anchor("src/lib.rs", 10, ResolutionFallback::Ignore, "run");
        let semantic = vec![
            // A qualified call such as `Type::<run>::run` starts at the path.
            SemanticFact::Call {
                caller: run(),
                callee: run(),
                call_sites: locations(&[("src/lib.rs", 40, 53)]),
            },
            // The callee name ends the call site; a name inside the path stays a use.
            SemanticFact::Reference {
                target: run(),
                occurrences: locations(&[("src/lib.rs", 50, 53), ("src/lib.rs", 45, 48)]),
            },
        ];

        let edges = build_semantic_edges(&graph, Language::Rust, &semantic, &[])
            .expect("the call and the reference should resolve");

        assert_eq!(
            edges
                .iter()
                .map(|edge| (edge.source, edge.target, edge.kind, edge.weight))
                .collect::<Vec<_>>(),
            [
                (NodeId(0), NodeId(1), RelationKind::Calls, 1),
                (NodeId(0), NodeId(1), RelationKind::UsesValue, 1)
            ]
        );
    }

    #[test]
    fn ignored_call_anchor_does_not_hide_an_invalid_secondary_anchor() {
        let graph = rust_semantic_validation_graph();
        let semantic = vec![SemanticFact::Call {
            caller: anchor("src/lib.rs", 40, ResolutionFallback::Ignore, "generated"),
            callee: anchor("missing.rs", 0, ResolutionFallback::None, "missing"),
            call_sites: Vec::new(),
        }];

        let error = build_semantic_edges(&graph, Language::Rust, &semantic, &[])
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

        let error = build_semantic_edges(&graph, Language::Rust, &semantic, &[])
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

        let error = build_semantic_edges(&graph, Language::Rust, &semantic, &[])
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

        let error = build_semantic_edges(&graph, Language::Rust, &semantic, &[])
            .err()
            .expect("the invalid type target should fail");

        assert!(
            error
                .message
                .contains("no syntax graph node owns the type definition location")
        );
    }

    fn rust_semantic_validation_graph() -> Graph {
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
                relations: vec![relation(0, 1, RelationKind::Contains)],
                import_sites: Vec::new(),
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
                import_sites: Vec::new(),
            },
        )
        .expect("the syntax fixture must materialize");
        let semantic = vec![SemanticFact::Reference {
            target: anchor("app.py", 10, ResolutionFallback::Ignore, "value"),
            occurrences: locations(&[("missing.py", 20, 25)]),
        }];

        let error = build_semantic_edges(&graph, Language::Python, &semantic, &[])
            .err()
            .expect("the invalid occurrence should fail");

        assert!(error.message.contains("no syntax graph node owns"));
    }

    #[test]
    fn builds_direct_semantic_edges_with_deterministic_provenance() {
        let syntax_graph = semantic_edge_syntax_graph();
        let semantic_facts = semantic_edge_facts();

        let edges = build_semantic_edges(
            &syntax_graph,
            Language::Python,
            &semantic_facts,
            &[import_site(0, "app.py", (0, 9), &[])],
        )
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
    fn imports_the_declared_node_or_the_file_module() {
        let syntax_graph = semantic_edge_syntax_graph();
        let facts = vec![
            // The definition is the `User` declaration.
            SemanticFact::Definition {
                query: span("app.py", 16, 20),
                targets: locations(&[("lib.py", 10, 14)]),
            },
            // The definition is the start of `lib.py`, which declares nothing.
            SemanticFact::Definition {
                query: span("app.py", 22, 25),
                targets: locations(&[("lib.py", 0, 0)]),
            },
            // One definition is outside the project and one is the owner itself.
            SemanticFact::Definition {
                query: span("app.py", 27, 29),
                targets: vec![
                    ReportedLocation::Unselected,
                    ReportedLocation::Selected(span("app.py", 0, 0)),
                ],
            },
        ];

        let edges = build_semantic_edges(
            &syntax_graph,
            Language::Python,
            &facts,
            &[import_site(
                0,
                "app.py",
                (0, 30),
                &[(16, 20), (22, 25), (27, 29)],
            )],
        )
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
            vec![(0, "imports", 1, 1), (0, "imports", 2, 1)]
        );
        assert!(edges.iter().all(|edge| {
            edge.provenance.origin == ProvenanceOrigin::Semantic
                && edge.provenance.evidence.observations == [span("app.py", 0, 30)]
        }));
    }

    #[test]
    fn drops_a_reference_inside_an_import_statement() {
        let syntax_graph = semantic_edge_syntax_graph();
        let facts = vec![SemanticFact::Reference {
            target: anchor("lib.py", 10, ResolutionFallback::None, "User"),
            occurrences: locations(&[("app.py", 5, 9)]),
        }];

        let edges = build_semantic_edges(
            &syntax_graph,
            Language::Python,
            &facts,
            &[import_site(0, "app.py", (0, 9), &[(5, 9)])],
        )
        .expect("semantic edge construction must succeed");

        assert!(edges.is_empty());
    }

    #[test]
    fn counts_each_distinct_import_statement_once() {
        let syntax_graph = semantic_edge_syntax_graph();
        let facts = vec![
            // Two `import lib` statements name the same module.
            SemanticFact::Definition {
                query: span("app.py", 7, 10),
                targets: locations(&[("lib.py", 0, 0)]),
            },
            SemanticFact::Definition {
                query: span("app.py", 18, 21),
                targets: locations(&[("lib.py", 0, 0)]),
            },
            // Two names of one statement fall back to the same class.
            SemanticFact::Definition {
                query: span("app.py", 36, 37),
                targets: locations(&[("lib.py", 50, 51)]),
            },
            SemanticFact::Definition {
                query: span("app.py", 39, 40),
                targets: locations(&[("lib.py", 70, 71)]),
            },
        ];

        let edges = build_semantic_edges(
            &syntax_graph,
            Language::Python,
            &facts,
            &[
                import_site(0, "app.py", (0, 10), &[(7, 10)]),
                import_site(0, "app.py", (11, 21), &[(18, 21)]),
                import_site(0, "app.py", (22, 40), &[(36, 37), (39, 40)]),
            ],
        )
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
            vec![(0, "imports", 1, 2), (0, "imports", 2, 1)]
        );
    }

    #[test]
    fn imports_the_enclosing_node_of_a_definition_without_a_node() {
        let syntax_graph = rust_import_syntax_graph();
        let facts = vec![
            // `LIMIT` is a constant in `mod config`, which has no node.
            SemanticFact::Definition {
                query: span("src/lib.rs", 60, 65),
                targets: locations(&[("src/lib.rs", 25, 30)]),
            },
            // `Circle` is a variant of `Shape`, which has no node.
            SemanticFact::Definition {
                query: span("src/lib.rs", 70, 76),
                targets: locations(&[("src/lib.rs", 115, 121)]),
            },
            // Offset 0 names the file module, not `mod config` that starts there.
            SemanticFact::Definition {
                query: span("src/lib.rs", 80, 84),
                targets: locations(&[("src/lib.rs", 0, 0)]),
            },
        ];

        let edges = build_semantic_edges(
            &syntax_graph,
            Language::Rust,
            &facts,
            &[import_site(
                2,
                "src/lib.rs",
                (50, 90),
                &[(60, 65), (70, 76), (80, 84)],
            )],
        )
        .expect("semantic edge construction must succeed");

        assert_eq!(
            edges
                .iter()
                .map(|edge| (edge.source.0, edge_name(edge.kind), edge.target.0))
                .collect::<Vec<_>>(),
            vec![(2, "imports", 0), (2, "imports", 1), (2, "imports", 3)]
        );
    }

    #[test]
    fn imports_from_a_callable_and_drops_the_reference_in_its_use() {
        let syntax_graph = rust_import_syntax_graph();
        let run = anchor("src/lib.rs", 255, ResolutionFallback::Ignore, "run");
        let facts = vec![
            SemanticFact::Definition {
                query: span("src/lib.rs", 180, 183),
                targets: locations(&[("src/lib.rs", 255, 258)]),
            },
            // The first occurrence is inside the use statement of `main`.
            SemanticFact::Reference {
                target: run,
                occurrences: locations(&[("src/lib.rs", 180, 183), ("src/lib.rs", 190, 193)]),
            },
        ];

        let edges = build_semantic_edges(
            &syntax_graph,
            Language::Rust,
            &facts,
            &[import_site(4, "src/lib.rs", (165, 184), &[(180, 183)])],
        )
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
            vec![(4, "uses_value", 5, 1), (4, "imports", 5, 1)]
        );
    }

    /// Returns one import statement that the `owner` fixture node holds.
    fn import_site(
        owner: u32,
        path: &str,
        statement: (u32, u32),
        names: &[(u32, u32)],
    ) -> (NodeId, ImportSite) {
        (
            NodeId(owner),
            ImportSite {
                owner: fixture_key(owner),
                statement: span(path, statement.0, statement.1),
                names: ranges(names),
            },
        )
    }

    /// Builds one Rust file with inline modules, an enum, and two functions.
    fn rust_import_syntax_graph() -> Graph {
        let path = "src/lib.rs";
        to_syntax_graph(
            Language::Rust,
            SyntaxFacts {
                entities: vec![
                    entity(0, "lib", path, EntityKind::Module, None, Some((0, 300))),
                    entity(
                        1,
                        "config",
                        path,
                        EntityKind::Module,
                        Some((4, 10)),
                        Some((0, 40)),
                    ),
                    entity(
                        2,
                        "app",
                        path,
                        EntityKind::Module,
                        Some((45, 48)),
                        Some((49, 100)),
                    ),
                    entity(
                        3,
                        "Shape",
                        path,
                        class(),
                        Some((105, 110)),
                        Some((111, 150)),
                    ),
                    entity(
                        4,
                        "main",
                        path,
                        function(),
                        Some((155, 159)),
                        Some((160, 250)),
                    ),
                    entity(
                        5,
                        "run",
                        path,
                        function(),
                        Some((255, 258)),
                        Some((259, 280)),
                    ),
                ],
                relations: (1..=5)
                    .map(|child| relation(0, child, RelationKind::Contains))
                    .collect(),
                import_sites: Vec::new(),
            },
        )
        .expect("the Rust syntax fixture must materialize")
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

        let edges = build_semantic_edges(&syntax_graph, Language::Python, &facts, &[])
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

        let edges = build_semantic_edges(&syntax_graph, Language::Python, &facts, &[])
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

        let edges = build_semantic_edges(&syntax_graph, Language::Python, &facts, &[])
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

        let edges = build_semantic_edges(&syntax_graph, Language::Python, &facts, &[])
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

        let edges = build_semantic_edges(&syntax_graph, Language::Python, &facts, &[])
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

        let edges = build_semantic_edges(&syntax_graph, Language::Python, &facts, &[])
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

        let edges = build_semantic_edges(&syntax_graph, Language::Python, &facts, &[])
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

        let edges = build_semantic_edges(&syntax_graph, Language::Python, &facts, &[])
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
    fn signature_syntax_graph() -> Graph {
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
                    relation(0, 2, RelationKind::Contains),
                    relation(1, 1, RelationKind::Contains),
                ],
                import_sites: Vec::new(),
            },
        )
        .expect("the syntax fixture must materialize")
    }

    fn semantic_edge_syntax_graph() -> Graph {
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
                    relation(0, 5, RelationKind::Contains),
                    relation(1, 2, RelationKind::Contains),
                    relation(2, 3, RelationKind::Contains),
                    relation(2, 4, RelationKind::Contains),
                    relation(2, 6, RelationKind::Contains),
                    relation(1, 7, RelationKind::Contains),
                ],
                import_sites: Vec::new(),
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
            RelationKind::Imports => "imports",
            RelationKind::Contains => "unexpected",
        }
    }
}
