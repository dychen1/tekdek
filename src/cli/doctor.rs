//! Doctor command.
//!
//! This module owns `doctor`: it checks the language server of each language
//! that this build can index, with the same profiles as `index`. Command
//! arguments are the input. One report and an exit code are the output: 1 when
//! a needed server is missing or cannot run.
//!
//! # Sections
//!
//! `Entry` parses the arguments and builds the report.
//! `Checks` finds each server and runs its version command.
//! `Report` formats the human report.

use std::{
    ffi::OsStr,
    io::Read,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use serde::Serialize;
use tekdek::analysis::{
    PipelineConfig,
    config::configured_profiles,
    detect_project_languages, find_program, find_sibling_program,
    language::{Language, LanguageProfile},
};

use super::{failure, render, split_arguments};

pub(super) const USAGE: &str = "usage: tekdek doctor [<root>] [--config <path>] [--json]
doctor checks the language server of each language that this build indexes.
With a root, it checks only the languages that have source files in the root.
It prints the version or the install command of each server, and exits with 1
when a needed server is missing or cannot run.";

const VERSION_TIMEOUT: Duration = Duration::from_secs(5);

// -----------------------------------------------------------------------------
// Entry
// -----------------------------------------------------------------------------

/// The check of one language server.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ServerCheck {
    language: String,
    status: Status,
    program: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    path: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    note: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    install: Option<&'static str>,
}

/// The state of one server. `Broken` and `Missing` fail the check.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum Status {
    Ok,
    Broken,
    Missing,
    Disabled,
    NotNeeded,
}

impl Status {
    const fn fails(self) -> bool {
        matches!(self, Self::Broken | Self::Missing)
    }

    const fn label(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Broken => "broken",
            Self::Missing => "missing",
            Self::Disabled => "disabled",
            Self::NotNeeded => "not used",
        }
    }
}

/// Returns the exit code and one report of the language server checks.
pub(super) fn run(arguments: Vec<String>, json: bool) -> (u8, String) {
    check(arguments).unwrap_or_else(|message| failure(json, &message))
}

fn check(arguments: Vec<String>) -> Result<(u8, String), String> {
    let (operands, mut options) = split_arguments(arguments.into_iter(), &["--config"])
        .map_err(|message| format!("{message}\n{USAGE}"))?;
    let json = options.remove("--json").is_some();
    let config = options.remove("--config").map(PathBuf::from);
    let root = match operands.as_slice() {
        [] => None,
        [root] => Some(PathBuf::from(root)),
        [_, extra, ..] => return Err(format!("unexpected argument: {extra}\n{USAGE}")),
    };
    if let Some(unexpected) = options.keys().next() {
        return Err(format!("unexpected argument: {unexpected}\n{USAGE}"));
    }
    let profiles = configured_profiles(config.as_deref()).map_err(|error| error.to_string())?;
    let used = match &root {
        Some(root) => {
            let mut pipeline = PipelineConfig::new(root.clone());
            pipeline.profiles.clone_from(&profiles);
            Some(detect_project_languages(&pipeline).map_err(|error| error.to_string())?)
        }
        None => None,
    };
    let checks: Vec<ServerCheck> = profiles
        .iter()
        .map(|profile| check_server(profile, used.as_deref()))
        .collect();
    let code = u8::from(checks.iter().any(|check| check.status.fails()));
    let human = describe(&checks, root.as_deref());
    let value = serde_json::json!({
        "version": env!("CARGO_PKG_VERSION"),
        "root": root,
        "languages": checks,
    });
    Ok((
        code,
        render(json, &value, human).map_err(|error| error.to_string())?,
    ))
}

// -----------------------------------------------------------------------------
// Checks
// -----------------------------------------------------------------------------

