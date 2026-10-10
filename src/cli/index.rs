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
    process::ExitCode,
};

use tekdek::{
    analysis::{
        self, PipelineConfig,
        config::{ConfigError, apply_cli_overrides, configured_profiles},
        language::Language,
    },
    store::{PROJECT_STORE, create_project_store, find_project_store, graph::GraphArtifact},
};

use super::ERROR_CODE;

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
pub(super) fn run(raw: impl Iterator<Item = String>) -> ExitCode {
    let arguments = match Arguments::parse(raw) {
        Ok(arguments) => arguments,
        Err(message) => {
            eprintln!("error: {message}");
            eprintln!("{USAGE}");
            return ExitCode::from(ERROR_CODE);
        }
    };
    let destination = match artifact_destination(&arguments) {
        Ok(destination) => destination,
        Err(message) => {
            eprintln!("error: {message}");
            return ExitCode::from(ERROR_CODE);
        }
    };
    if destination.project_root {
        println!("indexing the project root {}", destination.root.display());
    }
    let config = match pipeline_config(&arguments, &destination.root) {
        Ok(config) => config,
        Err(error) => {
            eprintln!("error: {error}");
            return ExitCode::from(ERROR_CODE);
        }
    };
    let result = match analysis::run(&config) {
        Ok(result) => result,
        Err(error) => {
            eprintln!("error: {error}");
            return ExitCode::from(ERROR_CODE);
        }
    };
    // Create a new store only after the analysis succeeds, so a failed first index leaves no store.
    if destination.create_store
        && let Some((store, _)) = &destination.indexed
    {
        if let Err(error) = create_project_store(store) {
            eprintln!(
                "error: cannot create the store {}: {error}",
                store.display()
            );
            return ExitCode::from(ERROR_CODE);
        }
        println!("created the store {}", store.display());
    }
    if let Err(error) = GraphArtifact::new(&result.graph).write(&destination.output) {
        eprintln!(
            "error: cannot write {}: {error}",
            destination.output.display()
        );
        return ExitCode::from(ERROR_CODE);
    }

    println!(
        "{} nodes, {} edges -> {}",
        result.graph.nodes.len(),
        result.graph.edges.len(),
        destination.output.display()
    );
    ExitCode::SUCCESS
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

/// Consumes and returns the next option value.
///
/// # Errors
///
/// Returns a message if the option has no value.
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

        let config = pipeline_config(&arguments, &arguments.root)
            .expect("the pipeline configuration should build");
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
