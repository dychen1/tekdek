//! Diff commands.
//!
//! This module owns the arguments and the output of `diff` commands. It calls
//! the diff backend. Command arguments and standard input are the inputs. One
//! exit code and one output document are the output.

use std::{
    error::Error,
    fs,
    io::Read,
    path::{Path, PathBuf},
    process::Command as Process,
};

use tekdek::diff::{
    self, AGENT_DIFF_SCHEMA, DiffError, ImportRequest,
    models::{ChangeKind, DiffArtifact},
};

use super::{failure, render, required, split_arguments, store_directory};

pub(super) const USAGE: &str = "usage: tekdek diff <command> [--store <directory>] [--json]
  prepare [--base <commit or branch>]
  import <name> --base <commit or branch> --before <flow> --after <flow>
    --input <path or -> [--check] [--author agent|human|system]
    [--author-name <name>] [--model-id <id>]
  verify <name>
  show <name>
  list
  agent-contract
prepare checks out the base commit in a temporary worktree, indexes it, and
lists the flows that fit the base. The default base is the merge-base of HEAD
and the default branch. Pass --store <base store> to graph and flow commands to
read or write the base. import checks the agent diff document and freezes the
diff in diffs/<name>.json; --check writes nothing. The Before flow comes from
the base store, or else from this store. The After flow comes from this store
and must be fresh. A freeze removes the base worktree. verify derives the diff
again from git. agent-contract prints the agent diff JSON Schema.";

const VALUE_OPTIONS: &[&str] = &[
    "--store",
    "--base",
    "--before",
    "--after",
    "--input",
    "--author",
    "--author-name",
    "--model-id",
];

enum Action {
    Prepare { base: Option<String> },
    Import(ImportCommand),
    Verify { name: String },
    Show { name: String },
    List,
    AgentContract,
}

struct ImportCommand {
    name: String,
    base: String,
    before: String,
    after: String,
    input: PathBuf,
    author: super::flow::AuthorOptions,
    check: bool,
}

/// Returns the exit code and one output document for a diff command.
pub(super) fn run(arguments: Vec<String>, json: bool, input: &mut impl Read) -> (u8, String) {
    let result = parse(arguments.into_iter())
        .map_err(|message| format!("{message}\n{USAGE}"))
        .and_then(|(store, action)| {
            execute(store, json, action, input).map_err(|error| {
                match error.downcast::<DiffError>() {
                    Ok(error) if !error.issues().is_empty() => rejection(json, &error),
                    Ok(error) => error.to_string(),
                    Err(error) => error.to_string(),
                }
            })
        });
    result.unwrap_or_else(|message| {
        if message.starts_with('{') {
            (2, message)
        } else {
            failure(json, &message)
        }
    })
}

/// Formats a rejection with every issue. JSON output is one document.
fn rejection(json: bool, error: &DiffError) -> String {
    if json {
        serde_json::json!({"error": error.to_string(), "issues": error.issues()}).to_string()
    } else {
        let lines: Vec<_> = error
            .issues()
            .iter()
            .map(|issue| format!("  {} {}", issue.path, issue.message))
            .collect();
        format!("{error}\n{}", lines.join("\n"))
    }
}

/// Parses the arguments after `diff` without file access.
fn parse(arguments: impl Iterator<Item = String>) -> Result<(Option<PathBuf>, Action), String> {
    let (operands, mut options) = split_arguments(arguments, VALUE_OPTIONS)?;
    options.remove("--json");
    let store = options.remove("--store").map(PathBuf::from);
    let words: Vec<_> = operands.iter().map(String::as_str).collect();
    let action = match words.as_slice() {
        // Check out and index the base.
        ["prepare"] => Action::Prepare {
            base: options.remove("--base"),
        },
        // Freeze one agent diff document.
        ["import", name] => Action::Import(ImportCommand {
            name: (*name).to_owned(),
            base: required(&mut options, "--base")?,
            before: required(&mut options, "--before")?,
            after: required(&mut options, "--after")?,
            input: required(&mut options, "--input")?.into(),
            author: super::flow::AuthorOptions {
                kind: options.remove("--author"),
                name: options.remove("--author-name"),
                model_id: options.remove("--model-id"),
            },
            check: options.remove("--check").is_some(),
        }),
        // Derive one diff again from git.
        ["verify", name] => Action::Verify {
            name: (*name).to_owned(),
        },
        // Show one diff.
        ["show", name] => Action::Show {
            name: (*name).to_owned(),
        },
        // List the diffs.
        ["list"] => Action::List,
        // Print the agent diff contract.
        ["agent-contract"] => Action::AgentContract,
        // Reject missing commands and extra operands.
        _ => return Err("invalid diff command or operands".to_owned()),
    };
    if let Some(option) = options.keys().next() {
        return Err(format!("unexpected option: {option}"));
    }
    Ok((store, action))
}

fn execute(
    store: Option<PathBuf>,
    json: bool,
    action: Action,
    input: &mut impl Read,
) -> Result<(u8, String), Box<dyn Error>> {
    let store = store_directory(store)?;
    let store = store.as_path();
    match action {
        // Check out and index the base, then list the flows that fit it.
        Action::Prepare { base } => prepare(store, json, base.as_deref()),
        // Freeze or check one agent document.
        Action::Import(command) => import(store, json, command, input),
        // Derive one diff again; a problem exits with 1.
        Action::Verify { name } => verify(store, json, &name),
        // Show one diff. JSON output is the frozen artifact.
        Action::Show { name } => {
            let artifact = diff::read_diff(store, &name)?;
            Ok((0, render(json, &artifact, describe(&artifact))?))
        }
        // List the diffs.
        Action::List => list(store, json),
        // Print the embedded contract as it is.
        Action::AgentContract => Ok((0, AGENT_DIFF_SCHEMA.trim_end().to_owned())),
    }
}

