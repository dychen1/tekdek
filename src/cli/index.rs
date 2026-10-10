//! Index command.
//!
//! This module owns `index`: it analyzes one codebase root and writes the graph
//! artifact into a store. Command arguments are the input. One graph file and
//! status lines are the output.
//!
//! Use `tekdek index [<root>]` with optional artifact, store, configuration,
//! and language server overrides.
//!
//! # Sections
//!
//! `Entry` runs the analysis and reports the result.
//! `Arguments` parses the command line.
//! `Artifact destination` selects indexed or explicit output.

use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use tekdek::{
    analysis::{
        self, PipelineConfig,
        config::{ConfigError, apply_cli_overrides, configured_profiles},
        language::Language,
    },
    store::{PROJECT_STORE, create_project_store, find_project_store, graph::GraphArtifact},
};

use super::split_arguments;

// -----------------------------------------------------------------------------
// Entry
// -----------------------------------------------------------------------------

pub(super) const USAGE: &str = "usage: tekdek index [<root>]
  [--name <name>] [--store <directory>] | [--output <path>]
  [--config <path>]
  [--server <language>=<program>]
  [--server-arg <language>=<argument>]...
The default store is the nearest .tekdek folder in the root or its parents, up
to the top folder of the Git work tree. Without a root, index analyzes the
working directory, or the project root when a store exists: the folder that
holds the store. A root inside the project of a store needs --name. Without a
store, index creates <root>/.tekdek after the analysis succeeds. index writes
<store>/<name>.json. --output writes one file and changes no store.";

/// Analyzes one codebase and writes its graph into the store.
pub(super) fn run(raw: impl Iterator<Item = String>) -> Result<(), String> {
    let arguments = Arguments::parse(raw).map_err(|message| format!("{message}\n{USAGE}"))?;
    let destination = artifact_destination(&arguments)?;
    if destination.project_root {
        println!("indexing the project root {}", destination.root.display());
    }
    let config =
        pipeline_config(&arguments, &destination.root).map_err(|error| error.to_string())?;
    let result = analysis::run(&config).map_err(|error| error.to_string())?;
    // Create a new store only after the analysis succeeds, so a failed first index leaves no store.
    if destination.create_store
        && let Some((store, _)) = &destination.indexed
    {
        create_project_store(store)
            .map_err(|error| format!("cannot create the store {}: {error}", store.display()))?;
        println!("created the store {}", store.display());
    }
    GraphArtifact::new(&result.graph)
        .write(&destination.output)
        .map_err(|error| format!("cannot write {}: {error}", destination.output.display()))?;

    println!(
        "{} nodes, {} edges -> {}",
        result.graph.nodes.len(),
        result.graph.edges.len(),
        destination.output.display()
    );
    Ok(())
}

/// Builds pipeline configuration from files and command-line overrides.
///
/// # Errors
///
/// Returns an error if configuration loading or override application fails.
fn pipeline_config(arguments: &Arguments, root: &Path) -> Result<PipelineConfig, ConfigError> {
    let mut profiles = configured_profiles(arguments.config.as_deref())?;
    apply_cli_overrides(
        &mut profiles,
        &arguments.servers,
        &arguments.server_arguments,
    )?;
    let mut config = PipelineConfig::new(root.to_path_buf());
    config.profiles = profiles;
    Ok(config)
}

// -----------------------------------------------------------------------------
// Arguments
// -----------------------------------------------------------------------------

