//! Command-line interface.
//!
//! This module routes one command line: `flow ...` goes to the flow commands,
//! `help` prints the usage, and every other command line indexes a codebase.
//! Command arguments and standard input are the input. The exit code is the output.

mod flow;
mod index;

use std::{io::Read, process::ExitCode};

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
        // Print both usages.
        Some("help" | "--help" | "-h") => {
            println!("{}\n\n{}", index::USAGE, flow::USAGE);
            ExitCode::SUCCESS
        }
        // Index one codebase.
        _ => index::run(arguments),
    }
}
