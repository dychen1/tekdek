//! Semantic capture and normalization.
//!
//! This module owns semantic requests, protocol-site extraction, response
//! conversion, capture validation, and normalization. An initialized client,
//! selected sources, captured data, and a fallback policy are the inputs.
//! Captured protocol facts and semantic facts are the outputs.
//!
//! # Sections
//!
//! `Full Capture` requests facts for all document symbols.
//! `Per-Symbol Requests` requests each supported fact type for one symbol.
//! `Symbol Sites` converts hierarchical symbols into ordered protocol sites.
//! `Response Conversion` converts protocol responses into captured facts.
//! `Capture Validation` validates captured documents, symbols, and facts.
//! `Protocol Validation` validates protocol kinds, sites, and locations.
//! `Normalization` converts captured protocol facts into semantic facts.
//! `Source Paths` converts selected source paths to UTF-8 text.

use std::{collections::HashMap, path::Path, time::Duration};

use lsp_types::{
    CallHierarchyIncomingCall, CallHierarchyIncomingCallsParams, CallHierarchyItem,
    CallHierarchyOutgoingCall, CallHierarchyOutgoingCallsParams, CallHierarchyPrepareParams,
    DocumentSymbol, DocumentSymbolResponse, GotoDefinitionParams, GotoDefinitionResponse, Location,
    PartialResultParams, Position, Range, ReferenceContext, ReferenceParams, SymbolKind,
    TextDocumentIdentifier, TextDocumentPositionParams, Uri, WorkDoneProgressParams,
    request::{
        CallHierarchyIncomingCalls, CallHierarchyOutgoingCalls, CallHierarchyPrepare,
        GotoTypeDefinition, References, Request,
    },
};

use super::{
    CapturedDocument, CapturedSemanticFact, FallbackPolicy, LspClient, LspError,
    ProtocolSymbolSite, SemanticCapture, position_to_byte_offset, uri_path,
};
use crate::analysis::{
    fact::{ReportedLocation, SemanticFact, SymbolAnchor},
    source::{DocumentSnapshot, SourceRange, SourceSpan, Sources},
};

// -----------------------------------------------------------------------------
// Full Capture
// -----------------------------------------------------------------------------

/// Captures all supported semantic facts for every document symbol.
/// Fact order follows document order and parent-first symbol order.
/// For each symbol, prepared items retain server order.
/// Each item adds outgoing calls, then incoming calls, in response order.
/// References follow all prepared items. Type definitions follow references.
///
/// # Errors
///
/// Returns an error if any semantic request fails.
pub(crate) fn request_facts(
    client: &mut LspClient,
    documents: &[CapturedDocument],
) -> Result<Vec<CapturedSemanticFact>, LspError> {
    let mut facts = Vec::new();
    for document in documents {
        for symbol in symbol_sites(&document.uri, &document.symbols) {
            capture_symbol(client, &document.path, &symbol, &mut facts)?;
        }
    }

    Ok(facts)
}

// -----------------------------------------------------------------------------
// Per-Symbol Requests
// -----------------------------------------------------------------------------

