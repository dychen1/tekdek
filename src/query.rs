//! Graph queries.
//!
//! This module answers the questions that a coding agent asks about one graph
//! artifact before it writes a flow: which node keys match a text, and what one
//! node declares and calls. A graph artifact is the input. Node summaries and
//! one node detail are the outputs.
//!
//! # Sections
//!
//! `Models` defines the query results.
//! `Search` ranks the nodes that match a text.
//! `Show` collects one node and its neighbors.

use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;

use crate::store::graph::{ArtifactGraph, ArtifactNode, NodeKind, RelationKind};

// -----------------------------------------------------------------------------
// Models
// -----------------------------------------------------------------------------

/// One node in a search result.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeSummary {
    pub key: String,
    pub name: String,
    pub kind: NodeKind,
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,
}

/// The ranked matches of one search. `total` counts the matches before the limit.
#[derive(Debug, Serialize)]
pub struct SearchResult {
    pub total: usize,
    pub nodes: Vec<NodeSummary>,
}

/// One node reference in a neighbor list.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct NodeRef {
    pub key: String,
    pub kind: NodeKind,
}

/// One node with its declaration facts and its static neighbors.
///
/// `callers` and `callees` come from `calls` edges only. `container` and
/// `members` come from `contains` edges.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeDetail {
    pub key: String,
    pub name: String,
    pub kind: NodeKind,
    pub language: String,
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub declared_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub documentation: Option<String>,
    pub container: Option<NodeRef>,
    pub members: Vec<NodeRef>,
    pub callers: Vec<NodeRef>,
    pub callees: Vec<NodeRef>,
}

// -----------------------------------------------------------------------------
// Search
// -----------------------------------------------------------------------------

/// Returns the nodes whose key contains every word of `text`, best match first.
///
/// Matching ignores case. Spaces, `::`, `:`, `.`, and `/` separate words, so a
/// qualified name such as `Command::parse` or a path such as `flow/import`
/// works. The name decides the rank: the exact name, then a name that starts
/// with the first word and holds every word, then a name that holds every
/// word, then a name that holds some words, then a key match only. The exact
/// name is the words joined with `_`; for a qualified or path query it is the
/// last word. A file module ranks by its file stem, not by its path. Nodes in
/// tests come after all other nodes. In each rank, callables come before
/// types, modules, and fields; the key breaks ties.
#[must_use]
pub fn search(
    graph: &ArtifactGraph,
    text: &str,
    kind: Option<NodeKind>,
    limit: usize,
) -> SearchResult {
    let needle = text.trim().to_lowercase();
    let words: Vec<&str> = needle
        .split(|character: char| character.is_whitespace() || matches!(character, ':' | '.' | '/'))
        .filter(|word| !word.is_empty())
        .collect();
    let qualified = needle.contains([':', '.', '/']);
    let exact = if qualified {
        words
            .last()
            .map(|word| (*word).to_owned())
            .unwrap_or_default()
    } else {
        words.join("_")
    };
    let mut matches: Vec<((bool, u8, u8), &ArtifactNode)> = graph
        .nodes
        .iter()
        .filter(|node| kind.is_none_or(|kind| node.kind == kind))
        .filter_map(|node| {
            let key = node.key.to_lowercase();
            if words.is_empty() || !words.iter().all(|word| key.contains(word)) {
                return None;
            }
            let name = rank_name(node).to_lowercase();
            let held = words.iter().filter(|word| name.contains(**word)).count();
            let rank = if name == exact {
                0
            } else if held == words.len() && name.starts_with(words[0]) {
                1
            } else if held == words.len() {
                2
            } else if held > 0 {
                3
            } else {
                4
            };
            Some(((is_test(&key), rank, kind_order(node.kind)), node))
        })
        .collect();
    matches.sort_by(|left, right| (left.0, &left.1.key).cmp(&(right.0, &right.1.key)));
    SearchResult {
        total: matches.len(),
        nodes: matches
            .into_iter()
            .take(limit)
            .map(|(_, node)| NodeSummary {
                key: node.key.clone(),
                name: node.name.clone(),
                kind: node.kind,
                path: node.path.clone(),
                signature: node.signature.as_deref().map(one_line),
            })
            .collect(),
    }
}

