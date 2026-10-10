//! Rust fact adapter.
//!
//! This module owns the adapter that reports syntax and semantic facts from Rust sources.

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

pub struct RustAdapter {
    root: PathBuf,
    profile: LanguageProfile,
}

impl RustAdapter {
    #[must_use]
    pub fn new(root: PathBuf, profile: LanguageProfile) -> Self {
        Self { root, profile }
    }
}

impl LanguageAdapter for RustAdapter {
    fn language(&self) -> Language {
        Language::Rust
    }

    /// Selects Rust source files.
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

    /// Produces native Rust syntax facts.
    ///
    /// # Errors
    ///
    /// Returns an error if parsing or syntax extraction fails.
    fn produce_syntax_facts(&self, sources: &Sources<'_>) -> Result<SyntaxFacts, AnalysisError> {
        syntax::produce_syntax_facts(sources)
    }

    /// Produces normalized semantic facts from rust-analyzer.
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
        "cannot analyze Rust with the language server: {}",
        error.into_message()
    ))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use crate::analysis::{
        language::{Language, LanguageAdapter, builtin_profiles},
        source::{DocumentSnapshot, Sources},
    };

    use super::RustAdapter;

    #[test]
    fn produces_rust_syntax_facts() {
        let profile = builtin_profiles()
            .into_iter()
            .find(|profile| profile.language == Language::Rust)
            .expect("the Rust profile must exist");
        let adapter = RustAdapter::new(PathBuf::from("/workspace"), profile);
        let documents = [DocumentSnapshot {
            path: PathBuf::from("lib.rs"),
            content: "pub struct Item;".to_owned(),
        }];
        let sources = adapter
            .select_sources(&documents)
            .expect("Rust source selection must succeed");

        let facts = adapter
            .produce_syntax_facts(&sources)
            .expect("Rust syntax fact production must succeed");

        assert_eq!(facts.entities.len(), 2);
    }

    #[test]
    fn selects_configured_rust_extensions() {
        let mut profile = builtin_profiles()
            .into_iter()
            .find(|profile| profile.language == Language::Rust)
            .expect("the Rust profile must exist");
        profile.extensions = vec!["rust".to_owned()];
        let adapter = RustAdapter::new(PathBuf::from("/workspace"), profile);
        let documents = [DocumentSnapshot {
            path: PathBuf::from("lib.rust"),
            content: "pub struct Item;".to_owned(),
        }];

        let sources = adapter
            .select_sources(&documents)
            .expect("the configured source should be valid");

        assert_eq!(sources.len(), 1);
    }

    #[test]
    fn maps_rust_language_server_errors() {
        let profile = builtin_profiles()
            .into_iter()
            .find(|profile| profile.language == Language::Rust)
            .expect("the Rust profile must exist");
        let mut adapter = RustAdapter::new(PathBuf::from("/missing-rust-root"), profile);
        let sources = Sources::new(Vec::new());

        let error = adapter
            .produce_semantic_facts(&sources)
            .expect_err("a missing Rust root must fail");

        assert!(
            error
                .message
                .starts_with("cannot analyze Rust with the language server: ")
        );
    }
}
