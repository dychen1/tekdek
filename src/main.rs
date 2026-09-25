//! Command-line entry.
//!
//! This binary passes the command line and standard input to the CLI and
//! returns its exit code. Run `tekdek help` for the usage.

use std::process::ExitCode;

mod cli;

fn main() -> ExitCode {
    cli::run(
        std::env::args().skip(1).collect(),
        &mut std::io::stdin().lock(),
    )
}
