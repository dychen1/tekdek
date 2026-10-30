//! Graph commands.
//!
//! This module owns the arguments and the output of `graph` commands: node
//! search by text and one node with its neighbors. They let a coding agent find
//! node keys without reading the graph JSON. Command arguments are the input.
//! One exit code and one output document are the output. `show` first resolves
//! the semantic edges of its node into the semantic cache of the graph.
//!
//! # Sections
//!
//! `Commands` defines the parsed input.
//! `Execution` reads the graph and formats one output document.

use std::{error::Error, path::PathBuf};

use serde::Serialize;
use tekdek::{
    catalog,
    query::{self, NodeDetail, NodeRef, SearchResult},
    store::{
        graph::{ArtifactGraph, NODE_KINDS, NodeKind, read_graph},
        semantic::read_merged_graph,
    },
};

use super::{
    Ensure, failure, index::refresh_project_graph, render, split_arguments, store_directory,
};

// -----------------------------------------------------------------------------
// Commands
// -----------------------------------------------------------------------------

pub(super) const USAGE: &str =
    "usage: tekdek graph <command> [--store <directory>] [--graph <file>] [--json]
  search <text> [--kind module|type|callable|field] [--limit <n>]
  show <key>... [--config <path>]
search lists the node keys that contain every word of the text, best match
first. show prints each node with its signature, container, members, callers,
and callees. Several keys resolve in one daemon request; JSON output then puts
the nodes in a `nodes` list. Before it reads the graph, show reindexes the project graph if the
source changed, as serve does. Then it gets the callers and callees of the node
from the language servers, through the daemon of the store, and keeps them in
<graph>.semantic.json. --config selects the language servers, as for index.
Known limit: a Rust callee in a macro or a function passed as a value shows
only after a show of that callee. The default store is the nearest .tekdek
folder, and the default graph is the default graph of the store. JSON output
names the graph file.";

const VALUE_OPTIONS: &[&str] = &["--store", "--graph", "--kind", "--limit", "--config"];
const DEFAULT_LIMIT: usize = 20;

struct Command {
    store: Option<PathBuf>,
    graph: Option<String>,
    json: bool,
    action: Action,
}

enum Action {
    Search {
        text: String,
        kind: Option<NodeKind>,
        limit: usize,
    },
    Show {
        keys: Vec<String>,
        config: Option<PathBuf>,
    },
}

impl Command {
    /// Parses the arguments after `graph` without file access.
    fn parse(arguments: impl Iterator<Item = String>) -> Result<Self, String> {
        let (operands, mut options) = split_arguments(arguments, VALUE_OPTIONS)?;
        let json = options.remove("--json").is_some();
        let store = options.remove("--store").map(PathBuf::from);
        let graph = options.remove("--graph");
        let words: Vec<_> = operands.iter().map(String::as_str).collect();
        let action = match words.as_slice() {
            // Search node keys; the text can be several operands.
            ["search", text @ ..] if !text.is_empty() => Action::Search {
                text: text.join(" "),
                kind: options
                    .remove("--kind")
                    .map(|kind| {
                        NODE_KINDS
                            .into_iter()
                            .find(|known| known.to_string() == kind)
                            .ok_or_else(|| {
                                let known: Vec<String> =
                                    NODE_KINDS.iter().map(ToString::to_string).collect();
                                format!("unknown node kind {kind}: use {}", known.join(", "))
                            })
                    })
                    .transpose()?,
                limit: options
                    .remove("--limit")
                    .map(|limit| {
                        limit
                            .parse::<usize>()
                            .ok()
                            .filter(|limit| *limit > 0)
                            .ok_or_else(|| format!("--limit needs a positive number, not {limit}"))
                    })
                    .transpose()?
                    .unwrap_or(DEFAULT_LIMIT),
            },
            // Show one or more nodes by their exact keys.
            ["show", keys @ ..] if !keys.is_empty() => Action::Show {
                keys: keys.iter().map(|key| (*key).to_owned()).collect(),
                config: options.remove("--config").map(PathBuf::from),
            },
            // Reject missing commands and extra operands.
            _ => return Err("invalid graph command or operands".to_owned()),
        };
        if let Some(option) = options.keys().next() {
            return Err(format!("unexpected option: {option}"));
        }
        Ok(Self {
            store,
            graph,
            json,
            action,
        })
    }
}

