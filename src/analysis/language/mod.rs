//! How each language reports facts about its source.
//!
//! Source documents are the input. One fact set is the output.
//!
//! This module defines the producer contract and the fixed production order.
//! Each adapter supplies language knowledge only. An adapter never builds a
//! graph, and this layer never depends on the graph layer.
//!
//! # Modules
//!
//! `python` analyzes Python with Pyright. `rust` analyzes Rust with
//! rust-analyzer. `typescript` analyzes TypeScript and JavaScript with the
//! TypeScript server. `profile` names each language and its built-in settings.
//!
//! # Sections
//!
//! `Language contract` defines the adapter trait and the production order.
//! `Document paths` checks the source path rule that every syntax adapter shares.

mod profile;
#[cfg(feature = "python")]
pub mod python;
#[cfg(feature = "rust")]
pub mod rust;
#[cfg(feature = "typescript")]
pub mod typescript;

use std::path::Component;

use crate::analysis::{
    fact::{AnalysisError, FactSet, SyntaxFacts},
    source::{DocumentSnapshot, Sources},
    timing,
};

pub use profile::{Language, LanguageProfile, ServerCommand, builtin_profiles};
#[cfg(feature = "python")]
pub use python::PythonAdapter;
#[cfg(feature = "rust")]
pub use rust::RustAdapter;
#[cfg(feature = "typescript")]
pub use typescript::TypeScriptAdapter;

// -----------------------------------------------------------------------------
// Language contract
// -----------------------------------------------------------------------------
//
// An adapter implements the two production steps. `produce_facts` fixes
// the order of the index steps. Graph construction reads the result.

/// What one language must report to produce a graph.
///
/// An adapter selects its sources and reads its syntax. It returns facts only.
/// `graph::build` turns them into a graph, and `resolve` asks the language
/// server of the profile for semantic edges.
pub trait LanguageAdapter {
    /// Returns the language that this adapter analyzes.
    fn language(&self) -> Language;

    /// Selects the source documents that this language analyzes.
    ///
    /// # Errors
    ///
    /// Returns an error if a selected source path is invalid.
    fn select_sources<'a>(
        &self,
        documents: &'a [DocumentSnapshot],
    ) -> Result<Sources<'a>, AnalysisError>;

    /// Produces facts from selected source syntax.
    ///
    /// # Errors
    ///
    /// Returns an error if parsing or syntax extraction fails.
    fn produce_syntax_facts(&self, sources: &Sources<'_>) -> Result<SyntaxFacts, AnalysisError>;

    /// Runs the index steps in order and returns the syntax facts.
    ///
    /// The order is source selection, then syntax. No language server starts:
    /// semantic facts come on demand, for the nodes that a consumer reads.
    ///
    /// # Errors
    ///
    /// Returns an error when the adapter cannot complete a step.
    fn produce_facts(&mut self, documents: &[DocumentSnapshot]) -> Result<FactSet, AnalysisError> {
        let started = timing::start();
        let sources = self.select_sources(documents)?;
        let syntax = self.produce_syntax_facts(&sources)?;
        timing::record_stage(started, || format!("{} syntax facts", self.language()));

        Ok(FactSet::new(self.language(), syntax))
    }
}

/// Returns the language-server session configuration of one profile.
///
/// A profile exists only for a compiled adapter, so the last arm cannot run.
pub(crate) fn session_config<'a>(
    root: &'a std::path::Path,
    profile: &'a LanguageProfile,
) -> Option<crate::analysis::lsp::session::SessionConfig<'a>> {
    match profile.language {
        #[cfg(feature = "python")]
        Language::Python => Some(python::server::session_config(root, profile)),
        #[cfg(feature = "rust")]
        Language::Rust => Some(rust::server::session_config(root, profile)),
        #[cfg(feature = "typescript")]
        Language::TypeScript => Some(typescript::server::session_config(root, profile)),
        #[allow(
            unreachable_patterns,
            reason = "a build with every language feature compiles every adapter"
        )]
        _ => None,
    }
}

/// Returns the language id that opens one document in its server.
///
/// The TypeScript server parses a document by its id, so its id follows the
/// extension. Every other language keeps the configured id.
pub(crate) fn document_language_id<'a>(
    language: Language,
    configured: &'a str,
    path: &str,
) -> &'a str {
    match language {
        #[cfg(feature = "typescript")]
        Language::TypeScript => {
            typescript::server::document_language_id(path).unwrap_or(configured)
        }
        #[allow(
            unreachable_patterns,
            reason = "a build without the typescript feature has no dialect ids"
        )]
        _ => configured,
    }
}

// -----------------------------------------------------------------------------
// Document paths
// -----------------------------------------------------------------------------

/// Returns one strict relative UTF-8 document path.
///
/// # Errors
///
/// Returns an error if the path is absolute, has a part that is not a normal
/// name, or is not valid UTF-8.
fn document_path(document: &DocumentSnapshot) -> Result<&str, AnalysisError> {
    if document.path.is_absolute() {
        return Err(AnalysisError::new(format!(
            "the document path must be relative: {}",
            document.path.display()
        )));
    }
    if document
        .path
        .components()
        .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(AnalysisError::new(format!(
            "the document path must contain only normal components: {}",
            document.path.display()
        )));
    }

    document.path.to_str().ok_or_else(|| {
        AnalysisError::new(format!(
            "the document path is not valid UTF-8: {}",
            document.path.display()
        ))
    })
}