/// Checks one profile. `used` lists the languages of the root, when a root is given.
fn check_server(profile: &LanguageProfile, used: Option<&[Language]>) -> ServerCheck {
    let mut check = ServerCheck {
        language: profile.language.as_str().to_owned(),
        status: Status::Disabled,
        program: profile.server.program.clone(),
        path: None,
        version: None,
        note: None,
        install: None,
    };
    if !profile.enabled {
        return check;
    }
    if used.is_some_and(|used| !used.contains(&profile.language)) {
        check.status = Status::NotNeeded;
        return check;
    }
    let Some(path) = find_program(&profile.server.program) else {
        check.status = Status::Missing;
        check.install = Some(profile.language.server_install_command());
        return check;
    };
    match probe(&path) {
        // Report the version line.
        Probe::Version(version) => {
            check.status = Status::Ok;
            check.version = Some(version);
        }
        // Accept the server, and say why its version is unknown.
        Probe::Unverified(note) => {
            check.status = Status::Ok;
            check.note = Some(note);
        }
        // Report a server that exists but cannot run.
        Probe::Failed(note) => {
            check.status = Status::Broken;
            check.note = Some(note);
            check.install = Some(profile.language.server_install_command());
        }
    }
    check.path = Some(path);
    check
}

/// The result of one version command.
#[derive(Debug, PartialEq, Eq)]
enum Probe {
    Version(String),
    Unverified(String),
    Failed(String),
}

/// Runs the version command of one server.
///
/// A server named `<tool>-langserver` has no version option, so the probe runs
/// `<tool> --version` from the same folder: pyright installs both programs.
/// Without that program, or when the command runs longer than the timeout or
/// prints nothing, the version is unverified. A command that exits with an
/// error is a failure: the rustup proxy of a missing component does this.
fn probe(server: &Path) -> Probe {
    let stem = server
        .file_stem()
        .and_then(OsStr::to_str)
        .unwrap_or_default();
    let (command, name) = match stem.strip_suffix("-langserver") {
        Some(tool) => match find_sibling_program(server, tool) {
            Some(path) => (path, tool),
            None => {
                return Probe::Unverified(format!(
                    "no {tool} program next to the server reports its version"
                ));
            }
        },
        None => (server.to_path_buf(), stem),
    };
    let Ok(mut child) = Command::new(&command)
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    else {
        return Probe::Failed(format!("`{name} --version` cannot start"));
    };
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if started.elapsed() > VERSION_TIMEOUT => {
                let _ = child.kill();
                let _ = child.wait();
                return Probe::Unverified(format!(
                    "`{name} --version` did not finish in {} s",
                    VERSION_TIMEOUT.as_secs()
                ));
            }
            Ok(None) => thread::sleep(Duration::from_millis(20)),
            Err(_) => return Probe::Failed(format!("`{name} --version` cannot run")),
        }
    };
    let read = |stream: Option<&mut dyn Read>| {
        let mut text = String::new();
        if let Some(stream) = stream {
            let _ = stream.read_to_string(&mut text);
        }
        text.lines()
            .map(str::trim)
            .find(|line| !line.is_empty())
            .map(str::to_owned)
    };
    let output = read(child.stdout.as_mut().map(|stream| stream as &mut dyn Read));
    if !status.success() {
        let error = read(child.stderr.as_mut().map(|stream| stream as &mut dyn Read));
        return Probe::Failed(match error {
            Some(line) => format!("`{name} --version` failed: {line}"),
            None => format!("`{name} --version` failed with {status}"),
        });
    }
    output.map_or_else(
        || Probe::Unverified(format!("`{name} --version` printed nothing")),
        Probe::Version,
    )
}

// -----------------------------------------------------------------------------
// Report
// -----------------------------------------------------------------------------

