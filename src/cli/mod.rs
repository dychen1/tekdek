//! Command-line interface.
//!
//! This module routes one command line to `index`, `graph`, `flow`, `serve`,
//! or `doctor`, and prints the usage for `help`. Command arguments and standard
//! input are the input. The exit code is the output.
//!
//! Every command uses the same exit codes: 0 for success, 1 when a check finds
//! a problem, and 2 for an error. In JSON mode, every result and every error is
//! one document on standard output. Otherwise, errors go to standard error.
//!
//! # Sections
//!
//! `Routing` selects the command.
//! `Arguments` holds the option parser and the store rule that the commands share.

mod doctor;
mod flow;
mod graph;
mod index;
mod serve;

use std::{
    collections::BTreeMap,
    env, io,
    io::Read,
    path::{Path, PathBuf},
    process::ExitCode,
};

use serde::Serialize;
use tekdek::store::{PROJECT_STORE, find_project_store};

// -----------------------------------------------------------------------------
// Routing
// -----------------------------------------------------------------------------

const SUMMARY: &str = "usage: tekdek <command>
  index    Index a codebase into its .tekdek store.
  graph    Search the nodes of a graph and show one node.
  flow     Import, check, verify, show, and list frozen flows.
  serve    Serve the Lift Deck for a store.
  doctor   Check the language servers.
  help     Print the usage of every command.
  version  Print the version.
Exit codes: 0 success, 1 a check found a problem, 2 an error. With --json, the
result or the error is one JSON document on standard output; otherwise errors
go to standard error.";

/// The exit code of every failed command.
const ERROR_CODE: u8 = 2;

