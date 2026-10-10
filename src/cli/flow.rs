//! Flow commands.
//!
//! This module owns the arguments and the output of `flow` commands. It calls
//! the flow backend. Command arguments and standard input are the inputs. One exit code and one output
//! document are the output.
//!
//! # Sections
//!
//! `Commands` defines the parsed input.
//! `Arguments` validates options without file access.
//! `Execution` calls the backend and formats one output document.

use std::{
    error::Error,
    fs,
    io::Read,
    path::{Path, PathBuf},
};

use tekdek::{
    catalog,
    flow::{
        AGENT_OUTPUT_SCHEMA, current_flow,
        import::{self, ImportOutcome, ImportRequest},
        models::{Author, FlowArtifact, FlowIssue, wire_name},
        verify,
    },
};

use super::{failure, render, required, split_arguments, store_directory};

// -----------------------------------------------------------------------------
// Commands
// -----------------------------------------------------------------------------

pub(super) const USAGE: &str = "usage: tekdek flow <command> [--store <directory>] [--json]
  list [--graph <file>]
  show <name>
  import <name> --graph <file> --input <path or -> [--check]
    [--author agent|human|system] [--author-name <name>] [--model-id <id>]
  verify <name>
  agent-contract
import freezes one agent output as a new generation; --check runs the complete
import and writes nothing. verify recomputes the current generation. agent-contract prints the agent output JSON Schema.
The default store is the nearest .tekdek folder. Common options can appear anywhere after flow.
Use --option=value for values that start with a hyphen.";

const VALUE_OPTIONS: &[&str] = &[
    "--store",
    "--graph",
    "--input",
    "--author",
    "--author-name",
    "--model-id",
];

struct Command {
    store: Option<PathBuf>,
    json: bool,
    action: Action,
}

enum Action {
    List { graph: Option<String> },
    Show { name: String },
    Import(ImportCommand),
    Verify { name: String },
    AgentContract,
}

struct ImportCommand {
    name: String,
    graph: String,
    input: PathBuf,
    author: AuthorOptions,
    check: bool,
}

struct AuthorOptions {
    kind: Option<String>,
    name: Option<String>,
    model_id: Option<String>,
}

// -----------------------------------------------------------------------------
// Arguments
// -----------------------------------------------------------------------------

impl Command {
    /// Parses the arguments after `flow` without file access.
    fn parse(arguments: impl Iterator<Item = String>) -> Result<Self, String> {
        let (operands, mut options) = split_arguments(arguments, VALUE_OPTIONS)?;
        let json = options.remove("--json").is_some();
        let store = options.remove("--store").map(PathBuf::from);
        let words: Vec<_> = operands.iter().map(String::as_str).collect();
        let action = match words.as_slice() {
            // List flows with an optional graph filter.
            ["list"] => Action::List {
                graph: options.remove("--graph"),
            },
            // Show the current generation of one flow.
            ["show", name] => Action::Show {
                name: (*name).to_owned(),
            },
            // Freeze one agent output as a new generation.
            ["import", name] => Action::Import(ImportCommand {
                name: (*name).to_owned(),
                graph: required(&mut options, "--graph")?,
                input: required(&mut options, "--input")?.into(),
                author: AuthorOptions {
                    kind: options.remove("--author"),
                    name: options.remove("--author-name"),
                    model_id: options.remove("--model-id"),
                },
                check: options.remove("--check").is_some(),
            }),
            // Verify the current generation.
            ["verify", name] => Action::Verify {
                name: (*name).to_owned(),
            },
            // Print the agent output contract.
            ["agent-contract"] => Action::AgentContract,
            // Reject missing commands and extra operands.
            _ => return Err("invalid flow command or operands".to_owned()),
        };
        if let Some(option) = options.keys().next() {
            return Err(format!("unexpected option: {option}"));
        }
        Ok(Self {
            store,
            json,
            action,
        })
    }
}

impl AuthorOptions {
    /// Builds the provenance author. The default author is `system`.
    fn resolve(self) -> Result<Author, String> {
        let kind = self.kind.as_deref().unwrap_or(if self.name.is_some() {
            "agent"
        } else {
            "system"
        });
        match (kind, self.name, self.model_id) {
            // Record a coding agent and its optional model.
            ("agent", Some(name), model_id) => Ok(Author::Agent { name, model_id }),
            // Record a person.
            ("human", Some(name), None) => Ok(Author::Human { name }),
            // Record an automated run.
            ("system", None, None) => Ok(Author::System),
            // Reject a name or model ID that does not fit the author kind.
            ("agent" | "human" | "system", _, _) => Err(
                "use --author-name with agent or human, and --model-id only with agent".to_owned(),
            ),
            // Reject an unknown author kind.
            (other, _, _) => Err(format!(
                "unknown author kind {other}: use agent, human, or system"
            )),
        }
    }
}

