//! Semantic capture.
//!
//! This module owns every document request: document opening, document
//! symbols, the semantic requests of each symbol, and response conversion. An
//! initialized client, a session configuration, and selected sources are the
//! inputs. One protocol capture is the output.
//!
//! # Sections
//!
//! `Document Capture` opens the documents and captures their symbols and facts.
//! `Per-Symbol Requests` requests each supported fact type for one symbol.
//! `Symbol Sites` converts hierarchical symbols into ordered protocol sites.
//! `Response Conversion` converts protocol responses into captured facts.

use std::{path::Path, time::Duration};

use lsp_types::{
    CallHierarchyIncomingCall, CallHierarchyIncomingCallsParams, CallHierarchyItem,
    CallHierarchyOutgoingCall, CallHierarchyOutgoingCallsParams, CallHierarchyPrepareParams,
    DidOpenTextDocumentParams, DocumentSymbol, DocumentSymbolParams, DocumentSymbolResponse,
    GotoDefinitionParams, GotoDefinitionResponse, Location, PartialResultParams, Range,
    ReferenceContext, ReferenceParams, TextDocumentIdentifier, TextDocumentItem,
    TextDocumentPositionParams, Uri, WorkDoneProgressParams,
    notification::DidOpenTextDocument,
    request::{
        CallHierarchyIncomingCalls, CallHierarchyOutgoingCalls, CallHierarchyPrepare,
        DocumentSymbolRequest, GotoTypeDefinition, References, Request,
    },
};

use super::{
    CapturedDocument, CapturedSemanticFact, LspClient, LspError, ProtocolSymbolSite,
    SemanticCapture, file_uri,
    normalize::source_path,
    session::{SessionConfig, language_title},
};
use crate::analysis::source::Sources;

// -----------------------------------------------------------------------------
// Document Capture
// -----------------------------------------------------------------------------

/// Opens all documents, then captures facts for every hierarchical symbol.
///
/// # Errors
///
/// Returns an error if a document cannot open or a request fails.
pub(crate) fn capture_document_semantics(
    client: &mut LspClient,
    config: &SessionConfig<'_>,
    root: &Path,
    sources: &Sources<'_>,
) -> Result<SemanticCapture, LspError> {
    let mut files = Vec::with_capacity(sources.len());

    for document in sources.iter() {
        let path = source_path(document)?;
        let uri = file_uri(&root.join(path))?;
        client
            .notify::<DidOpenTextDocument>(DidOpenTextDocumentParams {
                text_document: TextDocumentItem::new(
                    uri.clone(),
                    config.language_id.to_owned(),
                    1,
                    document.content.clone(),
                ),
            })
            .map_err(|error| LspError::new(format!("cannot open {path}: {error}")))?;
        files.push((path.to_owned(), uri));
    }

    let files = files
        .into_iter()
        .enumerate()
        .map(|(index, (path, uri))| {
            let timeout = if index == 0 {
                FIRST_REQUEST_TIMEOUT
            } else {
                REQUEST_TIMEOUT
            };
            let response = client
                .request::<DocumentSymbolRequest>(
                    DocumentSymbolParams {
                        text_document: TextDocumentIdentifier::new(uri.clone()),
                        work_done_progress_params: WorkDoneProgressParams::default(),
                        partial_result_params: PartialResultParams::default(),
                    },
                    timeout,
                )
                .map_err(|error| {
                    LspError::new(format!(
                        "cannot read {} symbols from {path}: {error}",
                        language_title(config.language)
                    ))
                })?;

            Ok(CapturedDocument {
                symbols: nested_symbols(response, &path)?,
                path,
                uri,
            })
        })
        .collect::<Result<Vec<_>, LspError>>()?;

    let facts = request_facts(client, &files)?;

    Ok(SemanticCapture {
        documents: files,
        facts,
    })
}

/// Captures all supported semantic facts for every document symbol.
/// Fact order follows document order and parent-first symbol order.
/// For each symbol, prepared items retain server order.
/// Each item adds outgoing calls, then incoming calls, in response order.
/// References follow all prepared items. Type definitions follow references.
///
/// # Errors
///
/// Returns an error if any semantic request fails.
fn request_facts(
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

const FIRST_REQUEST_TIMEOUT: Duration = Duration::from_mins(5);
const REQUEST_TIMEOUT: Duration = Duration::from_mins(1);

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
fn nested_symbols(
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