/// Runs one command line and returns its exit code.
pub fn run(arguments: Vec<String>, input: &mut impl Read) -> ExitCode {
    let mut arguments = arguments.into_iter();
    let command = arguments.next();
    let arguments: Vec<String> = arguments.collect();
    // Detect JSON mode before parsing so invalid commands also return JSON.
    let json = arguments.iter().any(|argument| argument == "--json");
    let report = |(code, output): (u8, String)| emit(json, code, &output);
    match command.as_deref() {
        // Index one codebase.
        Some("index") => finish(index::run(arguments.into_iter())),
        // Run one graph query; its output is one document on standard output.
        Some("graph") => report(graph::run(arguments, json, &tekdek::semantic::ensure)),
        // Run one flow command; its output is one document on standard output.
        Some("flow") => report(flow::run(arguments, json, input, &tekdek::semantic::ensure)),
        // Serve the viewer until the process ends.
        Some("serve") => finish(serve::run(arguments.into_iter())),
        // Check the language servers; the report is one document on standard output.
        Some("doctor") => report(doctor::run(arguments, json)),
        // Print every usage.
        Some("help" | "--help" | "-h") => {
            println!(
                "{SUMMARY}\n\n{}\n\n{}\n\n{}\n\n{}\n\n{}",
                index::USAGE,
                graph::USAGE,
                flow::USAGE,
                serve::USAGE,
                doctor::USAGE
            );
            ExitCode::SUCCESS
        }
        // Serve the semantic requests of one store; clients start it, so the summary does not list it.
        #[cfg(unix)]
        Some("daemon") => finish(daemon(arguments)),
        // Print the version.
        Some("version" | "--version" | "-V") => {
            println!("tekdek {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        // Reject an unknown command.
        Some(command) => emit(
            false,
            ERROR_CODE,
            &format!("error: unknown command: {command}\n{SUMMARY}"),
        ),
        // Print the command list.
        None => emit(false, ERROR_CODE, SUMMARY),
    }
}

// -----------------------------------------------------------------------------
// Arguments
// -----------------------------------------------------------------------------

/// The options that take no value. Each command removes the flags it accepts.
const FLAGS: [&str; 4] = ["--json", "--check", "--open", "--no-index"];

/// The operands and the options of one command line.
type SplitArguments = (Vec<String>, BTreeMap<String, String>);

/// Separates options from command operands.
///
/// Each option in `FLAGS` takes no value. Each option in `value_options` takes
/// one value, as `--option value` or `--option=value`. A separate value cannot
/// start with a hyphen, except `-` itself. An option cannot repeat.
fn split_arguments(
    mut arguments: impl Iterator<Item = String>,
    value_options: &[&str],
) -> Result<SplitArguments, String> {
    let mut operands = Vec::new();
    let mut options = BTreeMap::new();
    while let Some(argument) = arguments.next() {
        if !argument.starts_with('-') {
            if argument.trim().is_empty() {
                return Err("a command operand cannot be empty".to_owned());
            }
            operands.push(argument);
            continue;
        }
        let (argument, inline) = argument
            .split_once('=')
            .map_or((argument.as_str(), None), |(option, value)| {
                (option, Some(value))
            });
        if options.contains_key(argument) {
            return Err(format!("duplicate option: {argument}"));
        }
        let value = if FLAGS.contains(&argument) {
            if inline.is_some() {
                return Err(format!("{argument} does not accept a value"));
            }
            String::new()
        } else if value_options.contains(&argument) {
            let value = if let Some(value) = inline {
                value.to_owned()
            } else {
                let value = arguments
                    .next()
                    .ok_or_else(|| format!("{argument} needs a value"))?;
                if value.starts_with("--") || (value.starts_with('-') && value != "-") {
                    return Err(format!("{argument} needs a value, not an option"));
                }
                value
            };
            if value.trim().is_empty() {
                return Err(format!("{argument} needs a non-empty value"));
            }
            value
        } else {
            return Err(format!("unknown option: {argument}"));
        };
        options.insert(argument.to_owned(), value);
    }
    Ok((operands, options))
}

/// Returns the store of a command: the `--store` value, or the nearest project store.
///
/// The search starts in the working directory and moves up through its parents.
fn store_directory(explicit: Option<PathBuf>) -> Result<PathBuf, String> {
    if let Some(store) = explicit {
        return Ok(store);
    }
    let start = env::current_dir()
        .map_err(|error| format!("cannot read the working directory: {error}"))?;
    find_project_store(&start).ok_or_else(|| {
        format!(
            "no {PROJECT_STORE} store in {} or its parents: run `tekdek index` in the project root, or pass --store <directory>",
            start.display()
        )
    })
}

/// Runs the semantic daemon of the `--store` folder until it is idle.
#[cfg(unix)]
fn daemon(arguments: Vec<String>) -> Result<(), String> {
    let (operands, mut options) = split_arguments(arguments.into_iter(), &["--store"])?;
    let store = required(&mut options, "--store")?;
    if !operands.is_empty() {
        return Err("usage: tekdek daemon --store <directory>".to_owned());
    }
    tekdek::semantic::daemon::serve(store.as_ref(), |config| {
        tekdek::analysis::config::configured_profiles(config).map_err(std::io::Error::other)
    })
    .map_err(|error| error.to_string())
}

/// Resolves the semantic edges of node keys into the cache of one graph, as `tekdek::semantic::ensure` does.
type Ensure<'a> = dyn Fn(&Path, &str, &[String], Option<&Path>) -> io::Result<()> + 'a;

/// Removes one required option value.
fn required(options: &mut BTreeMap<String, String>, key: &str) -> Result<String, String> {
    options
        .remove(key)
        .ok_or_else(|| format!("the command needs {key} <value>"))
}

/// Formats one successful command result.
fn render(json: bool, value: &impl Serialize, human: String) -> Result<String, serde_json::Error> {
    if json {
        serde_json::to_string(value)
    } else {
        Ok(human)
    }
}

/// Prints one command output and returns its exit code.
///
/// The output of an error goes to standard error, except in JSON mode, where
/// every output is one document on standard output.
fn emit(json: bool, code: u8, output: &str) -> ExitCode {
    if code == ERROR_CODE && !json {
        eprintln!("{output}");
    } else {
        println!("{output}");
    }
    ExitCode::from(code)
}

/// Prints the error of a failed `index` or `serve` command and returns its exit code.
fn finish(result: Result<(), String>) -> ExitCode {
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => emit(false, ERROR_CODE, &format!("error: {message}")),
    }
}

/// Formats one failed command with exit code 2. JSON mode returns `{"error": message}`.
fn failure(json: bool, message: &str) -> (u8, String) {
    (
        ERROR_CODE,
        if json {
            serde_json::json!({"error": message}).to_string()
        } else {
            format!("error: {message}")
        },
    )
}

// -----------------------------------------------------------------------------
// Tests
// -----------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    #[test]
    fn the_plugin_version_matches_the_crate_version() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/plugin/.claude-plugin/plugin.json"
        );
        let manifest: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(path).expect("plugin manifest"))
                .expect("plugin manifest JSON");

        assert_eq!(manifest["version"], env!("CARGO_PKG_VERSION"));
    }

    #[test]
    fn the_skills_require_the_crate_version() {
        let requirement = format!("needs tekdek {} or later", env!("CARGO_PKG_VERSION"));
        for skill in ["flow", "flow-view"] {
            let path = format!(
                "{}/plugin/skills/{skill}/SKILL.md",
                env!("CARGO_MANIFEST_DIR")
            );
            let text = std::fs::read_to_string(&path).expect("skill file");

            assert!(
                text.contains(&requirement),
                "{path} must say: {requirement}"
            );
        }
    }
}
