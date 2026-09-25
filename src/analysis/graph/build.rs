//! Graph construction.
//!
//! Builds a graph from syntax and semantic facts. This module builds the syntax
//! nodes and edges. `semantic` adds the semantic edges, and `lookup` matches
//! source positions to nodes.
//!
//! # Sections
//!
//! `Construction` assembles the graph.
//! `Syntax Graph` holds the intermediate graph.
//! `Syntax Materialization` builds the syntax graph.

use std::collections::HashMap;

use super::semantic::build_semantic_edges;
use crate::analysis::{
    fact::{
        AnalysisError, EntityFact, EntityKey, FactSet, RelationFact, RelationKind, SyntaxFacts,
    },
    graph::{Edge, Graph, Node, NodeId, Provenance, ProvenanceOrigin},
    language::Language,
    source::{ContentHash, DocumentSnapshot, SourceRange},
};

// -----------------------------------------------------------------------------
// Construction
// -----------------------------------------------------------------------------

/// Builds one graph from language-tagged fact sets, and hashes each entity's
/// content against the documents.
///
/// The function builds all syntax nodes before it resolves semantic facts.
/// A node whose path or range matches no document keeps no hash.
///
/// # Errors
///
/// Returns an error if a fact is invalid or a semantic position has no owner.
pub fn build_graph(
    fact_sets: Vec<FactSet>,
    documents: &[DocumentSnapshot],
) -> Result<Graph, AnalysisError> {
    let mut syntax_graph = SyntaxGraph {
        nodes: Vec::new(),
        syntax_edges: Vec::new(),
    };
    let mut node_ids = HashMap::<EntityKey, NodeId>::new();
    let mut edge_indexes = HashMap::<(NodeId, NodeId, RelationKind), usize>::new();
    let mut semantic_batches = Vec::with_capacity(fact_sets.len());

    for FactSet {
        language,
        syntax,
        semantic,
    } in fact_sets
    {
        materialize_syntax(
            &mut syntax_graph,
            &mut node_ids,
            &mut edge_indexes,
            language,
            syntax,
        )?;
        semantic_batches.push((language, semantic));
    }
    normalize_edge_evidence(&mut syntax_graph.syntax_edges)?;

    for (language, semantic) in semantic_batches {
        let semantic_edges = build_semantic_edges(&syntax_graph, language, &semantic)?;
        syntax_graph.syntax_edges.extend(semantic_edges);
    }

    hash_content(&mut syntax_graph.nodes, documents);

    Ok(Graph {
        nodes: syntax_graph.nodes,
        edges: syntax_graph.syntax_edges,
    })
}

/// Fills the content hash of every node that has a derivable range.
fn hash_content(nodes: &mut [Node], documents: &[DocumentSnapshot]) {
    let documents_by_path: HashMap<&str, &DocumentSnapshot> = documents
        .iter()
        .filter_map(|document| document.path.to_str().map(|path| (path, document)))
        .collect();

    for node in nodes {
        node.content_hash = content_span(node)
            .and_then(|span| documents_by_path.get(node.path())?.text(span))
            .map(ContentHash::of);
    }
}

/// Returns the byte range whose text identifies one node.
///
/// The range runs from the first declaration to the body end, so a nested
/// change marks every ancestor of its owner.
fn content_span(node: &Node) -> Option<SourceRange> {
    let start = node.declarations().iter().map(|range| range.start).min();
    let end = node.declarations().iter().map(|range| range.end).max();

    if let Some(body) = node.body() {
        return Some(SourceRange {
            start: start.unwrap_or(body.start),
            end: body.end,
        });
    }

    match (start, end) {
        (Some(start), Some(end)) => Some(SourceRange { start, end }),
        _ => node.scope(),
    }
}

// -----------------------------------------------------------------------------
// Syntax Graph
// -----------------------------------------------------------------------------

pub(super) struct SyntaxGraph {
    pub(super) nodes: Vec<Node>,
    pub(super) syntax_edges: Vec<Edge>,
}

impl SyntaxGraph {
    pub(super) fn identified_nodes(&self) -> impl Iterator<Item = (NodeId, &Node)> {
        (0u32..).map(NodeId).zip(&self.nodes)
    }
}

// -----------------------------------------------------------------------------
// Syntax Materialization
// -----------------------------------------------------------------------------

