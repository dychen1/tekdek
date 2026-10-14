//! Semantic capture.
//!
//! This module owns every document request: document opening, document
//! symbols, the semantic requests of each symbol, the definition request of
//! each imported name, and response conversion. An initialized client, a
//! session configuration, selected sources, and import sites are the inputs.
//! One protocol capture is the output.
//!
//! # Sections
//!
//! `Document Capture` opens the documents and captures their symbols and facts.
//! `Requests` sends the requests of one phase and builds their parameters.
//! `Explained References` finds the references that need no incoming calls,
//! and the symbols that are syntax entities.
//! `Import Definitions` builds the definition request of each imported name.
//! `Symbol Sites` converts hierarchical symbols into ordered protocol sites.
//! `Response Conversion` converts protocol responses into captured facts.

use std::{
    collections::{HashMap, HashSet},
    path::Path,
    time::Duration,
    vec,
};

use lsp_types::{
    CallHierarchyIncomingCall, CallHierarchyIncomingCallsParams, CallHierarchyItem,
    CallHierarchyOutgoingCall, CallHierarchyOutgoingCallsParams, CallHierarchyPrepareParams,
    DidOpenTextDocumentParams, DocumentSymbol, DocumentSymbolParams, DocumentSymbolResponse,
    GotoDefinitionParams, GotoDefinitionResponse, Location, PartialResultParams, Position, Range,
    ReferenceContext, ReferenceParams, TextDocumentIdentifier, TextDocumentItem,
    TextDocumentPositionParams, Uri, WorkDoneProgressParams,
    notification::DidOpenTextDocument,
    request::{
        CallHierarchyIncomingCalls, CallHierarchyOutgoingCalls, CallHierarchyPrepare,
        DocumentSymbolRequest, GotoDefinition, GotoTypeDefinition, References, Request,
    },
};

use serde_json::Value;

use super::{
    CapturedDocument, CapturedSemanticFact, LspClient, LspError, ProtocolSymbolSite,
    SemanticCapture, file_uri,
    normalize::{byte_offset_to_position, source_path},
    session::{SessionConfig, language_title},
};
use crate::analysis::{
    fact::{EntityKind, ImportSite, SyntaxFacts},
    source::{SourceRange, SourceSpan, Sources},
};

// -----------------------------------------------------------------------------
// Document Capture
// -----------------------------------------------------------------------------

/// Opens all documents, then captures facts for every hierarchical symbol and
/// every imported name.
///
/// # Errors
///
/// Returns an error if a document cannot open or a request fails.
pub(crate) fn capture_document_semantics(
    client: &mut LspClient,
    config: &SessionConfig<'_>,
    root: &Path,
    sources: &Sources<'_>,
    syntax: &SyntaxFacts,
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

    let symbol_parameters = |uri: &Uri| DocumentSymbolParams {
        text_document: TextDocumentIdentifier::new(uri.clone()),
        work_done_progress_params: WorkDoneProgressParams::default(),
        partial_result_params: PartialResultParams::default(),
    };
    let title = language_title(config.language);
    let label = format!("{title} symbols");
    let mut responses = Vec::with_capacity(files.len());
    // The first answer waits until the server analyzes the workspace.
    if let Some((path, uri)) = files.first() {
        let response = client
            .request::<DocumentSymbolRequest>(symbol_parameters(uri), FIRST_REQUEST_TIMEOUT)
            .map_err(|error| {
                LspError::new(format!("cannot read {title} symbols from {path}: {error}"))
            })?;
        responses.push(response);
    }
    responses.extend(request_all::<DocumentSymbolRequest>(
        client,
        files
            .iter()
            .skip(1)
            .map(|(path, uri)| (path.as_str(), label.as_str(), symbol_parameters(uri)))
            .collect(),
    )?);
    let files = files
        .into_iter()
        .zip(responses)
        .map(|((path, uri), response)| {
            Ok(CapturedDocument {
                symbols: nested_symbols(response, &path)?,
                path,
                uri,
            })
        })
        .collect::<Result<Vec<_>, LspError>>()?;

    let facts = request_facts(client, &files, sources, syntax)?;

    Ok(SemanticCapture {
        documents: files,
        facts,
    })
}

