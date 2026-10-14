//! Graph construction.
//!
//! Builds a graph from syntax and semantic facts. This module builds the syntax
//! nodes and edges. `semantic` adds the semantic edges, and `lookup` matches
//! source positions to nodes.
//!
//! # Sections
//!
//! `Construction` assembles the graph.
//! `Syntax Materialization` adds the syntax nodes and edges.
//! `Edge Merging` joins repeated relations into one weighted edge.

use std::collections::HashMap;

use super::semantic::build_semantic_edges;
use crate::analysis::{
    fact::{
        AnalysisError, EntityFact, EntityKey, FactSet, ImportSite, RelationEvidence, RelationFact,
        RelationKind, SyntaxFacts,
    },
    graph::{Edge, Graph, Node, NodeId, Provenance, ProvenanceOrigin},
    language::Language,
    source::{ContentHash, DocumentSnapshot, SourceSpan, content_span},
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
    let mut graph = Graph {
        nodes: Vec::new(),
        edges: Vec::new(),
    };
    let mut node_ids = HashMap::<EntityKey, NodeId>::new();
    let mut syntax_edges = EdgeAccumulator::new(ProvenanceOrigin::Syntax);
    let mut semantic_batches = Vec::with_capacity(fact_sets.len());

    for FactSet {
        language,
        syntax,
        semantic,
    } in fact_sets
    {
        let imports = materialize_syntax(
            &mut graph,
            &mut node_ids,
            &mut syntax_edges,
            language,
            syntax,
        )?;
        semantic_batches.push((language, imports, semantic));
    }
    graph.edges = syntax_edges.finish()?;

    for (language, imports, semantic) in semantic_batches {
        let semantic_edges = build_semantic_edges(&graph, language, &semantic, &imports)?;
        graph.edges.extend(semantic_edges);
    }

    hash_content(&mut graph.nodes, documents);

    Ok(graph)
}

/// Fills the content hash of every node that has a derivable range.
fn hash_content(nodes: &mut [Node], documents: &[DocumentSnapshot]) {
    let documents_by_path: HashMap<&str, &DocumentSnapshot> = documents
        .iter()
        .filter_map(|document| document.path.to_str().map(|path| (path, document)))
        .collect();

    for node in nodes {
        node.content_hash = content_span(node.declarations(), node.body(), node.scope())
            .and_then(|span| documents_by_path.get(node.path())?.text(span))
            .map(ContentHash::of);
    }
}

// -----------------------------------------------------------------------------
// Syntax Materialization
// -----------------------------------------------------------------------------

