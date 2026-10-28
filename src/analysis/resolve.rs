//! On-demand semantic resolution.
//!
//! This module owns the resolver: it parses one codebase once, starts the
//! language server of a language when a key of that language needs it, and
//! asks that server for the semantic edges that touch some node keys. Node keys
//! are the input. Semantic edges of the parsed graph are the output. The graph
//! semantic stage builds the edges, so kinds, observations, and weights follow
//! the index rules.
//!
//! # Rules
//!
//! - Callees: the outgoing calls of the key.
//! - Callers and uses: the references of the key, grouped by the deepest body
//!   that holds each one. A call site in the outgoing calls of such an owner
//!   explains a reference that it holds. A reference that no call site and no
//!   import statement explains gets one incoming-calls request on the key, for
//!   example a call from module-level code, which has no call hierarchy item.
//! - A server can search references by name, so an import that renames the
//!   key (`name as alias`) adds the references of the alias.
//! - The last direct constructor of a class also asks about its class, because
//!   a call of the class is a call of that constructor. Only the calls of the
//!   class references become edges.
//! - Types: the type definitions of the document symbols in the scope of the key.
//! - Imports: the definitions of the names of each import statement in the file
//!   of a key or around a reference of a key. A key without a declaration, such
//!   as a file module, has no references, so every import statement of its
//!   language gets a definition request.
//!
//! The edges are the edges that touch a key, and every import edge of those
//! import statements. Some edges of a key come only from the other end, when
//! that end is resolved too: a callee that only a macro or a function value
//! names, a value or a field that the key uses, the signature types of the key
//! (until then a parameter type is a `uses_type` edge), and the calls of
//! module-level code. A weight counts the observations of one resolution, so
//! keys that resolve together can count more: a weight is a lower bound.
//!
//! # Sections
//!
//! `Resolver` parses the codebase and keeps one server for each language.
//! `Scope` sends the requests of some keys and builds their edges.

use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet, btree_map::Entry},
    fs, iter,
    path::{Path, PathBuf},
};

use lsp_types::{
    CallHierarchyItem, DidOpenTextDocumentParams, SymbolKind, TextDocumentItem,
    notification::DidOpenTextDocument,
    request::{
        CallHierarchyIncomingCalls, CallHierarchyOutgoingCalls, CallHierarchyPrepare,
        DocumentSymbolRequest, GotoDefinition, GotoTypeDefinition, References,
    },
};

use super::{
    PipelineConfig, PipelineError,
    fact::{AnalysisError, EntityKind, ImportSite, RelationKind, ReportedLocation, SemanticFact},
    graph::{
        Edge, Graph, NodeId, SyntaxGraphIndex, build_semantic_edges, collect_call_sites,
        constructors_by_class, is_call_site,
    },
    language::{Language, LanguageProfile, document_language_id, session_config},
    lsp::{
        CapturedDocument, CapturedSemanticFact, FallbackPolicy, LspClient, LspError,
        ProtocolSymbolSite, SemanticCapture,
        capture::{
            Batch, DocumentIndex, call_facts, definition_facts, definition_requests,
            describe_request, incoming_call_fact, incoming_parameters, item_requests,
            nested_symbols, outgoing_call_fact, outgoing_parameters, prepare_parameters,
            protocol_range, range_holds, reference_parameters, request_all, symbol_parameters,
            symbol_requests, symbol_sites, type_definition_facts, type_definition_parameters,
        },
        file_uri,
        normalize::{normalize_capture, source_path, validate_capture},
        session,
    },
    pipeline::{Parsed, adapter, parse},
    preflight::preflight_server_programs,
    source::{SourceRange, SourceSpan, Sources},
};

// -----------------------------------------------------------------------------
// Resolver
// -----------------------------------------------------------------------------

/// One parsed codebase and the language servers that resolve its node keys.
///
/// A server starts at the first key of its language and stays until `shutdown`.
pub struct Resolver {
    root: PathBuf,
    parsed: Parsed,
    ids: HashMap<String, NodeId>,
    /// The class of each last direct constructor.
    classes: HashMap<NodeId, NodeId>,
    sessions: BTreeMap<Language, Session>,
}

impl Resolver {
    /// Parses the codebase at `root` with the given language profiles.
    ///
    /// # Errors
    ///
    /// Returns an error if the root cannot resolve or the parse fails.
    pub fn new(root: &Path, profiles: Vec<LanguageProfile>) -> Result<Self, PipelineError> {
        let root = fs::canonicalize(root)?;
        let mut config = PipelineConfig::new(root.clone());
        config.profiles = profiles;
        let parsed = parse(&config)?;
        let ids = parsed
            .graph
            .identified_nodes()
            .map(|(id, node)| (node.key.as_str().to_owned(), id))
            .collect();
        let classes = constructors_by_class(&parsed.graph)
            .into_iter()
            .map(|(class, constructor)| (constructor, class))
            .collect();
        Ok(Self {
            root,
            parsed,
            ids,
            classes,
            sessions: BTreeMap::new(),
        })
    }

