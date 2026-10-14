//! Semantic requests.
//!
//! This module owns the requests that the resolver sends: request batches,
//! their parameters, the definition request of each imported name, and the
//! conversion of answers into captured facts. An initialized client and the
//! parsed sources are the inputs. Captured protocol facts are the output.
//!
//! # Sections
//!
//! `Facts` converts the answers of many symbols, items, or names into facts.
//! `Requests` sends the requests of one round and builds their parameters.
//! `Documents` finds the URI and the text of each source, and converts ranges.
//! `Import Definitions` builds the definition request of each imported name.
//! `Symbol Sites` converts hierarchical symbols into ordered protocol sites.
//! `Response Conversion` converts one protocol response into captured facts.

use std::{collections::HashMap, time::Duration, vec};

use lsp_types::{
    CallHierarchyIncomingCall, CallHierarchyIncomingCallsParams, CallHierarchyItem,
    CallHierarchyOutgoingCall, CallHierarchyOutgoingCallsParams, CallHierarchyPrepareParams,
    DocumentSymbol, DocumentSymbolParams, DocumentSymbolResponse, GotoDefinitionParams,
    GotoDefinitionResponse, Location, PartialResultParams, Position, Range, ReferenceContext,
    ReferenceParams, TextDocumentIdentifier, TextDocumentPositionParams, Uri,
    WorkDoneProgressParams, request::Request,
};

use serde_json::Value;

use super::{
    CapturedDocument, CapturedSemanticFact, LspClient, LspError, ProtocolSymbolSite,
    normalize::{byte_offset_to_position, source_path},
};
use crate::analysis::{
    fact::ImportSite,
    source::{SourceRange, SourceSpan, Sources},
};

// -----------------------------------------------------------------------------
// Facts
// -----------------------------------------------------------------------------

/// Converts the type definitions of some symbols into facts. An empty answer adds no fact.
pub(crate) fn type_definition_facts(
    symbols: &[(&str, ProtocolSymbolSite)],
    responses: Vec<Option<GotoDefinitionResponse>>,
) -> Vec<CapturedSemanticFact> {
    symbols
        .iter()
        .zip(responses)
        .filter_map(|((_, symbol), response)| {
            let targets = response.map(definition_locations).unwrap_or_default();
            (!targets.is_empty()).then(|| CapturedSemanticFact::TypeDefinition {
                queried_symbol: symbol.clone(),
                targets,
            })
        })
        .collect()
}

/// Converts the definitions of some imported names into facts. An empty answer adds no fact.
pub(crate) fn definition_facts(
    queries: Vec<SourceSpan>,
    responses: Vec<Option<GotoDefinitionResponse>>,
) -> Vec<CapturedSemanticFact> {
    queries
        .into_iter()
        .zip(responses)
        .filter_map(|(query, response)| {
            let targets = response.map(definition_locations).unwrap_or_default();
            (!targets.is_empty()).then_some(CapturedSemanticFact::Definition { query, targets })
        })
        .collect()
}

/// Converts each call response of one item into caller-to-callee facts.
pub(crate) fn call_facts<C>(
    items: &[(&str, &CallHierarchyItem)],
    responses: Vec<Option<Vec<C>>>,
    fact: fn(&CallHierarchyItem, C) -> CapturedSemanticFact,
) -> Vec<CapturedSemanticFact> {
    items
        .iter()
        .zip(responses)
        .flat_map(|((_, item), calls)| {
            calls
                .unwrap_or_default()
                .into_iter()
                .map(move |call| fact(item, call))
        })
        .collect()
}

const FIRST_REQUEST_TIMEOUT: Duration = Duration::from_mins(5);
const REQUEST_TIMEOUT: Duration = Duration::from_mins(1);

// -----------------------------------------------------------------------------
// Requests
// -----------------------------------------------------------------------------

/// One request: the source path and the queried name for its error, and its
/// parameters.
pub(crate) type Labeled<'a, P> = (&'a str, &'a str, P);

/// The number of requests that one batch keeps in flight.
const REQUEST_WINDOW: usize = 64;

