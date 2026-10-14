//! Analysis pipeline.
//!
//! This module runs the analysis stages in order and returns the code graph.
//!
//! # Sections
//!
//! `Stages` runs the analysis.
//! `Configuration` defines the required root and optional settings.
//! `Errors` reports a failure from any stage.

use std::{
    error::Error,
    fmt, io, panic,
    path::{Path, PathBuf},
    thread,
};

#[cfg(feature = "python")]
use super::language::PythonAdapter;
#[cfg(feature = "rust")]
use super::language::RustAdapter;
use super::{
    fact::{AnalysisError, FactSet},
    graph::{Graph, build::build_graph},
    language::{Language, LanguageAdapter, LanguageProfile, builtin_profiles},
    preflight::{
        detect_languages, detect_required_languages, enabled_extensions, preflight_server_programs,
    },
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

/// Analyzes one codebase and returns its graph.
///
/// Each detected language runs on its own thread, and the fact sets keep
/// language order. The source hash comes from the same document read as the
/// graph. With
/// `TEKDEK_TIMINGS` set, a successful run prints its timings to standard error.
///
/// # Errors
///
/// Returns an error if loading, fact production, or graph construction fails.
pub fn run(config: &PipelineConfig) -> Result<Indexed, PipelineError> {
    let started = timing::start();
    let documents = load_documents(config)?;
    let source_hash = source::sources_hash(&documents).to_string();
    let profiles = detect_required_languages(&documents, &config.profiles)?;
    preflight_server_programs(&profiles)?;
    timing::record_stage(started, || "load".to_owned());

    // Each language runs its own server on its own thread. The first error in
    // language order wins, as in a run of one language after the other.
    let fact_sets = thread::scope(|scope| {
        let runs = profiles
            .iter()
            .map(|profile| scope.spawn(|| produce_facts(&config.root, profile, &documents)))
            .collect::<Vec<_>>();
        runs.into_iter()
            .map(|run| {
                run.join()
                    .unwrap_or_else(|panic| panic::resume_unwind(panic))
            })
            .collect::<Result<Vec<_>, _>>()
    })?;

    let graph_started = timing::start();
    let graph = build_graph(fact_sets, &documents).map_err(PipelineError::Graph)?;
    timing::record_stage(graph_started, || "graph build".to_owned());
    timing::report(started);
    Ok(Indexed { graph, source_hash })
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
///
/// A profile exists only for a compiled adapter, so the last arm cannot run.
fn produce_facts(
    root: &Path,
    profile: &LanguageProfile,
    documents: &[source::DocumentSnapshot],
) -> Result<FactSet, PipelineError> {
    let root = root.to_path_buf();
    let result = match profile.language {
        #[cfg(feature = "python")]
        Language::Python => PythonAdapter::new(root, profile.clone()).produce_facts(documents),
        #[cfg(feature = "rust")]
        Language::Rust => RustAdapter::new(root, profile.clone()).produce_facts(documents),
        #[allow(
            unreachable_patterns,
            reason = "a build with both language features compiles every adapter"
        )]
        language => Err(AnalysisError::new(format!(
            "the {language} adapter is not available in this build"
        ))),
    };

    result.map_err(|source| PipelineError::Language {
        language: profile.language,
        source,
    })
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
            Self::Setup(message) => formatter.write_str(message),
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
            Self::Setup(_) => None,
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
        analysis::{
            fact::RelationKind,
            graph::ProvenanceOrigin,
            language::{Language, LanguageProfile},
            lsp::fake_server::FakeServer,
        },
        store::graph::GraphArtifact,
    };

    #[test]
    fn writes_one_artifact_for_python_and_rust() {
        let repository = TestRepository::new();
        repository.write("app.py", "def run():\n    pass\n");
        repository.write("src/lib.rs", "pub fn run() {}\n");
        let server = FakeServer::new();
        let config = pipeline_config(&repository, &server, "serve", "serve");

        let graph = run(&config)
            .expect("both fake language servers should complete")
            .graph;
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
    fn names_the_language_whose_server_fails() {
        let repository = TestRepository::new();
        repository.write("app.py", "def run():\n    pass\n");
        repository.write("src/lib.rs", "pub fn run() {}\n");
        let server = FakeServer::new();
        let config = pipeline_config(&repository, &server, "serve", "protocol-error");

        let Err(error) = run(&config) else {
            panic!("the Rust fake server should return an error");
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
    fn names_the_first_language_when_every_server_fails() {
        let repository = TestRepository::new();
        repository.write("app.py", "def run():\n    pass\n");
        repository.write("src/lib.rs", "pub fn run() {}\n");
        let server = FakeServer::new();
        let config = pipeline_config(&repository, &server, "protocol-error", "protocol-error");

        let Err(error) = run(&config) else {
            panic!("both fake servers should return an error");
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

    #[test]
    fn imports_the_node_that_the_server_reports_as_the_definition() {
        let repository = TestRepository::new();
        // The server answers `null` for `lib` on line 2, so that statement adds no edge.
        repository.write("app.py", "from lib import run\nimport lib\n");
        repository.write("lib.py", "def run():\n    pass\n");
        let server = FakeServer::new();
        let config = pipeline_config(&repository, &server, "serve", "serve");

        let graph = run(&config)
            .expect("the fake language server should complete")
            .graph;
        let imports = graph
            .edges
            .iter()
            .filter(|edge| edge.kind == RelationKind::Imports)
            .map(|edge| {
                (
                    graph.nodes[edge.source.index()].key.as_str(),
                    graph.nodes[edge.target.index()].key.as_str(),
                    edge.provenance.origin,
                    edge.weight,
                    edge.provenance.evidence.observations[0].range.end,
                )
            })
            .collect::<Vec<_>>();

        assert_eq!(
            imports,
            [(
                "app.py",
                "lib.py::callable:run",
                ProvenanceOrigin::Semantic,
                1,
                19
            )]
        );
    }

    #[test]
    fn adds_no_import_edge_for_a_definition_outside_the_project() {
        let repository = TestRepository::new();
        repository.write("app.py", "from lib import run\n");
        repository.write("lib.py", "def run():\n    pass\n");
        let server = FakeServer::new();
        let config = pipeline_config(&repository, &server, "definition-outside", "serve");

        let graph = run(&config)
            .expect("the fake language server should complete")
            .graph;

        assert!(
            graph
                .edges
                .iter()
                .all(|edge| edge.kind != RelationKind::Imports)
        );
    }

    #[test]
    fn adds_a_call_edge_from_the_module_for_a_module_level_call() {
        let repository = TestRepository::new();
        repository.write("app.py", "from lib import run\n\nrun()\n");
        repository.write("lib.py", "def run():\n    pass\n");
        let server = FakeServer::new();
        let config = pipeline_config(&repository, &server, "module-call", "serve");

        let graph = run(&config)
            .expect("the fake language server should complete")
            .graph;
        let edges = graph
            .edges
            .iter()
            .filter(|edge| matches!(edge.kind, RelationKind::Calls | RelationKind::UsesValue))
            .map(|edge| {
                (
                    graph.nodes[edge.source.index()].key.as_str(),
                    graph.nodes[edge.target.index()].key.as_str(),
                    edge.kind,
                    edge.provenance.evidence.observations[0].range.start,
                )
            })
            .collect::<Vec<_>>();

        assert_eq!(
            edges,
            [("app.py", "lib.py::callable:run", RelationKind::Calls, 21)]
        );
    }

    fn profile_mut(profiles: &mut [LanguageProfile], language: Language) -> &mut LanguageProfile {
        profiles
            .iter_mut()
            .find(|profile| profile.language == language)
            .expect("the profile should exist")
    }

    fn pipeline_config(
        repository: &TestRepository,
        server: &FakeServer,
        python_mode: &str,
        rust_mode: &str,
    ) -> PipelineConfig {
        let mut config = PipelineConfig::new(repository.path().to_path_buf());
        profile_mut(&mut config.profiles, Language::Python).server = server.command(python_mode);
        profile_mut(&mut config.profiles, Language::Rust).server = server.command(rust_mode);
        config
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
