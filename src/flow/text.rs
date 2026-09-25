//! Flow text rules.
//!
//! This module owns the canonical form of agent text and the line and block text
//! rules of the flow schemas. Lengths count Unicode scalar values, as JSON Schema
//! counts them.

use std::fmt;

use icu_normalizer::ComposingNormalizerBorrowed;

/// Returns true for the characters that ECMAScript `\s` matches.
#[must_use]
pub fn is_space(character: char) -> bool {
    (character.is_whitespace() && character != '\u{85}') || character == '\u{feff}'
}

/// Returns true for the control characters that the schemas forbid.
#[must_use]
pub fn is_control(character: char) -> bool {
    matches!(character, '\u{0}'..='\u{1f}' | '\u{7f}')
}

/// Normalizes text to Unicode NFC and removes outer whitespace.
///
/// This is canonicalization, not repair: the result must still pass the text rules.
#[must_use]
pub fn canonical(text: &str) -> String {
    ComposingNormalizerBorrowed::new_nfc()
        .normalize(text)
        .trim_matches(is_space)
        .to_owned()
}

/// Checks one single-line text value.
///
/// # Errors
/// Returns the first rule that the value breaks.
pub fn check_line(text: &str, min: usize, max: usize) -> Result<(), TextFault> {
    check(text, min, max, false)
}

/// Checks one text value that can hold tabs and line feeds.
///
/// # Errors
/// Returns the first rule that the value breaks.
pub fn check_block(text: &str, min: usize, max: usize) -> Result<(), TextFault> {
    check(text, min, max, true)
}

fn check(text: &str, min: usize, max: usize, block: bool) -> Result<(), TextFault> {
    let count = text.chars().count();
    if count < min || count > max {
        return Err(TextFault::Length { min, max, count });
    }
    let edge = |character: char| is_space(character) || is_control(character);
    if text.starts_with(edge) || text.ends_with(edge) {
        return Err(TextFault::Edge);
    }
    if text
        .chars()
        .any(|character| is_control(character) && !(block && matches!(character, '\t' | '\n')))
    {
        return Err(TextFault::Control);
    }
    if canonical(text) != text {
        return Err(TextFault::NotCanonical);
    }
    Ok(())
}

// -----------------------------------------------------------------------------
// Errors
// -----------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextFault {
    Length {
        min: usize,
        max: usize,
        count: usize,
    },
    Edge,
    Control,
    NotCanonical,
}

impl fmt::Display for TextFault {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            // State the allowed range and the actual length.
            Self::Length { min, max, count } => write!(
                formatter,
                "the text must have {min} to {max} characters, but it has {count}"
            ),
            // Name the outer character rule.
            Self::Edge => write!(
                formatter,
                "the text must not start or end with whitespace or a control character"
            ),
            // Name the inner character rule.
            Self::Control => write!(formatter, "the text contains a forbidden control character"),
            // Name the normalization rule.
            Self::NotCanonical => write!(formatter, "the text is not in Unicode NFC form"),
        }
    }
}

// -----------------------------------------------------------------------------
// Tests
// -----------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_text_is_nfc_and_trimmed() {
        assert_eq!(canonical("  Cafe\u{301}\n"), "Caf\u{e9}");
        assert_eq!(canonical("\u{a0}x\u{feff}"), "x");
    }

    #[test]
    fn line_and_block_rules_follow_the_schema_patterns() {
        assert!(check_line("Load and verify", 3, 60).is_ok());
        assert_eq!(
            check_line("ab", 3, 60),
            Err(TextFault::Length {
                min: 3,
                max: 60,
                count: 2
            })
        );
        assert_eq!(check_line("a\nb c", 3, 60), Err(TextFault::Control));
        assert!(check_block("a\nb\tc", 1, 60).is_ok());
        assert_eq!(check_block("a\rb", 1, 60), Err(TextFault::Control));
        assert_eq!(check_block("abc ", 1, 60), Err(TextFault::Edge));
        assert_eq!(
            check_line("Cafe\u{301}", 1, 60),
            Err(TextFault::NotCanonical)
        );
    }
}