/// Requests that the client sends together, with the label of each.
pub(crate) struct Batch<'a> {
    requests: Vec<(&'static str, Value)>,
    labels: Vec<(&'a str, &'a str)>,
    describe: Describe,
    timeout: Duration,
}

/// Describes one request from its method, source path, and queried name.
type Describe = fn(&str, &str, &str) -> String;

pub(crate) fn describe_request(method: &str, path: &str, name: &str) -> String {
    format!("cannot request {method} for {name} in {path}")
}

impl<'a> Batch<'a> {
    pub(crate) fn new(describe: Describe) -> Self {
        Self {
            requests: Vec::new(),
            labels: Vec::new(),
            describe,
            timeout: REQUEST_TIMEOUT,
        }
    }

    /// Waits as long as for the first answer of a server, which can analyze the workspace first.
    pub(crate) fn first(self) -> Self {
        Self {
            timeout: FIRST_REQUEST_TIMEOUT,
            ..self
        }
    }

    /// Adds the requests of one method and returns their count.
    ///
    /// # Errors
    ///
    /// Returns an error if parameters cannot be encoded.
    pub(crate) fn add<R: Request>(
        &mut self,
        requests: Vec<Labeled<'a, R::Params>>,
    ) -> Result<usize, LspError> {
        let count = requests.len();
        for (path, name, parameters) in requests {
            let parameters = serde_json::to_value(parameters).map_err(|error| {
                LspError::new(format!(
                    "cannot encode the {} parameters: {error}",
                    R::METHOD
                ))
            })?;
            self.requests.push((R::METHOD, parameters));
            self.labels.push((path, name));
        }
        Ok(count)
    }

    /// Sends every request and returns the raw results in input order.
    ///
    /// # Errors
    ///
    /// Returns a server failure for the first request that fails.
    pub(crate) fn send(self, client: &mut LspClient) -> Result<BatchResults<'a>, LspError> {
        match client.request_batch(&self.requests, REQUEST_WINDOW, self.timeout) {
            Ok(values) => Ok(BatchResults {
                batch: self,
                values: values.into_iter(),
                next: 0,
            }),
            Err((index, error)) => Err(self.error(index, &error).server_failure()),
        }
    }

    /// Adds the method, source path, and queried name of one request to its error.
    fn error(&self, index: usize, error: &LspError) -> LspError {
        let (path, name) = self.labels[index];
        LspError::new(format!(
            "{}: {error}",
            (self.describe)(self.requests[index].0, path, name)
        ))
    }
}

/// The raw results of one batch, decoded group by group in input order.
pub(crate) struct BatchResults<'a> {
    batch: Batch<'a>,
    values: vec::IntoIter<Value>,
    next: usize,
}

impl BatchResults<'_> {
    /// Decodes the next `count` raw results as results of one method.
    ///
    /// # Errors
    ///
    /// Returns an error if a result does not decode.
    pub(crate) fn decode<R: Request>(
        &mut self,
        client: &mut LspClient,
        count: usize,
    ) -> Result<Vec<R::Result>, LspError> {
        let mut decoded = Vec::with_capacity(count);
        for value in self.values.by_ref().take(count) {
            let index = self.next;
            self.next += 1;
            decoded.push(serde_json::from_value(value).map_err(|error| {
                let error = client.with_standard_error(LspError::new(format!(
                    "cannot decode the {} result: {error}",
                    R::METHOD
                )));
                self.batch.error(index, &error)
            })?);
        }
        Ok(decoded)
    }
}

/// Sends one batch of requests of one method and returns the results in
/// input order.
///
/// # Errors
///
/// Returns an error for the first request that fails.
pub(crate) fn request_all<R: Request>(
    client: &mut LspClient,
    requests: Vec<Labeled<'_, R::Params>>,
) -> Result<Vec<R::Result>, LspError> {
    let mut batch = Batch::new(describe_request);
    let count = batch.add::<R>(requests)?;
    batch.send(client)?.decode::<R>(client, count)
}