/// Builds a syntax graph from syntax facts.
///
/// # Errors
///
/// Returns an error if an entity key repeats or a relation names no entity.
#[cfg(test)]
fn to_syntax_graph(language: Language, facts: SyntaxFacts) -> Result<SyntaxGraph, AnalysisError> {
    let mut syntax_graph = SyntaxGraph {
        nodes: Vec::new(),
        syntax_edges: Vec::new(),
    };
    let mut node_ids = HashMap::new();
    let mut edge_indexes = HashMap::new();
    materialize_syntax(
        &mut syntax_graph,
        &mut node_ids,
        &mut edge_indexes,
        language,
        facts,
    )?;
    normalize_edge_evidence(&mut syntax_graph.syntax_edges)?;
    Ok(syntax_graph)
}

fn materialize_syntax(
    syntax_graph: &mut SyntaxGraph,
    node_ids: &mut HashMap<EntityKey, NodeId>,
    edge_indexes: &mut HashMap<(NodeId, NodeId, RelationKind), usize>,
    language: Language,
    facts: SyntaxFacts,
) -> Result<(), AnalysisError> {
    let SyntaxFacts {
        entities,
        relations,
    } = facts;
    for EntityFact {
        key,
        name,
        kind,
        mut evidence,
    } in entities
    {
        let id = NodeId(u32::try_from(syntax_graph.nodes.len()).map_err(|_| {
            AnalysisError::new("the syntax has too many entities for u32 identities")
        })?);
        if let Some(existing) = node_ids.get(&key).copied() {
            let qualifier = if syntax_graph.nodes[existing.index()].language == language {
                ""
            } else {
                " across languages"
            };
            return Err(AnalysisError::new(format!(
                "a syntax entity key repeats{qualifier}: {key}"
            )));
        }
        node_ids.insert(key.clone(), id);
        evidence
            .declarations
            .sort_by_key(|range| (range.start, range.end));
        evidence.declarations.dedup();

        syntax_graph.nodes.push(Node {
            language,
            key,
            name,
            kind,
            content_hash: None,
            provenance: Provenance {
                origin: ProvenanceOrigin::Syntax,
                evidence,
            },
        });
    }

    for RelationFact {
        source,
        target,
        kind,
        evidence,
    } in relations
    {
        let source = resolve_endpoint_key(syntax_graph, node_ids, language, &source)?;
        let target = resolve_endpoint_key(syntax_graph, node_ids, language, &target)?;

        let key = (source, target, kind);
        if let Some(index) = edge_indexes.get(&key).copied() {
            syntax_graph.syntax_edges[index].weight += 1;
            syntax_graph.syntax_edges[index]
                .provenance
                .evidence
                .observations
                .extend(evidence.observations);
        } else {
            let index = syntax_graph.syntax_edges.len();
            syntax_graph.syntax_edges.push(Edge {
                source: key.0,
                target: key.1,
                kind: key.2,
                weight: 1,
                provenance: Provenance {
                    origin: ProvenanceOrigin::Syntax,
                    evidence,
                },
            });
            edge_indexes.insert(key, index);
        }
    }

    Ok(())
}

/// Resolves a relation endpoint to a node.
///
/// # Errors
///
/// Returns an error if no entity fact declared that key.
fn resolve_endpoint_key(
    syntax_graph: &SyntaxGraph,
    node_ids: &HashMap<EntityKey, NodeId>,
    language: Language,
    key: &EntityKey,
) -> Result<NodeId, AnalysisError> {
    let id = node_ids.get(key).copied().ok_or_else(|| {
        AnalysisError::new(format!("a syntax relation names an unknown entity: {key}"))
    })?;
    let node = syntax_graph.nodes.get(id.index()).ok_or_else(|| {
        AnalysisError::new("a syntax relation resolved to an invalid node identity")
    })?;
    if node.language != language {
        return Err(AnalysisError::new(format!(
            "a syntax relation names an entity from a different language: {key}"
        )));
    }

    Ok(id)
}

/// Normalizes syntax edge observations and weights.
///
/// # Errors
///
/// Returns an error if an observation count cannot fit in an edge weight.
fn normalize_edge_evidence(edges: &mut [Edge]) -> Result<(), AnalysisError> {
    for edge in edges {
        edge.provenance.evidence.normalize_observations();
        let observation_count = edge.provenance.evidence.observations.len();
        if observation_count > 0 {
            edge.weight = u32::try_from(observation_count).map_err(|_| AnalysisError {
                message: "a syntax edge has too many source observations".to_owned(),
            })?;
        }
    }

    Ok(())
}