/// Captures all supported semantic facts for every document symbol and every
/// imported name.
///
/// Only an entity symbol, whose selection starts at a declared entity name,
/// gets a call hierarchy item. Every symbol gets a type-definition request,
/// because the type of a local or a parameter states a type use of its owner.
/// An entity symbol and a symbol outside every callable get a reference
/// request, because a reference to a module-level name, such as a type alias,
/// can state a signature type. Other queries of other symbols add no edge.
///
/// The capture runs in phases, and each phase sends its requests in one batch.
/// It prepares the call hierarchy items of all symbols. Then it requests
/// outgoing calls, references, type definitions, and import definitions
/// together. Last, it requests incoming calls only for the items of a
/// symbol that has a reference outside every outgoing call site and every
/// import statement. Each incoming call site is also a reference, so only such
/// a symbol can have a caller that is not a document symbol, for example
/// module-level code.
///
/// Facts are outgoing calls, incoming calls, references, type definitions,
/// and import definitions, in this order. Each group follows symbol order (or
/// import-site order), then item order, then response order. An empty
/// response adds no fact.
///
/// # Errors
///
/// Returns an error if an import site is invalid or a request fails.
fn request_facts(
    client: &mut LspClient,
    documents: &[CapturedDocument],
    sources: &Sources<'_>,
    syntax: &SyntaxFacts,
) -> Result<Vec<CapturedSemanticFact>, LspError> {
    let import_sites = &syntax.import_sites;
    let symbols = documents
        .iter()
        .flat_map(|document| {
            symbol_sites(&document.uri, &document.symbols)
                .into_iter()
                .map(|symbol| (document.path.as_str(), symbol))
        })
        .collect::<Vec<_>>();

    let (is_entity, wants_references) =
        EntityPositions::new(&DocumentIndex::new(documents, sources)?, syntax)?.flags(&symbols);

    let prepared = request_all::<CallHierarchyPrepare>(
        client,
        symbol_requests(selected(&symbols, &is_entity), prepare_parameters),
    )?
    .into_iter()
    .map(Option::unwrap_or_default)
    .collect::<Vec<_>>();
    let prepared = scatter(&is_entity, prepared);
    let items = symbols
        .iter()
        .zip(&prepared)
        .flat_map(|((path, _), items)| items.iter().map(move |item| (*path, item)))
        .collect::<Vec<_>>();

    let (queries, definition_requests) = definition_requests(documents, sources, import_sites)?;
    let mut batch = Batch::default();
    let outgoing_count =
        batch.add::<CallHierarchyOutgoingCalls>(item_requests(&items, outgoing_parameters))?;
    let reference_count = batch.add::<References>(symbol_requests(
        selected(&symbols, &wants_references),
        reference_parameters,
    ))?;
    let type_count =
        batch.add::<GotoTypeDefinition>(symbol_requests(&symbols, type_definition_parameters))?;
    let definition_count = batch.add::<GotoDefinition>(definition_requests)?;
    let mut results = batch.send(client)?;
    let outgoing = decode::<CallHierarchyOutgoingCalls>(&mut results, outgoing_count)?;
    let references = decode::<References>(&mut results, reference_count)?
        .into_iter()
        .map(Option::unwrap_or_default)
        .collect::<Vec<_>>();
    let references = scatter(&wants_references, references);
    let type_targets = decode::<GotoTypeDefinition>(&mut results, type_count)?;
    let definitions = decode::<GotoDefinition>(&mut results, definition_count)?;

    let mut facts = call_facts(&items, outgoing, outgoing_call_fact);
    let explained = ExplainedPositions::new(&facts, documents, sources, import_sites)?;
    let callees = symbols
        .iter()
        .zip(&prepared)
        .zip(&references)
        .filter(|(_, occurrences)| {
            !occurrences
                .iter()
                .all(|location| explained.contains(location))
        })
        .flat_map(|(((path, _), items), _)| items.iter().map(move |item| (*path, item)))
        .collect::<Vec<_>>();
    let incoming = request_all::<CallHierarchyIncomingCalls>(
        client,
        item_requests(&callees, incoming_parameters),
    )?;
    facts.extend(call_facts(&callees, incoming, incoming_call_fact));

    facts.extend(
        symbols
            .iter()
            .zip(references)
            .filter(|(_, occurrences)| !occurrences.is_empty())
            .map(
                |((_, symbol), occurrences)| CapturedSemanticFact::Reference {
                    target: symbol.clone(),
                    occurrences,
                },
            ),
    );
    facts.extend(
        symbols
            .iter()
            .zip(type_targets)
            .filter_map(|((_, symbol), response)| {
                let targets = response.map(definition_locations).unwrap_or_default();
                (!targets.is_empty()).then(|| CapturedSemanticFact::TypeDefinition {
                    queried_symbol: symbol.clone(),
                    targets,
                })
            }),
    );
    facts.extend(
        queries
            .into_iter()
            .zip(definitions)
            .filter_map(|(query, response)| {
                let targets = response.map(definition_locations).unwrap_or_default();
                (!targets.is_empty()).then_some(CapturedSemanticFact::Definition { query, targets })
            }),
    );

    Ok(facts)
}

