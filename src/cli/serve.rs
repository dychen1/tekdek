//! Serve command.
//!
//! This module owns the arguments of `serve`. It binds the viewer server for
//! one store, prints the viewer URL, opens a browser on request, and answers
//! requests until the process ends. Command arguments are the input.

use std::{
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use tekdek::{catalog::build_catalog, serve::ViewerServer};

use super::{split_arguments, store_directory};

pub(super) const USAGE: &str =
    "usage: tekdek serve [--store <directory>] [--port <n>] [--flow <name>] [--open]
serve starts the Lift Deck on 127.0.0.1 and prints its URL. The default store
is the nearest .tekdek folder. The default port is 8000; port 0 selects a free
port. --flow prints the URL that opens one flow. --open opens the URL in a browser.";

const DEFAULT_PORT: u16 = 8000;

struct Arguments {
    store: Option<PathBuf>,
    port: u16,
    flow: Option<String>,
    open: bool,
}

/// Serves the viewer for one store until the process ends.
pub(super) fn run(raw: impl Iterator<Item = String>) -> Result<(), String> {
    let arguments = parse(raw).map_err(|message| format!("{message}\n{USAGE}"))?;
    let store = store_directory(arguments.store)?;
    let server = ViewerServer::bind(&store, arguments.port).map_err(|error| error.to_string())?;
    let url = match &arguments.flow {
        Some(flow) => flow_url(&server.url(), &store, flow)?,
        None => server.url(),
    };
    println!("{url}");
    eprintln!("Serving {}. Press Ctrl-C to stop.", store.display());
    if arguments.open
        && let Err(error) = open_browser(&url)
    {
        eprintln!("warning: cannot open a browser: {error}");
    }
    server.run();
    Ok(())
}

/// Parses the arguments after `serve`.
fn parse(raw: impl Iterator<Item = String>) -> Result<Arguments, String> {
    let (operands, mut options, _) = split_arguments(raw, &["--store", "--port", "--flow"], &[])?;
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
    let flow = options.remove("--flow");
    let open = options.remove("--open").is_some();
    if let Some(option) = options.keys().next() {
        return Err(format!("unexpected option: {option}"));
    }
    Ok(Arguments {
        store,
        port,
        flow,
        open,
    })
}

/// Returns the viewer URL that opens one flow under the graph that the store catalog lists it with.
fn flow_url(base: &str, store: &Path, flow: &str) -> Result<String, String> {
    let catalog =
        build_catalog(store).map_err(|error| format!("cannot read the store catalog: {error}"))?;
    let artifact = catalog
        .graph_of_flow(flow)
        .ok_or_else(|| format!("the store lists no flow named {flow}: run `tekdek flow list`"))?;
    Ok(viewer_url(base, artifact, flow))
}

/// Returns the viewer URL of one flow under one graph file.
fn viewer_url(base: &str, artifact: &str, flow: &str) -> String {
    format!("{base}?artifact={}&flow={}", encode(artifact), encode(flow))
}

/// Percent-encodes every byte except the unreserved URL characters.
fn encode(text: &str) -> String {
    text.bytes()
        .map(|byte| {
            if byte.is_ascii_alphanumeric() || b"-._~".contains(&byte) {
                char::from(byte).to_string()
            } else {
                format!("%{byte:02X}")
            }
        })
        .collect()
}

/// Opens the URL with the browser of the operating system.
fn open_browser(url: &str) -> std::io::Result<()> {
    let (program, arguments) = browser_command(std::env::consts::OS, url);
    Command::new(program)
        .args(arguments)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(drop)
}

/// Returns the program and the arguments that open a URL on one operating system.
///
/// Windows uses the URL protocol handler directly. `cmd /C start` would split
/// the URL at each `&` and drop the flow and the step.
fn browser_command<'a>(os: &str, url: &'a str) -> (&'static str, Vec<&'a str>) {
    match os {
        "macos" => ("open", vec![url]),
        "windows" => ("rundll32.exe", vec!["url.dll,FileProtocolHandler", url]),
        _ => ("xdg-open", vec![url]),
    }
}

#[cfg(test)]
mod tests {
    use super::{browser_command, flow_url, parse, viewer_url};

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
    fn builds_the_url_of_a_flow_and_rejects_an_unlisted_flow() {
        assert_eq!(
            viewer_url("http://127.0.0.1:1/viewer/", "my app.json", "request-flow"),
            "http://127.0.0.1:1/viewer/?artifact=my%20app.json&flow=request-flow"
        );
        let store = tempfile::tempdir().expect("store");
        std::fs::write(store.path().join("app.json"), "{}").expect("graph");
        let error = flow_url("http://127.0.0.1:1/viewer/", store.path(), "missing")
            .expect_err("unlisted flow");
        assert!(error.contains("no flow named missing"), "{error}");
    }

    #[test]
    fn opens_the_complete_url_on_each_operating_system() {
        let url = "http://127.0.0.1:1/viewer/?artifact=a.json&flow=b&step=3";
        assert_eq!(
            browser_command("windows", url),
            ("rundll32.exe", vec!["url.dll,FileProtocolHandler", url])
        );
        assert_eq!(browser_command("macos", url), ("open", vec![url]));
        assert_eq!(browser_command("linux", url), ("xdg-open", vec![url]));
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
