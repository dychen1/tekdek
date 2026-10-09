//! Analysis pipeline.
//!
//! This module runs the analysis stages in order and writes the graph artifact.
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
    language::{Language, LanguageProfile, builtin_profiles},
    preflight::{
        detect_languages, detect_required_languages, enabled_extensions, preflight_server_programs,
        profile,
    },
    source::{self, DEFAULT_EXCLUDED_DIRECTORIES},
};
use crate::store::graph::GraphArtifact;

// -----------------------------------------------------------------------------
// Stages
// -----------------------------------------------------------------------------

/// Analyzes one codebase and returns its graph and detected languages.
///
/// # Errors
///
/// Returns an error if loading, fact production, or graph construction fails.
pub fn run(config: &PipelineConfig) -> Result<PipelineResult, PipelineError> {
    let documents = load_documents(config)?;
    let languages = detect_required_languages(&documents, &config.profiles)?;
    preflight_server_programs(&config.profiles, &languages)?;

    let mut fact_sets = Vec::with_capacity(languages.len());
    for language in &languages {
        let profile = profile(&config.profiles, *language)?;
        fact_sets.push(produce_facts(&config.root, profile, &documents)?);
    }

    let graph = build_graph(fact_sets, &documents).map_err(PipelineError::Graph)?;
    Ok(PipelineResult { graph, languages })
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
    Ok(detect_languages(&documents, &config.profiles))
}

fn load_documents(config: &PipelineConfig) -> Result<Vec<source::DocumentSnapshot>, PipelineError> {
    let excluded = config
        .excluded_directories
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>();
    let extensions = enabled_extensions(&config.profiles);
    Ok(source::load_documents(
        &config.root,
        &extensions,
        &excluded,
    )?)
}

/// Analyzes one codebase and writes its graph artifact.
///
/// The function does not change the output if analysis fails.
///
/// # Errors
///
/// Returns an analysis error or an artifact write error.
pub fn run_and_write(
    config: &PipelineConfig,
    output: &Path,
) -> Result<PipelineResult, PipelineOutputError> {
    let result = run(config).map_err(PipelineOutputError::Analysis)?;
    GraphArtifact::new(&result.graph)
        .write(output)
        .map_err(|source| PipelineOutputError::Artifact {
            path: output.to_path_buf(),
            source,
        })?;
    Ok(result)
}

fn produce_facts(
    root: &Path,
    profile: &LanguageProfile,
    documents: &[source::DocumentSnapshot],
) -> Result<FactSet, PipelineError> {
    let result = match profile.language {
        Language::Python => produce_python_facts(root, profile, documents),
        Language::Rust => produce_rust_facts(root, profile, documents),
    };

    result.map_err(|source| PipelineError::Language {
        language: profile.language,
        source,
    })
}

#[cfg(feature = "python")]
fn produce_python_facts(
    root: &Path,
    profile: &LanguageProfile,
    documents: &[source::DocumentSnapshot],
) -> Result<FactSet, AnalysisError> {
    use super::language::LanguageAdapter;
    let mut adapter = PythonAdapter::new(root.to_path_buf(), profile.clone());
    adapter.produce_facts(documents)
}

#[cfg(not(feature = "python"))]
fn produce_python_facts(
    _root: &Path,
    _profile: &LanguageProfile,
    _documents: &[source::DocumentSnapshot],
) -> Result<FactSet, AnalysisError> {
    Err(AnalysisError::new(
        "the Python adapter is not available in this build",
    ))
}

#[cfg(feature = "rust")]
fn produce_rust_facts(
    root: &Path,
    profile: &LanguageProfile,
    documents: &[source::DocumentSnapshot],
) -> Result<FactSet, AnalysisError> {
    use super::language::LanguageAdapter;
    let mut adapter = RustAdapter::new(root.to_path_buf(), profile.clone());
    adapter.produce_facts(documents)
}

#[cfg(not(feature = "rust"))]
fn produce_rust_facts(
    _root: &Path,
    _profile: &LanguageProfile,
    _documents: &[source::DocumentSnapshot],
) -> Result<FactSet, AnalysisError> {
    Err(AnalysisError::new(
        "the Rust adapter is not available in this build",
    ))
}

// -----------------------------------------------------------------------------
// Configuration
// -----------------------------------------------------------------------------

pub struct PipelineConfig {
    pub root: PathBuf,
    pub profiles: Vec<LanguageProfile>,
    pub excluded_directories: Vec<String>,
}

impl PipelineConfig {
    #[must_use]
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            profiles: builtin_profiles(),
            excluded_directories: DEFAULT_EXCLUDED_DIRECTORIES
                .iter()
                .map(|directory| (*directory).to_owned())
                .collect(),
        }
    }
}

// -----------------------------------------------------------------------------
// Result
// -----------------------------------------------------------------------------

pub struct PipelineResult {
    pub graph: Graph,
    pub languages: Vec<Language>,
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

#[derive(Debug)]
pub enum PipelineOutputError {
    /// Analysis failed before artifact writing.
    Analysis(PipelineError),
    /// The analyzed graph could not be written.
    Artifact { path: PathBuf, source: io::Error },
}

impl fmt::Display for PipelineOutputError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Analysis(error) => error.fmt(formatter),
            Self::Artifact { path, source } => {
                write!(formatter, "cannot write {}: {source}", path.display())
            }
        }
    }
}

impl Error for PipelineOutputError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Analysis(error) => Some(error),
            Self::Artifact { source, .. } => Some(source),
        }
    }
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

    use super::{PipelineConfig, PipelineError, PipelineOutputError, run, run_and_write};
    use crate::{
        analysis::{
            language::{Language, LanguageProfile},
            lsp::session::tests::FakeServer,
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
    fn preserves_an_existing_artifact_when_one_language_server_fails() {
        let repository = TestRepository::new();
        repository.write("app.py", "def run():\n    pass\n");
        repository.write("src/lib.rs", "pub fn run() {}\n");
        let output = repository.path().join("result.json");
        let sentinel = b"existing artifact";
        fs::write(&output, sentinel).expect("the existing artifact should be written");
        let server = FakeServer::new();
        let config = pipeline_config(&repository, &server, "protocol-error");

        let Err(error) = run_and_write(&config, &output) else {
            panic!("the Rust fake server should return an error");
        };

        assert!(
            matches!(
                error,
                PipelineOutputError::Analysis(PipelineError::Language {
                    language: Language::Rust,
                    ..
                })
            ),
            "the error should identify the failed adapter"
        );
        assert_eq!(
            fs::read(&output).expect("the existing artifact should remain readable"),
            sentinel,
            "a failed pipeline must preserve the requested artifact"
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
