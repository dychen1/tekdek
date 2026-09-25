//! Semantic Fact Models
//!
//! `SymbolAnchor` identifies one symbol reported by an LSP server.
//! `ReportedLocation` records one location reported by an LSP server.
//! Selected values use source paths and UTF-8 byte offsets.
//! Unselected values come from outside the selected sources.
//!
//! # Sections
//!
//! `Reports` defines symbols and locations.
//! `Resolution` defines symbol fallback rules.

use crate::analysis::source::{SourceRange, SourceSpan};

// -----------------------------------------------------------------------------
// Reports
// -----------------------------------------------------------------------------

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SymbolAnchor {
    Selected {
        path: String,
        range: SourceRange,
        fallback: ResolutionFallback,
        name: String,
    },
    Unselected,
}

impl SymbolAnchor {
    /// Returns the source span of a selected symbol.
    #[must_use]
    pub fn span(&self) -> Option<SourceSpan> {
        match self {
            Self::Selected { path, range, .. } => Some(SourceSpan {
                path: path.clone(),
                range: *range,
            }),
            Self::Unselected => None,
        }
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum ReportedLocation {
    Selected(SourceSpan),
    Unselected,
}

// -----------------------------------------------------------------------------
// Resolution
// -----------------------------------------------------------------------------

/// Controls symbol matching when no declaration matches.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ResolutionFallback {
    /// Requires an exact declaration.
    None,
    /// Drops a generated symbol without a declaration.
    Ignore,
    /// Uses the smallest containing lexical scope.
    ScopeOwner,
    /// Uses the smallest containing callable or module body.
    ///
    /// This keeps a lambda default in its enclosing body.
    CallableOrModuleBodyOwner,
}