    /// Returns the hash of the source files that the resolver parsed.
    #[must_use]
    pub fn source_hash(&self) -> &str {
        &self.parsed.source_hash
    }

    /// Returns the parsed syntax graph. Resolved edges name its nodes.
    #[must_use]
    pub fn graph(&self) -> &Graph {
        &self.parsed.graph
    }

    /// Returns the semantic edges that touch the given node keys.
    ///
    /// A failed request or transport stops the server of its language, and the
    /// next key of that language starts a new one. Other errors, such as an
    /// answer that is not valid, keep the server.
    ///
    /// # Errors
    ///
    /// Returns an error if a key names no node, or a server or request fails.
    pub fn resolve(&mut self, keys: &[String]) -> Result<Vec<Edge>, PipelineError> {
        let mut ids = BTreeSet::new();
        for key in keys {
            let id = self.ids.get(key).copied().ok_or_else(|| {
                PipelineError::Setup(format!("the graph has no node with the key {key}"))
            })?;
            ids.insert(id);
        }
        let mut edges = Vec::new();
        for index in 0..self.parsed.profiles.len() {
            edges.extend(self.resolve_language(index, &ids)?);
        }
        Ok(edges)
    }

    /// Stops every running server.
    ///
    /// # Errors
    ///
    /// Returns the first shutdown error after every server stops.
    pub fn shutdown(self) -> Result<(), PipelineError> {
        let mut result = Ok(());
        for (language, session) in self.sessions {
            let stopped = session
                .client
                .shutdown()
                .map_err(|error| PipelineError::Language {
                    language,
                    source: server_error(error),
                });
            result = result.and(stopped);
        }
        result
    }

    /// Resolves the keys of the language of one profile.
    fn resolve_language(
        &mut self,
        index: usize,
        ids: &BTreeSet<NodeId>,
    ) -> Result<Vec<Edge>, PipelineError> {
        let Self {
            root,
            parsed,
            classes,
            sessions,
            ..
        } = self;
        let profile = &parsed.profiles[index];
        let language = profile.language;
        let keys = ids
            .iter()
            .copied()
            .filter(|id| parsed.graph.nodes[id.index()].language == language)
            .collect::<BTreeSet<_>>();
        if keys.is_empty() {
            return Ok(Vec::new());
        }
        let failed = |source| PipelineError::Language { language, source };
        let config = session_config(root, profile).ok_or_else(|| {
            failed(AnalysisError::new(
                "this build has no language server for the language",
            ))
        })?;
        let session = match sessions.entry(language) {
            Entry::Occupied(entry) => entry.into_mut(),
            Entry::Vacant(entry) => {
                preflight_server_programs(&[profile])?;
                let (client, _) =
                    session::start(&config).map_err(|error| failed(server_error(error)))?;
                entry.insert(Session {
                    client,
                    opened: HashSet::new(),
                    answered: false,
                })
            }
        };
        let sources = adapter(profile)
            .and_then(|adapter| adapter.select_sources(&parsed.documents))
            .map_err(failed)?;
        let documents = sources
            .iter()
            .map(|source| {
                let path = source_path(source)?;
                Ok(CapturedDocument {
                    path: path.to_owned(),
                    uri: file_uri(&root.join(path))?,
                    symbols: Vec::new(),
                })
            })
            .collect::<Result<Vec<_>, LspError>>()
            .map_err(|error| failed(server_error(error)))?;
        let imports = parsed
            .imports
            .iter()
            .find(|(each, _)| *each == language)
            .map_or(&[][..], |(_, imports)| imports.as_slice());
        let scope = Scope {
            root,
            language,
            language_id: config.language_id,
            graph: &parsed.graph,
            lookup: SyntaxGraphIndex::new(&parsed.graph, imports),
            imports,
            documents: DocumentIndex::new(&documents, &sources)
                .map_err(|error| failed(server_error(error)))?,
            sources: &sources,
            classes,
            fallback: config.fallback,
        };
        let resolved = scope.resolve(session, &keys);
        if resolved.as_ref().is_err_and(LspError::is_server_failure) {
            // A failed request can leave the server in a broken state.
            sessions.remove(&language);
        }
        resolved.map_err(|error| failed(server_error(error)))
    }
}

/// One running language server and the documents it has open.
struct Session {
    client: LspClient,
    opened: HashSet<String>,
    /// True after the first batch: before it, the server can still analyze the workspace.
    answered: bool,
}

impl Session {
    /// Opens each document that is not open yet.
    ///
    /// # Errors
    ///
    /// Returns an error if a path names no source or the notification fails.
    fn open<'p>(
        &mut self,
        scope: &Scope<'_>,
        paths: impl IntoIterator<Item = &'p str>,
    ) -> Result<(), LspError> {
        for path in paths {
            if self.opened.contains(path) {
                continue;
            }
            let (uri, text) = scope.documents.get(path)?;
            self.client
                .notify::<DidOpenTextDocument>(DidOpenTextDocumentParams {
                    text_document: TextDocumentItem::new(
                        uri.clone(),
                        document_language_id(scope.language, scope.language_id, path).to_owned(),
                        1,
                        text.to_owned(),
                    ),
                })
                .map_err(LspError::server_failure)?;
            self.opened.insert(path.to_owned());
        }
        Ok(())
    }
}

