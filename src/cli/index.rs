//! Index command.
//!
//! This module owns the default command: it analyzes one codebase root, writes
//! the graph artifact, and refreshes the catalog of the store. Command
//! arguments are the input. One graph file and a status line are the output.
//!
//! Use `tekdek <root>` with optional artifact, store,
//! configuration, and language server overrides.
//!
//! # Sections
//!
//! `Entry` runs the analysis and reports the result.
//! `Arguments` parses the command line.
//! `Artifact destination` selects indexed or explicit output.

use std::process::ExitCode;

#[cfg(any(feature = "python", feature = "rust"))]
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};
#[cfg(any(feature = "python", feature = "rust"))]
use tekdek::{
    analysis::{
        self, PipelineConfig,
        config::{ConfigError, apply_cli_overrides, configured_profiles},
        language::Language,
    },
    catalog::write_artifact_index,
};

// -----------------------------------------------------------------------------
// Entry
// -----------------------------------------------------------------------------

pub(super) const USAGE: &str = "usage: tekdek <root>
  [--name <name>] [--store <directory>] | [--output <path>]
  [--config <path>]
  [--server <language>=<program>]
  [--server-arg <language>=<argument>]...
The default store is artifacts. --name and --store write <store>/<name>.json and
refresh <store>/index.json. --output writes one file and leaves every index as it is.";

/// Analyzes one codebase, writes its graph, and refreshes the catalog.
#[cfg(any(feature = "python", feature = "rust"))]
pub(super) fn run(raw: impl Iterator<Item = String>) -> ExitCode {
    let arguments = match Arguments::parse(raw) {
        Ok(arguments) => arguments,
        Err(message) => {
            eprintln!("error: {message}");
            eprintln!("{USAGE}");
            return ExitCode::FAILURE;
        }
    };
    let destination = match artifact_destination(&arguments) {
        Ok(destination) => destination,
        Err(message) => {
            eprintln!("error: {message}");
            return ExitCode::FAILURE;
        }
    };
    let config = match pipeline_config(&arguments) {
        Ok(config) => config,
        Err(error) => {
            eprintln!("error: {error}");
            return ExitCode::FAILURE;
        }
    };
    let result = match analysis::run_and_write(&config, &destination.output) {
        Ok(result) => result,
        Err(error) => {
            eprintln!("error: {error}");
            return ExitCode::FAILURE;
        }
    };
    if let Some((store, file)) = &destination.indexed
        && let Err(error) = write_artifact_index(store, file)
    {
        eprintln!("error: cannot update the artifact index: {error}");
        return ExitCode::FAILURE;
    }

    println!(
        "{} nodes, {} edges -> {}",
        result.graph.nodes.len(),
        result.graph.edges.len(),
        destination.output.display()
    );
    ExitCode::SUCCESS
}

/// Reports that this build cannot analyze any language.
#[cfg(not(any(feature = "python", feature = "rust")))]
pub(super) fn run(_raw: impl Iterator<Item = String>) -> ExitCode {
    eprintln!("error: this build has no language adapters");
    ExitCode::FAILURE
}

/// Builds pipeline configuration from files and command-line overrides.
///
/// # Errors
///
/// Returns an error if configuration loading or override application fails.
#[cfg(any(feature = "python", feature = "rust"))]
fn pipeline_config(arguments: &Arguments) -> Result<PipelineConfig, ConfigError> {
    let mut profiles = configured_profiles(arguments.config.as_deref())?;
    apply_cli_overrides(
        &mut profiles,
        &arguments.servers,
        &arguments.server_arguments,
    )?;
    let mut config = PipelineConfig::new(arguments.root.clone());
    config.profiles = profiles;
    Ok(config)
}

// -----------------------------------------------------------------------------
// Arguments
// -----------------------------------------------------------------------------

#[cfg(any(feature = "python", feature = "rust"))]
#[derive(Debug)]
struct Arguments {
    root: PathBuf,
    output: Option<PathBuf>,
    name: Option<String>,
    store: Option<PathBuf>,
    config: Option<PathBuf>,
    servers: Vec<(Language, String)>,
    #[allow(
        clippy::struct_field_names,
        reason = "server_arguments is the required command-line term"
    )]
    server_arguments: BTreeMap<Language, Vec<String>>,
}

