//! Language profiles.
//!
//! This module owns stable language names and built-in adapter settings.

use std::fmt;

use serde_json::Value;
#[cfg(feature = "rust")]
use serde_json::json;

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Language {
    Python,
    Rust,
    TypeScript,
}

impl Language {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Python => "python",
            Self::Rust => "rust",
            Self::TypeScript => "typescript",
        }
    }

    /// Returns the command that installs the default language server.
    #[must_use]
    pub const fn server_install_command(self) -> &'static str {
        match self {
            Self::Python => "npm install -g pyright",
            Self::Rust => "rustup component add rust-analyzer",
            Self::TypeScript => "npm install -g typescript",
        }
    }

    /// Parses one stable language name.
    ///
    /// # Errors
    ///
    /// Returns an error if the language is not supported.
    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "python" => Ok(Self::Python),
            "rust" => Ok(Self::Rust),
            "typescript" => Ok(Self::TypeScript),
            _ => Err(format!("unsupported language: {value}")),
        }
    }
}

impl fmt::Display for Language {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ServerCommand {
    pub program: String,
    pub arguments: Vec<String>,
    pub initialization_options: Value,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LanguageProfile {
    pub language: Language,
    pub enabled: bool,
    pub extensions: Vec<String>,
    pub language_id: String,
    pub server: ServerCommand,
}

/// Returns profiles for the adapters in this build.
#[must_use]
#[allow(
    unused_mut,
    clippy::vec_init_then_push,
    reason = "the language features decide how many profiles the list gets"
)]
pub fn builtin_profiles() -> Vec<LanguageProfile> {
    let mut profiles = Vec::new();

    #[cfg(feature = "python")]
    profiles.push(LanguageProfile {
        language: Language::Python,
        enabled: true,
        extensions: vec!["py".to_owned(), "pyi".to_owned()],
        language_id: "python".to_owned(),
        server: ServerCommand {
            program: "pyright-langserver".to_owned(),
            arguments: vec!["--stdio".to_owned()],
            initialization_options: Value::Null,
        },
    });

    #[cfg(feature = "rust")]
    profiles.push(LanguageProfile {
        language: Language::Rust,
        enabled: true,
        extensions: vec!["rs".to_owned()],
        language_id: "rust".to_owned(),
        server: ServerCommand {
            program: "rust-analyzer".to_owned(),
            arguments: Vec::new(),
            initialization_options: json!({
                "cargo": {"buildScripts": {"enable": false}},
                "procMacro": {"enable": false},
                "checkOnSave": false,
                "cachePriming": {"enable": false},
            }),
        },
    });

    // One profile reads JavaScript too: the same parser and server handle both.
    #[cfg(feature = "typescript")]
    profiles.push(LanguageProfile {
        language: Language::TypeScript,
        enabled: true,
        extensions: ["ts", "tsx", "mts", "cts", "js", "jsx", "mjs", "cjs"]
            .map(str::to_owned)
            .to_vec(),
        language_id: "typescript".to_owned(),
        server: ServerCommand {
            program: "tsc".to_owned(),
            arguments: vec!["--lsp".to_owned(), "--stdio".to_owned()],
            initialization_options: Value::Null,
        },
    });

    profiles
}

#[cfg(test)]
mod tests {
    use super::{Language, builtin_profiles};

    #[test]
    fn includes_only_profiles_for_compiled_adapters() {
        let languages = builtin_profiles()
            .into_iter()
            .map(|profile| profile.language)
            .collect::<Vec<_>>();
        let expected = [
            cfg!(feature = "python").then_some(Language::Python),
            cfg!(feature = "rust").then_some(Language::Rust),
            cfg!(feature = "typescript").then_some(Language::TypeScript),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();

        assert_eq!(languages, expected);
    }

    #[cfg(all(feature = "python", feature = "rust", feature = "typescript"))]
    #[test]
    fn orders_profiles_by_language_name() {
        let mut profiles = builtin_profiles();
        profiles.reverse();
        profiles.sort_by_key(|profile| profile.language);

        assert_eq!(profiles[0].language, Language::Python);
        assert_eq!(profiles[1].language, Language::Rust);
        assert_eq!(profiles[2].language, Language::TypeScript);
    }

    #[test]
    fn parses_every_language_name() {
        for language in [Language::Python, Language::Rust, Language::TypeScript] {
            assert_eq!(Language::parse(language.as_str()), Ok(language));
        }
    }

    #[cfg(feature = "rust")]
    #[test]
    fn disables_rust_repository_execution_by_default() {
        let rust = builtin_profiles()
            .into_iter()
            .find(|profile| profile.language == Language::Rust)
            .expect("the Rust profile should exist");

        assert_eq!(rust.server.program, "rust-analyzer");
        assert_eq!(
            rust.server.initialization_options["cargo"]["buildScripts"]["enable"],
            false
        );
        assert_eq!(
            rust.server.initialization_options["procMacro"]["enable"],
            false
        );
        assert_eq!(rust.server.initialization_options["checkOnSave"], false);
        assert_eq!(
            rust.server.initialization_options["cachePriming"]["enable"],
            false
        );
    }
}