fn server_error(error: LspError) -> AnalysisError {
    AnalysisError::new(format!(
        "cannot resolve semantic edges with the language server: {}",
        error.into_message()
    ))
}

// -----------------------------------------------------------------------------
// Scope
// -----------------------------------------------------------------------------

/// The parsed sources of one language during one resolution.
struct Scope<'a> {
    root: &'a Path,
    language: Language,
    language_id: &'a str,
    graph: &'a Graph,
    lookup: SyntaxGraphIndex<'a>,
    imports: &'a [(NodeId, ImportSite)],
    documents: DocumentIndex<'a>,
    sources: &'a Sources<'a>,
    classes: &'a HashMap<NodeId, NodeId>,
    fallback: FallbackPolicy,
}

/// One protocol site with its source path.
type Site<'a> = (&'a str, ProtocolSymbolSite);

impl<'a> Scope<'a> {
    /// Sends the requests of some keys of this language and returns their edges.
    ///
    /// The requests go in four rounds: the sites of the keys, then their
    /// callees and the reference owners, then the callees of those owners, and
    /// last the incoming calls of the keys with an unexplained reference. An
    /// import that renames a key adds one round for the references of its alias.
    ///
    /// # Errors
    ///
    /// Returns an error if a request fails or a fact has no permitted node.
    #[allow(
        clippy::too_many_lines,
        reason = "the four request rounds share their answers"
    )]
    fn resolve(
        &self,
        session: &mut Session,
        keys: &BTreeSet<NodeId>,
    ) -> Result<Vec<Edge>, LspError> {
        // Each key, and the class of a last constructor, has one site.
        let mut sites = Vec::<(NodeId, NodeId, Site<'_>)>::new();
        for &key in keys {
            for node in iter::once(key).chain(self.classes.get(&key).copied()) {
                if let Some(site) = self.site(node)? {
                    sites.push((key, node, site));
                }
            }
        }
        let files = keys
            .iter()
            .map(|key| self.graph.nodes[key.index()].path())
            .collect::<BTreeSet<_>>();
        session.open(
            self,
            files
                .iter()
                .copied()
                .chain(sites.iter().map(|(_, _, (path, _))| *path)),
        )?;
        let key_sites = sites
            .iter()
            .map(|(_, _, site)| site.clone())
            .collect::<Vec<_>>();
        let file_uris = files
            .iter()
            .map(|path| Ok((*path, self.documents.get(path)?.0)))
            .collect::<Result<Vec<_>, LspError>>()?;

        let mut batch = Batch::new(describe_request);
        let prepare_count =
            batch.add::<CallHierarchyPrepare>(symbol_requests(&key_sites, prepare_parameters))?;
        let reference_count =
            batch.add::<References>(symbol_requests(&key_sites, reference_parameters))?;
        let symbol_count = batch.add::<DocumentSymbolRequest>(
            file_uris
                .iter()
                .map(|(path, uri)| (*path, "the document symbols", symbol_parameters(uri)))
                .collect(),
        )?;
        let batch = if session.answered {
            batch
        } else {
            batch.first()
        };
        let client = &mut session.client;
        let mut results = batch.send(client)?;
        session.answered = true;
        let prepared = results
            .decode::<CallHierarchyPrepare>(client, prepare_count)?
            .into_iter()
            .map(Option::unwrap_or_default)
            .collect::<Vec<_>>();
        let references = results.decode::<References>(client, reference_count)?;
        let mut symbols = HashMap::new();
        for ((path, uri), response) in file_uris
            .iter()
            .zip(results.decode::<DocumentSymbolRequest>(client, symbol_count)?)
        {
            symbols.insert(*path, symbol_sites(uri, &nested_symbols(response, path)?));
        }

        let mut reference_facts = self.references(&key_sites, references)?;
        let mut aliases = Vec::<(usize, Site<'_>)>::new();
        for (index, fact) in reference_facts.iter().enumerate() {
            let SemanticFact::Reference { occurrences, .. } = fact else {
                continue;
            };
            for occurrence in occurrences {
                if let ReportedLocation::Selected(span) = occurrence
                    && self
                        .lookup
                        .is_import_position(self.language, &span.path, span.range.start)
                    && let Some(site) = self.alias_site(span)?
                {
                    aliases.push((index, site));
                }
            }
        }
        if !aliases.is_empty() {
            let alias_sites = aliases
                .iter()
                .map(|(_, site)| site.clone())
                .collect::<Vec<_>>();
            session.open(self, alias_sites.iter().map(|(path, _)| *path))?;
            let found = request_all::<References>(
                &mut session.client,
                symbol_requests(&alias_sites, reference_parameters),
            )?;
            let found = self.references(&alias_sites, found)?;
            for ((index, _), fact) in aliases.iter().zip(found) {
                if let (
                    SemanticFact::Reference { occurrences, .. },
                    Some(SemanticFact::Reference {
                        occurrences: all, ..
                    }),
                ) = (fact, reference_facts.get_mut(*index))
                {
                    all.extend(occurrences);
                }
            }
        }

        // A reference is in an import statement, or in the body of an owner.
        let mut owners = BTreeSet::new();
        let mut imported = Vec::new();
        let mut in_bodies = Vec::new();
        for ((key, _, _), fact) in sites.iter().zip(&reference_facts) {
            let SemanticFact::Reference { occurrences, .. } = fact else {
                continue;
            };
            for occurrence in occurrences {
                let ReportedLocation::Selected(span) = occurrence else {
                    continue;
                };
                let (path, start) = (span.path.as_str(), span.range.start);
                if self.lookup.is_import_position(self.language, path, start) {
                    imported.push(span);
                } else {
                    owners.extend(self.lookup.body_owner_at(self.language, path, start));
                    in_bodies.push((*key, span));
                }
            }
        }
        let owner_sites = owners
            .iter()
            .filter(|owner| !keys.contains(owner))
            .map(|owner| self.site(*owner))
            .filter_map(Result::transpose)
            .collect::<Result<Vec<_>, LspError>>()?;
        let mut type_sites = Vec::new();
        for &key in keys {
            let node = &self.graph.nodes[key.index()];
            let Some(scope) = node
                .scope()
                .or_else(|| node.declarations().first().copied())
            else {
                continue;
            };
            let (_, text) = self.documents.get(node.path())?;
            let range = protocol_range(text, node.path(), scope)?;
            type_sites.extend(
                symbols
                    .get(node.path())
                    .into_iter()
                    .flatten()
                    .filter(|site| range_holds(range, site.selection.start))
                    .map(|site| (node.path(), site.clone())),
            );
        }
        let unnamed = keys
            .iter()
            .any(|key| self.graph.nodes[key.index()].declarations().is_empty());
        let import_sites = self
            .imports
            .iter()
            .map(|(_, site)| site)
            .filter(|site| {
                let statement = &site.statement;
                unnamed
                    || files.contains(statement.path.as_str())
                    || imported.iter().any(|span| {
                        span.path == statement.path
                            && statement.range.start <= span.range.start
                            && span.range.start < statement.range.end
                    })
            })
            .collect::<Vec<_>>();
        session.open(
            self,
            owner_sites
                .iter()
                .map(|(path, _)| *path)
                .chain(import_sites.iter().map(|site| site.statement.path.as_str())),
        )?;
        let (queries, definition_parameters) =
            definition_requests(&self.documents, import_sites.iter().copied())?;

        let key_items = items(&key_sites, &prepared);
        let mut batch = Batch::new(describe_request);
        let outgoing_count = batch
            .add::<CallHierarchyOutgoingCalls>(item_requests(&key_items, outgoing_parameters))?;
        let owner_count =
            batch.add::<CallHierarchyPrepare>(symbol_requests(&owner_sites, prepare_parameters))?;
        let type_count = batch
            .add::<GotoTypeDefinition>(symbol_requests(&type_sites, type_definition_parameters))?;
        let definition_count = batch.add::<GotoDefinition>(definition_parameters)?;
        let client = &mut session.client;
        let mut results = batch.send(client)?;
        let key_outgoing = results.decode::<CallHierarchyOutgoingCalls>(client, outgoing_count)?;
        let owner_prepared = results
            .decode::<CallHierarchyPrepare>(client, owner_count)?
            .into_iter()
            .map(Option::unwrap_or_default)
            .collect::<Vec<_>>();
        let type_targets = results.decode::<GotoTypeDefinition>(client, type_count)?;
        let definitions = results.decode::<GotoDefinition>(client, definition_count)?;

        let owner_items = items(&owner_sites, &owner_prepared);
        let owner_outgoing = request_all::<CallHierarchyOutgoingCalls>(
            client,
            item_requests(&owner_items, outgoing_parameters),
        )?;
        let outgoing = self.normalize(
            call_facts(&key_items, key_outgoing, outgoing_call_fact)
                .into_iter()
                .chain(call_facts(&owner_items, owner_outgoing, outgoing_call_fact))
                .collect(),
        )?;

        // A key with a use that no call site holds asks for its incoming calls.
        let call_sites = collect_call_sites(&outgoing);
        let unexplained = in_bodies
            .iter()
            .filter(|(_, span)| !is_call_site(&call_sites, span))
            .map(|(key, _)| *key)
            .collect::<HashSet<_>>();
        let fallback_items = sites
            .iter()
            .zip(&prepared)
            .filter(|((key, _, _), _)| unexplained.contains(key))
            .flat_map(|((_, _, (path, _)), items)| items.iter().map(move |item| (*path, item)))
            .collect::<Vec<_>>();
        let incoming = request_all::<CallHierarchyIncomingCalls>(
            client,
            item_requests(&fallback_items, incoming_parameters),
        )?;
        let mut incoming =
            self.normalize(call_facts(&fallback_items, incoming, incoming_call_fact))?;
        // An incoming call site inside an outgoing call site is the same call.
        for fact in &mut incoming {
            if let SemanticFact::Call {
                call_sites: each, ..
            } = fact
            {
                each.retain(|site| {
                    !matches!(site, ReportedLocation::Selected(span) if is_call_site(&call_sites, span))
                });
            }
        }

        let mut facts = outgoing;
        facts.extend(incoming);
        // The other references of a class do not touch its constructor.
        facts.extend(
            reference_facts
                .into_iter()
                .zip(&sites)
                .filter(|(_, (key, node, _))| key == node)
                .map(|(fact, _)| fact),
        );
        facts.extend(
            self.normalize(
                type_definition_facts(&type_sites, type_targets)
                    .into_iter()
                    .chain(definition_facts(queries, definitions))
                    .collect(),
            )?,
        );
        let edges = build_semantic_edges(self.graph, self.language, &facts, self.imports)
            .map_err(|error| LspError::new(error.message))?;
        Ok(edges
            .into_iter()
            .filter(|edge| {
                keys.contains(&edge.source)
                    || keys.contains(&edge.target)
                    || edge.kind == RelationKind::Imports
            })
            .collect())
    }

    /// Returns the normalized reference facts of some sites.
    ///
    /// # Errors
    ///
    /// Returns an error if a location is invalid.
    fn references(
        &self,
        sites: &[Site<'_>],
        responses: Vec<Option<Vec<lsp_types::Location>>>,
    ) -> Result<Vec<SemanticFact>, LspError> {
        self.normalize(
            sites
                .iter()
                .zip(responses)
                .map(|((_, site), occurrences)| CapturedSemanticFact::Reference {
                    target: site.clone(),
                    occurrences: occurrences.unwrap_or_default(),
                })
                .collect(),
        )
    }

    /// Returns the site of the alias that an import statement binds to the name at one reference.
    ///
    /// # Errors
    ///
    /// Returns an error if the import statement is outside its source.
    fn alias_site(&self, reference: &SourceSpan) -> Result<Option<Site<'a>>, LspError> {
        let Some(statement) =
            self.imports
                .iter()
                .map(|(_, site)| &site.statement)
                .find(|statement| {
                    statement.path == reference.path
                        && statement.range.start <= reference.range.start
                        && reference.range.end <= statement.range.end
                })
        else {
            return Ok(None);
        };
        let path = statement.path.as_str();
        let (uri, text) = self.documents.get(path)?;
        let Some(alias) = alias_after(text, reference.range.end, statement.range.end) else {
            return Ok(None);
        };
        Ok(Some((
            path,
            ProtocolSymbolSite {
                uri: uri.clone(),
                selection: protocol_range(text, path, alias)?,
                kind: SymbolKind::VARIABLE,
                name: text
                    .get(alias.start as usize..alias.end as usize)
                    .unwrap_or("an alias")
                    .to_owned(),
            },
        )))
    }

    /// Returns the protocol site at the first declaration of one node, if it has one.
    ///
    /// # Errors
    ///
    /// Returns an error if the declaration is outside its source.
    fn site(&self, id: NodeId) -> Result<Option<Site<'_>>, LspError> {
        let node = &self.graph.nodes[id.index()];
        let Some(declaration) = node.declarations().first() else {
            return Ok(None);
        };
        let path = node.path();
        let (uri, text) = self.documents.get(path)?;
        Ok(Some((
            path,
            ProtocolSymbolSite {
                uri: uri.clone(),
                selection: protocol_range(text, path, *declaration)?,
                // A kind selects a fallback only when no declaration matches,
                // and a key always matches its own declaration.
                kind: if matches!(node.kind, EntityKind::Type(_)) {
                    SymbolKind::CLASS
                } else {
                    SymbolKind::FUNCTION
                },
                name: node.name.clone(),
            },
        )))
    }

    /// Validates and normalizes captured facts.
    ///
    /// # Errors
    ///
    /// Returns an error if a fact is invalid or names a position outside its source.
    fn normalize(&self, facts: Vec<CapturedSemanticFact>) -> Result<Vec<SemanticFact>, LspError> {
        let capture = SemanticCapture {
            documents: Vec::new(),
            facts,
        };
        validate_capture(&capture)?;
        normalize_capture(self.root, self.sources, capture, self.fallback)
    }
}