#[cfg(any(feature = "python", feature = "rust"))]
impl Arguments {
    /// Parses command-line arguments.
    ///
    /// Repeated server programs use the last value. Repeated server arguments
    /// accumulate in input order and replace profile defaults as one group.
    ///
    /// # Errors
    ///
    /// Returns a message if the root is missing, a second positional value
    /// exists, an option is invalid, or `--output` appears with `--name` or `--store`.
    fn parse(arguments: impl Iterator<Item = String>) -> Result<Self, String> {
        let mut root = None;
        let mut output = None;
        let mut name = None;
        let mut store = None;
        let mut config = None;
        let mut servers = Vec::new();
        let mut server_arguments = BTreeMap::<Language, Vec<String>>::new();
        let mut arguments = arguments.peekable();

        while let Some(argument) = arguments.next() {
            match argument.as_str() {
                // Set the indexed artifact name.
                "--name" => {
                    name = Some(next_value(&mut arguments, "--name")?);
                }
                // Set the store of the indexed artifact.
                "--store" => {
                    store = Some(PathBuf::from(next_value(&mut arguments, "--store")?));
                }
                // Set the artifact path.
                "--output" => {
                    output = Some(PathBuf::from(next_value(&mut arguments, "--output")?));
                }
                // Set the user configuration path.
                "--config" => {
                    config = Some(PathBuf::from(next_value(&mut arguments, "--config")?));
                }
                // Set the language server.
                "--server" => {
                    let value = next_value(&mut arguments, "--server")?;
                    servers.push(parse_language_value(
                        &value,
                        "--server",
                        "<language>=<program>",
                        "program",
                    )?);
                }
                // Add one language server argument.
                "--server-arg" => {
                    let value = next_value(&mut arguments, "--server-arg")?;
                    let (language, value) = parse_language_value(
                        &value,
                        "--server-arg",
                        "<language>=<argument>",
                        "argument",
                    )?;
                    server_arguments.entry(language).or_default().push(value);
                }
                // Reject an unknown option.
                value if value.starts_with("--") => {
                    return Err(format!("unknown option: {value}"));
                }
                // Set the codebase root.
                value if root.is_none() => root = Some(PathBuf::from(value)),
                // Reject a second positional value.
                value => return Err(format!("unexpected argument: {value}")),
            }
        }

        if output.is_some() && (name.is_some() || store.is_some()) {
            return Err("--output cannot be used with --name or --store".to_owned());
        }

        Ok(Self {
            root: root.ok_or("the codebase root is required")?,
            output,
            name,
            store,
            config,
            servers,
            server_arguments,
        })
    }
}

/// Parses one language-qualified option value.
///
/// # Errors
///
/// Returns a message if either component is empty or the language is unknown.
#[cfg(any(feature = "python", feature = "rust"))]
fn parse_language_value(
    value: &str,
    option: &str,
    expected: &str,
    value_name: &str,
) -> Result<(Language, String), String> {
    let (language, value) = value
        .split_once('=')
        .ok_or_else(|| format!("{option} needs {expected}"))?;
    if language.is_empty() {
        return Err(format!("{option} needs a non-empty language"));
    }
    if value.is_empty() {
        return Err(format!("{option} needs a non-empty {value_name}"));
    }
    let language = Language::parse(language)?;
    Ok((language, value.to_owned()))
}

/// Consumes and returns the next option value.
///
/// # Errors
///
/// Returns a message if the option has no value.
#[cfg(any(feature = "python", feature = "rust"))]
fn next_value(
    arguments: &mut impl Iterator<Item = String>,
    option: &str,
) -> Result<String, String> {
    arguments
        .next()
        .ok_or_else(|| format!("{option} needs a value"))
}

// -----------------------------------------------------------------------------
// Artifact destination
// -----------------------------------------------------------------------------

#[cfg(any(feature = "python", feature = "rust"))]
const DEFAULT_STORE: &str = "artifacts";

/// The graph file to write, and the store and file name to index when it is indexed.
#[cfg(any(feature = "python", feature = "rust"))]
#[derive(Debug, PartialEq, Eq)]
struct Destination {
    output: PathBuf,
    indexed: Option<(PathBuf, String)>,
}

