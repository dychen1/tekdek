//! Rust language-server policy.

use std::path::Path;

use lsp_types::SymbolKind;

use crate::analysis::{
    fact::{ResolutionFallback, SemanticFact, SyntaxFacts},
    language::{Language, LanguageProfile},
    lsp::{
        LspError, ProtocolSymbolSite,
        session::{self, ServerReadiness, SessionConfig},
    },
    source::Sources,
};

/// Produces semantic facts from one rust-analyzer session.
///
/// # Errors
///
/// Returns an error if the shared session fails.
pub(super) fn produce_semantic_facts(
    root: &Path,
    profile: &LanguageProfile,
    sources: &Sources<'_>,
    syntax: &SyntaxFacts,
) -> Result<Vec<SemanticFact>, LspError> {
    session::produce_semantic_facts(&session_config(root, profile), sources, syntax)
}

fn session_config<'a>(root: &'a Path, profile: &'a LanguageProfile) -> SessionConfig<'a> {
    SessionConfig {
        root,
        language: Language::Rust,
        language_id: &profile.language_id,
        server: &profile.server,
        fallback: resolution_fallback,
        // rust-analyzer answers semantic requests with empty results until
        // the workspace loads, so the session waits for a quiescent status.
        readiness: ServerReadiness::QuiescentServerStatus {
            timeout: READINESS_TIMEOUT,
        },
    }
}

const READINESS_TIMEOUT: std::time::Duration = std::time::Duration::from_mins(5);

/// Returns the resolution fallback for one rust-analyzer protocol symbol.
///
/// A variable describes lexical activity and resolves to its scope owner.
/// Every declaration must match exact Rust syntax or be ignored.
fn resolution_fallback(symbol: &ProtocolSymbolSite) -> ResolutionFallback {
    if symbol.kind == SymbolKind::VARIABLE {
        ResolutionFallback::ScopeOwner
    } else {
        ResolutionFallback::Ignore
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use lsp_types::{Position, Range, SymbolKind, Uri};
    use serde_json::Value;

    use super::{resolution_fallback, session_config};
    use crate::analysis::{
        fact::ResolutionFallback,
        language::{Language, LanguageProfile, ServerCommand},
        lsp::ProtocolSymbolSite,
    };

    #[test]
    fn passes_the_configured_rust_language_id_to_the_session() {
        let root = Path::new("/workspace");
        let profile = LanguageProfile {
            language: Language::Rust,
            enabled: true,
            extensions: vec!["rs".to_owned()],
            language_id: "custom-rust".to_owned(),
            server: ServerCommand {
                program: "rust-analyzer".to_owned(),
                arguments: Vec::new(),
                initialization_options: Value::Null,
            },
        };

        let config = session_config(root, &profile);

        assert_eq!(config.language_id, "custom-rust");
    }

    #[test]
    fn ignores_generated_rust_symbols_without_syntax_nodes() {
        let symbol = ProtocolSymbolSite {
            kind: SymbolKind::FUNCTION,
            name: "generated".to_owned(),
            uri: uri("file:///workspace/src/lib.rs"),
            selection: Range::new(Position::new(10, 4), Position::new(10, 13)),
        };

        assert_eq!(resolution_fallback(&symbol), ResolutionFallback::Ignore);
    }

    #[test]
    fn ignores_rust_graph_declarations_when_exact_matching_fails() {
        for kind in [
            SymbolKind::STRUCT,
            SymbolKind::ENUM,
            SymbolKind::INTERFACE,
            SymbolKind::FUNCTION,
            SymbolKind::METHOD,
            SymbolKind::FIELD,
        ] {
            let symbol = ProtocolSymbolSite {
                kind,
                name: "declared".to_owned(),
                uri: uri("file:///workspace/src/lib.rs"),
                selection: Range::new(Position::new(0, 0), Position::new(0, 8)),
            };

            assert_eq!(resolution_fallback(&symbol), ResolutionFallback::Ignore);
        }
    }

    #[test]
    fn scopes_rust_local_symbols_to_their_lexical_owner() {
        let symbol = ProtocolSymbolSite {
            kind: SymbolKind::VARIABLE,
            name: "value".to_owned(),
            uri: uri("file:///workspace/src/lib.rs"),
            selection: Range::new(Position::new(2, 8), Position::new(2, 13)),
        };

        assert_eq!(resolution_fallback(&symbol), ResolutionFallback::ScopeOwner);
    }

    fn uri(value: &str) -> Uri {
        value.parse().expect("the fixture URI should be valid")
    }
}