/// Converts each call response of one item into caller-to-callee facts.
fn call_facts<C>(
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
type Labeled<'a, P> = (&'a str, &'a str, P);

/// The number of requests that one batch keeps in flight.
const REQUEST_WINDOW: usize = 64;

/// Requests that the client sends together, with the label of each.
#[derive(Default)]
struct Batch<'a> {
    requests: Vec<(&'static str, Value)>,
    labels: Vec<(&'a str, &'a str)>,
}

impl<'a> Batch<'a> {
    /// Adds the requests of one method and returns their count.
    ///
    /// # Errors
    ///
    /// Returns an error if parameters cannot be encoded.
    fn add<R: Request>(
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
    /// Returns an error for the first request that fails.
    fn send(&self, client: &mut LspClient) -> Result<vec::IntoIter<Value>, LspError> {
        client
            .request_batch(&self.requests, REQUEST_WINDOW, REQUEST_TIMEOUT)
            .map(Vec::into_iter)
            .map_err(|(index, error)| {
                let (path, name) = self.labels[index];
                LspError::new(format!(
                    "cannot request {} for {name} in {path}: {error}",
                    self.requests[index].0
                ))
            })
    }
}

/// Decodes the next `count` raw results as results of one method.
///
/// # Errors
///
/// Returns an error if a result does not decode.
fn decode<R: Request>(
    results: &mut impl Iterator<Item = Value>,
    count: usize,
) -> Result<Vec<R::Result>, LspError> {
    results
        .take(count)
        .map(|result| {
            serde_json::from_value(result).map_err(|error| {
                LspError::new(format!("cannot decode the {} result: {error}", R::METHOD))
            })
        })
        .collect()
}

/// Sends one batch of requests of one method and returns the results in
/// input order.
///
/// # Errors
///
/// Returns an error for the first request that fails.
fn request_all<R: Request>(
    client: &mut LspClient,
    requests: Vec<Labeled<'_, R::Params>>,
) -> Result<Vec<R::Result>, LspError> {
    let mut batch = Batch::default();
    let count = batch.add::<R>(requests)?;
    decode::<R>(&mut batch.send(client)?, count)
}

fn symbol_requests<'a, P>(
    symbols: impl IntoIterator<Item = &'a (&'a str, ProtocolSymbolSite)>,
    parameters: fn(&ProtocolSymbolSite) -> P,
) -> Vec<Labeled<'a, P>> {
    symbols
        .into_iter()
        .map(|(path, symbol)| (*path, symbol.name.as_str(), parameters(symbol)))
        .collect()
}

