//! Flow test fixtures.
//!
//! This module builds a small store for flow tests: one graph with six callables
//! in four files, and one valid agent output with two clusters and six steps.

use std::path::Path;

use serde_json::{Value, json};

use super::{
    FlowError,
    import::{FrozenGeneration, ImportOutcome, ImportRequest, import_flow},
    lineage,
    models::{Author, FlowArtifact, FlowId},
};
use crate::store::graph::{
    ArtifactEdge, ArtifactGraph, ArtifactNode, GraphArtifact, NodeKind, Origin, RelationKind,
};

/// The generation digest of the fixture output frozen against the fixture graph.
pub const GOLDEN_GENERATION: &str =
    "sha256:91d34bf8f88faf1cab591cb2d41b1b3c4a911d443998b39a00d08a6ad92e5f64";

const MAIN: &str = "src/main.rs::callable:main";
const PARSE: &str = "src/parse.rs::callable:parse_args";
const READ: &str = "src/parse.rs::callable:read_input";
const WRITE: &str = "src/store.rs::callable:write_result";
const LOCK: &str = "src/store.rs::callable:lock_store";

/// Writes `graph.json` with the flow callables, one extra callable, and their calls.
pub fn write_graph(directory: &Path, extra: &str) -> GraphArtifact {
    let callables = [
        (MAIN, "main", "src/main.rs", Some("fn main()")),
        (
            PARSE,
            "parse_args",
            "src/parse.rs",
            Some("fn parse_args() -> Args"),
        ),
        (READ, "read_input", "src/parse.rs", None),
        (WRITE, "write_result", "src/store.rs", None),
        (LOCK, "lock_store", "src/store.rs", None),
        (extra, "extra", "src/extra.rs", None),
    ];
    let nodes: Vec<ArtifactNode> = callables
        .iter()
        .zip(0_u32..)
        .map(|((key, name, path, signature), id)| ArtifactNode {
            id,
            language: "rust".to_owned(),
            key: (*key).to_owned(),
            name: (*name).to_owned(),
            kind: NodeKind::Callable,
            content_hash: Some(format!("{:016x}", u64::from(id) + 1)),
            path: (*path).to_owned(),
            origin: Origin::Syntax,
            declarations: vec![],
            scope: None,
            body: None,
            signature: signature.map(str::to_owned),
            parameters: vec![],
            return_type: None,
            declared_type: None,
            documentation: None,
        })
        .collect();
    let edge = |source: u32, target: u32| ArtifactEdge {
        source,
        target,
        kind: RelationKind::Calls,
        weight: 1,
        origin: Origin::Semantic,
        observations: vec![],
    };
    let artifact = GraphArtifact {
        schema_version: 1,
        graph: ArtifactGraph {
            languages: vec!["rust".to_owned()],
            nodes,
            edges: vec![edge(0, 1), edge(1, 2), edge(0, 3), edge(3, 4), edge(0, 5)],
        },
    };
    artifact
        .write(&directory.join("graph.json"))
        .expect("write graph");
    artifact
}

/// Builds a temporary store with the fixture graph.
pub fn store() -> tempfile::TempDir {
    let directory = tempfile::tempdir().expect("store");
    write_graph(directory.path(), "src/extra.rs::callable:helper");
    directory
}

/// Returns one valid agent output for a new flow.
pub fn output() -> Value {
    let step = |node: &str, cluster: &str, title: &str, annotation: &str| {
        json!({
            "priorStepId": null, "nodeKey": node, "clusterKey": cluster,
            "title": title, "annotation": annotation
        })
    };
    json!({
        "contractVersion": 1,
        "outcome": "flow",
        "flow": {
            "behavior": "How the demo command parses input and saves a result.",
            "boundaries": "Starts at main and follows the success path only.",
            "evidenceBasis": "source_walkthrough",
            "revisionSummary": null,
            "clusters": [
                {
                    "key": "parse-input", "label": "Parse the input",
                    "description": "The command reads its arguments and the input file before any write.",
                    "phase": "input_validation", "phaseNote": null
                },
                {
                    "key": "write-result", "label": "Write the result",
                    "description": "The command stores the parsed result under the store lock.",
                    "phase": "persist", "phaseNote": null
                }
            ],
            "steps": [
                step(MAIN, "parse-input", "Start the command", "The entry point receives the arguments."),
                step(PARSE, "parse-input", "Parse the arguments", "Parse the flags into typed arguments."),
                step(READ, "parse-input", "Read the input", "Read the input file that the arguments name."),
                step(MAIN, "parse-input", "Return to main", "Control returns to main with the parsed input."),
                step(WRITE, "write-result", "Write the result", "Write the result file atomically."),
                step(LOCK, "write-result", "Lock the store", "Hold the store lock during the write.")
            ]
        }
    })
}

/// Returns the agent author that the fixture imports record.
pub fn agent() -> Author {
    Author::Agent {
        name: "claude-code".to_owned(),
        model_id: None,
    }
}

/// Imports one agent output against `graph.json`.
pub fn import(directory: &Path, name: &str, output: &Value) -> Result<ImportOutcome, FlowError> {
    let input = serde_json::to_vec(output).expect("serialize output");
    import_flow(
        directory,
        ImportRequest {
            name,
            graph_file: "graph.json",
            input: &input,
            author: agent(),
            dry_run: false,
        },
    )
}

/// Returns the frozen generation of an import, or fails the test.
pub fn frozen(outcome: Result<ImportOutcome, FlowError>) -> FrozenGeneration {
    match outcome {
        Ok(ImportOutcome::Frozen(generation)) => generation,
        Ok(ImportOutcome::NoFlow { .. }) => panic!("expected a frozen generation, got no_flow"),
        Err(error) => panic!(
            "expected a frozen generation, got {error}: {:?}",
            error.issues()
        ),
    }
}

/// Reads the current generation of one flow.
pub fn current(directory: &Path, name: &str) -> FlowArtifact {
    lineage::read_current(directory, &FlowId::try_from(name).expect("flow ID"))
        .expect("read")
        .expect("lineage")
        .artifact
}

/// Returns every stored file and its bytes, except the lock file, in path order.
pub fn snapshot_files(directory: &Path) -> Vec<(String, Vec<u8>)> {
    let mut files = Vec::new();
    let mut pending = vec![directory.to_owned()];
    while let Some(path) = pending.pop() {
        for entry in std::fs::read_dir(&path).expect("read directory") {
            let entry = entry.expect("entry");
            if entry.file_type().expect("type").is_dir() {
                pending.push(entry.path());
            } else if entry.file_name() != ".store.lock" {
                files.push((
                    entry.path().display().to_string(),
                    std::fs::read(entry.path()).expect("read"),
                ));
            }
        }
    }
    files.sort();
    files
}