// -----------------------------------------------------------------------------
// Tests
// -----------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use crate::analysis::{
        fact::{
            CallableData, CallableKind, EntityEvidence, EntityFact, EntityKey, EntityKind, FactSet,
            FieldData, RelationEvidence, RelationFact, RelationKind, ReportedLocation,
            ResolutionFallback, SemanticFact, SymbolAnchor, SyntaxFacts, TypeData, TypeKind,
        },
        graph::{Node, NodeId, Provenance, ProvenanceOrigin},
        language::Language,
        source::{ContentHash, DocumentSnapshot, SourceRange, SourceSpan},
    };
    use std::path::PathBuf;

    use super::{SyntaxGraph, build_graph, hash_content, to_syntax_graph};
    use crate::analysis::graph::{
        lookup::SyntaxGraphIndex,
        semantic::{build_semantic_edges, match_symbol},
    };

    #[test]
    fn materializes_stable_nodes_and_coalesced_syntax_edges() {
        let facts = SyntaxFacts {
            entities: vec![
                entity(0, "app", "app.py", EntityKind::Module, None, None),
                entity(1, "lib", "lib.py", EntityKind::Module, None, None),
                entity(2, "run", "app.py", function(), None, None),
            ],
            relations: vec![
                relation(0, 2, RelationKind::Contains, None),
                relation(2, 1, RelationKind::Imports, None),
                relation(2, 1, RelationKind::Imports, None),
            ],
        };

        let syntax = to_syntax_graph(Language::Python, facts).expect("the facts must be valid");

        assert_eq!(syntax.nodes.len(), 3);
        assert_eq!(syntax.nodes[2].key, fixture_key(2));
        assert_eq!(syntax.syntax_edges.len(), 2);
        assert!(matches!(
            syntax.syntax_edges[0].kind,
            RelationKind::Contains
        ));
        assert!(matches!(syntax.syntax_edges[1].kind, RelationKind::Imports));
        assert_eq!(syntax.syntax_edges[1].weight, 2);
    }

    #[test]
    fn rejects_duplicate_entity_keys_across_languages() {
        let python = module_facts(Language::Python, "shared/source");
        let rust = module_facts(Language::Rust, "shared/source");

        let error = build_graph(vec![python, rust], &[])
            .err()
            .expect("duplicate keys should fail");

        assert!(error.message.contains("repeats across languages"));
    }

    fn module_facts(language: Language, path: &str) -> FactSet {
        FactSet::new(
            language,
            SyntaxFacts {
                entities: vec![EntityFact {
                    key: EntityKey::module(path),
                    name: path.to_owned(),
                    kind: EntityKind::Module,
                    evidence: EntityEvidence {
                        path: path.to_owned(),
                        declarations: Vec::new(),
                        scope: None,
                        body: None,
                        type_inputs: Vec::new(),
                        type_outputs: Vec::new(),
                    },
                }],
                relations: Vec::new(),
            },
            Vec::new(),
        )
    }

    #[test]
    fn rejects_cross_language_syntax_relations_in_any_fact_set_order() {
        for python_first in [true, false] {
            let python = module_facts(Language::Python, "python.py");
            let rust_key = EntityKey::module("lib.rs");
            let rust = FactSet::new(
                Language::Rust,
                SyntaxFacts {
                    entities: vec![EntityFact {
                        key: rust_key.clone(),
                        name: "lib.rs".to_owned(),
                        kind: EntityKind::Module,
                        evidence: EntityEvidence {
                            path: "lib.rs".to_owned(),
                            declarations: Vec::new(),
                            scope: None,
                            body: None,
                            type_inputs: Vec::new(),
                            type_outputs: Vec::new(),
                        },
                    }],
                    relations: vec![RelationFact {
                        source: rust_key,
                        target: EntityKey::module("python.py"),
                        kind: RelationKind::Imports,
                        evidence: RelationEvidence {
                            observations: Vec::new(),
                        },
                    }],
                },
                Vec::new(),
            );
            let fact_sets = if python_first {
                vec![python, rust]
            } else {
                vec![rust, python]
            };

            assert!(
                build_graph(fact_sets, &[]).is_err(),
                "a cross-language relation must fail in either fact-set order"
            );
        }
    }

    #[test]
    fn resolves_semantic_anchors_only_in_the_fact_language() {
        let rust = semantic_language_facts(Language::Rust, "rust-root", false);
        let python = semantic_language_facts(Language::Python, "python-root", true);

        let graph =
            build_graph(vec![rust, python], &[]).expect("graph construction should succeed");
        let edge = graph
            .edges
            .iter()
            .find(|edge| edge.kind == RelationKind::UsesValue)
            .expect("the Python reference edge should exist");

        assert_eq!(graph.nodes[edge.source.index()].language, Language::Python);
        assert_eq!(graph.nodes[edge.target.index()].language, Language::Python);
    }

    fn semantic_language_facts(
        language: Language,
        root_key: &str,
        include_semantic: bool,
    ) -> FactSet {
        let module_key = EntityKey::module(root_key);
        let field_kind = field();
        let field_key = module_key.child(&field_kind, "value", 0);
        let semantic = include_semantic
            .then(|| SemanticFact::Reference {
                target: anchor("shared/source", 10, ResolutionFallback::None, "value"),
                occurrences: locations(&[("shared/source", 50, 55)]),
            })
            .into_iter()
            .collect();

        FactSet::new(
            language,
            SyntaxFacts {
                entities: vec![
                    EntityFact {
                        key: module_key.clone(),
                        name: root_key.to_owned(),
                        kind: EntityKind::Module,
                        evidence: EntityEvidence {
                            path: "shared/source".to_owned(),
                            declarations: Vec::new(),
                            scope: Some(SourceRange { start: 0, end: 100 }),
                            body: Some(SourceRange { start: 0, end: 100 }),
                            type_inputs: Vec::new(),
                            type_outputs: Vec::new(),
                        },
                    },
                    EntityFact {
                        key: field_key.clone(),
                        name: "value".to_owned(),
                        kind: field_kind,
                        evidence: EntityEvidence {
                            path: "shared/source".to_owned(),
                            declarations: vec![SourceRange { start: 10, end: 15 }],
                            scope: Some(SourceRange { start: 10, end: 15 }),
                            body: None,
                            type_inputs: Vec::new(),
                            type_outputs: Vec::new(),
                        },
                    },
                ],
                relations: vec![RelationFact {
                    source: module_key,
                    target: field_key,
                    kind: RelationKind::Contains,
                    evidence: RelationEvidence {
                        observations: Vec::new(),
                    },
                }],
            },
            semantic,
        )
    }

    #[test]
    fn resolves_semantic_anchors_after_all_syntax_is_materialized() {
        let first = FactSet::new(
            Language::Python,
            SyntaxFacts {
                entities: vec![EntityFact {
                    key: EntityKey::module("caller"),
                    name: "caller".to_owned(),
                    kind: EntityKind::Module,
                    evidence: EntityEvidence {
                        path: "caller.py".to_owned(),
                        declarations: Vec::new(),
                        scope: Some(SourceRange { start: 0, end: 100 }),
                        body: Some(SourceRange { start: 0, end: 100 }),
                        type_inputs: Vec::new(),
                        type_outputs: Vec::new(),
                    },
                }],
                relations: Vec::new(),
            },
            vec![SemanticFact::Reference {
                target: anchor("target.py", 10, ResolutionFallback::None, "target"),
                occurrences: locations(&[("caller.py", 50, 55)]),
            }],
        );
        let second = FactSet::new(
            Language::Python,
            SyntaxFacts {
                entities: vec![EntityFact {
                    key: EntityKey::module("target"),
                    name: "target".to_owned(),
                    kind: field(),
                    evidence: EntityEvidence {
                        path: "target.py".to_owned(),
                        declarations: vec![SourceRange { start: 10, end: 16 }],
                        scope: Some(SourceRange { start: 10, end: 16 }),
                        body: None,
                        type_inputs: Vec::new(),
                        type_outputs: Vec::new(),
                    },
                }],
                relations: Vec::new(),
            },
            Vec::new(),
        );

        let graph =
            build_graph(vec![first, second], &[]).expect("graph construction should succeed");

        assert!(graph.edges.iter().any(|edge| {
            edge.source == NodeId(0)
                && edge.target == NodeId(1)
                && edge.kind == RelationKind::UsesValue
        }));
    }

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
        let graph = to_syntax_graph(
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
        .expect("the Rust syntax fixture must materialize");
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

    fn fixture_key(id: u32) -> EntityKey {
        EntityKey::module(&format!("e{id}"))
    }

    fn entity(
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

    fn relation(
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

    fn anchor(path: &str, offset: u32, fallback: ResolutionFallback, name: &str) -> SymbolAnchor {
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

    fn locations(values: &[(&str, u32, u32)]) -> Vec<ReportedLocation> {
        values
            .iter()
            .map(|(path, start, end)| ReportedLocation::Selected(span(path, *start, *end)))
            .collect()
    }

    fn span(path: &str, start: u32, end: u32) -> SourceSpan {
        SourceSpan {
            path: path.to_owned(),
            range: SourceRange { start, end },
        }
    }

    fn class() -> EntityKind {
        EntityKind::Type(TypeData {
            kind: TypeKind::Class,
            documentation: None,
        })
    }

    fn function() -> EntityKind {
        callable(CallableKind::Function)
    }

    fn constructor() -> EntityKind {
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

    fn field() -> EntityKind {
        EntityKind::Field(FieldData {
            declared_type: None,
        })
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

    #[test]
    fn finds_exact_declarations_and_deepest_body_owners() {
        let graph = SyntaxGraph {
            nodes: vec![
                node(0, "app.py", EntityKind::Module, Vec::new(), Some((0, 100))),
                node(1, "app.py", function(), vec![(10, 15)], Some((20, 90))),
                node(2, "app.py", function(), vec![(30, 35)], Some((40, 80))),
            ],
            syntax_edges: Vec::new(),
        };
        let lookup = SyntaxGraphIndex::new(&graph);

        assert_eq!(
            lookup.declaration_at(Language::Python, "app.py", 32),
            Some(NodeId(2))
        );
        assert_eq!(
            lookup.body_owner_at(Language::Python, "app.py", 50),
            Some(NodeId(2))
        );
        assert_eq!(
            lookup.body_owner_at(Language::Python, "app.py", 32),
            Some(NodeId(1))
        );
        assert_eq!(
            lookup.scope_owner_at(Language::Python, "app.py", 12),
            Some(NodeId(1))
        );
        assert_eq!(
            lookup.callable_or_module_body_owner_at(Language::Python, "app.py", 12),
            Some(NodeId(0))
        );
        assert_eq!(lookup.body_owner_at(Language::Python, "other.py", 50), None);
    }

    #[test]
    fn hashes_from_the_first_declaration_to_the_body_end() {
        let mut nodes = vec![node(
            0,
            "app.py",
            EntityKind::Module,
            vec![(4, 5)],
            Some((4, 13)),
        )];
        let documents = [document("zzzzPAYLOAD--")];

        hash_content(&mut nodes, &documents);

        assert_eq!(nodes[0].content_hash, Some(ContentHash::of("PAYLOAD--")));
    }

    #[test]
    fn keeps_the_hash_when_lines_above_a_node_move() {
        let mut before = vec![node(0, "app.py", function(), vec![(4, 6)], Some((4, 13)))];
        let mut after = vec![node(0, "app.py", function(), vec![(5, 7)], Some((5, 14)))];

        hash_content(&mut before, &[document("zzzzPAYLOAD--")]);
        hash_content(&mut after, &[document("wwwwwPAYLOAD--")]);

        assert_eq!(before[0].content_hash, after[0].content_hash);
    }

    #[test]
    fn changes_a_class_hash_when_a_method_body_changes() {
        fn class_hash(text: &str) -> Option<ContentHash> {
            let mut nodes = vec![node(0, "app.py", class(), Vec::new(), Some((0, 12)))];
            hash_content(&mut nodes, &[document(text)]);
            nodes[0].content_hash
        }

        assert_ne!(class_hash("class A: x=1"), class_hash("class A: x=2"));
    }

    #[test]
    fn keeps_no_hash_without_ranges_or_a_matching_document() {
        let mut nodes = vec![
            node(0, "app.py", EntityKind::Module, Vec::new(), None),
            node(1, "other.py", EntityKind::Module, Vec::new(), Some((0, 5))),
        ];

        hash_content(&mut nodes, &[document("value = 1\n")]);

        assert_eq!(nodes[0].content_hash, None);
        assert_eq!(nodes[1].content_hash, None);
    }

    fn document(content: &str) -> DocumentSnapshot {
        DocumentSnapshot {
            path: PathBuf::from("app.py"),
            content: content.to_owned(),
        }
    }

    fn node(
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
}