/// Adds the syntax nodes and edges, and returns each import site with its
/// owner node.
fn materialize_syntax(
    graph: &mut Graph,
    node_ids: &mut HashMap<EntityKey, NodeId>,
    edges: &mut EdgeAccumulator,
    language: Language,
    facts: SyntaxFacts,
) -> Result<Vec<(NodeId, ImportSite)>, AnalysisError> {
    let SyntaxFacts {
        entities,
        relations,
        import_sites,
    } = facts;
    for EntityFact {
        key,
        name,
        kind,
        mut evidence,
    } in entities
    {
        let id = NodeId(u32::try_from(graph.nodes.len()).map_err(|_| {
            AnalysisError::new("the syntax has too many entities for u32 identities")
        })?);
        if let Some(existing) = node_ids.get(&key).copied() {
            let qualifier = if graph.nodes[existing.index()].language == language {
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

        graph.nodes.push(Node {
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
        let source = resolve_endpoint_key(graph, node_ids, language, &source)?;
        let target = resolve_endpoint_key(graph, node_ids, language, &target)?;
        edges.observe(source, target, kind, evidence.observations);
    }

    import_sites
        .into_iter()
        .map(|site| {
            Ok((
                resolve_endpoint_key(graph, node_ids, language, &site.owner)?,
                site,
            ))
        })
        .collect()
}

/// Resolves a relation endpoint or an import owner to a node.
///
/// # Errors
///
/// Returns an error if no entity fact declared that key.
fn resolve_endpoint_key(
    graph: &Graph,
    node_ids: &HashMap<EntityKey, NodeId>,
    language: Language,
    key: &EntityKey,
) -> Result<NodeId, AnalysisError> {
    let id = node_ids.get(key).copied().ok_or_else(|| {
        AnalysisError::new(format!("a syntax relation names an unknown entity: {key}"))
    })?;
    if graph.nodes[id.index()].language != language {
        return Err(AnalysisError::new(format!(
            "a syntax relation names an entity from a different language: {key}"
        )));
    }

    Ok(id)
}

// -----------------------------------------------------------------------------
// Edge Merging
// -----------------------------------------------------------------------------

/// Edges of one provenance origin, with one edge per source, target, and kind.
pub(super) struct EdgeAccumulator {
    origin: ProvenanceOrigin,
    edges: Vec<Edge>,
    indexes: HashMap<(NodeId, NodeId, RelationKind), usize>,
}

impl EdgeAccumulator {
    pub(super) fn new(origin: ProvenanceOrigin) -> Self {
        Self {
            origin,
            edges: Vec::new(),
            indexes: HashMap::new(),
        }
    }

    /// Adds one relation and its observations to the matching edge.
    pub(super) fn observe(
        &mut self,
        source: NodeId,
        target: NodeId,
        kind: RelationKind,
        observations: Vec<SourceSpan>,
    ) {
        let key = (source, target, kind);
        if let Some(index) = self.indexes.get(&key).copied() {
            let edge = &mut self.edges[index];
            edge.weight += 1;
            edge.provenance.evidence.observations.extend(observations);
            return;
        }

        self.indexes.insert(key, self.edges.len());
        self.edges.push(Edge {
            source,
            target,
            kind,
            weight: 1,
            provenance: Provenance {
                origin: self.origin,
                evidence: RelationEvidence { observations },
            },
        });
    }

    /// Returns the edges in first-observation order with final weights.
    ///
    /// The weight is the number of distinct observations. An edge without
    /// observations keeps the number of relations that it joins.
    ///
    /// # Errors
    ///
    /// Returns an error if an observation count cannot fit in an edge weight.
    pub(super) fn finish(mut self) -> Result<Vec<Edge>, AnalysisError> {
        for edge in &mut self.edges {
            edge.provenance.evidence.normalize_observations();
            let observation_count = edge.provenance.evidence.observations.len();
            if observation_count > 0 {
                edge.weight = u32::try_from(observation_count)
                    .map_err(|_| AnalysisError::new("an edge has too many source observations"))?;
            }
        }

        Ok(self.edges)
    }
}

// -----------------------------------------------------------------------------
// Tests
// -----------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::{build_graph, hash_content};
    use crate::analysis::{
        fact::{
            EntityEvidence, EntityFact, EntityKey, EntityKind, FactSet, RelationEvidence,
            RelationFact, RelationKind, ResolutionFallback, SemanticFact, SyntaxFacts,
        },
        graph::{
            NodeId,
            fixtures::{
                anchor, class, entity, field, fixture_key, function, locations, node, relation,
                to_syntax_graph,
            },
        },
        language::Language,
        source::{ContentHash, DocumentSnapshot, SourceRange},
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
                relation(1, 2, RelationKind::Contains, None),
                relation(1, 2, RelationKind::Contains, None),
            ],
            import_sites: Vec::new(),
        };

        let syntax = to_syntax_graph(Language::Python, facts).expect("the facts must be valid");

        assert_eq!(syntax.nodes.len(), 3);
        assert_eq!(syntax.nodes[2].key, fixture_key(2));
        assert_eq!(syntax.edges.len(), 2);
        assert!(matches!(syntax.edges[0].kind, RelationKind::Contains));
        assert_eq!(syntax.edges[1].source, NodeId(1));
        assert_eq!(syntax.edges[1].weight, 2);
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
                import_sites: Vec::new(),
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
                        kind: RelationKind::Contains,
                        evidence: RelationEvidence {
                            observations: Vec::new(),
                        },
                    }],
                    import_sites: Vec::new(),
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
                import_sites: Vec::new(),
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
                import_sites: Vec::new(),
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
                import_sites: Vec::new(),
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
}
