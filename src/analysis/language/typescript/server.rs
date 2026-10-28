//! TypeScript language-server policy.
//!
//! This module owns TypeScript session configuration, document language ids,
//! and symbol fallback.

use std::{ffi::OsStr, path::Path};

use lsp_types::SymbolKind;

use crate::analysis::{
    fact::ResolutionFallback,
    language::{Language, LanguageProfile},
    lsp::{
        ProtocolSymbolSite,
        session::{self, SessionConfig},
    },
};

/// Returns the session configuration of one TypeScript profile.
pub(crate) fn session_config<'a>(
    root: &'a Path,
    profile: &'a LanguageProfile,
) -> SessionConfig<'a> {
    SessionConfig {
        root,
        language: Language::TypeScript,
        language_id: &profile.language_id,
        server: &profile.server,
        fallback: resolution_fallback,
        // The TypeScript server loads the project of a document when it opens.
        readiness: session::ServerReadiness::Immediate,
    }
}

/// Returns the dialect id that the server parses one document with, or
/// `None` when the configured id applies.
pub(crate) fn document_language_id(path: &str) -> Option<&'static str> {
    match Path::new(path).extension().and_then(OsStr::to_str) {
        Some("tsx") => Some("typescriptreact"),
        Some("js" | "mjs" | "cjs") => Some("javascript"),
        Some("jsx") => Some("javascriptreact"),
        _ => None,
    }
}

/// Returns the resolution fallback for one TypeScript protocol symbol.
///
/// The fallback applies only when no declaration matches. A function without
/// a declaration is a callback, an object method, or an anonymous default
/// export: it resolves to its enclosing callable or module. Every other
/// symbol resolves to its smallest lexical scope.
fn resolution_fallback(symbol: &ProtocolSymbolSite) -> ResolutionFallback {
    if matches!(
        symbol.kind,
        SymbolKind::FUNCTION | SymbolKind::METHOD | SymbolKind::CONSTRUCTOR
    ) {
        ResolutionFallback::CallableOrModuleBodyOwner
    } else {
        ResolutionFallback::ScopeOwner
    }
}

#[cfg(test)]
mod tests {
    use lsp_types::{Position, Range, SymbolKind};

    use super::{ResolutionFallback, resolution_fallback};
    use crate::analysis::{
        language::{Language, document_language_id},
        lsp::ProtocolSymbolSite,
    };

    #[test]
    fn opens_typescript_documents_with_the_id_of_their_dialect() {
        for (path, id) in [
            ("src/a.ts", "typescript"),
            ("src/a.mts", "typescript"),
            ("src/a.tsx", "typescriptreact"),
            ("src/a.js", "javascript"),
            ("src/a.cjs", "javascript"),
            ("src/a.jsx", "javascriptreact"),
        ] {
            assert_eq!(
                document_language_id(Language::TypeScript, "typescript", path),
                id,
                "{path}"
            );
        }
        // A custom id keeps the dialect mapping; other languages keep their id.
        assert_eq!(
            document_language_id(Language::TypeScript, "custom", "src/a.tsx"),
            "typescriptreact"
        );
        assert_eq!(
            document_language_id(Language::Python, "python", "src/a.js"),
            "python"
        );
    }

    #[test]
    fn folds_undeclared_functions_into_the_enclosing_callable_or_module() {
        for (name, kind) in [
            ("map() callback", SymbolKind::FUNCTION),
            ("<function>", SymbolKind::FUNCTION),
            ("run", SymbolKind::METHOD),
        ] {
            assert_eq!(
                resolution_fallback(&symbol(name, kind)),
                ResolutionFallback::CallableOrModuleBodyOwner
            );
        }
    }

    #[test]
    fn resolves_other_symbols_to_the_smallest_scope() {
        for kind in [
            SymbolKind::VARIABLE,
            SymbolKind::PROPERTY,
            SymbolKind::CLASS,
        ] {
            assert_eq!(
                resolution_fallback(&symbol("value", kind)),
                ResolutionFallback::ScopeOwner
            );
        }
    }

    fn symbol(name: &str, kind: SymbolKind) -> ProtocolSymbolSite {
        ProtocolSymbolSite {
            uri: "file:///workspace/app.ts"
                .parse()
                .expect("the fixture URI should be valid"),
            selection: Range::new(Position::new(0, 0), Position::new(0, 1)),
            kind,
            name: name.to_owned(),
        }
    }
}