/// Returns an explicit output or an indexed artifact destination.
///
/// Without `--name`, the name is the final component of the resolved root.
///
/// # Errors
///
/// Returns a message if the root or artifact name is invalid.
#[cfg(any(feature = "python", feature = "rust"))]
fn artifact_destination(arguments: &Arguments) -> Result<Destination, String> {
    if let Some(output) = &arguments.output {
        return Ok(Destination {
            output: output.clone(),
            indexed: None,
        });
    }

    let name = if let Some(name) = &arguments.name {
        name.clone()
    } else {
        let root = fs::canonicalize(&arguments.root)
            .map_err(|error| format!("cannot resolve {}: {error}", arguments.root.display()))?;
        match root.file_name() {
            Some(name) => name
                .to_str()
                .ok_or_else(|| "the codebase root has no valid UTF-8 name".to_owned())?
                .to_owned(),
            None => "root".to_owned(),
        }
    };
    validate_artifact_name(&name)?;
    let file = format!("{name}.json");
    let store = arguments
        .store
        .clone()
        .unwrap_or_else(|| PathBuf::from(DEFAULT_STORE));

    Ok(Destination {
        output: store.join(&file),
        indexed: Some((store, file)),
    })
}

/// Validates one artifact file stem.
///
/// # Errors
///
/// Returns a message if the name is not one normal path component.
#[cfg(any(feature = "python", feature = "rust"))]
fn validate_artifact_name(name: &str) -> Result<(), String> {
    let path = Path::new(name);
    if name.is_empty()
        || name.eq_ignore_ascii_case("index")
        || name.contains('\\')
        || path
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("json"))
        || path.file_name().and_then(|part| part.to_str()) != Some(name)
    {
        return Err(format!("invalid artifact name: {name}"));
    }
    Ok(())
}

// -----------------------------------------------------------------------------
// Tests
// -----------------------------------------------------------------------------

#[cfg(all(test, any(feature = "python", feature = "rust")))]
mod tests {
    use std::path::PathBuf;

    #[cfg(feature = "rust")]
    use std::{
        fs,
        sync::atomic::{AtomicUsize, Ordering},
    };

    use tekdek::analysis::language::Language;

    #[cfg(feature = "rust")]
    use super::pipeline_config;
    use super::{Arguments, Destination, artifact_destination, validate_artifact_name};

    fn parse(arguments: &[&str]) -> Result<Arguments, String> {
        Arguments::parse(arguments.iter().map(|value| (*value).to_owned()))
    }

    #[test]
    fn accepts_language_qualified_server_overrides() {
        let arguments = parse(&[
            ".",
            "--server",
            "rust=/opt/rust-analyzer",
            "--server-arg",
            "rust=--log-file",
            "--server-arg",
            "rust=/tmp/ra.log",
        ])
        .expect("the arguments should be valid");

        assert_eq!(
            arguments.servers,
            vec![(Language::Rust, "/opt/rust-analyzer".to_owned())]
        );
        assert_eq!(
            arguments.server_arguments[&Language::Rust],
            ["--log-file", "/tmp/ra.log"]
        );
    }

    #[test]
    fn rejects_invalid_server_overrides() {
        for (arguments, message) in [
            (
                vec![".", "--server", "rust-analyzer"],
                "--server needs <language>=<program>",
            ),
            (
                vec![".", "--server", "rust="],
                "--server needs a non-empty program",
            ),
            (
                vec![".", "--server-arg", "rust="],
                "--server-arg needs a non-empty argument",
            ),
            (
                vec![".", "--server", "=program"],
                "--server needs a non-empty language",
            ),
            (
                vec![".", "--server", "go=gopls"],
                "unsupported language: go",
            ),
        ] {
            assert_eq!(parse(&arguments).expect_err("invalid"), message);
        }
    }

    #[test]
    fn preserves_later_equals_characters_in_override_values() {
        let arguments = parse(&[
            ".",
            "--server",
            "rust=server=debug",
            "--server-arg",
            "rust=--setting=value",
        ])
        .expect("the arguments should be valid");

        assert_eq!(
            arguments.servers,
            [(Language::Rust, "server=debug".to_owned())]
        );
        assert_eq!(
            arguments.server_arguments[&Language::Rust],
            ["--setting=value"]
        );
    }