/// Appends calls, references, and type definitions for one symbol.
///
/// Empty reference and type-definition responses add no fact.
///
/// # Errors
///
/// Returns an error if a semantic request fails.
fn capture_symbol(
    client: &mut LspClient,
    path: &str,
    symbol: &ProtocolSymbolSite,
    facts: &mut Vec<CapturedSemanticFact>,
) -> Result<(), LspError> {
    let prepared = request::<CallHierarchyPrepare>(
        client,
        CallHierarchyPrepareParams {
            text_document_position_params: symbol_position(symbol),
            work_done_progress_params: WorkDoneProgressParams::default(),
        },
        path,
        symbol,
    )?
    .unwrap_or_default();

    for item in prepared {
        let outgoing = request::<CallHierarchyOutgoingCalls>(
            client,
            CallHierarchyOutgoingCallsParams {
                item: item.clone(),
                work_done_progress_params: WorkDoneProgressParams::default(),
                partial_result_params: PartialResultParams::default(),
            },
            path,
            symbol,
        )?
        .unwrap_or_default();
        facts.extend(
            outgoing
                .into_iter()
                .map(|call| outgoing_call_fact(&item, call)),
        );

        let incoming = request::<CallHierarchyIncomingCalls>(
            client,
            CallHierarchyIncomingCallsParams {
                item: item.clone(),
                work_done_progress_params: WorkDoneProgressParams::default(),
                partial_result_params: PartialResultParams::default(),
            },
            path,
            symbol,
        )?
        .unwrap_or_default();
        facts.extend(
            incoming
                .into_iter()
                .map(|call| incoming_call_fact(&item, call)),
        );
    }

    let occurrences = request::<References>(
        client,
        ReferenceParams {
            text_document_position: symbol_position(symbol),
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
            context: ReferenceContext {
                include_declaration: false,
            },
        },
        path,
        symbol,
    )?
    .unwrap_or_default();
    if !occurrences.is_empty() {
        facts.push(CapturedSemanticFact::Reference {
            target: symbol.clone(),
            occurrences,
        });
    }

    let targets = request::<GotoTypeDefinition>(
        client,
        GotoDefinitionParams {
            text_document_position_params: symbol_position(symbol),
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
        },
        path,
        symbol,
    )?
    .map(type_definition_locations)
    .unwrap_or_default();
    if !targets.is_empty() {
        facts.push(CapturedSemanticFact::TypeDefinition {
            queried_symbol: symbol.clone(),
            targets,
        });
    }

    Ok(())
}

/// Sends one request with symbol context in its error.
fn request<R>(
    client: &mut LspClient,
    parameters: R::Params,
    path: &str,
    symbol: &ProtocolSymbolSite,
) -> Result<R::Result, LspError>
where
    R: Request,
{
    client
        .request::<R>(parameters, REQUEST_TIMEOUT)
        .map_err(|error| {
            LspError::new(format!(
                "cannot request {} for {} in {path}: {error}",
                R::METHOD,
                symbol.name
            ))
        })
}

fn symbol_position(symbol: &ProtocolSymbolSite) -> TextDocumentPositionParams {
    TextDocumentPositionParams::new(
        TextDocumentIdentifier::new(symbol.uri.clone()),
        symbol.selection.start,
    )
}

const REQUEST_TIMEOUT: Duration = Duration::from_mins(1);

// -----------------------------------------------------------------------------
// Symbol Sites
// -----------------------------------------------------------------------------

/// Flattens one symbol tree in parent-first order.
/// Each site uses the symbol selection start.
fn symbol_sites(uri: &Uri, symbols: &[DocumentSymbol]) -> Vec<ProtocolSymbolSite> {
    let mut flattened = Vec::new();
    append_symbol_sites(uri, symbols, &mut flattened);
    flattened
}

fn append_symbol_sites(
    uri: &Uri,
    symbols: &[DocumentSymbol],
    output: &mut Vec<ProtocolSymbolSite>,
) {
    for symbol in symbols {
        output.push(ProtocolSymbolSite {
            uri: uri.clone(),
            selection: symbol.selection_range,
            kind: symbol.kind,
            name: symbol.name.clone(),
        });
        if let Some(children) = &symbol.children {
            append_symbol_sites(uri, children, output);
        }
    }
}

// -----------------------------------------------------------------------------
// Response Conversion
// -----------------------------------------------------------------------------

