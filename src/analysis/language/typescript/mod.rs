//! TypeScript and JavaScript fact adapter.
//!
//! This module owns the adapter that reports syntax facts from TypeScript and
//! JavaScript sources. One parser reads both languages.
//!
//! `syntax` selects and parses sources. It reports modules, namespaces,
//! classes, interfaces, enums, callables, fields, containment, import sites,
//! and declared metadata.
//! `server` configures the TypeScript session and the fallback of its symbols.

pub(crate) mod server;
mod syntax;

use crate::analysis::{
    fact::{AnalysisError, SyntaxFacts},
    source::{DocumentSnapshot, Sources},
};

use super::{Language, LanguageAdapter, LanguageProfile};

// -----------------------------------------------------------------------------
// Adapter
// -----------------------------------------------------------------------------

pub struct TypeScriptAdapter {
    profile: LanguageProfile,
}

impl TypeScriptAdapter {
    #[must_use]
    pub fn new(profile: LanguageProfile) -> Self {
        Self { profile }
    }
}

impl LanguageAdapter for TypeScriptAdapter {
    fn language(&self) -> Language {
        Language::TypeScript
    }

    /// Selects TypeScript and JavaScript sources and drops declaration files.
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

    /// Produces syntax facts from selected sources.
    ///
    /// # Errors
    ///
    /// Returns an error if a source does not parse.
    fn produce_syntax_facts(&self, sources: &Sources<'_>) -> Result<SyntaxFacts, AnalysisError> {
        syntax::produce_syntax_facts(sources)
    }
}