pub(crate) fn symbol_requests<'a, P>(
    symbols: impl IntoIterator<Item = &'a (&'a str, ProtocolSymbolSite)>,
    parameters: fn(&ProtocolSymbolSite) -> P,
) -> Vec<Labeled<'a, P>> {
    symbols
        .into_iter()
        .map(|(path, symbol)| (*path, symbol.name.as_str(), parameters(symbol)))
        .collect()
}

pub(crate) fn item_requests<'a, P>(
    items: &[(&'a str, &'a CallHierarchyItem)],
    parameters: fn(&CallHierarchyItem) -> P,
) -> Vec<Labeled<'a, P>> {
    items
        .iter()
        .map(|(path, item)| (*path, item.name.as_str(), parameters(item)))
        .collect()
}

fn symbol_position(symbol: &ProtocolSymbolSite) -> TextDocumentPositionParams {
    TextDocumentPositionParams::new(
        TextDocumentIdentifier::new(symbol.uri.clone()),
        symbol.selection.start,
    )
}

pub(crate) fn prepare_parameters(symbol: &ProtocolSymbolSite) -> CallHierarchyPrepareParams {
    CallHierarchyPrepareParams {
        text_document_position_params: symbol_position(symbol),
        work_done_progress_params: WorkDoneProgressParams::default(),
    }
}

pub(crate) fn type_definition_parameters(symbol: &ProtocolSymbolSite) -> GotoDefinitionParams {
    position_parameters(symbol_position(symbol))
}

fn position_parameters(position: TextDocumentPositionParams) -> GotoDefinitionParams {
    GotoDefinitionParams {
        text_document_position_params: position,
        work_done_progress_params: WorkDoneProgressParams::default(),
        partial_result_params: PartialResultParams::default(),
    }
}

pub(crate) fn reference_parameters(symbol: &ProtocolSymbolSite) -> ReferenceParams {
    ReferenceParams {
        text_document_position: symbol_position(symbol),
        work_done_progress_params: WorkDoneProgressParams::default(),
        partial_result_params: PartialResultParams::default(),
        context: ReferenceContext {
            include_declaration: false,
        },
    }
}

pub(crate) fn outgoing_parameters(item: &CallHierarchyItem) -> CallHierarchyOutgoingCallsParams {
    CallHierarchyOutgoingCallsParams {
        item: item.clone(),
        work_done_progress_params: WorkDoneProgressParams::default(),
        partial_result_params: PartialResultParams::default(),
    }
}

pub(crate) fn incoming_parameters(item: &CallHierarchyItem) -> CallHierarchyIncomingCallsParams {
    CallHierarchyIncomingCallsParams {
        item: item.clone(),
        work_done_progress_params: WorkDoneProgressParams::default(),
        partial_result_params: PartialResultParams::default(),
    }
}

pub(crate) fn symbol_parameters(uri: &Uri) -> DocumentSymbolParams {
    DocumentSymbolParams {
        text_document: TextDocumentIdentifier::new(uri.clone()),
        work_done_progress_params: WorkDoneProgressParams::default(),
        partial_result_params: PartialResultParams::default(),
    }
}

// -----------------------------------------------------------------------------
// Documents
// -----------------------------------------------------------------------------

/// Converts one byte range of a source into a protocol range.
///
/// # Errors
///
/// Returns an error if the range is outside the source.
pub(crate) fn protocol_range(
    text: &str,
    path: &str,
    range: SourceRange,
) -> Result<Range, LspError> {
    Ok(Range::new(
        byte_offset_to_position(text, path, range.start)?,
        byte_offset_to_position(text, path, range.end)?,
    ))
}

pub(crate) fn range_holds(range: Range, position: Position) -> bool {
    range.start <= position && position < range.end
}

/// The URI and text of each open document by source path.
pub(crate) struct DocumentIndex<'a> {
    documents: HashMap<&'a str, (&'a Uri, &'a str)>,
}

