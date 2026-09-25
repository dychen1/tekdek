//! Language Server Protocol integration.
//!
//! This module owns shared capture models, the resolution policy, and LSP
//! errors. `client` transports messages and converts coordinates. `capture`
//! requests and normalizes semantic facts.
//!
//! # Sections
//!
//! `Captured Facts` defines the protocol capture models.
//! `Policy` defines resolution fallback selection.
//! `Errors` defines transport and protocol errors.

#[cfg(any(feature = "python", feature = "rust"))]
pub(crate) mod capture;
mod client;
#[cfg(any(feature = "python", feature = "rust"))]
pub(crate) mod session;

use std::{error::Error, fmt, io};

#[cfg(any(feature = "python", feature = "rust"))]
use lsp_types::{DocumentSymbol, Location, Range, SymbolKind, Uri};

#[cfg(any(feature = "python", feature = "rust"))]
use crate::analysis::fact::ResolutionFallback;

#[cfg(any(feature = "python", feature = "rust"))]
pub(crate) use client::position_to_byte_offset;
pub use client::{LspClient, file_uri, uri_path};

// -----------------------------------------------------------------------------
// Captured Facts
// -----------------------------------------------------------------------------

/// Preserves protocol values, response grouping, and fact order until normalization.
#[cfg(any(feature = "python", feature = "rust"))]
pub(crate) struct SemanticCapture {
    pub(crate) documents: Vec<CapturedDocument>,
    pub(crate) facts: Vec<CapturedSemanticFact>,
}

/// The relative path identifies the document, exactly as it does in a source
/// snapshot. Normalization joins the two on that path.
#[cfg(any(feature = "python", feature = "rust"))]
pub(crate) struct CapturedDocument {
    pub(crate) path: String,
    pub(crate) uri: Uri,
    pub(crate) symbols: Vec<DocumentSymbol>,
}

/// A protocol symbol site that later becomes a `SymbolAnchor`.
///
/// The selection covers the declared name.
#[cfg(any(feature = "python", feature = "rust"))]
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ProtocolSymbolSite {
    pub(crate) uri: Uri,
    pub(crate) selection: Range,
    pub(crate) kind: SymbolKind,
    pub(crate) name: String,
}

/// The vectors preserve each grouped protocol response.
#[cfg(any(feature = "python", feature = "rust"))]
#[derive(Debug, PartialEq)]
pub(crate) enum CapturedSemanticFact {
    Call {
        caller: ProtocolSymbolSite,
        callee: ProtocolSymbolSite,
        call_sites: Vec<Location>,
    },
    Reference {
        target: ProtocolSymbolSite,
        occurrences: Vec<Location>,
    },
    TypeDefinition {
        queried_symbol: ProtocolSymbolSite,
        targets: Vec<Location>,
    },
}

// -----------------------------------------------------------------------------
// Policy
// -----------------------------------------------------------------------------

/// Returns the resolution fallback for one captured protocol symbol.
#[cfg(any(feature = "python", feature = "rust"))]
pub(crate) type FallbackPolicy = fn(&ProtocolSymbolSite) -> ResolutionFallback;

// -----------------------------------------------------------------------------
// Errors
// -----------------------------------------------------------------------------

#[derive(Debug)]
pub struct LspError {
    message: String,
}

impl LspError {
    pub(crate) fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    #[cfg(any(feature = "python", feature = "rust"))]
    pub(crate) fn into_message(self) -> String {
        self.message
    }

    pub(crate) fn timeout(method: &str) -> Self {
        Self::new(format!("the {method} request timed out"))
    }
}

impl fmt::Display for LspError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl Error for LspError {}

impl From<io::Error> for LspError {
    fn from(error: io::Error) -> Self {
        Self::new(error.to_string())
    }
}

impl From<serde_json::Error> for LspError {
    fn from(error: serde_json::Error) -> Self {
        Self::new(error.to_string())
    }
}
