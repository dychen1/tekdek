//! Command-line interface.
//!
//! This module routes one command line: `flow ...` goes to the flow commands,
//! `graph ...` goes to the graph queries, `help` prints the usage, and every
//! other command line indexes a codebase. Command arguments and standard input
//! are the input. The exit code is the output.
//!
//! # Sections
//!
//! `Routing` selects the command.
//! `Arguments` holds the option parser that the document commands share.

mod flow;
mod graph;
mod index;

use std::{collections::BTreeMap, io::Read, process::ExitCode};

use serde::Serialize;

// -----------------------------------------------------------------------------
// Routing
// -----------------------------------------------------------------------------

/// Runs one command line and returns its exit code.
pub fn run(arguments: Vec<String>, input: &mut impl Read) -> ExitCode {
    let mut arguments = arguments.into_iter().peekable();
    match arguments.peek().map(String::as_str) {
        // Run one flow command; its output is one document on standard output.
        Some("flow") => {
            arguments.next();
            let (code, output) = flow::run(arguments.collect(), input);
            println!("{output}");
            ExitCode::from(code)
        }
        // Run one graph query; its output is one document on standard output.
        Some("graph") => {
            arguments.next();
            let (code, output) = graph::run(arguments.collect());
            println!("{output}");
            ExitCode::from(code)
        }
        // Print every usage.
        Some("help" | "--help" | "-h") => {
            println!("{}\n\n{}\n\n{}", index::USAGE, flow::USAGE, graph::USAGE);
            ExitCode::SUCCESS
        }
        // Index one codebase.
        _ => index::run(arguments),
    }
}

// -----------------------------------------------------------------------------
// Arguments
// -----------------------------------------------------------------------------

/// Separates options from command operands.
///
/// `--json` is a flag. Each option in `value_options` takes one value, as
/// `--option value` or `--option=value`. A separate value cannot start with a
/// hyphen, except `-` itself.
fn split_arguments(
    mut arguments: impl Iterator<Item = String>,
    value_options: &[&str],
) -> Result<(Vec<String>, BTreeMap<String, String>), String> {
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
        let value = if argument == "--json" {
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

/// Formats one failed command with exit code 2. JSON mode returns `{"error": message}`.
fn failure(json: bool, message: &str) -> (u8, String) {
    (
        2,
        if json {
            serde_json::json!({"error": message}).to_string()
        } else {
            format!("error: {message}")
        },
    )
}
