//! User configuration.
//!
//! This module loads strict language overrides and applies command-line overrides.

use std::{
    collections::BTreeMap,
    error::Error,
    fmt, fs, io,
    path::{Path, PathBuf},
};

use serde::Deserialize;

use crate::analysis::language::{Language, LanguageProfile, builtin_profiles};

/// Loads built-in profiles and applies one optional configuration file.
///
/// A missing default file does not change the built-in profiles. A missing
/// explicit file returns an error.
///
/// # Errors
///
/// Returns an error if the file cannot be read or its content is invalid.
pub fn configured_profiles(path: Option<&Path>) -> Result<Vec<LanguageProfile>, ConfigError> {
    let explicit = path.is_some();
    let path = path.map_or_else(default_config_path, |path| Some(path.to_path_buf()));
    let Some(path) = path else {
        return Ok(builtin_profiles());
    };

    let document = match fs::read_to_string(&path) {
        Ok(document) => document,
        Err(error) if !explicit && error.kind() == io::ErrorKind::NotFound => {
            return Ok(builtin_profiles());
        }
        Err(error) => return Err(ConfigError::read(path, error)),
    };

    profiles_from_toml(&document)
}

/// Applies language server command-line overrides.
///
/// Server program overrides are applied in input order. The last program for
/// one language wins. Argument overrides then replace configured arguments.
///
/// # Errors
///
/// Returns an error if an override names an adapter that is not in this build.
pub fn apply_cli_overrides(
    profiles: &mut [LanguageProfile],
    servers: &[(Language, String)],
    arguments: &BTreeMap<Language, Vec<String>>,
) -> Result<(), ConfigError> {
    for (language, program) in servers {
        profile_mut(profiles, *language)?
            .server
            .program
            .clone_from(program);
    }
    for (language, values) in arguments {
        profile_mut(profiles, *language)?
            .server
            .arguments
            .clone_from(values);
    }

    Ok(())
}

#[derive(Debug)]
pub enum ConfigError {
    Read { path: PathBuf, source: io::Error },
    Parse(toml::de::Error),
    Invalid(String),
}

impl ConfigError {
    fn read(path: PathBuf, source: io::Error) -> Self {
        Self::Read { path, source }
    }
}

impl fmt::Display for ConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Read { path, source } => {
                write!(
                    formatter,
                    "cannot read configuration {}: {source}",
                    path.display()
                )
            }
            Self::Parse(error) => write!(formatter, "invalid configuration: {error}"),
            Self::Invalid(message) => formatter.write_str(message),
        }
    }
}

impl Error for ConfigError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Read { source, .. } => Some(source),
            Self::Parse(error) => Some(error),
            Self::Invalid(_) => None,
        }
    }
}