/// Returns the alias in `name as alias` when the name ends at `name_end`, before `limit`.
fn alias_after(text: &str, name_end: u32, limit: u32) -> Option<SourceRange> {
    let rest = text.get(name_end as usize..limit as usize)?;
    let keyword = rest.trim_start();
    let after = keyword.strip_prefix("as")?;
    let alias = after.trim_start();
    let length = alias
        .find(|character: char| !(character.is_alphanumeric() || matches!(character, '_' | '$')))
        .unwrap_or(alias.len());
    if keyword.len() == rest.len() || alias.len() == after.len() || length == 0 {
        return None;
    }
    let start = name_end + u32::try_from(rest.len() - alias.len()).ok()?;
    Some(SourceRange {
        start,
        end: start + u32::try_from(length).ok()?,
    })
}

/// Returns each call hierarchy item of some sites with its source path.
fn items<'a>(
    sites: &'a [Site<'a>],
    prepared: &'a [Vec<CallHierarchyItem>],
) -> Vec<(&'a str, &'a CallHierarchyItem)> {
    sites
        .iter()
        .zip(prepared)
        .flat_map(|((path, _), items)| items.iter().map(move |item| (*path, item)))
        .collect()
}

// -----------------------------------------------------------------------------
// Tests
// -----------------------------------------------------------------------------