/// Formats one line for each language after the tekdek version.
fn describe(checks: &[ServerCheck], root: Option<&Path>) -> String {
    let mut lines = vec![format!("tekdek {}", env!("CARGO_PKG_VERSION"))];
    for check in checks {
        let at = check
            .path
            .as_deref()
            .map_or_else(String::new, |path| format!(" at {}", path.display()));
        let detail = match check.status {
            // Name the server, its version, and where it is.
            Status::Ok => match (&check.version, &check.note) {
                (Some(version), _) => format!("{version}{at}"),
                (None, Some(note)) => format!("{}{at} (version unverified: {note})", check.program),
                (None, None) => format!("{}{at}", check.program),
            },
            // Say why the server cannot run, and give the install command.
            Status::Broken => format!(
                "{}{at} cannot run: {}. Install it with `{}`",
                check.program,
                check
                    .note
                    .as_deref()
                    .unwrap_or_default()
                    .trim_end_matches('.'),
                check.install.unwrap_or_default()
            ),
            // Give the install command.
            Status::Missing => format!(
                "{} is not on PATH: install it with `{}`",
                check.program,
                check.install.unwrap_or_default()
            ),
            // Say that the configuration turns the language off.
            Status::Disabled => "disabled in the configuration".to_owned(),
            // Say that the root has no source file of the language.
            Status::NotNeeded => format!(
                "no source files in {}",
                root.map_or_else(String::new, |root| root.display().to_string())
            ),
        };
        lines.push(format!(
            "{:<8}{:<10}{detail}",
            check.language,
            check.status.label()
        ));
    }
    if checks.iter().any(|check| check.status.fails()) {
        lines.push(format!(
            "Other install sources: {}#language-servers",
            env!("CARGO_PKG_REPOSITORY")
        ));
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use std::{fmt::Write as _, fs, path::PathBuf};

    use super::check;

    /// Writes one configuration that sets `setting` for every compiled language.
    fn config(directory: &tempfile::TempDir, setting: &str) -> PathBuf {
        let mut text = String::new();
        for language in [
            cfg!(feature = "python").then_some("python"),
            cfg!(feature = "rust").then_some("rust"),
        ]
        .into_iter()
        .flatten()
        {
            writeln!(text, "[languages.{language}]\n{setting}").expect("write text");
        }
        let path = directory.path().join("config.toml");
        fs::write(&path, text).expect("write configuration");
        path
    }

    #[test]
    fn reports_a_missing_server_with_its_install_command() {
        let directory = tempfile::tempdir().expect("configuration");
        let path = config(&directory, "program = \"tekdek-missing-server\"");

        let (code, output) = check(vec![
            "--config".to_owned(),
            path.display().to_string(),
            "--json".to_owned(),
        ])
        .expect("report");

        assert_eq!(code, 1);
        let report: serde_json::Value = serde_json::from_str(&output).expect("JSON");
        let first = &report["languages"][0];
        assert_eq!(first["status"], "missing");
        assert!(
            first["install"]
                .as_str()
                .is_some_and(|text| !text.is_empty())
        );
    }

    #[test]
    fn names_a_root_that_cannot_be_read_once() {
        let directory = tempfile::tempdir().expect("folder");
        let missing = directory.path().join("no-root").display().to_string();

        let error = check(vec![missing.clone()]).expect_err("a missing root");

        assert!(error.starts_with("cannot read the codebase: "), "{error}");
        assert_eq!(error.matches(&missing).count(), 1, "{error}");
    }

    #[test]
    fn reports_a_disabled_language_without_failing() {
        let directory = tempfile::tempdir().expect("configuration");
        let path = config(&directory, "enabled = false");

        let (code, output) =
            check(vec!["--config".to_owned(), path.display().to_string()]).expect("report");

        assert_eq!(code, 0);
        assert!(output.contains("disabled in the configuration"), "{output}");
        assert!(!output.contains("#language-servers"), "{output}");
    }

    #[test]
    fn names_other_install_sources_for_a_missing_server() {
        let directory = tempfile::tempdir().expect("configuration");
        let path = config(&directory, "program = \"tekdek-missing-server\"");

        let (code, output) =
            check(vec!["--config".to_owned(), path.display().to_string()]).expect("report");

        assert_eq!(code, 1);
        assert!(
            output.ends_with(
                "Other install sources: https://github.com/dychen1/tekdek#language-servers"
            ),
            "{output}"
        );
    }

    #[test]
    fn rejects_extra_operands_and_unknown_options() {
        assert!(check(vec!["one".to_owned(), "two".to_owned()]).is_err());
        assert!(check(vec!["--store".to_owned(), "x".to_owned()]).is_err());
    }

    /// Writes an executable shell script that prints `output` and exits with `code`.
    #[cfg(unix)]
    fn script(directory: &std::path::Path, name: &str, output: &str, code: u8) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;

        let path = directory.join(name);
        fs::write(&path, format!("#!/bin/sh\n{output}\nexit {code}\n")).expect("script");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).expect("permissions");
        path
    }

    #[cfg(unix)]
    #[test]
    fn reports_a_server_that_cannot_run_as_broken() {
        let directory = tempfile::tempdir().expect("configuration");
        let server = script(
            directory.path(),
            "rust-analyzer",
            "echo \"error: Unknown binary 'rust-analyzer'\" >&2",
            1,
        );
        let path = config(&directory, &format!("program = \"{}\"", server.display()));

        let (code, output) = check(vec![
            "--config".to_owned(),
            path.display().to_string(),
            "--json".to_owned(),
        ])
        .expect("report");

        assert_eq!(code, 1);
        let report: serde_json::Value = serde_json::from_str(&output).expect("JSON");
        let first = &report["languages"][0];
        assert_eq!(first["status"], "broken");
        assert!(
            first["note"]
                .as_str()
                .is_some_and(|note| note.contains("Unknown binary")),
            "{first}"
        );
        assert!(first["install"].is_string());
    }

    #[cfg(unix)]
    #[test]
    fn reads_the_version_of_a_server_or_of_its_sibling_program() {
        use super::{Probe, probe};

        let directory = tempfile::tempdir().expect("servers");
        let plain = script(directory.path(), "plain-server", "echo 'plain 1.2'", 0);
        assert_eq!(probe(&plain), Probe::Version("plain 1.2".to_owned()));

        let langserver = script(directory.path(), "tool-langserver", "exit 1", 1);
        assert!(matches!(probe(&langserver), Probe::Unverified(_)));
        script(directory.path(), "tool", "echo 'tool 3.4'", 0);
        assert_eq!(probe(&langserver), Probe::Version("tool 3.4".to_owned()));

        let silent = script(directory.path(), "silent", "", 0);
        assert!(matches!(probe(&silent), Probe::Unverified(_)));
    }

    #[cfg(all(unix, feature = "python", feature = "rust"))]
    #[test]
    fn checks_only_the_languages_of_a_root() {
        let directory = tempfile::tempdir().expect("configuration");
        let server = script(directory.path(), "fake-rust-server", "echo 'fake 1.0'", 0);
        let path = directory.path().join("config.toml");
        fs::write(
            &path,
            format!(
                "[languages.rust]\nprogram = \"{}\"\n[languages.python]\nprogram = \"tekdek-missing-server\"\n",
                server.display()
            ),
        )
        .expect("configuration");
        let root = directory.path().join("project");
        fs::create_dir_all(root.join("src")).expect("project");
        fs::write(root.join("src/main.rs"), "fn main() {}\n").expect("source");

        let (code, output) = check(vec![
            root.display().to_string(),
            "--config".to_owned(),
            path.display().to_string(),
        ])
        .expect("report");

        assert_eq!(code, 0, "{output}");
        assert!(
            output.contains("python  not used  no source files in"),
            "{output}"
        );
        assert!(output.contains("rust    ok        fake 1.0"), "{output}");

        let (code, _) =
            check(vec!["--config".to_owned(), path.display().to_string()]).expect("report");
        assert_eq!(code, 1);
    }
}