impl From<toml::de::Error> for ConfigError {
    fn from(error: toml::de::Error) -> Self {
        Self::Parse(error)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UserConfig {
    #[serde(default)]
    languages: BTreeMap<String, LanguageOverride>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LanguageOverride {
    enabled: Option<bool>,
    extensions: Option<Vec<String>>,
    language_id: Option<String>,
    program: Option<String>,
    arguments: Option<Vec<String>>,
    initialization_options: Option<toml::Value>,
}

fn profiles_from_toml(document: &str) -> Result<Vec<LanguageProfile>, ConfigError> {
    let config: UserConfig = toml::from_str(document)?;
    let mut profiles = builtin_profiles();

    for (name, profile_override) in config.languages {
        let language = Language::parse(&name).map_err(ConfigError::Invalid)?;
        let profile = profiles
            .iter_mut()
            .find(|profile| profile.language == language)
            .ok_or_else(|| unavailable_language(language))?;
        apply_profile_override(profile, profile_override)?;
    }

    Ok(profiles)
}

fn apply_profile_override(
    profile: &mut LanguageProfile,
    profile_override: LanguageOverride,
) -> Result<(), ConfigError> {
    if let Some(enabled) = profile_override.enabled {
        profile.enabled = enabled;
    }
    if let Some(extensions) = profile_override.extensions {
        profile.extensions = extensions;
    }
    if let Some(language_id) = profile_override.language_id {
        profile.language_id = language_id;
    }
    if let Some(program) = profile_override.program {
        profile.server.program = program;
    }
    if let Some(arguments) = profile_override.arguments {
        profile.server.arguments = arguments;
    }
    if let Some(options) = profile_override.initialization_options {
        profile.server.initialization_options = serde_json::to_value(options).map_err(|error| {
            ConfigError::Invalid(format!("invalid initialization options: {error}"))
        })?;
    }

    Ok(())
}

fn default_config_path() -> Option<PathBuf> {
    dirs::config_dir().map(|path| path.join("tekdek/config.toml"))
}

fn profile_mut(
    profiles: &mut [LanguageProfile],
    language: Language,
) -> Result<&mut LanguageProfile, ConfigError> {
    profiles
        .iter_mut()
        .find(|profile| profile.language == language)
        .ok_or_else(|| unavailable_language(language))
}

fn unavailable_language(language: Language) -> ConfigError {
    ConfigError::Invalid(format!(
        "language is not available in this build: {language}"
    ))
}

#[cfg(test)]
mod tests {
    use std::{
        collections::BTreeMap,
        fs,
        path::PathBuf,
        sync::atomic::{AtomicUsize, Ordering},
    };

    use super::{apply_cli_overrides, configured_profiles, profiles_from_toml};
    use crate::analysis::language::builtin_profiles;

    #[cfg(feature = "rust")]
    #[test]
    fn overrides_one_builtin_profile() {
        use crate::analysis::language::Language;

        let document = r#"
            [languages.rust]
            program = "/opt/rust-analyzer"
            arguments = ["--log-file", "/tmp/ra.log"]
        "#;

        let profiles = profiles_from_toml(document).expect("the configuration should be valid");
        let rust = profile(&profiles, Language::Rust);

        assert_eq!(rust.server.program, "/opt/rust-analyzer");
        assert_eq!(rust.server.arguments, ["--log-file", "/tmp/ra.log"]);
    }

    #[test]
    fn rejects_unknown_configuration_fields() {
        let error = profiles_from_toml("[languages.rust]\nunknown = true")
            .expect_err("the unknown field should fail");

        assert!(error.to_string().contains("unknown field"));
    }

    #[test]
    fn rejects_unknown_languages() {
        let error = profiles_from_toml("[languages.go]\nenabled = true")
            .expect_err("the unknown language should fail");

        assert_eq!(error.to_string(), "unsupported language: go");
    }

    #[test]
    fn reports_a_missing_explicit_file() {
        let path = test_path("missing");
        let error =
            configured_profiles(Some(&path)).expect_err("the explicit file should be required");

        assert!(error.to_string().contains("cannot read configuration"));
        assert!(error.to_string().contains(&path.display().to_string()));
    }

    #[test]
    fn loads_an_explicit_file() {
        let path = test_path("configured");
        let language = builtin_profiles()
            .first()
            .expect("this test requires a compiled adapter")
            .language;
        fs::write(
            &path,
            format!("[languages.{language}]\nprogram = \"configured-server\"\n"),
        )
        .expect("the test configuration should be written");

        let profiles =
            configured_profiles(Some(&path)).expect("the explicit configuration should load");
        fs::remove_file(&path).expect("the test configuration should be removed");

        assert_eq!(
            profile(&profiles, language).server.program,
            "configured-server"
        );
    }

    #[cfg(feature = "rust")]
    #[test]
    fn converts_initialization_options_to_json() {
        use crate::analysis::language::Language;

        let document = r"
            [languages.rust.initialization_options]
            diagnostics = { enable = true }
        ";

        let profiles = profiles_from_toml(document).expect("the configuration should be valid");
        let options = &profile(&profiles, Language::Rust)
            .server
            .initialization_options;

        assert_eq!(options["diagnostics"]["enable"], true);
    }

    #[test]
    fn applies_cli_overrides_after_configured_values() {
        let mut profiles = builtin_profiles();
        let language = profiles
            .first()
            .expect("this test requires a compiled adapter")
            .language;
        let mut arguments = BTreeMap::new();
        arguments.insert(language, vec!["cli-argument".to_owned()]);

        apply_cli_overrides(
            &mut profiles,
            &[
                (language, "first-server".to_owned()),
                (language, "last-server".to_owned()),
            ],
            &arguments,
        )
        .expect("the compiled language should accept overrides");
        let profile = profile(&profiles, language);

        assert_eq!(profile.server.program, "last-server");
        assert_eq!(profile.server.arguments, ["cli-argument"]);
    }

    fn profile(
        profiles: &[crate::analysis::language::LanguageProfile],
        language: crate::analysis::language::Language,
    ) -> &crate::analysis::language::LanguageProfile {
        profiles
            .iter()
            .find(|profile| profile.language == language)
            .expect("the language profile should exist")
    }

    fn test_path(label: &str) -> PathBuf {
        static NEXT_PATH: AtomicUsize = AtomicUsize::new(0);

        std::env::temp_dir().join(format!(
            "tekdek-{label}-{}-{}",
            std::process::id(),
            NEXT_PATH.fetch_add(1, Ordering::Relaxed)
        ))
    }
}
