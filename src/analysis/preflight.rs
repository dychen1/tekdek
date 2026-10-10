//! Language preflight.
//!
//! This module selects the languages that one analysis runs and checks that
//! their language server programs exist. Language profiles and loaded
//! documents are the inputs. Detected languages or a setup error are the output.
//!
//! # Sections
//!
//! `Detection` selects enabled profiles with matching sources.
//! `Server programs` finds each server program on the search path.

use super::{
    PipelineError,
    language::{Language, LanguageProfile},
    program::find_program,
    source::{self, DocumentSnapshot},
};

// -----------------------------------------------------------------------------
// Detection
// -----------------------------------------------------------------------------

/// Returns the sorted, distinct extensions of enabled profiles.
pub(super) fn enabled_extensions(profiles: &[LanguageProfile]) -> Vec<String> {
    let mut extensions = profiles
        .iter()
        .filter(|profile| profile.enabled)
        .flat_map(|profile| profile.extensions.iter().cloned())
        .collect::<Vec<_>>();
    extensions.sort();
    extensions.dedup();
    extensions
}

/// Returns the detected languages in stable order.
///
/// # Errors
///
/// Returns a setup error when no enabled language has a source file.
pub(super) fn detect_required_languages(
    documents: &[DocumentSnapshot],
    profiles: &[LanguageProfile],
) -> Result<Vec<Language>, PipelineError> {
    let languages = detect_languages(documents, profiles);
    if languages.is_empty() {
        return Err(PipelineError::Setup(
            "the codebase has no enabled supported source files".to_owned(),
        ));
    }
    Ok(languages)
}

/// Returns the profile of one detected language.
///
/// # Errors
///
/// Returns a setup error when no profile names the language.
pub(super) fn profile(
    profiles: &[LanguageProfile],
    language: Language,
) -> Result<&LanguageProfile, PipelineError> {
    profiles
        .iter()
        .find(|profile| profile.language == language)
        .ok_or_else(|| {
            PipelineError::Setup(format!("the detected language has no profile: {language}"))
        })
}

pub(super) fn detect_languages(
    documents: &[DocumentSnapshot],
    profiles: &[LanguageProfile],
) -> Vec<Language> {
    let mut languages = profiles
        .iter()
        .filter(|profile| {
            profile.enabled
                && documents
                    .iter()
                    .any(|document| source::document_has_extension(document, &profile.extensions))
        })
        .map(|profile| profile.language)
        .collect::<Vec<_>>();
    languages.sort();
    languages.dedup();
    languages
}

// -----------------------------------------------------------------------------
// Server programs
// -----------------------------------------------------------------------------

/// Checks that every detected language has an executable server program.
///
/// # Errors
///
/// Returns one setup error that lists every missing program.
pub(super) fn preflight_server_programs(
    profiles: &[LanguageProfile],
    languages: &[Language],
) -> Result<(), PipelineError> {
    preflight_server_programs_with(profiles, languages, program_is_available)
}

fn preflight_server_programs_with(
    profiles: &[LanguageProfile],
    languages: &[Language],
    mut is_available: impl FnMut(&str) -> bool,
) -> Result<(), PipelineError> {
    let mut missing = Vec::new();
    for language in languages {
        let profile = profile(profiles, *language)?;
        if !is_available(&profile.server.program) {
            missing.push(format!(
                "{} needs {}",
                profile.language, profile.server.program
            ));
        }
    }
    if missing.is_empty() {
        Ok(())
    } else {
        Err(PipelineError::Setup(format!(
            "cannot start language servers: {}. Run `tekdek doctor` for the install commands",
            missing.join("; ")
        )))
    }
}

fn program_is_available(program: &str) -> bool {
    find_program(program).is_some()
}