/// A flat symbol has no parent identity, so a nonempty flat list is invalid.
/// An empty flat list is equivalent to an empty hierarchical list.
///
/// # Errors
///
/// Returns an error if the server returns one or more flat symbols.
pub(crate) fn nested_symbols(
    response: Option<DocumentSymbolResponse>,
    path: &str,
) -> Result<Vec<DocumentSymbol>, LspError> {
    match response {
        // Map no response to no symbols.
        None => Ok(Vec::new()),
        // Preserve hierarchical symbols.
        Some(DocumentSymbolResponse::Nested(symbols)) => Ok(symbols),
        // Accept an empty flat response.
        Some(DocumentSymbolResponse::Flat(symbols)) if symbols.is_empty() => Ok(Vec::new()),
        // Reject flat symbols without parent identity.
        Some(DocumentSymbolResponse::Flat(_)) => Err(LspError::new(format!(
            "the language server returned flat symbols for {path}"
        ))),
    }
}

/// Converts one outgoing call into caller-to-callee direction.
fn outgoing_call_fact(
    caller: &CallHierarchyItem,
    call: CallHierarchyOutgoingCall,
) -> CapturedSemanticFact {
    CapturedSemanticFact::Call {
        caller: call_item_symbol_site(caller),
        callee: call_item_symbol_site(&call.to),
        call_sites: call_locations(&caller.uri, call.from_ranges),
    }
}

/// Converts one incoming call into caller-to-callee direction.
fn incoming_call_fact(
    callee: &CallHierarchyItem,
    call: CallHierarchyIncomingCall,
) -> CapturedSemanticFact {
    CapturedSemanticFact::Call {
        caller: call_item_symbol_site(&call.from),
        callee: call_item_symbol_site(callee),
        call_sites: call_locations(&call.from.uri, call.from_ranges),
    }
}

/// Reshapes every type-definition response form into locations.
///
/// The protocol supports a scalar, an array, and a link response.
/// A link uses its target selection range.
fn type_definition_locations(response: GotoDefinitionResponse) -> Vec<Location> {
    match response {
        GotoDefinitionResponse::Scalar(location) => vec![location],
        GotoDefinitionResponse::Array(locations) => locations,
        GotoDefinitionResponse::Link(links) => links
            .into_iter()
            .map(|link| Location::new(link.target_uri, link.target_selection_range))
            .collect(),
    }
}

fn call_item_symbol_site(item: &CallHierarchyItem) -> ProtocolSymbolSite {
    ProtocolSymbolSite {
        uri: item.uri.clone(),
        selection: item.selection_range,
        kind: item.kind,
        name: item.name.clone(),
    }
}