// -----------------------------------------------------------------------------
// Execution
// -----------------------------------------------------------------------------

/// Returns the exit code and one output document for a flow command.
pub(super) fn run(arguments: Vec<String>, input: &mut impl Read) -> (u8, String) {
    // Detect JSON mode before parsing so invalid commands also return JSON.
    let json = arguments.iter().any(|argument| argument == "--json");
    let result = Command::parse(arguments.into_iter())
        .map_err(|message| format!("{message}\n{USAGE}"))
        .and_then(|command| execute(command, input).map_err(|error| error.to_string()));
    result.unwrap_or_else(|message| failure(json, &message))
}

/// Executes one validated command through the flow backend.
fn execute(command: Command, input: &mut impl Read) -> Result<(u8, String), Box<dyn Error>> {
    let store = store_directory(command.store)?;
    let directory = store.as_path();
    let json = command.json;
    match command.action {
        // Return the flow listings without changing storage.
        Action::List { graph } => list(directory, json, graph.as_deref()),
        // Return the current generation and readable step details.
        Action::Show { name } => show(directory, json, &name),
        // Freeze one agent output and report every issue of a rejection.
        Action::Import(import_command) => import(directory, json, import_command, input),
        // Recompute the current generation without writes.
        Action::Verify { name } => verify(directory, json, &name),
        // Print the embedded contract as it is.
        Action::AgentContract => Ok((0, AGENT_OUTPUT_SCHEMA.trim_end().to_owned())),
    }
}

/// Lists flows with their freshness.
fn list(directory: &Path, json: bool, graph: Option<&str>) -> Result<(u8, String), Box<dyn Error>> {
    let listings = catalog::list_flows(directory, graph)?;
    let lines: Vec<_> = listings
        .iter()
        .map(|item| {
            format!(
                "flow {}: {} steps in {} clusters, {}\n  {}\n  {}",
                item.name,
                item.step_count,
                item.cluster_count,
                if item.stale { "stale" } else { "fresh" },
                item.behavior,
                item.file
            )
        })
        .collect();
    let human = if lines.is_empty() {
        "No flows.".to_owned()
    } else {
        lines.join("\n")
    };
    Ok((0, render(json, &listings, human)?))
}

/// Shows the current generation of one flow. JSON output is the frozen artifact.
fn show(directory: &Path, json: bool, name: &str) -> Result<(u8, String), Box<dyn Error>> {
    let current = current_flow(directory, name)?;
    let artifact = &current.artifact;
    Ok((
        0,
        render(json, artifact, describe(artifact, &current.file))?,
    ))
}

/// Formats one generation for a reader: header, clusters, then steps.
fn describe(artifact: &FlowArtifact, file: &str) -> String {
    let intent = &artifact.intent;
    let mut lines = vec![
        format!("flow {}: {}", artifact.flow_id, intent.behavior),
        format!(
            "generation {} ({}), graph {}\n  {}",
            artifact.generation.ordinal,
            wire_name(&artifact.generation.purpose),
            artifact.facts.binding.graph_artifact,
            file
        ),
    ];
    for (cluster, view) in intent.clusters.iter().zip(&artifact.view.clusters) {
        let runs: Vec<_> = view
            .visits
            .iter()
            .map(|run| {
                if run.first_step == run.last_step {
                    run.first_step.to_string()
                } else {
                    format!("{}-{}", run.first_step, run.last_step)
                }
            })
            .collect();
        lines.push(format!(
            "cluster {} ({}): {}, steps {}",
            cluster.key,
            wire_name(&view.color),
            cluster.label,
            runs.join(", ")
        ));
    }
    for (index, step) in intent.steps.iter().enumerate() {
        lines.push(format!(
            "step {} [{}] {}\n  node: {}\n  {}",
            index + 1,
            step.cluster_key,
            step.title,
            step.node_key,
            step.annotation
        ));
    }
    lines.join("\n")
}