// -----------------------------------------------------------------------------
// Tests
// -----------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    #[cfg(feature = "rust")]
    use super::detect_languages;
    use super::detect_required_languages;
    #[cfg(all(feature = "python", feature = "rust"))]
    use super::{enabled_extensions, preflight_server_programs_with};
    #[cfg(feature = "rust")]
    use crate::analysis::language::Language;
    #[cfg(all(feature = "python", feature = "rust"))]
    use crate::analysis::language::LanguageProfile;
    use crate::analysis::{language::builtin_profiles, source::DocumentSnapshot};

    #[cfg(all(feature = "python", feature = "rust"))]
    #[test]
    fn detects_all_enabled_languages_in_stable_order() {
        let documents = vec![
            snapshot("src/lib.rs", "pub fn run() {}"),
            snapshot("tool.py", "def run():\n    pass\n"),
        ];
        let mut profiles = builtin_profiles();
        profiles.reverse();

        let detected = detect_languages(&documents, &profiles);

        assert_eq!(detected, vec![Language::Python, Language::Rust]);
    }

    #[cfg(all(feature = "python", feature = "rust"))]
    #[test]
    fn ignores_a_disabled_detected_language() {
        let documents = vec![
            snapshot("src/lib.rs", "pub fn run() {}"),
            snapshot("tool.py", "def run():\n    pass\n"),
        ];
        let mut profiles = builtin_profiles();
        profile_mut(&mut profiles, Language::Rust).enabled = false;

        let detected = detect_languages(&documents, &profiles);

        assert_eq!(detected, vec![Language::Python]);
    }

    #[cfg(all(feature = "rust", not(feature = "python")))]
    #[test]
    fn detects_rust_in_a_rust_only_build() {
        let detected = detect_languages(
            &[snapshot("src/lib.rs", "pub fn run() {}")],
            &builtin_profiles(),
        );

        assert_eq!(detected, vec![Language::Rust]);
    }

    #[test]
    fn rejects_a_repository_without_supported_sources() {
        let error =
            detect_required_languages(&[snapshot("README.md", "# Readme")], &builtin_profiles())
                .expect_err("the repository should have no supported language");

        assert_eq!(
            error.to_string(),
            "the codebase has no enabled supported source files"
        );
    }

    #[cfg(all(feature = "python", feature = "rust"))]
    #[test]
    fn collects_enabled_extensions_in_stable_order() {
        let mut profiles = builtin_profiles();
        profiles.reverse();
        profile_mut(&mut profiles, Language::Python)
            .extensions
            .push("py".to_owned());

        let extensions = enabled_extensions(&profiles);

        assert_eq!(extensions, ["py", "pyi", "rs"]);
    }

    #[cfg(all(feature = "python", feature = "rust"))]
    #[test]
    fn reports_every_missing_detected_server() {
        let profiles = builtin_profiles();
        let detected = vec![Language::Python, Language::Rust];

        let error = preflight_server_programs_with(&profiles, &detected, |_| false)
            .expect_err("both language servers should be missing");

        assert_eq!(
            error.to_string(),
            "cannot start language servers: python needs pyright-langserver; rust needs rust-analyzer. Run `tekdek doctor` for the install commands"
        );
    }

    #[cfg(all(feature = "python", feature = "rust"))]
    #[test]
    fn preflights_only_detected_languages() {
        let profiles = builtin_profiles();
        let mut checked = Vec::new();

        preflight_server_programs_with(&profiles, &[Language::Python], |program| {
            checked.push(program.to_owned());
            true
        })
        .expect("the detected server should exist");

        assert_eq!(checked, ["pyright-langserver"]);
    }

    fn snapshot(path: &str, content: &str) -> DocumentSnapshot {
        DocumentSnapshot {
            path: PathBuf::from(path),
            content: content.to_owned(),
        }
    }

    #[cfg(all(feature = "python", feature = "rust"))]
    fn profile_mut(profiles: &mut [LanguageProfile], language: Language) -> &mut LanguageProfile {
        profiles
            .iter_mut()
            .find(|profile| profile.language == language)
            .expect("the profile should exist")
    }
}