/// Returns the name that a node ranks by.
///
/// A file module has its path as its name, so it ranks by its file stem, or by
/// its folder for `mod.rs` and `__init__.py`.
fn rank_name(node: &ArtifactNode) -> &str {
    if node.kind != NodeKind::Module || node.name != node.path {
        return &node.name;
    }
    let mut parts = node.path.rsplit('/');
    let file = parts.next().unwrap_or(&node.path);
    let stem = file.rsplit_once('.').map_or(file, |(stem, _)| stem);
    if matches!(stem, "mod" | "__init__") {
        parts.next().unwrap_or(stem)
    } else {
        stem
    }
}

/// True for a node in a test module or a test file.
///
/// A test module is an inline `tests` or `test` module. A test file is in a
/// `tests`, `test`, or `__tests__` folder, or its stem is `conftest`, `test`,
/// or `tests`, starts with `test_`, or ends with `_test` or `_tests`.
fn is_test(key: &str) -> bool {
    let mut parts: Vec<&str> = key.split("::").next().unwrap_or(key).split('/').collect();
    let file = parts.pop().unwrap_or_default();
    let stem = file.rsplit_once('.').map_or(file, |(stem, _)| stem);
    key.contains("::module:tests")
        || key.contains("::module:test::")
        || key.ends_with("::module:test")
        || parts
            .iter()
            .any(|part| matches!(*part, "tests" | "test" | "__tests__"))
        || matches!(stem, "conftest" | "test" | "tests")
        || stem.starts_with("test_")
        || stem.ends_with("_test")
        || stem.ends_with("_tests")
}

fn kind_order(kind: NodeKind) -> u8 {
    match kind {
        // Rank callables first.
        NodeKind::Callable => 0,
        // Rank types after callables.
        NodeKind::Type => 1,
        // Rank modules after types.
        NodeKind::Module => 2,
        // Rank fields last.
        NodeKind::Field => 3,
    }
}

/// Joins a multi-line signature into one line.
#[must_use]
fn one_line(signature: &str) -> String {
    signature
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
        .replace("( ", "(")
        .replace(", )", ")")
        .replace(" )", ")")
}

// -----------------------------------------------------------------------------
// Show
// -----------------------------------------------------------------------------

/// Returns one node by its exact key, or `None` when no node has the key.
#[must_use]
pub fn show(graph: &ArtifactGraph, key: &str) -> Option<NodeDetail> {
    let node = graph.nodes.iter().find(|node| node.key == key)?;
    let by_id: BTreeMap<u32, &ArtifactNode> =
        graph.nodes.iter().map(|node| (node.id, node)).collect();
    let reference = |id: u32| {
        by_id.get(&id).map(|node| NodeRef {
            key: node.key.clone(),
            kind: node.kind,
        })
    };
    let mut container = None;
    let mut members = BTreeSet::new();
    let mut calling = BTreeSet::new();
    let mut called = BTreeSet::new();
    for edge in &graph.edges {
        match edge.kind {
            // Record the node that contains this node.
            RelationKind::Contains if edge.target == node.id => container = reference(edge.source),
            // Record a node that this node contains.
            RelationKind::Contains if edge.source == node.id => {
                members.extend(reference(edge.target));
            }
            // Record a caller of this node.
            RelationKind::Calls if edge.target == node.id => calling.extend(reference(edge.source)),
            // Record a node that this node calls.
            RelationKind::Calls if edge.source == node.id => called.extend(reference(edge.target)),
            // Skip every other edge.
            _ => {}
        }
    }
    Some(NodeDetail {
        key: node.key.clone(),
        name: node.name.clone(),
        kind: node.kind,
        language: node.language.clone(),
        path: node.path.clone(),
        signature: node.signature.as_deref().map(one_line),
        declared_type: node.declared_type.clone(),
        documentation: node.documentation.clone(),
        container,
        members: members.into_iter().collect(),
        callers: calling.into_iter().collect(),
        callees: called.into_iter().collect(),
    })
}

