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
//! `python` analyzes Python with Pyright.
//!
//! # Sections
//!
//! `Language contract` defines the adapter trait and the production order.
//! `Document paths` checks the source path rule that both syntax adapters share.

mod profile;
#[cfg(feature = "python")]
pub mod python;
#[cfg(feature = "rust")]
pub mod rust;

#[cfg(any(feature = "python", feature = "rust"))]
use std::path::Component;

use crate::analysis::{
    fact::{AnalysisError, FactSet, SemanticFact, SyntaxFacts},
    source::{DocumentSnapshot, Sources},
};

pub use profile::{Language, LanguageProfile, ServerCommand, builtin_profiles};
#[cfg(feature = "python")]
pub use python::PythonAdapter;
#[cfg(feature = "rust")]
pub use rust::RustAdapter;

// -----------------------------------------------------------------------------
// Language contract
// -----------------------------------------------------------------------------
//
// An adapter implements the three production steps. `produce_facts` fixes
// their order. Graph construction reads the result.

/// What one language must report to produce a graph.
///
/// An adapter selects its sources, reads its syntax, and captures its resolved
/// relations. It returns facts only. `graph::build` turns them into a graph.
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

    /// Produces normalized semantic facts for selected sources.
    ///
    /// A syntax-only language can use the default empty result.
    ///
    /// # Errors
    ///
    /// Returns an error if semantic analysis fails.
    fn produce_semantic_facts(
        &mut self,
        sources: &Sources<'_>,
    ) -> Result<Vec<SemanticFact>, AnalysisError> {
        let _ = sources;
        Ok(Vec::new())
    }

    /// Runs the fixed production order and returns every fact.
    ///
    /// The order is source selection, then syntax, then semantics. Semantic
    /// capture can start a language server, so it runs last.
    ///
    /// # Errors
    ///
    /// Returns an error when the adapter cannot complete a step.
    fn produce_facts(&mut self, documents: &[DocumentSnapshot]) -> Result<FactSet, AnalysisError> {
        let sources = self.select_sources(documents)?;
        let syntax = self.produce_syntax_facts(&sources)?;
        let semantic = self.produce_semantic_facts(&sources)?;

        Ok(FactSet::new(self.language(), syntax, semantic))
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
#[cfg(any(feature = "python", feature = "rust"))]
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
