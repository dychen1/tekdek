//! Capture validation and normalization.
//!
//! This module owns the pure checks of captured protocol data and the
//! conversion of that data into semantic facts. A protocol capture, selected
//! sources, and a fallback policy are the inputs. Semantic facts are the
//! output.
//!
//! # Sections
//!
//! `Capture Validation` validates captured symbols and facts.
//! `Protocol Validation` validates protocol kinds, sites, and locations.
//! `Normalization` converts captured protocol facts into semantic facts.
//! `Source Paths` converts selected source paths to UTF-8 text.
//! `Source Positions` converts UTF-16 protocol positions to byte offsets.

use std::{collections::HashMap, path::Path};

use lsp_types::{DocumentSymbol, Location, Position, Range, SymbolKind, Uri};

use super::{
    CapturedDocument, CapturedSemanticFact, FallbackPolicy, LspError, ProtocolSymbolSite,
    SemanticCapture, uri_path,
};
use crate::analysis::{
    fact::{ReportedLocation, SemanticFact, SymbolAnchor},
    source::{DocumentSnapshot, SourceRange, SourceSpan, Sources},
};

// -----------------------------------------------------------------------------
// Capture Validation
// -----------------------------------------------------------------------------

/// Validates the symbols and facts that the server returned.
///
/// The session builds the document paths and URIs itself, so they need no check.
///
/// # Errors
///
/// Returns an error if a symbol or fact is invalid.
pub(crate) fn validate_capture(capture: &SemanticCapture) -> Result<(), LspError> {
    for document in &capture.documents {
        validate_document_symbols(&document.symbols, &document.path)?;
    }
    validate_facts(&capture.facts)
}

/// A selection outside its definition would query the wrong source position.
///
/// # Errors
///
/// Returns an error if a symbol has an invalid selection range.
fn validate_document_symbols(symbols: &[DocumentSymbol], path: &str) -> Result<(), LspError> {
    for symbol in symbols {
        if !range_contains(symbol.range, symbol.selection_range) {
            return Err(LspError::new(format!(
                "the {} selection range is outside its definition in {path}",
                symbol.name
            )));
        }
        if let Some(children) = &symbol.children {
            validate_document_symbols(children, path)?;
        }
    }

    Ok(())
}

fn range_contains(outer: Range, inner: Range) -> bool {
    position_before_or_equal(outer.start, inner.start)
        && position_before_or_equal(inner.end, outer.end)
}

// -----------------------------------------------------------------------------
// Protocol Validation
// -----------------------------------------------------------------------------

/// Returns all 26 symbol kinds from LSP 3.17.
pub(crate) fn supported_symbol_kinds() -> Vec<SymbolKind> {
    SUPPORTED_SYMBOL_KINDS.to_vec()
}

const SUPPORTED_SYMBOL_KINDS: [SymbolKind; 26] = [
    SymbolKind::FILE,
    SymbolKind::MODULE,
    SymbolKind::NAMESPACE,
    SymbolKind::PACKAGE,
    SymbolKind::CLASS,
    SymbolKind::METHOD,
    SymbolKind::PROPERTY,
    SymbolKind::FIELD,
    SymbolKind::CONSTRUCTOR,
    SymbolKind::ENUM,
    SymbolKind::INTERFACE,
    SymbolKind::FUNCTION,
    SymbolKind::VARIABLE,
    SymbolKind::CONSTANT,
    SymbolKind::STRING,
    SymbolKind::NUMBER,
    SymbolKind::BOOLEAN,
    SymbolKind::ARRAY,
    SymbolKind::OBJECT,
    SymbolKind::KEY,
    SymbolKind::NULL,
    SymbolKind::ENUM_MEMBER,
    SymbolKind::STRUCT,
    SymbolKind::EVENT,
    SymbolKind::OPERATOR,
    SymbolKind::TYPE_PARAMETER,
];

/// # Errors
///
/// Returns an error if one fact contains invalid protocol data.
fn validate_facts(facts: &[CapturedSemanticFact]) -> Result<(), LspError> {
    for fact in facts {
        match fact {
            CapturedSemanticFact::Call {
                caller,
                callee,
                call_sites,
            } => {
                validate_symbol_site(caller)?;
                validate_symbol_site(callee)?;
                validate_locations(call_sites)?;
            }
            CapturedSemanticFact::Reference {
                target,
                occurrences,
            } => {
                validate_symbol_site(target)?;
                validate_locations(occurrences)?;
            }
            CapturedSemanticFact::TypeDefinition {
                queried_symbol,
                targets,
            } => {
                validate_symbol_site(queried_symbol)?;
                validate_locations(targets)?;
            }
        }
    }

    Ok(())
}