/// Freezes or checks one agent output. A rejection prints every issue and exits with 2.
fn import(
    directory: &Path,
    json: bool,
    command: ImportCommand,
    input: &mut impl Read,
) -> Result<(u8, String), Box<dyn Error>> {
    let ImportCommand {
        name,
        graph,
        input: path,
        author,
        check,
    } = command;
    let author = author.resolve()?;
    let bytes = if path.as_os_str() == "-" {
        let mut bytes = Vec::new();
        input.read_to_end(&mut bytes)?;
        bytes
    } else {
        fs::read(&path).map_err(|error| {
            std::io::Error::new(
                error.kind(),
                format!("cannot read {}: {error}", path.display()),
            )
        })?
    };
    let request = ImportRequest {
        name: &name,
        graph_file: &graph,
        input: &bytes,
        author,
        dry_run: check,
    };
    match import::import_flow(directory, request) {
        // Report the frozen, unchanged, or checked generation.
        Ok(ImportOutcome::Frozen(generation)) => {
            let verb = match (generation.dry_run, generation.unchanged) {
                (true, true) => "check passed, would keep",
                (true, false) => "check passed, would freeze",
                (false, true) => "kept",
                (false, false) => "froze",
            };
            let mut lines = vec![format!(
                "flow {name}: {verb} generation {} ({})\n  {}\n  {}",
                generation.ordinal,
                wire_name(&generation.purpose),
                generation.generation_digest,
                generation.file
            )];
            lines.extend(generation.warnings.iter().map(|issue| format!("  {issue}")));
            Ok((0, render(json, &generation, lines.join("\n"))?))
        }
        // Report a typed answer that freezes nothing.
        Ok(ImportOutcome::NoFlow {
            reason,
            explanation,
        }) => {
            let reason = wire_name(&reason);
            let value = serde_json::json!({"name": name, "outcome": "no_flow", "reason": reason, "explanation": explanation});
            let human =
                format!("flow {name}: no flow ({reason}), nothing was written\n  {explanation}");
            Ok((1, render(json, &value, human)?))
        }
        // Print every issue of a rejected output.
        Err(error) if !error.issues().is_empty() => {
            let value = serde_json::json!({"error": error.to_string(), "issues": error.issues()});
            let mut lines = vec![format!("error: {error}")];
            lines.extend(error.issues().iter().map(|issue| format!("  {issue}")));
            Ok((2, render(json, &value, lines.join("\n"))?))
        }
        // Report an operational failure.
        Err(error) => Err(error.into()),
    }
}

/// Verifies the current generation. Issues give exit code 1.
fn verify(directory: &Path, json: bool, name: &str) -> Result<(u8, String), Box<dyn Error>> {
    let report = verify::verify_flow(directory, name)?;
    let code = u8::from(report.issues.iter().any(FlowIssue::is_error));
    let status = match (code != 0, report.stale) {
        (true, _) => "invalid",
        (false, true) => "valid, stale graph",
        (false, false) => "valid",
    };
    let mut lines = vec![format!(
        "flow {name}: generation {} ({}) {status}\n  {}",
        report.ordinal, report.generation_digest, report.file
    )];
    lines.extend(report.issues.iter().map(|issue| format!("  {issue}")));
    Ok((code, render(json, &report, lines.join("\n"))?))
}