// -----------------------------------------------------------------------------
// Execution
// -----------------------------------------------------------------------------

/// Returns the exit code and one output document for a graph command.
pub(super) fn run(arguments: Vec<String>, json: bool, ensure: &Ensure<'_>) -> (u8, String) {
    let result = Command::parse(arguments.into_iter())
        .map_err(|message| format!("{message}\n{USAGE}"))
        .and_then(|command| execute(command, ensure).map_err(|error| error.to_string()));
    result.unwrap_or_else(|message| failure(json, &message))
}

/// Reads the selected graph and answers one query.
fn execute(command: Command, ensure: &Ensure<'_>) -> Result<(u8, String), Box<dyn Error>> {
    let store = store_directory(command.store)?;
    if let Action::Show { config, .. } = &command.action
        && let Some(summary) = refresh_project_graph(&store, config.as_deref())?
    {
        eprintln!("{summary}");
    }
    let file = match command.graph {
        Some(file) => file,
        None => catalog::default_artifact(&store)
            .map_err(|error| format!("cannot select a graph: {error}"))?,
    };
    let (graph, _) = read_graph(&store, &file)?;
    match command.action {
        // Rank the matching keys.
        Action::Search { text, kind, limit } => {
            let result = query::search(&graph, &text, kind, limit);
            let human = describe_search(&result, &file);
            Ok((0, render(command.json, &in_graph(&file, &result), human)?))
        }
        // Describe each node with its resolved edges, or suggest keys that share the last segment of a missing key.
        Action::Show { keys, config } => {
            if let Some(key) = keys
                .iter()
                .find(|key| !graph.nodes.iter().any(|node| node.key == **key))
            {
                return Err(missing_key(&graph, &file, key).into());
            }
            ensure(&store, &file, &keys, config.as_deref())?;
            let (graph, _) = read_merged_graph(&store, &file)?;
            let details = keys
                .iter()
                .map(|key| query::show(&graph, key).ok_or_else(|| missing_key(&graph, &file, key)))
                .collect::<Result<Vec<_>, _>>()?;
            let human = details
                .iter()
                .map(describe_node)
                .collect::<Vec<_>>()
                .join("\n\n");
            let output = match details.as_slice() {
                [detail] => render(command.json, &in_graph(&file, detail), human)?,
                _ => render(
                    command.json,
                    &in_graph(&file, &Nodes { nodes: &details }),
                    human,
                )?,
            };
            Ok((0, output))
        }
    }
}

/// The nodes of a `show` with several keys, in key order.
#[derive(Serialize)]
struct Nodes<'a> {
    nodes: &'a [NodeDetail],
}

/// One query result with the graph file that answered it.
#[derive(Serialize)]
struct InGraph<'a, T: Serialize> {
    graph: &'a str,
    #[serde(flatten)]
    result: &'a T,
}

fn in_graph<'a, T: Serialize>(graph: &'a str, result: &'a T) -> InGraph<'a, T> {
    InGraph { graph, result }
}

/// Explains a missing key and names up to five keys that the agent likely meant.
///
/// Keys that share the last segment come first. Without one, the keys of the
/// same file and the same kind follow.
fn missing_key(graph: &ArtifactGraph, file: &str, key: &str) -> String {
    let segment = key.rsplit("::").next().unwrap_or(key);
    let (kind, name) = segment.split_once(':').unwrap_or(("", segment));
    let mut close: Vec<String> = query::search(graph, name, None, 5)
        .nodes
        .into_iter()
        .map(|node| node.key)
        .collect();
    if close.is_empty() {
        let path = key.split("::").next().unwrap_or(key);
        close = graph
            .nodes
            .iter()
            .filter(|node| node.path == path && node.key != path)
            .filter(|node| kind.is_empty() || node.kind.to_string() == kind)
            .map(|node| node.key.clone())
            .take(5)
            .collect();
    }
    if close.is_empty() {
        format!("no node in {file} has the key {key}")
    } else {
        format!(
            "no node in {file} has the key {key}. Close keys: {}",
            close.join(", ")
        )
    }
}

