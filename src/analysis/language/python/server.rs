//! Python language-server policy.
//!
//! This module owns Python session configuration and symbol fallback.

use std::path::Path;

use lsp_types::SymbolKind;

use crate::analysis::{
    fact::ResolutionFallback,
    language::{Language, LanguageProfile},
    lsp::{
        ProtocolSymbolSite,
        session::{self, SessionConfig},
    },
};

/// Returns the session configuration of one Python profile.
pub(crate) fn session_config<'a>(
    root: &'a Path,
    profile: &'a LanguageProfile,
) -> SessionConfig<'a> {
    SessionConfig {
        root,
        language: Language::Python,
        language_id: &profile.language_id,
        server: &profile.server,
        fallback: resolution_fallback,
        // Pyright analyzes each document before it answers a request.
        readiness: session::ServerReadiness::Immediate,
    }
}

/// Returns the resolution fallback for one Pyright protocol symbol.
///
/// A declared class, callable, or field must resolve to its exact declaration.
/// A lambda resolves to its enclosing callable or module. Every other symbol
/// resolves to its smallest lexical scope.
fn resolution_fallback(symbol: &ProtocolSymbolSite) -> ResolutionFallback {
    if symbol.name == LAMBDA_NAME {
        return ResolutionFallback::CallableOrModuleBodyOwner;
    }

    if matches!(
        symbol.kind,
        SymbolKind::CLASS
            | SymbolKind::FUNCTION
            | SymbolKind::METHOD
            | SymbolKind::CONSTRUCTOR
            | SymbolKind::FIELD
    ) {
        ResolutionFallback::None
    } else {
        ResolutionFallback::ScopeOwner
    }
}

/// The name Pyright gives a lambda.
const LAMBDA_NAME: &str = "(lambda)";

#[cfg(test)]
mod tests {
    use lsp_types::{Position, Range, SymbolKind};

    use super::{LAMBDA_NAME, ResolutionFallback, resolution_fallback};
    use crate::analysis::lsp::ProtocolSymbolSite;

    #[test]
    fn requires_exact_declarations_for_declared_symbols() {
        for kind in [
            SymbolKind::CLASS,
            SymbolKind::FUNCTION,
            SymbolKind::METHOD,
            SymbolKind::CONSTRUCTOR,
            SymbolKind::FIELD,
        ] {
            assert_eq!(
                resolution_fallback(&symbol("declared", kind)),
                ResolutionFallback::None
            );
        }
    }

    #[test]
    fn resolves_lambdas_to_the_enclosing_callable_or_module() {
        assert_eq!(
            resolution_fallback(&symbol(LAMBDA_NAME, SymbolKind::FUNCTION)),
            ResolutionFallback::CallableOrModuleBodyOwner
        );
    }

    #[test]
    fn resolves_other_symbols_to_the_smallest_scope() {
        assert_eq!(
            resolution_fallback(&symbol("value", SymbolKind::VARIABLE)),
            ResolutionFallback::ScopeOwner
        );
    }

    fn symbol(name: &str, kind: SymbolKind) -> ProtocolSymbolSite {
        ProtocolSymbolSite {
            uri: "file:///workspace/app.py"
                .parse()
                .expect("the fixture URI should be valid"),
            selection: Range::new(Position::new(0, 0), Position::new(0, 1)),
            kind,
            name: name.to_owned(),
        }
    }
}