/// Returns the entries whose flag is set.
fn selected<'a, T>(entries: &'a [T], flags: &'a [bool]) -> impl Iterator<Item = &'a T> {
    entries
        .iter()
        .zip(flags)
        .filter_map(|(entry, flag)| flag.then_some(entry))
}

/// Spreads the results of the selected entries over all entries. An entry
/// whose flag is not set gets the default.
fn scatter<T: Default>(flags: &[bool], results: Vec<T>) -> Vec<T> {
    let mut results = results.into_iter();
    flags
        .iter()
        .map(|flag| {
            if *flag {
                results.next().unwrap_or_default()
            } else {
                T::default()
            }
        })
        .collect()
}

fn item_requests<'a, P>(
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

fn prepare_parameters(symbol: &ProtocolSymbolSite) -> CallHierarchyPrepareParams {
    CallHierarchyPrepareParams {
        text_document_position_params: symbol_position(symbol),
        work_done_progress_params: WorkDoneProgressParams::default(),
    }
}

fn type_definition_parameters(symbol: &ProtocolSymbolSite) -> GotoDefinitionParams {
    position_parameters(symbol_position(symbol))
}

fn position_parameters(position: TextDocumentPositionParams) -> GotoDefinitionParams {
    GotoDefinitionParams {
        text_document_position_params: position,
        work_done_progress_params: WorkDoneProgressParams::default(),
        partial_result_params: PartialResultParams::default(),
    }
}

fn reference_parameters(symbol: &ProtocolSymbolSite) -> ReferenceParams {
    ReferenceParams {
        text_document_position: symbol_position(symbol),
        work_done_progress_params: WorkDoneProgressParams::default(),
        partial_result_params: PartialResultParams::default(),
        context: ReferenceContext {
            include_declaration: false,
        },
    }
}

fn outgoing_parameters(item: &CallHierarchyItem) -> CallHierarchyOutgoingCallsParams {
    CallHierarchyOutgoingCallsParams {
        item: item.clone(),
        work_done_progress_params: WorkDoneProgressParams::default(),
        partial_result_params: PartialResultParams::default(),
    }
}

fn incoming_parameters(item: &CallHierarchyItem) -> CallHierarchyIncomingCallsParams {
    CallHierarchyIncomingCallsParams {
        item: item.clone(),
        work_done_progress_params: WorkDoneProgressParams::default(),
        partial_result_params: PartialResultParams::default(),
    }
}

// -----------------------------------------------------------------------------
// Explained References
// -----------------------------------------------------------------------------

/// The protocol positions that an outgoing call site or an import statement
/// explains.
///
/// A reference at such a position needs no incoming-call request.
struct ExplainedPositions {
    call_sites: HashSet<(String, Position)>,
    import_statements: HashMap<String, Vec<Range>>,
}

impl ExplainedPositions {
    /// # Errors
    ///
    /// Returns an error if an import site is invalid.
    fn new(
        call_facts: &[CapturedSemanticFact],
        documents: &[CapturedDocument],
        sources: &Sources<'_>,
        import_sites: &[ImportSite],
    ) -> Result<Self, LspError> {
        let call_sites = call_facts
            .iter()
            .filter_map(|fact| match fact {
                CapturedSemanticFact::Call { call_sites, .. } => Some(call_sites),
                CapturedSemanticFact::Reference { .. }
                | CapturedSemanticFact::TypeDefinition { .. }
                | CapturedSemanticFact::Definition { .. } => None,
            })
            .flatten()
            .map(|site| (site.uri.as_str().to_owned(), site.range.start))
            .collect();
        let documents = DocumentIndex::new(documents, sources)?;
        let mut import_statements = HashMap::<String, Vec<Range>>::new();
        for site in import_sites {
            let path = site.statement.path.as_str();
            let (uri, text) = documents.get(path)?;
            import_statements
                .entry(uri.as_str().to_owned())
                .or_default()
                .push(protocol_range(text, path, site.statement.range)?);
        }

        Ok(Self {
            call_sites,
            import_statements,
        })
    }