/// Formats search matches: one key per line, with its kind and signature.
fn describe_search(result: &SearchResult, file: &str) -> String {
    let mut lines = vec![format!(
        "{} of {} matches in {file}",
        result.nodes.len(),
        result.total
    )];
    for node in &result.nodes {
        lines.push(format!("{}  ({})", node.key, node.kind));
        if let Some(signature) = &node.signature {
            lines.push(format!("  {signature}"));
        }
    }
    lines.join("\n")
}

/// Formats one node: facts, the first documentation paragraph, then neighbors.
fn describe_node(detail: &NodeDetail) -> String {
    let mut lines = vec![
        detail.key.clone(),
        format!("  {} in {} ({})", detail.kind, detail.path, detail.language),
    ];
    for text in [&detail.signature, &detail.declared_type]
        .into_iter()
        .flatten()
    {
        lines.push(format!("  {text}"));
    }
    if let Some(summary) = detail
        .documentation
        .as_deref()
        .and_then(|text| text.split("\n\n").next())
    {
        lines.push(format!("  {}", summary.replace('\n', " ")));
    }
    if let Some(container) = &detail.container {
        lines.push(format!("container: {}", container.key));
    }
    for (label, nodes) in [
        ("members", &detail.members),
        ("callers", &detail.callers),
        ("callees", &detail.callees),
    ] {
        push_list(&mut lines, label, nodes);
    }
    lines.join("\n")
}

fn push_list(lines: &mut Vec<String>, label: &str, nodes: &[NodeRef]) {
    if nodes.is_empty() {
        return;
    }
    lines.push(format!("{label} ({}):", nodes.len()));
    lines.extend(nodes.iter().map(|node| format!("  {}", node.key)));
}