#[derive(Debug)]
struct Arguments {
    root: PathBuf,
    root_given: bool,
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

impl Arguments {
    /// Parses command-line arguments.
    ///
    /// Repeated server programs use the last value. Repeated server arguments
    /// accumulate in input order and replace profile defaults as one group.
    ///
    /// # Errors
    ///
    /// Returns a message if a second positional value exists, an option is
    /// invalid, or `--output` appears with `--name` or `--store`.
    fn parse(arguments: impl Iterator<Item = String>) -> Result<Self, String> {
        let (operands, mut options, repeated) = split_arguments(
            arguments,
            &["--name", "--store", "--output", "--config"],
            &["--server", "--server-arg"],
        )?;
        let root = match operands.as_slice() {
            [] => None,
            [root] => Some(PathBuf::from(root)),
            [_, extra, ..] => return Err(format!("unexpected argument: {extra}")),
        };
        let output = options.remove("--output").map(PathBuf::from);
        let name = options.remove("--name");
        let store = options.remove("--store").map(PathBuf::from);
        let config = options.remove("--config").map(PathBuf::from);
        if let Some(option) = options.keys().next() {
            return Err(format!("unexpected option: {option}"));
        }
        if output.is_some() && (name.is_some() || store.is_some()) {
            return Err("--output cannot be used with --name or --store".to_owned());
        }

        let mut servers = Vec::new();
        let mut server_arguments = BTreeMap::<Language, Vec<String>>::new();
        for (option, value) in repeated {
            if option == "--server" {
                servers.push(parse_language_value(
                    &value,
                    "--server",
                    "<language>=<program>",
                    "program",
                )?);
            } else {
                let (language, value) = parse_language_value(
                    &value,
                    "--server-arg",
                    "<language>=<argument>",
                    "argument",
                )?;
                server_arguments.entry(language).or_default().push(value);
            }
        }

        Ok(Self {
            root_given: root.is_some(),
            root: root.unwrap_or_else(|| PathBuf::from(".")),
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

// -----------------------------------------------------------------------------
// Artifact destination
// -----------------------------------------------------------------------------

/// The root to analyze, the graph file to write, and the store and file name when it is indexed.
///
/// `create_store` is true when the destination is a new project store.
/// `project_root` is true when index analyzes the project root instead of the working directory.
#[derive(Debug, PartialEq, Eq)]
struct Destination {
    root: PathBuf,
    output: PathBuf,
    indexed: Option<(PathBuf, String)>,
    create_store: bool,
    project_root: bool,
}

/// Returns an explicit output or an indexed artifact destination.
///
/// Without `--name`, the name is the final component of the analyzed root.
/// Without `--store`, the store is the nearest project store of the resolved
/// root. A root inside the project of that store, which is the folder that
/// holds the store, has these rules: without a root argument, index analyzes
/// the project root, as Git commands do. With a root argument, the graph needs
/// `--name`, so a partial graph never replaces the project graph. Without a
/// store, the destination is a new `<root>/.tekdek`.
///
/// # Errors
///
/// Returns a message if the root or artifact name is invalid, or if a root
/// inside a project has no `--name`.
fn artifact_destination(arguments: &Arguments) -> Result<Destination, String> {
    if let Some(output) = &arguments.output {
        return Ok(Destination {
            root: arguments.root.clone(),
            output: output.clone(),
            indexed: None,
            create_store: false,
            project_root: false,
        });
    }

    let resolve = |root: &Path| {
        fs::canonicalize(root)
            .map_err(|error| format!("cannot resolve {}: {error}", root.display()))
    };
    let name_of = |root: &Path| match &arguments.name {
        Some(name) => Ok(name.clone()),
        None => folder_name(&resolve(root)?),
    };
    let mut project_root = false;
    let (root, store, name, create_store) = if let Some(store) = &arguments.store {
        let name = name_of(&arguments.root)?;
        (arguments.root.clone(), store.clone(), name, false)
    } else {
        let root = resolve(&arguments.root)?;
        if let Some(store) = find_project_store(&root) {
            let project = store
                .parent()
                .ok_or_else(|| format!("the store {} has no parent folder", store.display()))?
                .to_path_buf();
            let root = if root == project || arguments.name.is_some() {
                root
            } else if arguments.root_given {
                return Err(format!(
                    "{} is inside the project {} of the store {}: index the project root, or name this graph with --name <name>",
                    root.display(),
                    project.display(),
                    store.display()
                ));
            } else {
                project_root = true;
                project
            };
            let name = name_of(&root)?;
            (root, store, name, false)
        } else {
            let name = name_of(&root)?;
            (root.clone(), root.join(PROJECT_STORE), name, true)
        }
    };
    validate_artifact_name(&name)?;
    let file = format!("{name}.json");

    Ok(Destination {
        root,
        output: store.join(&file),
        indexed: Some((store, file)),
        create_store,
        project_root,
    })
}

/// Returns the final component of a resolved root, or `root` for a file system root.
///
/// # Errors
///
/// Returns a message if the name is not valid UTF-8.
fn folder_name(root: &Path) -> Result<String, String> {
    match root.file_name() {
        Some(name) => name
            .to_str()
            .map(str::to_owned)
            .ok_or_else(|| "the codebase root has no valid UTF-8 name".to_owned()),
        None => Ok("root".to_owned()),
    }
}

/// Validates one artifact file stem.
///
/// # Errors
///
/// Returns a message if the name is not one normal path component.
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

#[cfg(test)]
mod tests {
    #[cfg(feature = "rust")]
    use std::fs;
    use std::path::PathBuf;

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
        let directory = tempfile::tempdir().expect("the temporary directory should exist");
        let config_path = directory.path().join("config.toml");
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

        let config = pipeline_config(&arguments, &arguments.root)
            .expect("the pipeline configuration should build");
        let rust = config
            .profiles
            .iter()
            .find(|profile| profile.language == Language::Rust)
            .expect("the Rust profile should exist");

        assert_eq!(rust.server.program, "last-server");
        assert_eq!(rust.server.arguments, ["--first", "second"]);
    }

    #[test]
    fn builds_an_indexed_destination_in_the_named_store() {
        let named = artifact_destination(
            &parse(&["missing", "--name", "sample", "--store", "demo"]).expect("args"),
        )
        .expect("an explicit name and store should not require the root");

        assert_eq!(
            named,
            Destination {
                output: PathBuf::from("demo/sample.json"),
                indexed: Some((PathBuf::from("demo"), "sample.json".to_owned())),
                create_store: false,
                project_root: false,
                root: PathBuf::from("missing"),
            }
        );
    }

    #[test]
    fn indexes_the_project_root_from_a_subfolder_and_needs_a_name_for_a_nested_root() {
        let directory = tempfile::TempDir::new().expect("the temporary directory should exist");
        let root = directory.path().join("sample-app");
        let nested = root.join("services").join("api");
        std::fs::create_dir_all(&nested).expect("the codebase root should exist");
        let root = std::fs::canonicalize(root).expect("canonical root");
        let store = root.join(".tekdek");

        let fresh = artifact_destination(&parse(&[root.to_str().expect("path")]).expect("args"))
            .expect("destination");
        assert_eq!(fresh.output, store.join("sample-app.json"));
        assert!(fresh.create_store);

        std::fs::create_dir(&store).expect("project store");
        let nested = std::fs::canonicalize(nested).expect("canonical folder");
        let error = artifact_destination(&parse(&[nested.to_str().expect("path")]).expect("args"))
            .expect_err("a root inside the project needs a name");
        assert!(error.contains("--name"), "{error}");

        let named = artifact_destination(
            &parse(&[nested.to_str().expect("path"), "--name", "api"]).expect("args"),
        )
        .expect("destination");
        assert_eq!(named.output, store.join("api.json"));
        assert_eq!(named.root, nested);
        assert!(!named.create_store && !named.project_root);

        let mut working = parse(&[]).expect("args");
        assert_eq!(working.root, PathBuf::from("."));
        working.root = nested;
        let project = artifact_destination(&working).expect("destination");
        assert_eq!(project.root, root);
        assert_eq!(project.output, store.join("sample-app.json"));
        assert!(project.project_root);
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
    fn accepts_inline_option_values() {
        let arguments =
            parse(&["--name=sample", "--store=demo", "--server=rust=ra"]).expect("args");

        assert_eq!(arguments.name.as_deref(), Some("sample"));
        assert_eq!(arguments.store, Some(PathBuf::from("demo")));
        assert_eq!(arguments.servers, [(Language::Rust, "ra".to_owned())]);
    }

    #[test]
    fn rejects_a_short_option_as_an_unknown_option() {
        assert_eq!(
            parse(&["-h"]).expect_err("-h is not a root"),
            "unknown option: -h"
        );
    }

    #[test]
    fn rejects_a_repeated_single_value_option() {
        for option in ["--name", "--store", "--output", "--config"] {
            assert_eq!(
                parse(&[option, "a", option, "b"]).expect_err("a repeated option"),
                format!("duplicate option: {option}")
            );
        }
    }

    #[test]
    fn reserves_the_artifact_index_name() {
        assert_eq!(
            validate_artifact_name("INDEX"),
            Err("invalid artifact name: INDEX".to_owned())
        );
    }
}
