//! Language preflight.
//!
//! This module selects the languages that one analysis runs and checks that
//! their language server programs exist. Language profiles and loaded
//! documents are the inputs. Detected profiles or a setup error are the output.
//!
//! # Sections
//!
//! `Detection` selects enabled profiles with matching sources.
//! `Server programs` finds each server program on the search path.

use super::{
    PipelineError,
    language::LanguageProfile,
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

/// Returns the detected profiles in language order.
///
/// # Errors
///
/// Returns a setup error when no enabled language has a source file.
pub(super) fn detect_required_languages<'a>(
    documents: &[DocumentSnapshot],
    profiles: &'a [LanguageProfile],
) -> Result<Vec<&'a LanguageProfile>, PipelineError> {
    let detected = detect_languages(documents, profiles);
    if detected.is_empty() {
        return Err(PipelineError::Setup(
            "the codebase has no enabled supported source files".to_owned(),
        ));
    }
    Ok(detected)
}

/// Returns the enabled profiles that have a source file, one for each language,
/// in language order.
pub(super) fn detect_languages<'a>(
    documents: &[DocumentSnapshot],
    profiles: &'a [LanguageProfile],
) -> Vec<&'a LanguageProfile> {
    let mut detected = profiles
        .iter()
        .filter(|profile| {
            profile.enabled
                && documents
                    .iter()
                    .any(|document| source::document_has_extension(document, &profile.extensions))
        })
        .collect::<Vec<_>>();
    detected.sort_by_key(|profile| profile.language);
    detected.dedup_by_key(|profile| profile.language);
    detected
}

// -----------------------------------------------------------------------------
// Server programs
// -----------------------------------------------------------------------------

/// Checks that every detected profile has an executable server program.
///
/// # Errors
///
/// Returns one setup error that lists every missing program.
pub(super) fn preflight_server_programs(
    detected: &[&LanguageProfile],
) -> Result<(), PipelineError> {
    preflight_server_programs_with(detected, program_is_available)
}

fn preflight_server_programs_with(
    detected: &[&LanguageProfile],
    mut is_available: impl FnMut(&str) -> bool,
) -> Result<(), PipelineError> {
    let mut missing = Vec::new();
    for profile in detected {
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
    use crate::analysis::language::{Language, LanguageProfile};
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

        assert_eq!(languages(&detected), [Language::Python, Language::Rust]);
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

        assert_eq!(languages(&detected), [Language::Python]);
    }

    #[cfg(all(feature = "rust", not(feature = "python")))]
    #[test]
    fn detects_rust_in_a_rust_only_build() {
        let profiles = builtin_profiles();

        let detected = detect_languages(&[snapshot("src/lib.rs", "pub fn run() {}")], &profiles);

        assert_eq!(languages(&detected), [Language::Rust]);
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
        let detected = profiles.iter().collect::<Vec<_>>();

        let error = preflight_server_programs_with(&detected, |_| false)
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
        let detected = profiles
            .iter()
            .filter(|profile| profile.language == Language::Python)
            .collect::<Vec<_>>();
        let mut checked = Vec::new();

        preflight_server_programs_with(&detected, |program| {
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

    #[cfg(feature = "rust")]
    fn languages(detected: &[&LanguageProfile]) -> Vec<Language> {
        detected.iter().map(|profile| profile.language).collect()
    }

    #[cfg(all(feature = "python", feature = "rust"))]
    fn profile_mut(profiles: &mut [LanguageProfile], language: Language) -> &mut LanguageProfile {
        profiles
            .iter_mut()
            .find(|profile| profile.language == language)
            .expect("the profile should exist")
    }
}
