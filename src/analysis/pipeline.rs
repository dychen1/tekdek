//! Analysis pipeline.
//!
//! This module runs the analysis stages in order and returns the code graph.
//!
//! # Sections
//!
//! `Stages` runs the analysis.
//! `Configuration` defines the required root and optional settings.
//! `Result` names every output.
//! `Errors` reports a failure from any stage.

use std::{
    error::Error,
    fmt, io,
    path::{Path, PathBuf},
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
};

// -----------------------------------------------------------------------------
// Stages
// -----------------------------------------------------------------------------

/// Analyzes one codebase and returns its graph.
///
/// # Errors
///
/// Returns an error if loading, fact production, or graph construction fails.
pub fn run(config: &PipelineConfig) -> Result<PipelineResult, PipelineError> {
    let documents = load_documents(config)?;
    let profiles = detect_required_languages(&documents, &config.profiles)?;
    preflight_server_programs(&profiles)?;

    let mut fact_sets = Vec::with_capacity(profiles.len());
    for profile in profiles {
        fact_sets.push(produce_facts(&config.root, profile, &documents)?);
    }

    let graph = build_graph(fact_sets, &documents).map_err(PipelineError::Graph)?;
    Ok(PipelineResult { graph })
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
// Result
// -----------------------------------------------------------------------------

pub struct PipelineResult {
    pub graph: Graph,
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
    use std::{
        fs,
        path::{Path, PathBuf},
        process,
        sync::atomic::{AtomicU64, Ordering},
    };

    use super::{PipelineConfig, PipelineError, run};
    use crate::{
        analysis::{
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
        let config = pipeline_config(&repository, &server, "serve");

        let result = run(&config).expect("both fake language servers should complete");
        let artifact = GraphArtifact::new(&result.graph);
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
        let config = pipeline_config(&repository, &server, "protocol-error");

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

    fn profile_mut(profiles: &mut [LanguageProfile], language: Language) -> &mut LanguageProfile {
        profiles
            .iter_mut()
            .find(|profile| profile.language == language)
            .expect("the profile should exist")
    }

    fn pipeline_config(
        repository: &TestRepository,
        server: &FakeServer,
        rust_mode: &str,
    ) -> PipelineConfig {
        let mut config = PipelineConfig::new(repository.path().to_path_buf());
        profile_mut(&mut config.profiles, Language::Python).server = server.command("serve");
        profile_mut(&mut config.profiles, Language::Rust).server = server.command(rust_mode);
        config
    }

    struct TestRepository {
        directory: PathBuf,
    }

    impl TestRepository {
        fn new() -> Self {
            static NEXT_ID: AtomicU64 = AtomicU64::new(0);

            let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
            let directory =
                std::env::temp_dir().join(format!("tekdek-pipeline-{}-{id}", process::id()));
            fs::create_dir(&directory).expect("the test repository should be created");
            Self { directory }
        }

        fn path(&self) -> &Path {
            &self.directory
        }

        fn write(&self, path: &str, content: &str) {
            let path = self.directory.join(path);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).expect("the source directory should be created");
            }
            fs::write(path, content).expect("the source file should be written");
        }
    }

    impl Drop for TestRepository {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.directory);
        }
    }
}
