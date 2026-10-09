//! Doctor command.
//!
//! This module owns `doctor`: it checks the language server of each language
//! that this build can index, with the same profiles as `index`. Command
//! arguments are the input. One report and an exit code are the output: 1 when
//! an enabled language has no server.

use std::process::ExitCode;

#[cfg(any(feature = "python", feature = "rust"))]
use std::{
    io::Read,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

#[cfg(any(feature = "python", feature = "rust"))]
use serde::Serialize;
#[cfg(any(feature = "python", feature = "rust"))]
use tekdek::analysis::{config::configured_profiles, find_program};

#[cfg(any(feature = "python", feature = "rust"))]
use super::{failure, render, split_arguments};

pub(super) const USAGE: &str = "usage: tekdek doctor [--config <path>] [--json]
doctor checks the language server of each language that this build indexes.
It prints the version or the install command of each server and exits with 1
when an enabled language has no server.";

#[cfg(any(feature = "python", feature = "rust"))]
const VERSION_TIMEOUT: Duration = Duration::from_secs(5);

/// The check of one language server.
#[cfg(any(feature = "python", feature = "rust"))]
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
    install: Option<&'static str>,
}

#[cfg(any(feature = "python", feature = "rust"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum Status {
    Ok,
    Missing,
    Disabled,
}

/// Checks the language servers and prints one report.
#[cfg(any(feature = "python", feature = "rust"))]
pub(super) fn run(arguments: Vec<String>) -> ExitCode {
    let json = arguments.iter().any(|argument| argument == "--json");
    let (code, output) = check(arguments).unwrap_or_else(|message| failure(json, &message));
    println!("{output}");
    ExitCode::from(code)
}

/// Reports that this build cannot index any language.
#[cfg(not(any(feature = "python", feature = "rust")))]
pub(super) fn run(_arguments: Vec<String>) -> ExitCode {
    eprintln!("error: this build has no language adapters");
    ExitCode::FAILURE
}

#[cfg(any(feature = "python", feature = "rust"))]
fn check(arguments: Vec<String>) -> Result<(u8, String), String> {
    let (operands, mut options) = split_arguments(arguments.into_iter(), &["--config"])
        .map_err(|message| format!("{message}\n{USAGE}"))?;
    let json = options.remove("--json").is_some();
    let config = options.remove("--config").map(PathBuf::from);
    if let Some(unexpected) = operands.first().or(options.keys().next()) {
        return Err(format!("unexpected argument: {unexpected}\n{USAGE}"));
    }
    let profiles = configured_profiles(config.as_deref()).map_err(|error| error.to_string())?;
    let checks: Vec<ServerCheck> = profiles
        .iter()
        .map(|profile| {
            let program = profile.server.program.clone();
            let language = profile.language.as_str().to_owned();
            if !profile.enabled {
                return ServerCheck {
                    language,
                    status: Status::Disabled,
                    program,
                    path: None,
                    version: None,
                    install: None,
                };
            }
            match find_program(&program) {
                Some(path) => ServerCheck {
                    language,
                    status: Status::Ok,
                    version: version_of(&path),
                    program,
                    path: Some(path),
                    install: None,
                },
                None => ServerCheck {
                    language,
                    status: Status::Missing,
                    program,
                    path: None,
                    version: None,
                    install: Some(profile.language.server_install_command()),
                },
            }
        })
        .collect();
    let code = u8::from(checks.iter().any(|check| check.status == Status::Missing));
    let human = describe(&checks);
    let value = serde_json::json!({"version": env!("CARGO_PKG_VERSION"), "languages": checks});
    Ok((
        code,
        render(json, &value, human).map_err(|error| error.to_string())?,
    ))
}

/// Formats one line for each language after the tekdek version.
#[cfg(any(feature = "python", feature = "rust"))]
fn describe(checks: &[ServerCheck]) -> String {
    let mut lines = vec![format!("tekdek {}", env!("CARGO_PKG_VERSION"))];
    for check in checks {
        let detail = match check.status {
            // Name the server and where it is.
            Status::Ok => {
                let path = check
                    .path
                    .as_deref()
                    .map_or_else(String::new, |path| format!(" at {}", path.display()));
                match &check.version {
                    Some(version) => format!("{version}{path}"),
                    None => format!("{}{path}", check.program),
                }
            }
            // Give the install command.
            Status::Missing => format!(
                "{} is not on PATH: install it with `{}`",
                check.program,
                check.install.unwrap_or_default()
            ),
            // Say that the configuration turns the language off.
            Status::Disabled => "disabled in the configuration".to_owned(),
        };
        let status = match check.status {
            Status::Ok => "ok",
            Status::Missing => "missing",
            Status::Disabled => "disabled",
        };
        lines.push(format!("{:<8}{status:<10}{detail}", check.language));
    }
    if checks.iter().any(|check| check.status == Status::Missing) {
        lines.push(format!(
            "Other install sources: {}#language-servers",
            env!("CARGO_PKG_REPOSITORY")
        ));
    }
    lines.join("\n")
}

/// Returns the first output line of `<program> --version`, or `None` when the
/// program fails, prints nothing, or runs longer than the timeout.
#[cfg(any(feature = "python", feature = "rust"))]
fn version_of(program: &Path) -> Option<String> {
    let mut child = Command::new(program)
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().ok()? {
            break status;
        }
        if started.elapsed() > VERSION_TIMEOUT {
            let _ = child.kill();
            let _ = child.wait();
            return None;
        }
        thread::sleep(Duration::from_millis(20));
    };
    let mut output = String::new();
    child.stdout.take()?.read_to_string(&mut output).ok()?;
    let line = output.lines().next()?.trim();
    (status.success() && !line.is_empty()).then(|| line.to_owned())
}

#[cfg(all(test, any(feature = "python", feature = "rust")))]
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
    fn rejects_operands_and_unknown_options() {
        assert!(check(vec!["extra".to_owned()]).is_err());
        assert!(check(vec!["--store".to_owned(), "x".to_owned()]).is_err());
    }
}
