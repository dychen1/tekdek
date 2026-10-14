//! Language Server Protocol integration.
//!
//! This module owns shared capture models, the resolution policy, and LSP
//! errors. `client` runs the server process and transports messages.
//! `session` starts one server and waits until it is ready. `capture` builds
//! and sends the requests of the resolver. `normalize` checks the captured
//! facts and converts them into semantic facts.
//!
//! # Sections
//!
//! `Captured Facts` defines the protocol capture models.
//! `Policy` defines resolution fallback selection.
//! `Errors` defines transport and protocol errors.

pub(crate) mod capture;
mod client;
#[cfg(test)]
pub(crate) mod fake_server;
pub(crate) mod normalize;
pub(crate) mod session;

use std::{error::Error, fmt, io};

use lsp_types::{DocumentSymbol, Location, Range, SymbolKind, Uri};

use crate::analysis::{fact::ResolutionFallback, source::SourceSpan};

pub(crate) use client::{LspClient, file_uri, uri_path};

// -----------------------------------------------------------------------------
// Captured Facts
// -----------------------------------------------------------------------------

/// Preserves protocol values, response grouping, and fact order until normalization.
pub(crate) struct SemanticCapture {
    pub(crate) documents: Vec<CapturedDocument>,
    pub(crate) facts: Vec<CapturedSemanticFact>,
}

/// The relative path identifies the document, exactly as it does in a source
/// snapshot. Normalization joins the two on that path.
pub(crate) struct CapturedDocument {
    pub(crate) path: String,
    pub(crate) uri: Uri,
    pub(crate) symbols: Vec<DocumentSymbol>,
}

/// A protocol symbol site that later becomes a `SymbolAnchor`.
///
/// The selection covers the declared name.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ProtocolSymbolSite {
    pub(crate) uri: Uri,
    pub(crate) selection: Range,
    pub(crate) kind: SymbolKind,
    pub(crate) name: String,
}

/// The vectors preserve each grouped protocol response.
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
    /// The query is the source span of one imported name.
    Definition {
        query: SourceSpan,
        targets: Vec<Location>,
    },
}

// -----------------------------------------------------------------------------
// Policy
// -----------------------------------------------------------------------------

/// Returns the resolution fallback for one captured protocol symbol.
pub(crate) type FallbackPolicy = fn(&ProtocolSymbolSite) -> ResolutionFallback;

// -----------------------------------------------------------------------------
// Errors
// -----------------------------------------------------------------------------

#[derive(Debug)]
pub struct LspError {
    message: String,
    /// True when a request to the server or its transport failed, so the server can be broken.
    server_failure: bool,
}

impl LspError {
    pub(crate) fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            server_failure: false,
        }
    }

    /// Marks the error as a failed request to the server or a failed transport.
    pub(crate) fn server_failure(self) -> Self {
        Self {
            server_failure: true,
            ..self
        }
    }

    pub(crate) fn is_server_failure(&self) -> bool {
        self.server_failure
    }

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