// -----------------------------------------------------------------------------
// Tests
// -----------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::{fs, io::Cursor, path::Path};

    use serde_json::Value;

    use super::{Command, run};

    fn fixture(directory: &Path) {
        let graph = serde_json::json!({
            "schemaVersion": 1,
            "graph": {
                "languages": ["rust"],
                "nodes": [{
                    "id": 0, "language": "rust", "key": "src/main.rs::callable:entry", "name": "entry",
                    "kind": "callable", "path": "src/main.rs", "origin": "syntax", "declarations": []
                }],
                "edges": []
            }
        });
        fs::write(
            directory.join("graph.json"),
            serde_json::to_vec(&graph).expect("serialize fixture"),
        )
        .expect("write graph");
    }

    fn execute(directory: &Path, arguments: &[&str], input: &str, expected_code: u8) -> Value {
        let mut raw = vec!["--store".to_owned(), directory.display().to_string()];
        raw.extend(arguments.iter().map(|value| (*value).to_owned()));
        // A missing option value must not consume the JSON flag.
        raw.push("--json".to_owned());
        let (code, output) = run(raw, &mut Cursor::new(input));
        assert_eq!(code, expected_code, "{arguments:?}: {output}");
        serde_json::from_str(&output).expect("one JSON document")
    }

    fn output() -> Value {
        serde_json::json!({
            "contractVersion": 1, "outcome": "flow",
            "flow": {
                "behavior": "How the entry function starts the program.",
                "boundaries": null, "evidenceBasis": "indexed_facts_only", "revisionSummary": null,
                "clusters": [{
                    "key": "start-up", "label": "Start the program",
                    "description": "The entry point receives control and starts the program.",
                    "phase": "entry", "phaseNote": null
                }],
                "steps": [{
                    "priorStepId": null, "nodeKey": "src/main.rs::callable:entry", "clusterKey": "start-up",
                    "title": "Enter the program", "annotation": "Control starts here."
                }]
            }
        })
    }

    const IMPORT: [&str; 10] = [
        "import",
        "entry-flow",
        "--graph",
        "graph.json",
        "--input",
        "-",
        "--author",
        "agent",
        "--author-name",
        "claude-code",
    ];

    #[test]
    fn imports_shows_lists_verifies_and_prints_the_contract() {
        let directory = tempfile::tempdir().expect("temporary store");
        let path = directory.path();
        fixture(path);
        let mut missing = output();
        missing["flow"]["steps"][0]["nodeKey"] = "src/main.rs::callable:gone".into();
        let rejected = execute(path, &IMPORT, &missing.to_string(), 2);
        assert!(rejected["error"].is_string());
        assert_eq!(rejected["issues"][0]["code"], "unknown_node");
        assert!(!path.join("flows/entry-flow").exists());
        let mut author = IMPORT.to_vec();
        author[7] = "robot";
        assert!(execute(path, &author, &output().to_string(), 2)["error"].is_string());

        let mut check = IMPORT.to_vec();
        check.push("--check");
        let checked = execute(path, &check, &output().to_string(), 0);
        assert_eq!(checked["dryRun"], true);
        assert_eq!(checked["ordinal"], 1);
        assert!(!path.join("flows").exists());
        assert!(!path.join("index.json").exists());

        let frozen = execute(path, &IMPORT, &output().to_string(), 0);
        assert_eq!(frozen["dryRun"], false);
        assert_eq!(frozen["generationDigest"], checked["generationDigest"]);
        assert_eq!(frozen["file"], checked["file"]);
        assert_eq!(frozen["purpose"], "initial");
        assert_eq!(frozen["ordinal"], 1);
        assert!(!path.join("index.json").exists());
        assert_eq!(
            execute(path, &["verify", "entry-flow"], "", 0)["issues"],
            serde_json::json!([])
        );
        let shown = execute(path, &["show", "entry-flow"], "", 0);
        assert_eq!(shown["intent"]["steps"][0]["title"], "Enter the program");
        let listed = execute(path, &["list"], "", 0);
        assert_eq!(listed[0]["schemaVersion"], 3);
        assert_eq!(listed[0]["clusterCount"], 1);
        assert_eq!(listed[0]["stale"], false);
        assert_eq!(
            execute(path, &["agent-contract"], "", 0)["title"],
            "AgentFlowOutput"
        );
        let repeated = execute(path, &IMPORT, &output().to_string(), 2);
        assert_eq!(repeated["issues"][0]["code"], "revision_summary_required");

        let file = path
            .join("flows")
            .join(frozen["file"].as_str().expect("file"));
        let text = fs::read_to_string(&file).expect("generation");
        fs::write(
            &file,
            text.replacen("Enter the program", "Enter the main program", 1),
        )
        .expect("edit generation");
        let tampered = execute(path, &["verify", "entry-flow"], "", 1);
        assert_eq!(tampered["issues"][0]["code"], "digest_mismatch");
    }

    #[test]
    fn rejects_invalid_commands_and_options_with_json_errors() {
        let directory = tempfile::tempdir().expect("temporary store");
        let path = directory.path();
        fixture(path);
        for arguments in [
            vec![],
            vec!["create", "demo", "--graph", "graph.json"],
            vec!["import", "demo", "--input", "-"],
            vec!["import", "demo", "--graph", "graph.json", "--input"],
            vec!["list", "--unknown"],
            vec!["list", "--graph", "a.json", "--graph=b.json"],
            vec!["verify", "demo", "extra"],
            vec!["show", "missing-flow"],
            vec!["list", "--json=true"],
        ] {
            assert!(
                execute(path, &arguments, "", 2)["error"].is_string(),
                "{arguments:?}"
            );
        }
        let command = Command::parse(
            ["import", "demo", "--graph=-weird.json", "--input", "-"]
                .into_iter()
                .map(str::to_owned),
        )
        .expect("inline values");
        assert!(
            matches!(command.action, super::Action::Import(super::ImportCommand { graph, .. }) if graph == "-weird.json")
        );
    }

    #[test]
    fn reads_the_agent_output_from_a_file() {
        let directory = tempfile::tempdir().expect("temporary store");
        let path = directory.path();
        fixture(path);
        let input = path.join("output.json");
        fs::write(&input, output().to_string()).expect("write output");
        let input = input.to_str().expect("path");

        let frozen = execute(
            path,
            &[
                "import",
                "entry-flow",
                "--graph",
                "graph.json",
                "--input",
                input,
            ],
            "",
            0,
        );

        assert_eq!(frozen["name"], "entry-flow");
    }
}