#[cfg(all(test, feature = "python"))]
mod tests {
    use std::{fs, path::Path};

    use tempfile::TempDir;

    use super::Resolver;
    use crate::analysis::{
        language::{Language, ServerCommand, builtin_profiles},
        lsp::fake_server::FakeServer,
    };

    const RUN_SOURCE: (&str, &str) = ("lib.py", "def run():\n    pass\n");

    #[test]
    fn keeps_the_server_after_an_answer_that_is_not_valid() {
        let server = FakeServer::new();
        // Flat document symbols are a valid protocol answer that the resolver refuses.
        let script = format!(
            r#"textDocument/documentSymbol * [{{"name":"run","kind":12,"location":{}}}]"#,
            location("lib.py", 0, 4, 7)
        );
        let project = project(&[RUN_SOURCE]);
        let mut resolver = resolver(project.path(), &server.scripted(&script));

        for _ in 0..2 {
            assert!(
                resolver
                    .resolve(&["lib.py::callable:run".to_owned()])
                    .is_err(),
                "the resolver refuses flat symbols"
            );
        }
        resolver.shutdown().expect("the server should stop");

        let starts = server.requests();
        assert_eq!(
            starts
                .iter()
                .filter(|method| *method == "initialize")
                .count(),
            1
        );
    }

