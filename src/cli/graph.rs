//! Graph commands.
//!
//! This module owns the arguments and the output of `graph` commands: node
//! search by text and one node with its neighbors. They let a coding agent find
//! node keys without reading the graph JSON. Command arguments are the input.
//! One exit code and one output document are the output.
//!
//! # Sections
//!
//! `Commands` defines the parsed input.
//! `Execution` reads the graph and formats one output document.

use std::{error::Error, path::PathBuf};

use serde::Serialize;
use tekdek::{
    catalog,
    query::{self, NODE_KINDS, NodeDetail, NodeRef, SearchResult},
    store::graph::{ArtifactGraph, read_graph},
};

use super::{failure, render, split_arguments, store_directory};

// -----------------------------------------------------------------------------
// Commands
// -----------------------------------------------------------------------------

pub(super) const USAGE: &str =
    "usage: tekdek graph <command> [--store <directory>] [--graph <file>] [--json]
  search <text> [--kind module|type|callable|field] [--limit <n>]
  show <key>
search lists the node keys that contain every word of the text, best match
first. show prints one node with its signature, container, members, callers,
and callees. The default store is the nearest .tekdek folder, and the default
graph is the default graph of the store. JSON output names the graph file.";

const VALUE_OPTIONS: &[&str] = &["--store", "--graph", "--kind", "--limit"];
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
        kind: Option<String>,
        limit: usize,
    },
    Show {
        key: String,
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
                        if NODE_KINDS.contains(&kind.as_str()) {
                            Ok(kind)
                        } else {
                            Err(format!(
                                "unknown node kind {kind}: use {}",
                                NODE_KINDS.join(", ")
                            ))
                        }
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
            // Show one node by its exact key.
            ["show", key] => Action::Show {
                key: (*key).to_owned(),
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
pub(super) fn run(arguments: Vec<String>, json: bool) -> (u8, String) {
    let result = Command::parse(arguments.into_iter())
        .map_err(|message| format!("{message}\n{USAGE}"))
        .and_then(|command| execute(command).map_err(|error| error.to_string()));
    result.unwrap_or_else(|message| failure(json, &message))
}

/// Reads the selected graph and answers one query.
fn execute(command: Command) -> Result<(u8, String), Box<dyn Error>> {
    let store = store_directory(command.store)?;
    let file = match command.graph {
        Some(file) => file,
        None => catalog::default_artifact(&store)
            .map_err(|error| format!("cannot select a graph in {}: {error}", store.display()))?,
    };
    let (graph, _) = read_graph(&store, &file)?;
    match command.action {
        // Rank the matching keys.
        Action::Search { text, kind, limit } => {
            let result = query::search(&graph, &text, kind.as_deref(), limit);
            let human = describe_search(&result, &file);
            Ok((0, render(command.json, &in_graph(&file, &result), human)?))
        }
        // Describe one node, or suggest keys that share its last segment.
        Action::Show { key } => {
            let Some(detail) = query::show(&graph, &key) else {
                return Err(missing_key(&graph, &file, &key).into());
            };
            let human = describe_node(&detail);
            Ok((0, render(command.json, &in_graph(&file, &detail), human)?))
        }
    }
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
            .filter(|node| kind.is_empty() || node.kind == kind)
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
    use std::{fs, path::Path};

    use serde_json::Value;

    use super::run;

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
        let mut raw = vec!["--store".to_owned(), directory.display().to_string()];
        raw.extend(arguments.iter().map(|value| (*value).to_owned()));
        raw.push("--json".to_owned());
        let (code, output) = run(raw, true);
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
    fn suggests_close_keys_for_a_missing_key() {
        let directory = tempfile::tempdir().expect("store");
        fixture(directory.path());

        let missing = execute(directory.path(), &["show", "src/lib.rs::callable:start"], 2);

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
            vec!["search", "main", "--graph", "missing.json"],
        ] {
            assert!(
                execute(directory.path(), &arguments, 2)["error"].is_string(),
                "{arguments:?}"
            );
        }
    }
}