    fn contains(&self, location: &Location) -> bool {
        let uri = location.uri.as_str();
        let start = location.range.start;
        self.call_sites.contains(&(uri.to_owned(), start))
            || self
                .import_statements
                .get(uri)
                .is_some_and(|ranges| ranges.iter().any(|range| range_holds(*range, start)))
    }
}

/// The protocol positions of the syntax entities.
struct EntityPositions<'a> {
    names: HashSet<(&'a str, Position)>,
    callable_scopes: HashMap<&'a str, Vec<Range>>,
}

impl<'a> EntityPositions<'a> {
    /// # Errors
    ///
    /// Returns an error if an entity is in a document that is not open or
    /// outside its source.
    fn new(documents: &DocumentIndex<'_>, syntax: &'a SyntaxFacts) -> Result<Self, LspError> {
        let mut names = HashSet::new();
        let mut callable_scopes = HashMap::<&str, Vec<Range>>::new();
        for entity in &syntax.entities {
            let path = entity.evidence.path.as_str();
            let (_, text) = documents.get(path)?;
            for declaration in &entity.evidence.declarations {
                names.insert((
                    path,
                    byte_offset_to_position(text, path, declaration.start)?,
                ));
            }
            if let (EntityKind::Callable(_), Some(scope)) = (&entity.kind, entity.evidence.scope) {
                callable_scopes
                    .entry(path)
                    .or_default()
                    .push(protocol_range(text, path, scope)?);
            }
        }
        Ok(Self {
            names,
            callable_scopes,
        })
    }

    /// Returns, for each symbol, whether it is an entity and whether it gets a
    /// reference request: an entity or a symbol outside every callable.
    fn flags(&self, symbols: &[(&str, ProtocolSymbolSite)]) -> (Vec<bool>, Vec<bool>) {
        symbols
            .iter()
            .map(|(path, symbol)| {
                let entity = self.is_entity(path, symbol);
                (entity, entity || !self.in_callable(path, symbol))
            })
            .unzip()
    }

    /// Returns `true` when an entity name starts at the selection of the symbol.
    fn is_entity(&self, path: &str, symbol: &ProtocolSymbolSite) -> bool {
        self.names.contains(&(path, symbol.selection.start))
    }

    /// Returns `true` when a callable scope holds the selection of the symbol.
    fn in_callable(&self, path: &str, symbol: &ProtocolSymbolSite) -> bool {
        self.callable_scopes.get(path).is_some_and(|scopes| {
            scopes
                .iter()
                .any(|scope| range_holds(*scope, symbol.selection.start))
        })
    }
}

/// Converts one byte range of a source into a protocol range.
///
/// # Errors
///
/// Returns an error if the range is outside the source.
fn protocol_range(text: &str, path: &str, range: SourceRange) -> Result<Range, LspError> {
    Ok(Range::new(
        byte_offset_to_position(text, path, range.start)?,
        byte_offset_to_position(text, path, range.end)?,
    ))
}

fn range_holds(range: Range, position: Position) -> bool {
    range.start <= position && position < range.end
}

/// The URI and text of each open document by source path.
struct DocumentIndex<'a> {
    documents: HashMap<&'a str, (&'a Uri, &'a str)>,
}

