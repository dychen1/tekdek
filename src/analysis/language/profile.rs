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
}

impl Language {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Python => "python",
            Self::Rust => "rust",
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RequiredCapability {
    DocumentSymbols,
    CallHierarchy,
    References,
    TypeDefinitions,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CapabilityPolicy {
    required: &'static [RequiredCapability],
}

impl CapabilityPolicy {
    pub const NONE: Self = Self::new(&[]);

    pub const SEMANTIC_GRAPH: Self = Self::new(&[
        RequiredCapability::DocumentSymbols,
        RequiredCapability::CallHierarchy,
        RequiredCapability::References,
        RequiredCapability::TypeDefinitions,
    ]);

    #[must_use]
    pub const fn new(required: &'static [RequiredCapability]) -> Self {
        Self { required }
    }

    #[must_use]
    pub fn requires(self, capability: RequiredCapability) -> bool {
        self.required.contains(&capability)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LanguageProfile {
    pub language: Language,
    pub enabled: bool,
    pub extensions: Vec<String>,
    pub language_id: String,
    pub server: ServerCommand,
    pub required_capabilities: CapabilityPolicy,
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
        required_capabilities: CapabilityPolicy::SEMANTIC_GRAPH,
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
        required_capabilities: CapabilityPolicy::SEMANTIC_GRAPH,
    });

    profiles
}

#[cfg(test)]
mod tests {
    use super::{CapabilityPolicy, Language, builtin_profiles};

    #[test]
    fn includes_only_profiles_for_compiled_adapters() {
        let languages = builtin_profiles()
            .into_iter()
            .map(|profile| profile.language)
            .collect::<Vec<_>>();
        let expected = [
            cfg!(feature = "python").then_some(Language::Python),
            cfg!(feature = "rust").then_some(Language::Rust),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();

        assert_eq!(languages, expected);
    }

    #[test]
    fn requires_semantic_graph_capabilities_for_each_profile() {
        assert!(
            builtin_profiles()
                .iter()
                .all(|profile| profile.required_capabilities == CapabilityPolicy::SEMANTIC_GRAPH)
        );
    }

    #[cfg(all(feature = "python", feature = "rust"))]
    #[test]
    fn orders_profiles_by_language_name() {
        let mut profiles = builtin_profiles();
        profiles.reverse();
        profiles.sort_by_key(|profile| profile.language);

        assert_eq!(profiles[0].language, Language::Python);
        assert_eq!(profiles[1].language, Language::Rust);
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