// -----------------------------------------------------------------------------
// Tests
// -----------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::{cell::RefCell, fs, io, path::Path};

    use serde_json::Value;
    use tekdek::store::{
        digest_of,
        graph::RelationKind,
        semantic::{SemanticCache, SemanticEdge, write_cache},
    };

    use super::{Ensure, run};

    const MAIN: &str = "src/main.rs::callable:main";
    const START: &str = "src/main.rs::callable:start";

    fn fixture(directory: &Path) {
        let node = |id: u32, key: &str, name: &str| {
            serde_json::json!({
                "id": id, "language": "rust", "key": key, "name": name, "kind": "callable",
                "path": "src/main.rs", "origin": "syntax", "declarations": [],
                "signature": format!("fn {name}()")
            })
        };
        let graph = serde_json::json!({
            "schemaVersion": 1,
            "graph": {
                "languages": ["rust"],
                "nodes": [node(0, "src/main.rs::callable:main", "main"), node(1, "src/main.rs::callable:start", "start")],
                "edges": [{"source": 0, "target": 1, "kind": "calls", "weight": 1, "origin": "semantic"}]
            }
        });
        fs::write(
            directory.join("app.json"),
            serde_json::to_vec(&graph).expect("serialize"),
        )
        .expect("write graph");
    }

    fn execute(directory: &Path, arguments: &[&str], expected_code: u8) -> Value {
        execute_with(directory, arguments, expected_code, &|_, _, _, _| Ok(()))
    }

    fn execute_with(
        directory: &Path,
        arguments: &[&str],
        expected_code: u8,
        ensure: &Ensure<'_>,
    ) -> Value {
        let mut raw = vec!["--store".to_owned(), directory.display().to_string()];
        raw.extend(arguments.iter().map(|value| (*value).to_owned()));
        raw.push("--json".to_owned());
        let (code, output) = run(raw, true, ensure);
        assert_eq!(code, expected_code, "{arguments:?}: {output}");
        serde_json::from_str(&output).expect("one JSON document")
    }

    #[test]
    fn searches_and_shows_nodes_of_the_default_graph() {
        let directory = tempfile::tempdir().expect("store");
        fixture(directory.path());

        let found = execute(directory.path(), &["search", "start"], 0);
        assert_eq!(found["graph"], "app.json");
        assert_eq!(found["total"], 1);
        assert_eq!(found["nodes"][0]["key"], "src/main.rs::callable:start");
        assert_eq!(found["nodes"][0]["signature"], "fn start()");

        let shown = execute(
            directory.path(),
            &["show", "src/main.rs::callable:main", "--graph", "app.json"],
            0,
        );
        assert_eq!(shown["graph"], "app.json");
        assert_eq!(shown["callees"][0]["key"], "src/main.rs::callable:start");
        assert_eq!(shown["callers"], serde_json::json!([]));
    }

    #[test]
    fn shows_several_keys_with_one_resolution() {
        let directory = tempfile::tempdir().expect("store");
        fixture(directory.path());
        let asked = RefCell::new(Vec::new());

        let shown = execute_with(
            directory.path(),
            &["show", START, MAIN],
            0,
            &|_, _, keys, _| {
                asked.borrow_mut().push(keys.to_vec());
                Ok(())
            },
        );

        assert_eq!(shown["nodes"][0]["key"], START);
        assert_eq!(shown["nodes"][1]["key"], MAIN);
        assert_eq!(
            asked.into_inner(),
            [vec![START.to_owned(), MAIN.to_owned()]]
        );
    }

    #[test]
    fn resolves_the_shown_key_and_shows_the_edges_of_the_semantic_cache() {
        let directory = tempfile::tempdir().expect("store");
        fixture(directory.path());
        let asked = RefCell::new(Vec::new());
        // `start` calls `main` only in the semantic cache.
        let ensure = |store: &Path, file: &str, keys: &[String], config: Option<&Path>| {
            asked.borrow_mut().push((
                file.to_owned(),
                keys.to_vec(),
                config.map(Path::to_path_buf),
            ));
            let digest = digest_of(&fs::read(store.join(file))?);
            let edge = SemanticEdge {
                source: START.to_owned(),
                target: MAIN.to_owned(),
                kind: RelationKind::Calls,
                weight: 1,
            };
            let cache = SemanticCache {
                resolved: keys.iter().cloned().collect(),
                edges: vec![edge],
                ..SemanticCache::new(digest)
            };
            write_cache(store, file, &cache)
        };

        let shown = execute_with(
            directory.path(),
            &["show", START, "--config", "custom.toml"],
            0,
            &ensure,
        );

        assert_eq!(shown["callees"][0]["key"], MAIN);
        assert_eq!(shown["callers"][0]["key"], MAIN);
        assert_eq!(
            asked.into_inner(),
            [(
                "app.json".to_owned(),
                vec![START.to_owned()],
                Some("custom.toml".into())
            )]
        );
    }

    #[test]
    fn reports_a_failed_resolution_as_an_error() {
        let directory = tempfile::tempdir().expect("store");
        fixture(directory.path());

        let failed = execute_with(directory.path(), &["show", MAIN], 2, &|_, _, _, _| {
            Err(io::Error::other("the server failed"))
        });

        assert_eq!(failed["error"], "the server failed");
    }

    #[test]
    fn names_the_store_once_when_no_graph_can_be_selected() {
        let directory = tempfile::tempdir().expect("folder");
        let empty = directory.path().join("empty");
        fs::create_dir(&empty).expect("empty store");
        for store in [directory.path().join("no-store"), empty] {
            let failed = execute(&store, &["search", "main"], 2);
            let message = failed["error"].as_str().expect("message");
            assert_eq!(
                message.matches(&store.display().to_string()).count(),
                1,
                "{message}"
            );
        }
    }

    #[test]
    fn suggests_close_keys_for_a_missing_key() {
        let directory = tempfile::tempdir().expect("store");
        fixture(directory.path());

        let missing = execute_with(
            directory.path(),
            &["show", "src/lib.rs::callable:start"],
            2,
            &|_, _, _, _| panic!("a missing key resolves nothing"),
        );

        assert!(
            missing["error"]
                .as_str()
                .expect("message")
                .contains("Close keys: src/main.rs::callable:start")
        );
    }

    #[test]
    fn rejects_invalid_queries() {
        let directory = tempfile::tempdir().expect("store");
        fixture(directory.path());
        for arguments in [
            vec!["search"],
            vec!["search", "main", "--kind", "function"],
            vec!["search", "main", "--limit", "0"],
            vec!["show"],
            vec!["show", "a", "b"],
            vec!["list"],
            vec!["show", "main", "--input", "x"],
            vec!["search", "main", "--config", "custom.toml"],
            vec!["search", "main", "--graph", "missing.json"],
        ] {
            assert!(
                execute(directory.path(), &arguments, 2)["error"].is_string(),
                "{arguments:?}"
            );
        }
    }
}