/// Checks out and indexes the base, then lists the flows that fit it.
fn prepare(store: &Path, json: bool, base: Option<&str>) -> Result<(u8, String), Box<dyn Error>> {
    let prepared = diff::prepare(store, base, &index_base)?;
    let fits: Vec<_> = prepared
        .fitting_flows
        .iter()
        .map(|flow| {
            format!(
                "  {} ({} steps) in {}",
                flow.name,
                flow.steps,
                flow.store.display()
            )
        })
        .collect();
    let human = format!(
        "base {}\nhead {}\nbase store {}\n{}",
        prepared.base_commit,
        prepared.head_commit,
        prepared.base_store.display(),
        if fits.is_empty() {
            "No flow fits the base: write the Before flow in the base store.".to_owned()
        } else {
            format!("Flows that fit the base:\n{}", fits.join("\n"))
        }
    );
    Ok((0, render(json, &prepared, human)?))
}

/// Freezes or checks one agent document.
fn import(
    store: &Path,
    json: bool,
    command: ImportCommand,
    input: &mut impl Read,
) -> Result<(u8, String), Box<dyn Error>> {
    let bytes = if command.input.as_os_str() == "-" {
        let mut bytes = Vec::new();
        input.read_to_end(&mut bytes)?;
        bytes
    } else {
        fs::read(&command.input)
            .map_err(|error| format!("cannot read {}: {error}", command.input.display()))?
    };
    let frozen = diff::import(
        store,
        &ImportRequest {
            name: &command.name,
            base: &command.base,
            before: &command.before,
            after: &command.after,
            input: &bytes,
            author: command.author.resolve()?,
            dry_run: command.check,
        },
    )?;
    let counts: Vec<_> = frozen
        .counts
        .iter()
        .map(|(kind, count)| format!("{count} {kind}"))
        .collect();
    let human = format!(
        "{} diff {}: {} · +{} -{}\n  {}",
        if frozen.dry_run { "checked" } else { "froze" },
        frozen.name,
        counts.join(" · "),
        frozen.added,
        frozen.removed,
        frozen.file
    );
    Ok((0, render(json, &frozen, human)?))
}

/// Derives one diff again. A problem exits with 1.
fn verify(store: &Path, json: bool, name: &str) -> Result<(u8, String), Box<dyn Error>> {
    let report = diff::verify(store, name)?;
    let human = if report.valid {
        format!("diff {name}: valid")
    } else {
        let lines: Vec<_> = report
            .issues
            .iter()
            .map(|issue| format!("  {} {}", issue.path, issue.message))
            .collect();
        let state = if report.verifiable {
            "invalid"
        } else {
            "unverifiable"
        };
        format!("diff {name}: {state}\n{}", lines.join("\n"))
    };
    Ok((u8::from(!report.valid), render(json, &report, human)?))
}

/// Lists the diffs.
fn list(store: &Path, json: bool) -> Result<(u8, String), Box<dyn Error>> {
    let listings = diff::list_diffs(store)?;
    let lines: Vec<_> = listings
        .iter()
        .map(|item| {
            format!(
                "diff {}: {}..{}\n  {}",
                item.name,
                short(&item.base_commit),
                short(&item.head_commit),
                item.behavior
            )
        })
        .collect();
    let human = if lines.is_empty() {
        "No diffs.".to_owned()
    } else {
        lines.join("\n")
    };
    Ok((0, render(json, &listings, human)?))
}

fn short(commit: &str) -> &str {
    &commit[..commit.len().min(12)]
}

/// Formats one diff for a reader: the commits, then one line per row.
fn describe(artifact: &DiffArtifact) -> String {
    let (before, after) = (
        &artifact.base.flow.intent.steps,
        &artifact.head.flow.intent.steps,
    );
    let title = |steps: &[tekdek::flow::models::FlowStep],
                 id: &Option<tekdek::flow::models::StepId>| {
        id.as_ref()
            .and_then(|id| steps.iter().position(|step| &step.id == id))
            .map(|index| format!("{} {}", index + 1, steps[index].title))
    };
    let mut lines = vec![format!(
        "diff {}: {} steps became {}, {}..{}",
        artifact.name,
        before.len(),
        after.len(),
        short(&artifact.base.commit),
        short(&artifact.head.commit)
    )];
    for row in &artifact.rows {
        let kind = match row.kind {
            ChangeKind::New => "new",
            ChangeKind::Removed => "removed",
            ChangeKind::Changed => "changed",
            ChangeKind::Moved => "moved",
            ChangeKind::Unchanged => "unchanged",
        };
        let step = title(after, &row.after_step)
            .map(|text| format!("after {text}"))
            .or_else(|| title(before, &row.before_step).map(|text| format!("before {text}")))
            .unwrap_or_default();
        lines.push(format!("{kind:9} +{} -{} {step}", row.added, row.removed));
    }
    lines.join("\n")
}

/// Indexes the base project folder with the tekdek program that runs this command.
fn index_base(root: &Path, name: &str) -> Result<(), String> {
    let program =
        std::env::current_exe().map_err(|error| format!("cannot find tekdek: {error}"))?;
    let output = Process::new(program)
        .arg("index")
        .arg(root)
        .args(["--name", name])
        .output()
        .map_err(|error| format!("cannot run tekdek index: {error}"))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(format!(
            "cannot index the base: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    }
}