/// # Errors
///
/// Returns an error if one required symbol value is invalid.
fn validate_symbol_site(symbol: &ProtocolSymbolSite) -> Result<(), LspError> {
    if symbol.name.is_empty() {
        return Err(LspError::new("a language-server symbol has no name"));
    }
    if symbol.uri.as_str().is_empty() {
        return Err(LspError::new(format!(
            "the {} language-server symbol has no URI",
            symbol.name
        )));
    }
    if !SUPPORTED_SYMBOL_KINDS.contains(&symbol.kind) {
        return Err(LspError::new(format!(
            "the {} language-server symbol has an unknown kind",
            symbol.name
        )));
    }
    if !position_before_or_equal(symbol.selection.start, symbol.selection.end) {
        return Err(LspError::new(format!(
            "the {} language-server symbol range is reversed",
            symbol.name
        )));
    }

    Ok(())
}

/// # Errors
///
/// Returns an error if one protocol range is reversed.
fn validate_locations(locations: &[Location]) -> Result<(), LspError> {
    for location in locations {
        if !position_before_or_equal(location.range.start, location.range.end) {
            return Err(LspError::new(format!(
                "the language-server range is reversed in {}",
                location.uri.as_str()
            )));
        }
    }

    Ok(())
}

fn position_before_or_equal(left: Position, right: Position) -> bool {
    left.line < right.line || left.line == right.line && left.character <= right.character
}

// -----------------------------------------------------------------------------
// Normalization
// -----------------------------------------------------------------------------

/// Converts one protocol capture into semantic facts.
///
/// # Errors
///
/// Returns an error if a selected source path is not valid UTF-8.
/// Returns an error if a selected protocol position is outside its source.
pub(crate) fn normalize_capture(
    root: &Path,
    sources: &Sources<'_>,
    capture: SemanticCapture,
    fallback_policy: FallbackPolicy,
) -> Result<Vec<SemanticFact>, LspError> {
    let SemanticCapture { documents, facts } = capture;
    let lookup = SourceLookup::new(root, sources, &documents)?;

    facts
        .into_iter()
        .map(|fact| normalize_fact(fact, &lookup, fallback_policy))
        .collect()
}

/// Finds the selected source that one protocol URI names.
///
/// The lookup first matches exact URI text. After a miss, it converts the URI
/// to a path and matches that path.
struct SourceLookup<'a> {
    root: &'a Path,
    by_uri: HashMap<&'a str, &'a DocumentSnapshot>,
    by_path: HashMap<&'a str, &'a DocumentSnapshot>,
}

impl<'a> SourceLookup<'a> {
    /// # Errors
    ///
    /// Returns an error if a selected source path is not valid UTF-8.
    fn new(
        root: &'a Path,
        sources: &'a Sources<'_>,
        documents: &'a [CapturedDocument],
    ) -> Result<Self, LspError> {
        let by_path = sources
            .iter()
            .map(|source| Ok((source_path(source)?, source)))
            .collect::<Result<HashMap<_, _>, LspError>>()?;
        let by_uri = documents
            .iter()
            .filter_map(|document| {
                let source = by_path.get(document.path.as_str()).copied()?;
                Some((document.uri.as_str(), source))
            })
            .collect();

        Ok(Self {
            root,
            by_uri,
            by_path,
        })
    }

    /// Returns the selected source that one URI names.
    ///
    /// Returns `None` for a URI outside the selected sources.
    fn source(&self, uri: &Uri) -> Option<&'a DocumentSnapshot> {
        if let Some(source) = self.by_uri.get(uri.as_str()).copied() {
            return Some(source);
        }

        let path = uri_path(uri)?;
        let relative = path.strip_prefix(self.root).ok()?;
        self.by_path.get(relative.to_str()?).copied()
    }
}

/// # Errors
///
/// Returns an error if a selected path or position is invalid.
fn normalize_fact(
    fact: CapturedSemanticFact,
    lookup: &SourceLookup<'_>,
    fallback_policy: FallbackPolicy,
) -> Result<SemanticFact, LspError> {
    match fact {
        CapturedSemanticFact::Call {
            caller,
            callee,
            call_sites,
        } => Ok(SemanticFact::Call {
            caller: normalize_symbol(caller, lookup, fallback_policy)?,
            callee: normalize_symbol(callee, lookup, fallback_policy)?,
            call_sites: normalize_locations(&call_sites, lookup)?,
        }),
        CapturedSemanticFact::Reference {
            target,
            occurrences,
        } => Ok(SemanticFact::Reference {
            target: normalize_symbol(target, lookup, fallback_policy)?,
            occurrences: normalize_locations(&occurrences, lookup)?,
        }),
        CapturedSemanticFact::TypeDefinition {
            queried_symbol,
            targets,
        } => Ok(SemanticFact::TypeDefinition {
            queried_symbol: normalize_symbol(queried_symbol, lookup, fallback_policy)?,
            targets: normalize_locations(&targets, lookup)?,
        }),
    }
}

/// Converts one protocol symbol into a selected or unselected symbol anchor.
/// An unselected URI returns `SymbolAnchor::Unselected` without using the policy.
/// A selected symbol uses the fallback policy.
///
/// # Errors
///
/// Returns an error if the selected path or position is invalid.
fn normalize_symbol(
    symbol: ProtocolSymbolSite,
    lookup: &SourceLookup<'_>,
    fallback_policy: FallbackPolicy,
) -> Result<SymbolAnchor, LspError> {
    let Some(source) = lookup.source(&symbol.uri) else {
        return Ok(SymbolAnchor::Unselected);
    };
    let path = source_path(source)?;
    let start = position_to_byte_offset(&source.content, path, symbol.selection.start)?;
    let end = position_to_byte_offset(&source.content, path, symbol.selection.end)?;
    let fallback = fallback_policy(&symbol);

    Ok(SymbolAnchor::Selected {
        path: path.to_owned(),
        range: SourceRange { start, end },
        fallback,
        name: symbol.name,
    })
}