// -----------------------------------------------------------------------------
// Tests
// -----------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::{is_test, one_line, search, show};
    use crate::store::graph::{ArtifactGraph, GraphArtifact, NodeKind};

    fn graph() -> ArtifactGraph {
        let node = |id: u32, key: &str, name: &str, kind: &str| {
            serde_json::json!({
                "id": id, "language": "rust", "key": key, "name": name, "kind": kind,
                "path": key.split("::").next().expect("path"), "origin": "syntax", "declarations": []
            })
        };
        let edge = |source: u32, target: u32, kind: &str| serde_json::json!({"source": source, "target": target, "kind": kind, "weight": 1, "origin": "syntax"});
        let mut import = node(
            2,
            "src/flow/import.rs::callable:import_flow",
            "import_flow",
            "callable",
        );
        import["signature"] =
            "pub fn import_flow(\n    directory: &Path,\n) -> Result<(), Error>".into();
        let document = serde_json::json!({
            "schemaVersion": 1,
            "graph": {
                "languages": ["rust"],
                "nodes": [
                    node(0, "src/flow/import.rs", "src/flow/import.rs", "module"),
                    node(1, "src/cli/flow.rs::callable:import", "import", "callable"),
                    import,
                    node(3, "src/flow/lineage.rs::callable:write_current", "write_current", "callable"),
                    node(4, "src/flow/import.rs::type:ImportRequest", "ImportRequest", "type"),
                ],
                "edges": [
                    edge(0, 2, "contains"), edge(0, 4, "contains"),
                    edge(1, 2, "calls"), edge(2, 3, "calls"), edge(2, 4, "uses_type"),
                ]
            }
        });
        GraphArtifact::read(&serde_json::to_vec(&document).expect("serialize"))
            .expect("graph")
            .graph
    }

    #[test]
    fn ranks_exact_names_first_and_filters_by_kind_and_limit() {
        let graph = graph();

        let result = search(&graph, "IMPORT", None, 10);
        let keys: Vec<_> = result.nodes.iter().map(|node| node.key.as_str()).collect();
        assert_eq!(
            keys,
            [
                "src/cli/flow.rs::callable:import",
                "src/flow/import.rs",
                "src/flow/import.rs::callable:import_flow",
                "src/flow/import.rs::type:ImportRequest",
            ]
        );
        assert_eq!(
            result.nodes[2].signature.as_deref(),
            Some("pub fn import_flow(directory: &Path) -> Result<(), Error>")
        );

        let limited = search(&graph, "import", Some(NodeKind::Callable), 1);
        assert_eq!(limited.total, 2);
        assert_eq!(limited.nodes.len(), 1);
        assert_eq!(
            search(&graph, "import flow", None, 10).nodes[0].key,
            "src/flow/import.rs::callable:import_flow"
        );
        assert_eq!(search(&graph, "flow write", None, 10).total, 1);
        assert_eq!(search(&graph, "  ", None, 10).total, 0);
    }

    #[test]
    fn splits_qualified_and_path_queries_and_ranks_modules_by_their_file_stem() {
        let graph = graph();

        let typed = search(&graph, "import::ImportRequest", None, 10);
        assert_eq!(typed.nodes[0].key, "src/flow/import.rs::type:ImportRequest");
        let path = search(&graph, "flow/import", None, 10);
        assert_eq!(path.nodes[0].key, "src/cli/flow.rs::callable:import");
        assert_eq!(path.nodes[1].key, "src/flow/import.rs");
        let keys: Vec<_> = search(&graph, "import flow", None, 10)
            .nodes
            .into_iter()
            .map(|node| node.key)
            .collect();
        assert_eq!(
            keys[..2],
            [
                "src/flow/import.rs::callable:import_flow",
                "src/cli/flow.rs::callable:import",
            ]
        );
    }

    #[test]
    fn recognizes_test_files_and_modules_only() {
        for key in [
            "src/store/mod.rs::module:tests::callable:writes",
            "tests/cli.rs::callable:runs",
            "pkg/api_test.py::callable:test_get",
            "pkg/test_api.py",
            "pkg/conftest.py::callable:client",
        ] {
            assert!(is_test(key), "{key}");
        }
        for key in [
            "src/testing.rs::callable:helper",
            "src/contest.rs",
            "src/attest/mod.rs::callable:sign",
        ] {
            assert!(!is_test(key), "{key}");
        }
    }

    #[test]
    fn shows_a_node_with_its_container_members_callers_and_callees() {
        let graph = graph();

        let detail = show(&graph, "src/flow/import.rs::callable:import_flow").expect("node");
        assert_eq!(
            detail.container.map(|node| node.key).as_deref(),
            Some("src/flow/import.rs")
        );
        assert_eq!(detail.callers[0].key, "src/cli/flow.rs::callable:import");
        assert_eq!(
            detail.callees[0].key,
            "src/flow/lineage.rs::callable:write_current"
        );
        assert_eq!(detail.members, Vec::new());
        let module = show(&graph, "src/flow/import.rs").expect("module");
        assert_eq!(module.members.len(), 2);
        assert!(show(&graph, "src/flow/import.rs::callable:missing").is_none());
    }

    #[test]
    fn joins_a_multi_line_signature() {
        assert_eq!(
            one_line("fn run(\n    a: u8,\n    b: u8,\n) -> u8"),
            "fn run(a: u8, b: u8) -> u8"
        );
    }
}