impl<'a> DocumentIndex<'a> {
    /// # Errors
    ///
    /// Returns an error if a selected source path is not valid UTF-8.
    pub(crate) fn new(
        documents: &'a [CapturedDocument],
        sources: &Sources<'a>,
    ) -> Result<Self, LspError> {
        let texts = sources
            .iter()
            .map(|source| Ok((source_path(source)?, source.content.as_str())))
            .collect::<Result<HashMap<_, _>, LspError>>()?;
        let documents = documents
            .iter()
            .filter_map(|document| {
                let text = texts.get(document.path.as_str())?;
                Some((document.path.as_str(), (&document.uri, *text)))
            })
            .collect();
        Ok(Self { documents })
    }

    /// # Errors
    ///
    /// Returns an error if the path names no open document.
    pub(crate) fn get(&self, path: &str) -> Result<(&'a Uri, &'a str), LspError> {
        self.documents.get(path).copied().ok_or_else(|| {
            LspError::new(format!(
                "the syntax facts name a document that is not open: {path}"
            ))
        })
    }
}

// -----------------------------------------------------------------------------
// Import Definitions
// -----------------------------------------------------------------------------

/// Returns the source span and the definition request of every imported name,
/// in import-site order and name order.
///
/// # Errors
///
/// Returns an error if an import site names a document that is not open or a
/// name is outside its source.
pub(crate) fn definition_requests<'a>(
    documents: &DocumentIndex<'a>,
    import_sites: impl IntoIterator<Item = &'a ImportSite>,
) -> Result<(Vec<SourceSpan>, Vec<Labeled<'a, GotoDefinitionParams>>), LspError> {
    let mut queries = Vec::new();
    let mut requests = Vec::new();

    for site in import_sites {
        let path = site.statement.path.as_str();
        let (uri, text) = documents.get(path)?;
        for name in &site.names {
            let position = byte_offset_to_position(text, path, name.start)?;
            let name_text = text
                .get(name.start as usize..name.end as usize)
                .unwrap_or("an imported name");
            queries.push(SourceSpan {
                path: path.to_owned(),
                range: *name,
            });
            requests.push((
                path,
                name_text,
                position_parameters(TextDocumentPositionParams::new(
                    TextDocumentIdentifier::new(uri.clone()),
                    position,
                )),
            ));
        }
    }

    Ok((queries, requests))
}

// -----------------------------------------------------------------------------
// Symbol Sites
// -----------------------------------------------------------------------------

/// Flattens one symbol tree in parent-first order.
/// Each site uses the symbol selection start.
pub(crate) fn symbol_sites(uri: &Uri, symbols: &[DocumentSymbol]) -> Vec<ProtocolSymbolSite> {
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
pub(crate) fn outgoing_call_fact(
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
pub(crate) fn incoming_call_fact(
    callee: &CallHierarchyItem,
    call: CallHierarchyIncomingCall,
) -> CapturedSemanticFact {
    CapturedSemanticFact::Call {
        caller: call_item_symbol_site(&call.from),
        callee: call_item_symbol_site(callee),
        call_sites: call_locations(&call.from.uri, call.from_ranges),
    }
}

/// Reshapes every definition or type-definition response form into locations.
///
/// The protocol supports a scalar, an array, and a link response.
/// A link uses its target selection range.
fn definition_locations(response: GotoDefinitionResponse) -> Vec<Location> {
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
        CapturedSemanticFact, ProtocolSymbolSite, definition_locations, incoming_call_fact,
        nested_symbols, outgoing_call_fact, symbol_sites,
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
    fn normalizes_all_definition_shapes() {
        let first = Location::new(uri("file:///workspace/first.py"), range(1));
        let second = Location::new(uri("file:///workspace/second.py"), range(2));
        let link = LocationLink {
            origin_selection_range: None,
            target_uri: second.uri.clone(),
            target_range: range(20),
            target_selection_range: second.range,
        };

        assert_eq!(
            definition_locations(GotoDefinitionResponse::Scalar(first.clone())),
            vec![first.clone()]
        );
        assert_eq!(
            definition_locations(GotoDefinitionResponse::Array(vec![first.clone()])),
            vec![first]
        );
        assert_eq!(
            definition_locations(GotoDefinitionResponse::Link(vec![link])),
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