    #[test]
    fn preserves_repeated_overrides_in_input_order() {
        let arguments = parse(&[
            ".",
            "--server",
            "rust=first-server",
            "--server",
            "rust=last-server",
            "--server-arg",
            "python=--first",
            "--server-arg",
            "python=second",
            "--config",
            "custom.toml",
        ])
        .expect("the arguments should be valid");

        assert_eq!(
            arguments.servers,
            [
                (Language::Rust, "first-server".to_owned()),
                (Language::Rust, "last-server".to_owned()),
            ]
        );
        assert_eq!(
            arguments.server_arguments[&Language::Python],
            ["--first", "second"]
        );
        assert_eq!(arguments.config, Some(PathBuf::from("custom.toml")));
    }

    #[cfg(feature = "rust")]
    #[test]
    fn applies_cli_overrides_after_explicit_configuration() {
        let config_path = test_config_path();
        fs::write(
            &config_path,
            r#"
                [languages.rust]
                program = "configured-server"
                arguments = ["configured-argument"]
            "#,
        )
        .expect("the test configuration should be written");
        let config_value = config_path
            .to_str()
            .expect("the test path should be valid UTF-8");
        let arguments = parse(&[
            ".",
            "--config",
            config_value,
            "--server",
            "rust=first-server",
            "--server",
            "rust=last-server",
            "--server-arg",
            "rust=--first",
            "--server-arg",
            "rust=second",
        ])
        .expect("the arguments should be valid");

        let config = pipeline_config(&arguments).expect("the pipeline configuration should build");
        fs::remove_file(&config_path).expect("the test configuration should be removed");
        let rust = config
            .profiles
            .iter()
            .find(|profile| profile.language == Language::Rust)
            .expect("the Rust profile should exist");

        assert_eq!(rust.server.program, "last-server");
        assert_eq!(rust.server.arguments, ["--first", "second"]);
    }

    #[test]
    fn builds_an_indexed_destination_in_the_default_or_named_store() {
        let default = artifact_destination(&parse(&["missing", "--name", "sample"]).expect("args"))
            .expect("the explicit name should not require the root");
        let named = artifact_destination(
            &parse(&["missing", "--name", "sample", "--store", "demo"]).expect("args"),
        )
        .expect("destination");

        assert_eq!(
            default,
            Destination {
                output: PathBuf::from("artifacts/sample.json"),
                indexed: Some((PathBuf::from("artifacts"), "sample.json".to_owned())),
            }
        );
        assert_eq!(named.output, PathBuf::from("demo/sample.json"));
        assert_eq!(
            named.indexed,
            Some((PathBuf::from("demo"), "sample.json".to_owned()))
        );
    }

    #[test]
    fn names_the_artifact_after_the_root_folder() {
        let directory = tempfile::TempDir::new().expect("the temporary directory should exist");
        let root = directory.path().join("sample-app");
        std::fs::create_dir_all(&root).expect("the codebase root should exist");
        let root = root.to_str().expect("path");

        let destination =
            artifact_destination(&parse(&[root]).expect("args")).expect("destination");

        assert_eq!(
            destination.output,
            PathBuf::from("artifacts/sample-app.json")
        );
    }

    #[test]
    fn rejects_an_output_path_with_a_name_or_a_store() {
        for arguments in [
            vec!["project", "--name", "sample", "--output", "other.json"],
            vec!["project", "--store", "demo", "--output", "other.json"],
        ] {
            assert_eq!(
                parse(&arguments).expect_err("the arguments should conflict"),
                "--output cannot be used with --name or --store"
            );
        }
        let explicit =
            artifact_destination(&parse(&["project", "--output", "other.json"]).expect("args"))
                .expect("destination");
        assert_eq!(explicit.indexed, None);
    }

    #[test]
    fn reserves_the_artifact_index_name() {
        assert_eq!(
            validate_artifact_name("INDEX"),
            Err("invalid artifact name: INDEX".to_owned())
        );
    }

    #[cfg(feature = "rust")]
    fn test_config_path() -> PathBuf {
        static NEXT_PATH: AtomicUsize = AtomicUsize::new(0);

        std::env::temp_dir().join(format!(
            "tekdek-cli-{}-{}.toml",
            std::process::id(),
            NEXT_PATH.fetch_add(1, Ordering::Relaxed)
        ))
    }
}