    #[test]
    fn counts_an_incoming_call_inside_an_outgoing_call_site_once() {
        let server = FakeServer::new();
        let helper = item("helper", "lib.py", 0, 4, 10);
        let run = item("run", "lib.py", 2, 4, 7);
        let script = [
            format!(
                "textDocument/prepareCallHierarchy {} [{helper}]",
                at("lib.py", 0, 4)
            ),
            format!(
                "textDocument/references {} [{},{}]",
                at("lib.py", 0, 4),
                location("lib.py", 3, 8, 14),
                location("lib.py", 4, 10, 16)
            ),
            format!(
                "textDocument/prepareCallHierarchy {} [{run}]",
                at("lib.py", 2, 4)
            ),
            // The outgoing call site starts at the path; the incoming one at the name.
            calls("outgoingCalls", "run", "to", &helper, &range(3, 4, 14)),
            calls("incomingCalls", "helper", "from", &run, &range(3, 8, 14)),
        ];
        let files = [(
            "lib.py",
            "def helper():\n    pass\ndef run():\n    lib.helper()\nhandler = helper\n",
        )];

        let edges = resolve(
            &server,
            &script.join("\n"),
            &files,
            "lib.py::callable:helper",
        );

        assert_eq!(
            edges,
            [
                "lib.py -> lib.py::callable:helper UsesValue 1",
                "lib.py::callable:run -> lib.py::callable:helper Calls 1",
            ]
        );
        assert_eq!(incoming_requests(&server), 1);
    }

    #[test]
    fn finds_the_imports_of_a_module_key_in_every_import_statement() {
        let server = FakeServer::new();
        let script = [
            format!(
                "textDocument/definition {} {}",
                at("app.py", 0, 16),
                location("lib.py", 0, 4, 7)
            ),
            format!(
                "textDocument/definition {} {}",
                at("app.py", 1, 7),
                location("lib.py", 0, 0, 0)
            ),
            format!(
                r#"textDocument/definition {} {{"uri":"file:///outside/os.py","range":{}}}"#,
                at("app.py", 2, 7),
                range(0, 0, 0)
            ),
        ];
        let files = [
            RUN_SOURCE,
            ("app.py", "from lib import run\nimport lib\nimport os\n"),
        ];

        let edges = resolve(&server, &script.join("\n"), &files, "lib.py");

        assert_eq!(
            edges,
            [
                "app.py -> lib.py Imports 1",
                "app.py -> lib.py::callable:run Imports 1",
            ]
        );
    }

    #[test]
    fn reports_a_key_that_names_no_node() {
        let project = project(&[RUN_SOURCE]);
        let mut resolver = resolver(project.path(), &FakeServer::new().command("serve"));

        let Err(error) = resolver.resolve(&["lib.py::callable:none".to_owned()]) else {
            panic!("the key names no node");
        };

        assert_eq!(
            error.to_string(),
            "the graph has no node with the key lib.py::callable:none"
        );
    }

    #[test]
    fn reports_a_missing_server_program() {
        let project = project(&[RUN_SOURCE]);
        let mut server = FakeServer::new().command("serve");
        server.program = "tekdek-missing-language-server".to_owned();
        let mut resolver = resolver(project.path(), &server);

        let Err(error) = resolver.resolve(&["lib.py::callable:run".to_owned()]) else {
            panic!("the server program is missing");
        };

        assert!(
            error
                .to_string()
                .contains("python needs tekdek-missing-language-server"),
            "{error}"
        );
    }

