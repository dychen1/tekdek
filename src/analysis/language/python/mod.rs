//! Python fact adapter.
//!
//! This module owns the adapter that reports syntax and semantic facts from Python sources.
//!
//! `syntax` selects and parses Python sources. It reports modules, classes,
//! callables, fields, containment, imports, and declared metadata.
//! `server` runs Pyright. It reports resolved calls, references, and type targets.

mod server;
mod syntax;

use std::path::PathBuf;

use crate::analysis::{
    fact::{AnalysisError, SemanticFact, SyntaxFacts},
    lsp::LspError,
    source::{DocumentSnapshot, Sources},
};

use super::{Language, LanguageAdapter, LanguageProfile};

// -----------------------------------------------------------------------------
// Adapter
// -----------------------------------------------------------------------------

pub struct PythonAdapter {
    root: PathBuf,
    profile: LanguageProfile,
}

impl PythonAdapter {
    #[must_use]
    pub fn new(root: PathBuf, profile: LanguageProfile) -> Self {
        Self { root, profile }
    }
}

impl LanguageAdapter for PythonAdapter {
    fn language(&self) -> Language {
        Language::Python
    }

    /// Selects Python source files and prefers sources over matching stubs.
    ///
    /// # Errors
    ///
    /// Returns an error if a selected source path is invalid.
    fn select_sources<'a>(
        &self,
        documents: &'a [DocumentSnapshot],
    ) -> Result<Sources<'a>, AnalysisError> {
        syntax::select_sources(documents, &self.profile.extensions)
    }

    /// Produces Python syntax facts from selected sources.
    ///
    /// # Errors
    ///
    /// Returns an error if parsing or syntax extraction fails.
    fn produce_syntax_facts(&self, sources: &Sources<'_>) -> Result<SyntaxFacts, AnalysisError> {
        syntax::produce_syntax_facts(sources)
    }

    /// Produces normalized semantic facts from the Python language server.
    ///
    /// # Errors
    ///
    /// Returns an error if the configured language server fails.
    fn produce_semantic_facts(
        &mut self,
        sources: &Sources<'_>,
    ) -> Result<Vec<SemanticFact>, AnalysisError> {
        server::produce_semantic_facts(&self.root, &self.profile, sources)
            .map_err(language_server_error)
    }
}

fn language_server_error(error: LspError) -> AnalysisError {
    AnalysisError::new(format!(
        "cannot analyze Python with the language server: {}",
        error.into_message()
    ))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use serde_json::Value;

    use crate::analysis::{
        language::{CapabilityPolicy, Language, LanguageAdapter, LanguageProfile, ServerCommand},
        source::DocumentSnapshot,
    };

    use super::PythonAdapter;

    #[test]
    fn selects_configured_python_extensions() {
        let adapter = PythonAdapter::new(
            PathBuf::from("/workspace"),
            LanguageProfile {
                language: Language::Python,
                enabled: true,
                extensions: vec!["python".to_owned()],
                language_id: "python".to_owned(),
                server: ServerCommand {
                    program: "pyright-langserver".to_owned(),
                    arguments: vec!["--stdio".to_owned()],
                    initialization_options: Value::Null,
                },
                required_capabilities: CapabilityPolicy::SEMANTIC_GRAPH,
            },
        );
        let documents = [DocumentSnapshot {
            path: PathBuf::from("app.python"),
            content: "value = 1\n".to_owned(),
        }];

        let sources = adapter
            .select_sources(&documents)
            .expect("the configured source should be valid");

        assert_eq!(sources.len(), 1);
    }
}