impl<'a> DocumentIndex<'a> {
    /// # Errors
    ///
    /// Returns an error if a selected source path is not valid UTF-8.
    fn new(documents: &'a [CapturedDocument], sources: &Sources<'a>) -> Result<Self, LspError> {
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
    fn get(&self, path: &str) -> Result<(&'a Uri, &'a str), LspError> {
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
fn definition_requests<'a>(
    documents: &'a [CapturedDocument],
    sources: &Sources<'a>,
    import_sites: &'a [ImportSite],
) -> Result<(Vec<SourceSpan>, Vec<Labeled<'a, GotoDefinitionParams>>), LspError> {
    let documents = DocumentIndex::new(documents, sources)?;
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
        CapturedDocument, CapturedSemanticFact, DocumentIndex, EntityPositions, ExplainedPositions,
        ProtocolSymbolSite, definition_locations, incoming_call_fact, nested_symbols,
        outgoing_call_fact, symbol_sites,
    };
    use crate::analysis::{
        fact::{
            CallableData, CallableKind, EntityEvidence, EntityKey, EntityKind, ImportSite,
            SyntaxFactBuilder,
        },
        source::{DocumentSnapshot, SourceRange, SourceSpan, Sources},
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
    fn explains_references_at_call_sites_and_in_import_statements() {
        let uri = uri("file:///workspace/app.py");
        let source = DocumentSnapshot {
            path: "app.py".into(),
            content: "from lib import run\nrun()\nhandler = run\n".to_owned(),
        };
        let documents = [CapturedDocument {
            path: "app.py".to_owned(),
            uri: uri.clone(),
            symbols: Vec::new(),
        }];
        let item = call_item("caller", "file:///workspace/app.py", 1);
        let call = CapturedSemanticFact::Call {
            caller: symbol_site(&item),
            callee: symbol_site(&item),
            call_sites: vec![Location::new(uri.clone(), range(1))],
        };
        let import = ImportSite {
            owner: EntityKey::module("app.py"),
            statement: SourceSpan {
                path: "app.py".to_owned(),
                range: SourceRange { start: 0, end: 19 },
            },
            names: vec![SourceRange { start: 16, end: 19 }],
        };

        let explained =
            ExplainedPositions::new(&[call], &documents, &Sources::new(vec![&source]), &[import])
                .expect("the import site should be valid");
        let at = |line, character| {
            Location::new(
                uri.clone(),
                Range::new(
                    Position::new(line, character),
                    Position::new(line, character + 3),
                ),
            )
        };

        assert!(explained.contains(&at(0, 16)));
        assert!(explained.contains(&at(1, 0)));
        assert!(!explained.contains(&at(2, 10)));
    }

    #[test]
    fn finds_entity_symbols_and_symbols_inside_callables() {
        let content = "Alias = int\ndef run(value: Alias):\n    local = value\n";
        let source = DocumentSnapshot {
            path: "app.py".into(),
            content: content.to_owned(),
        };
        let documents = [CapturedDocument {
            path: "app.py".to_owned(),
            uri: uri("file:///workspace/app.py"),
            symbols: Vec::new(),
        }];
        let mut syntax = SyntaxFactBuilder::new();
        let module = syntax
            .add_module("app.py", content)
            .expect("the module should be new");
        syntax.add_entity(
            &module,
            "run".to_owned(),
            EntityKind::Callable(CallableData {
                kind: CallableKind::Function,
                signature: None,
                parameters: Vec::new(),
                return_type: None,
                documentation: None,
            }),
            EntityEvidence {
                path: "app.py".to_owned(),
                declarations: vec![SourceRange { start: 16, end: 19 }],
                scope: Some(SourceRange { start: 12, end: 53 }),
                body: None,
                type_inputs: Vec::new(),
                type_outputs: Vec::new(),
            },
        );
        let syntax = syntax.finish();
        let sources = Sources::new(vec![&source]);
        let index = DocumentIndex::new(&documents, &sources).expect("the document is open");

        let entities = EntityPositions::new(&index, &syntax).expect("the entities should be valid");
        let at = |line, character| ProtocolSymbolSite {
            uri: uri("file:///workspace/app.py"),
            selection: Range::new(
                Position::new(line, character),
                Position::new(line, character + 1),
            ),
            kind: SymbolKind::VARIABLE,
            name: "symbol".to_owned(),
        };

        // `run` is an entity, `Alias` is a module-level name, and the parameter
        // and the local are inside `run`.
        assert!(entities.is_entity("app.py", &at(1, 4)));
        assert!(!entities.is_entity("app.py", &at(0, 0)));
        assert!(!entities.in_callable("app.py", &at(0, 0)));
        assert!(entities.in_callable("app.py", &at(1, 8)));
        assert!(entities.in_callable("app.py", &at(2, 4)));
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