    #[test]
    fn stops_the_server_of_a_failed_request_and_keeps_its_standard_error() {
        let server = FakeServer::new();
        let project = project(&[RUN_SOURCE]);
        let mut resolver = resolver(project.path(), &server.command("semantic-reject-stderr"));

        let Err(error) = resolver.resolve(&["lib.py::callable:run".to_owned()]) else {
            panic!("the server rejects the document symbols");
        };
        resolver.shutdown().expect("no server runs");

        let message = error.to_string();
        assert!(
            message.contains("document symbol capture failed"),
            "{message}"
        );
        assert_eq!(
            message.matches("semantic request diagnostic").count(),
            1,
            "{message}"
        );
        assert!(
            !server.shutdown_marker().exists(),
            "the failed server stops without a shutdown request"
        );
    }

    #[test]
    fn finds_callees_callers_uses_and_imports_and_asks_incoming_calls_once() {
        let server = FakeServer::new();
        let run = item("run", "lib.py", 0, 4, 7);
        let helper = item("helper", "lib.py", 2, 4, 10);
        let main = item("main", "app.py", 1, 4, 8);
        let start = item("start", "alias.py", 1, 4, 9);
        let module = r#"{"name":"(module)","kind":2,"uri":"{root}/app.py","range":RANGE,"selectionRange":RANGE}"#
            .replace("RANGE", &range(0, 0, 0));
        let script = [
            format!(
                "textDocument/prepareCallHierarchy {} [{run}]",
                at("lib.py", 0, 4)
            ),
            format!(
                "textDocument/references {} [{},{},{},{},{}]",
                at("lib.py", 0, 4),
                location("app.py", 0, 16, 19),
                location("app.py", 2, 4, 7),
                location("app.py", 3, 14, 17),
                location("app.py", 4, 0, 3),
                location("alias.py", 0, 16, 19)
            ),
            // The server finds the uses of an alias only at the alias.
            format!(
                "textDocument/references {} [{}]",
                at("alias.py", 0, 23),
                location("alias.py", 2, 4, 6)
            ),
            format!(
                "textDocument/prepareCallHierarchy {} [{main}]",
                at("app.py", 1, 4)
            ),
            format!(
                "textDocument/prepareCallHierarchy {} [{start}]",
                at("alias.py", 1, 4)
            ),
            calls("outgoingCalls", "run", "to", &helper, &range(1, 4, 10)),
            calls("outgoingCalls", "main", "to", &run, &range(2, 4, 7)),
            calls("outgoingCalls", "start", "to", &run, &range(2, 4, 6)),
            calls("incomingCalls", "run", "from", &module, &range(4, 0, 3)),
            format!(
                "textDocument/definition {} {}",
                at("app.py", 0, 16),
                location("lib.py", 0, 4, 7)
            ),
        ];
        let files = [
            (
                "lib.py",
                "def run():\n    helper()\ndef helper():\n    pass\n",
            ),
            (
                "app.py",
                "from lib import run\ndef main():\n    run()\n    handler = run\nrun()\n",
            ),
            (
                "alias.py",
                "from lib import run as go\ndef start():\n    go()\n",
            ),
        ];

        let edges = resolve(&server, &script.join("\n"), &files, "lib.py::callable:run");

        assert_eq!(
            edges,
            [
                "alias.py::callable:start -> lib.py::callable:run Calls 1",
                "app.py -> lib.py::callable:run Calls 1",
                "app.py -> lib.py::callable:run Imports 1",
                "app.py::callable:main -> lib.py::callable:run Calls 1",
                "app.py::callable:main -> lib.py::callable:run UsesValue 1",
                "lib.py::callable:run -> lib.py::callable:helper Calls 1",
            ]
        );
        assert_eq!(incoming_requests(&server), 1);
    }

    #[test]
    fn explains_a_reference_inside_an_owner_call_site_and_finds_local_types() {
        let server = FakeServer::new();
        let helper = item("helper", "lib.py", 2, 4, 10);
        let run = item("run", "lib.py", 4, 4, 7);
        let symbols = format!(
            r#"[{{"name":"helper","kind":12,"range":{},"selectionRange":{},"children":[{{"name":"item","kind":13,"range":{},"selectionRange":{}}}]}}]"#,
            range(2, 0, 17),
            range(2, 4, 10),
            range(3, 4, 8),
            range(3, 4, 8)
        );
        let script = [
            format!(
                "textDocument/prepareCallHierarchy {} [{helper}]",
                at("lib.py", 2, 4)
            ),
            format!(
                "textDocument/references {} [{}]",
                at("lib.py", 2, 4),
                location("lib.py", 5, 8, 14)
            ),
            format!("textDocument/documentSymbol /lib.py\"}}}} {symbols}"),
            format!(
                "textDocument/prepareCallHierarchy {} [{run}]",
                at("lib.py", 4, 4)
            ),
            // A qualified call site starts before the referenced name.
            calls("outgoingCalls", "run", "to", &helper, &range(5, 4, 14)),
            format!(
                "textDocument/typeDefinition {} {}",
                at("lib.py", 3, 4),
                location("lib.py", 0, 6, 10)
            ),
        ];
        let files = [(
            "lib.py",
            "class Item:\n    pass\ndef helper():\n    item = Item()\ndef run():\n    lib.helper()\n",
        )];

        let edges = resolve(
            &server,
            &script.join("\n"),
            &files,
            "lib.py::callable:helper",
        );

        assert_eq!(
            edges,
            [
                "lib.py::callable:helper -> lib.py::type:Item UsesType 1",
                "lib.py::callable:run -> lib.py::callable:helper Calls 1",
            ]
        );
        assert_eq!(incoming_requests(&server), 0);
    }