/// # Errors
///
/// Returns an error if one selected path or range is invalid.
fn normalize_locations(
    locations: &[Location],
    lookup: &SourceLookup<'_>,
) -> Result<Vec<ReportedLocation>, LspError> {
    locations
        .iter()
        .map(|location| normalize_location(location, lookup))
        .collect()
}

/// # Errors
///
/// Returns an error if the selected path or range is invalid.
fn normalize_location(
    location: &Location,
    lookup: &SourceLookup<'_>,
) -> Result<ReportedLocation, LspError> {
    let Some(source) = lookup.source(&location.uri) else {
        return Ok(ReportedLocation::Unselected);
    };
    let path = source_path(source)?;
    let start = position_to_byte_offset(&source.content, path, location.range.start)?;
    let end = position_to_byte_offset(&source.content, path, location.range.end)?;

    Ok(ReportedLocation::Selected(SourceSpan {
        path: path.to_owned(),
        range: SourceRange { start, end },
    }))
}

// -----------------------------------------------------------------------------
// Source Paths
// -----------------------------------------------------------------------------

/// # Errors
///
/// Returns an error if the selected source path is not valid UTF-8.
pub(crate) fn source_path(source: &DocumentSnapshot) -> Result<&str, LspError> {
    source.path.to_str().ok_or_else(|| {
        LspError::new(format!(
            "the source path is not valid UTF-8: {}",
            source.path.display()
        ))
    })
}

// -----------------------------------------------------------------------------
// Source Positions
// -----------------------------------------------------------------------------
// The client relies on the LSP default UTF-16 position encoding. It advertises
// no other encoding.

/// Converts one zero-based UTF-16 LSP position into a UTF-8 byte offset.
///
/// # Errors
///
/// Returns an error if the line or character is outside the source text.
/// Returns an error if the character splits a UTF-16 surrogate pair.
fn position_to_byte_offset(source: &str, path: &str, position: Position) -> Result<u32, LspError> {
    let line_number = usize::try_from(position.line).map_err(|_| invalid_line(path, position))?;
    let line_start = source
        .split_inclusive('\n')
        .take(line_number)
        .map(str::len)
        .sum::<usize>();
    let line = source
        .get(line_start..)
        .and_then(|remainder| remainder.split('\n').next())
        .filter(|_| line_number < source.split('\n').count())
        .ok_or_else(|| invalid_line(path, position))?;
    let line = line.strip_suffix('\r').unwrap_or(line);
    let mut utf16_offset = 0_u32;

    for (byte_offset, character) in line.char_indices() {
        if utf16_offset == position.character {
            return absolute_offset(line_start, byte_offset, path);
        }

        let character_width = if character.len_utf16() == 2 { 2 } else { 1 };
        let next_offset = utf16_offset
            .checked_add(character_width)
            .ok_or_else(|| invalid_character(path, position))?;
        if position.character < next_offset {
            return Err(LspError::new(format!(
                "the LSP character {} splits a UTF-16 character in {path}",
                position.character
            )));
        }
        utf16_offset = next_offset;
    }

    if utf16_offset == position.character {
        return absolute_offset(line_start, line.len(), path);
    }

    Err(invalid_character(path, position))
}

/// # Errors
///
/// Returns an error if the combined offset does not fit in a source offset.
fn absolute_offset(line_start: usize, byte_offset: usize, path: &str) -> Result<u32, LspError> {
    line_start
        .checked_add(byte_offset)
        .and_then(|offset| u32::try_from(offset).ok())
        .ok_or_else(|| LspError::new(format!("the source offset is too large in {path}")))
}

fn invalid_line(path: &str, position: Position) -> LspError {
    LspError::new(format!("the LSP line {} is outside {path}", position.line))
}

fn invalid_character(path: &str, position: Position) -> LspError {
    LspError::new(format!(
        "the LSP character {} is outside line {} in {path}",
        position.character, position.line
    ))
}

// -----------------------------------------------------------------------------
// Tests
// -----------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use lsp_types::Position;

    use super::position_to_byte_offset;

    #[test]
    fn converts_utf16_positions_into_utf8_offsets() {
        let source = "alpha\né😀z\n";

        assert_eq!(
            position_to_byte_offset(source, "app.py", Position::new(1, 3))
                .expect("the position must be valid"),
            12
        );
        assert_eq!(
            position_to_byte_offset(source, "app.py", Position::new(2, 0))
                .expect("the final line must be valid"),
            14
        );
    }

    #[test]
    fn rejects_a_position_inside_a_utf16_surrogate_pair() {
        let result = position_to_byte_offset("😀", "app.py", Position::new(0, 1));

        assert!(result.is_err());
    }
}