fn call_locations(uri: &Uri, ranges: Vec<Range>) -> Vec<Location> {
    ranges
        .into_iter()
        .map(|range| Location::new(uri.clone(), range))
        .collect()
}

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
// Tests
// -----------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use lsp_types::{
        CallHierarchyIncomingCall, CallHierarchyItem, CallHierarchyOutgoingCall, DocumentSymbol,
        DocumentSymbolResponse, GotoDefinitionResponse, Location, LocationLink, Position, Range,
        SymbolInformation, SymbolKind, Uri,
    };

    use super::{
        CapturedSemanticFact, ProtocolSymbolSite, incoming_call_fact, nested_symbols,
        outgoing_call_fact, symbol_sites, type_definition_locations,
    };

    #[test]
    fn accepts_nested_document_symbols() {
        let symbols = nested_symbols(Some(DocumentSymbolResponse::Nested(Vec::new())), "app.py")
            .expect("nested symbols should be accepted");

        assert_eq!(symbols, Vec::new());
    }

    #[test]
    fn accepts_an_empty_flat_response() {
        let symbols = nested_symbols(Some(DocumentSymbolResponse::Flat(Vec::new())), "empty.py")
            .expect("an empty flat response states that a file has no symbol");

        assert_eq!(symbols, Vec::new());
    }

    #[test]
    fn rejects_flat_document_symbols() {
        let flat = vec![
            #[allow(
                deprecated,
                reason = "SymbolInformation requires the deprecated field."
            )]
            SymbolInformation {
                name: "run".to_owned(),
                kind: SymbolKind::FUNCTION,
                tags: None,
                deprecated: None,
                location: Location::new(
                    "file:///app.py"
                        .parse()
                        .expect("the fixture URI must parse"),
                    Range::new(Position::new(0, 0), Position::new(0, 3)),
                ),
                container_name: None,
            },
        ];

        let result = nested_symbols(Some(DocumentSymbolResponse::Flat(flat)), "app.py");

        assert!(result.is_err());
    }

    #[test]
    fn flattens_symbols_in_parent_first_order() {
        let uri = uri("file:///workspace/app.py");
        let symbols = vec![document_symbol(
            "outer",
            SymbolKind::CLASS,
            1,
            vec![document_symbol("inner", SymbolKind::METHOD, 2, Vec::new())],
        )];

        let flattened = symbol_sites(&uri, &symbols);

        assert_eq!(flattened.len(), 2);
        assert_eq!(flattened[0].name, "outer");
        assert_eq!(flattened[1].name, "inner");
        assert_eq!(flattened[1].selection, range(2));
    }

    #[test]
    fn normalizes_both_call_directions() {
        let source_item = call_item("caller", "file:///workspace/caller.py", 3);
        let target_item = call_item("callee", "file:///workspace/callee.py", 5);
        let call_site = range(8);
        let outgoing = outgoing_call_fact(
            &source_item,
            CallHierarchyOutgoingCall {
                to: target_item.clone(),
                from_ranges: vec![call_site],
            },
        );
        let incoming = incoming_call_fact(
            &target_item,
            CallHierarchyIncomingCall {
                from: source_item.clone(),
                from_ranges: vec![call_site],
            },
        );
        let expected = CapturedSemanticFact::Call {
            caller: symbol_site(&source_item),
            callee: symbol_site(&target_item),
            call_sites: vec![Location::new(source_item.uri.clone(), call_site)],
        };

        assert_eq!(outgoing, expected);
        assert_eq!(incoming, expected);
    }

    #[test]
    fn normalizes_all_type_definition_shapes() {
        let first = Location::new(uri("file:///workspace/first.py"), range(1));
        let second = Location::new(uri("file:///workspace/second.py"), range(2));
        let link = LocationLink {
            origin_selection_range: None,
            target_uri: second.uri.clone(),
            target_range: range(20),
            target_selection_range: second.range,
        };

        assert_eq!(
            type_definition_locations(GotoDefinitionResponse::Scalar(first.clone())),
            vec![first.clone()]
        );
        assert_eq!(
            type_definition_locations(GotoDefinitionResponse::Array(vec![first.clone()])),
            vec![first]
        );
        assert_eq!(
            type_definition_locations(GotoDefinitionResponse::Link(vec![link])),
            vec![second]
        );
    }

    #[allow(deprecated, reason = "The protocol type still requires this field.")]
    fn document_symbol(
        name: &str,
        kind: SymbolKind,
        line: u32,
        children: Vec<DocumentSymbol>,
    ) -> DocumentSymbol {
        DocumentSymbol {
            name: name.to_owned(),
            detail: None,
            kind,
            tags: None,
            deprecated: None,
            range: range(line),
            selection_range: range(line),
            children: (!children.is_empty()).then_some(children),
        }
    }

    fn call_item(name: &str, uri_value: &str, line: u32) -> CallHierarchyItem {
        CallHierarchyItem {
            name: name.to_owned(),
            kind: SymbolKind::FUNCTION,
            tags: None,
            detail: None,
            uri: uri(uri_value),
            range: range(line),
            selection_range: range(line),
            data: None,
        }
    }

    fn symbol_site(item: &CallHierarchyItem) -> ProtocolSymbolSite {
        ProtocolSymbolSite {
            uri: item.uri.clone(),
            selection: item.selection_range,
            kind: item.kind,
            name: item.name.clone(),
        }
    }

    fn range(line: u32) -> Range {
        Range::new(Position::new(line, 0), Position::new(line, 1))
    }

    fn uri(value: &str) -> Uri {
        value.parse().expect("the fixture URI must be valid")
    }
}
