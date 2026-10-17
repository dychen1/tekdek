//! Analysis pipeline.
//!
//! This module runs the analysis stages in order and returns the code graph.
//! The graph holds syntax facts only: no language server starts.
//!
//! # Sections
//!
//! `Stages` runs the analysis.
//! `Configuration` defines the required root and optional settings.
//! `Errors` reports a failure from any stage.

use std::{error::Error, fmt, io, panic, path::PathBuf, thread};

#[cfg(feature = "python")]
use super::language::PythonAdapter;
#[cfg(feature = "rust")]
use super::language::RustAdapter;
use super::{
    fact::{AnalysisError, FactSet},
    graph::{
        Graph,
        build::{LanguageImports, build_graph_with_imports},
    },
    language::{Language, LanguageAdapter, LanguageProfile, builtin_profiles},
    preflight::{detect_languages, detect_required_languages, enabled_extensions},
    source::{self, DEFAULT_EXCLUDED_DIRECTORIES},
    timing,
};

// -----------------------------------------------------------------------------
// Stages
// -----------------------------------------------------------------------------

/// A graph and the hash of the source files that it was built from.
pub struct Indexed {
    pub graph: Graph,
    pub source_hash: String,
}

/// Analyzes the syntax of one codebase and returns its graph.
///
/// Each detected language runs on its own thread, and the fact sets keep
/// language order. A failing language returns its error only after every
/// other language thread ends. The source hash comes from the same document
/// read as the graph. With `TEKDEK_TIMINGS` set, a successful run prints its
/// timings to standard error.
///
/// # Errors
///
/// Returns an error if loading, fact production, or graph construction fails.
pub fn run(config: &PipelineConfig) -> Result<Indexed, PipelineError> {
    let parsed = parse(config)?;
    Ok(Indexed {
        graph: parsed.graph,
        source_hash: parsed.source_hash,
    })
}

/// One parsed codebase: the input and output of `run`, with the import sites of each language.
pub(crate) struct Parsed {
    pub(crate) documents: Vec<source::DocumentSnapshot>,
    pub(crate) profiles: Vec<LanguageProfile>,
    pub(crate) graph: Graph,
    pub(crate) imports: Vec<LanguageImports>,
    pub(crate) source_hash: String,
}

/// Parses one codebase as `run` does.
///
/// # Errors
///
/// Returns an error if loading, fact production, or graph construction fails.
pub(crate) fn parse(config: &PipelineConfig) -> Result<Parsed, PipelineError> {
    let started = timing::start();
    let documents = load_documents(config)?;
    let source_hash = source::sources_hash(&documents).to_string();
    let profiles = detect_required_languages(&documents, &config.profiles)?;
    timing::record_stage(started, || "load".to_owned());

    // Each language runs on its own thread. The first error in language order
    // wins, as in a run of one language after the other.
    let fact_sets = thread::scope(|scope| {
        let runs = profiles
            .iter()
            .map(|profile| scope.spawn(|| produce_facts(profile, &documents)))
            .collect::<Vec<_>>();
        runs.into_iter()
            .map(|run| {
                run.join()
                    .unwrap_or_else(|panic| panic::resume_unwind(panic))
            })
            .collect::<Result<Vec<_>, _>>()
    })?;

    let graph_started = timing::start();
    let (graph, imports) =
        build_graph_with_imports(fact_sets, &documents).map_err(PipelineError::Graph)?;
    timing::record_stage(graph_started, || "graph build".to_owned());
    timing::report(started);
    let profiles = profiles.into_iter().cloned().collect();
    Ok(Parsed {
        documents,
        profiles,
        graph,
        imports,
        source_hash,
    })
}

/// Returns the hash of the source files that `run` reads now.
///
/// This function starts no language server.
///
/// # Errors
///
/// Returns an error if the source files cannot be read.
pub fn source_hash(config: &PipelineConfig) -> Result<String, PipelineError> {
    Ok(source::sources_hash(&load_documents(config)?).to_string())
}

/// Returns the enabled languages that have source files under the root, in stable order.
///
/// The list can be empty. This function starts no language server.
///
/// # Errors
///
/// Returns an error if the source files cannot be read.
pub fn detect_project_languages(config: &PipelineConfig) -> Result<Vec<Language>, PipelineError> {
    let documents = load_documents(config)?;
    Ok(detect_languages(&documents, &config.profiles)
        .into_iter()
        .map(|profile| profile.language)
        .collect())
}

fn load_documents(config: &PipelineConfig) -> Result<Vec<source::DocumentSnapshot>, PipelineError> {
    let extensions = enabled_extensions(&config.profiles);
    Ok(source::load_documents(
        &config.root,
        &extensions,
        DEFAULT_EXCLUDED_DIRECTORIES,
    )?)
}

/// Runs the adapter of one profile.
fn produce_facts(
    profile: &LanguageProfile,
    documents: &[source::DocumentSnapshot],
) -> Result<FactSet, PipelineError> {
    adapter(profile)
        .and_then(|mut adapter| adapter.produce_facts(documents))
        .map_err(|source| PipelineError::Language {
            language: profile.language,
            source,
        })
}

/// Returns the adapter of one profile.
///
/// A profile exists only for a compiled adapter, so the last arm cannot run.
pub(crate) fn adapter(
    profile: &LanguageProfile,
) -> Result<Box<dyn LanguageAdapter>, AnalysisError> {
    match profile.language {
        #[cfg(feature = "python")]
        Language::Python => Ok(Box::new(PythonAdapter::new(profile.clone()))),
        #[cfg(feature = "rust")]
        Language::Rust => Ok(Box::new(RustAdapter::new(profile.clone()))),
        #[allow(
            unreachable_patterns,
            reason = "a build with both language features compiles every adapter"
        )]
        language => Err(AnalysisError::new(format!(
            "the {language} adapter is not available in this build"
        ))),
    }
}

