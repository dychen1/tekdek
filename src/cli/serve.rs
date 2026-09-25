//! Serve command.
//!
//! This module owns the arguments of `serve`. It binds the viewer server for
//! one store, prints the viewer URL, opens a browser on request, and answers
//! requests until the process ends. Command arguments are the input.

use std::{
    path::PathBuf,
    process::{Command, ExitCode, Stdio},
};

use tekdek::serve::ViewerServer;

use super::{split_arguments, store_directory};

pub(super) const USAGE: &str = "usage: tekdek serve [--store <directory>] [--port <n>] [--open]
serve starts the Lift Deck on 127.0.0.1 and prints its URL. The default store
is the nearest .tekdek folder. The default port is 8000; port 0 selects a free
port. --open opens the URL in a browser.";

const DEFAULT_PORT: u16 = 8000;

struct Arguments {
    store: Option<PathBuf>,
    port: u16,
    open: bool,
}

/// Serves the viewer for one store until the process ends.
pub(super) fn run(raw: impl Iterator<Item = String>) -> ExitCode {
    let arguments = match parse(raw) {
        Ok(arguments) => arguments,
        Err(message) => {
            eprintln!("error: {message}\n{USAGE}");
            return ExitCode::FAILURE;
        }
    };
    let store = match store_directory(arguments.store) {
        Ok(store) => store,
        Err(message) => {
            eprintln!("error: {message}");
            return ExitCode::FAILURE;
        }
    };
    let server = match ViewerServer::bind(&store, arguments.port) {
        Ok(server) => server,
        Err(error) => {
            eprintln!("error: {error}");
            return ExitCode::FAILURE;
        }
    };
    let url = server.url();
    println!("{url}");
    eprintln!("Serving {}. Press Ctrl-C to stop.", store.display());
    if arguments.open
        && let Err(error) = open_browser(&url)
    {
        eprintln!("warning: cannot open a browser: {error}");
    }
    server.run();
    ExitCode::SUCCESS
}

/// Parses the arguments after `serve`.
fn parse(raw: impl Iterator<Item = String>) -> Result<Arguments, String> {
    let (operands, mut options) = split_arguments(raw, &["--store", "--port"])?;
    if let Some(operand) = operands.first() {
        return Err(format!("unexpected argument: {operand}"));
    }
    let store = options.remove("--store").map(PathBuf::from);
    let port = options
        .remove("--port")
        .map(|port| {
            port.parse::<u16>()
                .map_err(|_| format!("the port must be between 0 and 65535, not {port}"))
        })
        .transpose()?
        .unwrap_or(DEFAULT_PORT);
    let open = options.remove("--open").is_some();
    if let Some(option) = options.keys().next() {
        return Err(format!("unexpected option: {option}"));
    }
    Ok(Arguments { store, port, open })
}

/// Opens the URL with the browser of the operating system.
fn open_browser(url: &str) -> std::io::Result<()> {
    let mut command = if cfg!(target_os = "macos") {
        Command::new("open")
    } else if cfg!(windows) {
        let mut command = Command::new("cmd");
        command.args(["/C", "start", ""]);
        command
    } else {
        Command::new("xdg-open")
    };
    command
        .arg(url)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(drop)
}

#[cfg(test)]
mod tests {
    use super::parse;

    fn arguments(values: &[&str]) -> Result<super::Arguments, String> {
        parse(values.iter().map(|value| (*value).to_owned()))
    }

    #[test]
    fn parses_the_store_the_port_and_the_open_flag() {
        let parsed = arguments(&["--store", "demo", "--port=0", "--open"]).expect("arguments");
        assert_eq!(parsed.store, Some(std::path::PathBuf::from("demo")));
        assert_eq!(parsed.port, 0);
        assert!(parsed.open);
        let defaults = arguments(&[]).expect("defaults");
        assert_eq!(
            (defaults.store, defaults.port, defaults.open),
            (None, 8000, false)
        );
    }

    #[test]
    fn rejects_invalid_ports_operands_and_options() {
        for values in [
            vec!["--port", "-1"],
            vec!["--port", "65536"],
            vec!["--port", "1.5"],
            vec!["--port"],
            vec!["--unknown", "value"],
            vec!["--port", "0", "--port", "0"],
            vec!["--json"],
            vec!["demo"],
        ] {
            assert!(arguments(&values).is_err(), "{values:?}");
        }
    }
}