    #[test]
    fn finds_the_callers_of_a_last_constructor_through_its_class() {
        let server = FakeServer::new();
        let class = item("Item", "lib.py", 0, 6, 10).replace(r#""kind":12"#, r#""kind":5"#);
        let make = item("make", "lib.py", 3, 4, 8);
        let script = [
            format!(
                "textDocument/prepareCallHierarchy {} [{class}]",
                at("lib.py", 0, 6)
            ),
            format!(
                "textDocument/references {} [{}]",
                at("lib.py", 0, 6),
                location("lib.py", 4, 11, 15)
            ),
            format!(
                "textDocument/prepareCallHierarchy {} [{make}]",
                at("lib.py", 3, 4)
            ),
            calls("outgoingCalls", "make", "to", &class, &range(4, 11, 15)),
        ];
        let files = [(
            "lib.py",
            "class Item:\n    def __init__(self):\n        pass\ndef make():\n    return Item()\n",
        )];

        let edges = resolve(
            &server,
            &script.join("\n"),
            &files,
            "lib.py::type:Item::callable:__init__",
        );

        assert_eq!(
            edges,
            ["lib.py::callable:make -> lib.py::type:Item::callable:__init__ Calls 1"]
        );
        assert_eq!(incoming_requests(&server), 0);
    }

    /// Resolves one key of a Python project with a scripted server, and returns each edge as sorted text.
    fn resolve(
        server: &FakeServer,
        script: &str,
        files: &[(&str, &str)],
        key: &str,
    ) -> Vec<String> {
        let project = project(files);
        let mut resolver = resolver(project.path(), &server.scripted(script));
        let edges = resolver
            .resolve(&[key.to_owned()])
            .expect("the key should resolve");
        let nodes = &resolver.graph().nodes;
        let mut edges = edges
            .iter()
            .map(|edge| {
                format!(
                    "{} -> {} {:?} {}",
                    nodes[edge.source.index()].key.as_str(),
                    nodes[edge.target.index()].key.as_str(),
                    edge.kind,
                    edge.weight
                )
            })
            .collect::<Vec<_>>();
        edges.sort();
        resolver.shutdown().expect("the server should stop");
        edges
    }

    /// Writes the files of a project into a new folder.
    fn project(files: &[(&str, &str)]) -> TempDir {
        let project = tempfile::tempdir().expect("the project folder should be created");
        for (path, text) in files {
            fs::write(project.path().join(path), text).expect("the source should be written");
        }
        project
    }

    /// Parses a project with `server` as its Python server.
    fn resolver(project: &Path, server: &ServerCommand) -> Resolver {
        let mut profiles = builtin_profiles();
        for profile in &mut profiles {
            if profile.language == Language::Python {
                profile.server = server.clone();
            }
        }
        Resolver::new(project, profiles).expect("the project should parse")
    }

    fn incoming_requests(server: &FakeServer) -> usize {
        server
            .requests()
            .iter()
            .filter(|method| *method == "callHierarchy/incomingCalls")
            .count()
    }

    /// Returns the needle of a request at one position of one file.
    fn at(path: &str, line: u32, character: u32) -> String {
        format!(r#""position":{{"character":{character},"line":{line}}}&/{path}"}}"#)
    }

    fn range(line: u32, start: u32, end: u32) -> String {
        format!(
            r#"{{"start":{{"line":{line},"character":{start}}},"end":{{"line":{line},"character":{end}}}}}"#
        )
    }

    fn location(path: &str, line: u32, start: u32, end: u32) -> String {
        format!(
            r#"{{"uri":"{{root}}/{path}","range":{}}}"#,
            range(line, start, end)
        )
    }

    fn item(name: &str, path: &str, line: u32, start: u32, end: u32) -> String {
        let range = range(line, start, end);
        format!(
            r#"{{"name":"{name}","kind":12,"uri":"{{root}}/{path}","range":{range},"selectionRange":{range}}}"#
        )
    }

    /// Returns the script line of the outgoing or incoming calls of one item.
    fn calls(method: &str, name: &str, direction: &str, other: &str, site: &str) -> String {
        format!(
            r#"callHierarchy/{method} "name":"{name}" [{{"{direction}":{other},"fromRanges":[{site}]}}]"#
        )
    }
}