// -----------------------------------------------------------------------------
// Configuration
// -----------------------------------------------------------------------------

pub struct PipelineConfig {
    pub root: PathBuf,
    pub profiles: Vec<LanguageProfile>,
}

impl PipelineConfig {
    #[must_use]
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            profiles: builtin_profiles(),
        }
    }
}

// -----------------------------------------------------------------------------
// Errors
// -----------------------------------------------------------------------------

#[derive(Debug)]
pub enum PipelineError {
    /// The pipeline cannot read the codebase.
    Load(io::Error),
    /// The pipeline cannot prepare one or more language runs.
    Setup(String),
    /// A language server program is not on the search path of this process.
    MissingServer(String),
    /// One language adapter cannot analyze its selected sources.
    Language {
        language: Language,
        source: AnalysisError,
    },
    /// Graph construction cannot merge the language facts.
    Graph(AnalysisError),
}

impl fmt::Display for PipelineError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            // Report source loading.
            Self::Load(error) => write!(formatter, "cannot read the codebase: {error}"),
            // Report detection or preflight.
            Self::Setup(message) | Self::MissingServer(message) => formatter.write_str(message),
            // Report one adapter failure.
            Self::Language { language, source } => {
                write!(formatter, "cannot analyze {language}: {source}")
            }
            // Report graph construction.
            Self::Graph(error) => write!(formatter, "cannot build the code graph: {error}"),
        }
    }
}

impl Error for PipelineError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            // Chain the I/O failure.
            Self::Load(error) => Some(error),
            // Chain the adapter failure.
            Self::Language { source, .. } => Some(source),
            // Chain the graph failure.
            Self::Graph(error) => Some(error),
            // A boundary carries no wrapped error.
            Self::Setup(_) | Self::MissingServer(_) => None,
        }
    }
}

impl From<io::Error> for PipelineError {
    fn from(error: io::Error) -> Self {
        Self::Load(error)
    }
}

// -----------------------------------------------------------------------------
// Tests
// -----------------------------------------------------------------------------

#[cfg(all(test, feature = "python", feature = "rust"))]
mod tests {
    use std::{fs, path::Path};

    use tempfile::TempDir;

    use super::{PipelineConfig, PipelineError, run};
    use crate::{
        analysis::{fact::RelationKind, language::Language},
        store::graph::GraphArtifact,
    };

    #[test]
    fn indexes_python_and_rust_without_a_language_server() {
        let repository = TestRepository::new();
        repository.write("app.py", "def run():\n    pass\n\nrun()\n");
        repository.write("src/lib.rs", "pub fn run() {}\n");
        let mut config = PipelineConfig::new(repository.path().to_path_buf());
        for profile in &mut config.profiles {
            profile.server.program = "tekdek-missing-language-server".to_owned();
        }

        let graph = run(&config)
            .expect("the index should need no language server")
            .graph;
        assert!(
            graph
                .edges
                .iter()
                .all(|edge| edge.kind == RelationKind::Contains)
        );
        let artifact = GraphArtifact::new(&graph);
        let artifact =
            serde_json::to_value(artifact).expect("the artifact should serialize to JSON");

        assert_eq!(
            artifact["graph"]["languages"],
            serde_json::json!(["python", "rust"])
        );
        assert!(
            artifact["graph"]["nodes"]
                .as_array()
                .expect("nodes should be an array")
                .iter()
                .any(|node| node["language"] == "python")
        );
        assert!(
            artifact["graph"]["nodes"]
                .as_array()
                .expect("nodes should be an array")
                .iter()
                .any(|node| node["language"] == "rust")
        );
    }

    #[test]
    fn names_the_language_whose_syntax_fails() {
        let repository = TestRepository::new();
        repository.write("app.py", "def run():\n    pass\n");
        repository.write("src/lib.rs", "pub fn run( {}\n");
        let config = PipelineConfig::new(repository.path().to_path_buf());

        let Err(error) = run(&config) else {
            panic!("the Rust syntax should fail");
        };

        assert!(
            matches!(
                error,
                PipelineError::Language {
                    language: Language::Rust,
                    ..
                }
            ),
            "the error should identify the failed adapter"
        );
    }

    #[test]
    fn names_the_first_language_when_every_language_fails() {
        let repository = TestRepository::new();
        repository.write("app.py", "def run(:\n");
        repository.write("src/lib.rs", "pub fn run( {}\n");
        let config = PipelineConfig::new(repository.path().to_path_buf());

        let Err(error) = run(&config) else {
            panic!("both syntax steps should fail");
        };

        assert!(
            matches!(
                error,
                PipelineError::Language {
                    language: Language::Python,
                    ..
                }
            ),
            "the error should identify the first language"
        );
    }

    struct TestRepository {
        directory: TempDir,
    }

    impl TestRepository {
        fn new() -> Self {
            let directory = tempfile::tempdir().expect("the test repository should be created");
            Self { directory }
        }

        fn path(&self) -> &Path {
            self.directory.path()
        }

        fn write(&self, path: &str, content: &str) {
            let path = self.directory.path().join(path);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).expect("the source directory should be created");
            }
            fs::write(path, content).expect("the source file should be written");
        }
    }
}
