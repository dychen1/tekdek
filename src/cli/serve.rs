//! Serve command.
//!
//! This module owns the arguments of `serve`. It binds the viewer server for
//! one store, prints the viewer URL, opens a browser on request, and answers
//! requests until the process ends. Command arguments are the input.

use std::{
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use tekdek::{
    flow::{lineage::read_current, models::FlowId},
    serve::ViewerServer,
};
use url::Url;

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

/// Returns the viewer URL that opens one flow under the graph that its current generation binds.
fn flow_url(base: &str, store: &Path, flow: &str) -> Result<String, String> {
    let unlisted = || format!("the store lists no flow named {flow}: run `tekdek flow list`");
    let flow_id = FlowId::try_from(flow).map_err(|_| unlisted())?;
    let current = read_current(store, &flow_id)
        .map_err(|error| format!("cannot read the flow {flow}: {error}"))?
        .ok_or_else(unlisted)?;
    viewer_url(base, &current.artifact.facts.binding.graph_artifact, flow)
}

/// Returns the viewer URL of one flow under one graph file.
fn viewer_url(base: &str, artifact: &str, flow: &str) -> Result<String, String> {
    let mut url =
        Url::parse(base).map_err(|error| format!("invalid viewer URL {base}: {error}"))?;
    url.query_pairs_mut()
        .append_pair("artifact", artifact)
        .append_pair("flow", flow);
    Ok(url.into())
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
            Ok("http://127.0.0.1:1/viewer/?artifact=my+app.json&flow=request-flow".to_owned())
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
