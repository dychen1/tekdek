//! Rust syntax facts.
//!
//! This module selects Rust documents and extracts native syntax facts.
//! It reports modules, types, fields, callables, containment, local imports,
//! and declared metadata.
//!
//! `paths` maps files to crate roots and module paths. `signature` reads
//! declared callable metadata and documentation.
//!
//! # Sections
//!
//! `Entry Points` selects and parses source documents.
//! `Source Model` owns parsed syntax and extraction indexes.
//! `Fact Passes` registers entities, then adds relations.
//! `Entity Evidence` converts parser locations to byte ranges.

mod paths;
mod signature;

use std::collections::HashMap;

use paths::{ModulePath, crate_roots, module_path, relative_module_path};
use signature::{
    callable_parameters, callable_return_type, callable_type_inputs, callable_type_outputs,
    documentation,
};

use proc_macro2::{LineColumn, Span};
use syn::{
    Attribute, Block, Fields, FnArg, ImplItem, Item, ItemImpl, ItemMod, ItemTrait, PathArguments,
    Signature, TraitItem, Type, TypePath, UseTree, Visibility, spanned::Spanned,
};

use crate::analysis::{
    fact::{
        AnalysisError, CallableData, CallableKind, EntityEvidence, EntityKey, EntityKind,
        FieldData, RelationKind, SyntaxFactBuilder, SyntaxFacts, TypeData, TypeKind,
    },
    language::document_path,
    source::{DocumentSnapshot, SourceRange, SourceSpan, Sources, document_has_extension},
};

// -----------------------------------------------------------------------------
// Entry Points
// -----------------------------------------------------------------------------

/// Selects source documents that match the Rust profile.
///
/// # Errors
///
/// Returns an error if a selected document path is invalid.
pub(super) fn select_sources<'a>(
    documents: &'a [DocumentSnapshot],
    extensions: &[String],
) -> Result<Sources<'a>, AnalysisError> {
    let selected = documents
        .iter()
        .filter(|document| document_has_extension(document, extensions))
        .collect::<Vec<_>>();

    for document in &selected {
        document_path(document)?;
    }

    Ok(Sources::new(selected))
}

/// Produces syntax facts from selected Rust sources.
///
/// # Errors
///
/// Returns an error if parsing, identity, or source range extraction fails.
pub(super) fn produce_syntax_facts(sources: &Sources<'_>) -> Result<SyntaxFacts, AnalysisError> {
    produce_document_facts(&parse_documents(sources)?)
}

fn parse_documents<'a>(sources: &Sources<'a>) -> Result<Vec<ParsedDocument<'a>>, AnalysisError> {
    let mut documents = sources
        .iter()
        .map(parse_document)
        .collect::<Result<Vec<_>, _>>()?;
    let crate_roots = crate_roots(&documents);

    for document in &mut documents {
        document.module_path = module_path(&document.path, &crate_roots);
    }

    Ok(documents)
}

fn parse_document(document: &DocumentSnapshot) -> Result<ParsedDocument<'_>, AnalysisError> {
    let path = document_path(document)?.to_owned();
    let syntax = syn::parse_file(&document.content)
        .map_err(|error| AnalysisError::new(format!("cannot parse Rust source {path}: {error}")))?;
    let lines = LineIndex::new(&document.content);

    Ok(ParsedDocument {
        path,
        source: &document.content,
        syntax,
        lines,
        module_path: ModulePath::default(),
    })
}

// -----------------------------------------------------------------------------
// Source Model
// -----------------------------------------------------------------------------

#[derive(Eq, Hash, PartialEq)]
struct DeclarationId {
    path: String,
    start: u32,
}

enum Lookup<T> {
    Absent,
    Unique(T),
    Ambiguous,
}

struct ParsedDocument<'a> {
    path: String,
    source: &'a str,
    syntax: syn::File,
    lines: LineIndex,
    module_path: ModulePath,
}

struct LineIndex {
    starts: Vec<usize>,
    source_len: usize,
}

impl LineIndex {
    fn new(source: &str) -> Self {
        // The parser removes a byte order mark before it counts line-1 columns.
        let mut starts = vec![if source.starts_with('\u{feff}') {
            '\u{feff}'.len_utf8()
        } else {
            0
        }];
        starts.extend(
            source
                .bytes()
                .enumerate()
                .filter_map(|(offset, byte)| (byte == b'\n').then_some(offset + 1)),
        );
        Self {
            starts,
            source_len: source.len(),
        }
    }

    /// Converts a parser location to one UTF-8 byte offset.
    fn byte_offset(&self, source: &str, location: LineColumn) -> Result<u32, AnalysisError> {
        let line_start = location
            .line
            .checked_sub(1)
            .and_then(|line| self.starts.get(line))
            .copied()
            .ok_or_else(invalid_source_range)?;
        let line_end = source[line_start..]
            .find('\n')
            .map_or(self.source_len, |relative| line_start + relative);
        let line = source
            .get(line_start..line_end)
            .ok_or_else(invalid_source_range)?;
        let relative = if location.column == line.chars().count() {
            line.len()
        } else {
            line.char_indices()
                .nth(location.column)
                .map(|(offset, _)| offset)
                .ok_or_else(invalid_source_range)?
        };
        let offset = line_start
            .checked_add(relative)
            .ok_or_else(invalid_source_range)?;
        u32::try_from(offset).map_err(|_| invalid_source_range())
    }
}

struct RustSyntaxWalker {
    builder: SyntaxFactBuilder,
    modules: HashMap<ModulePath, Vec<EntityKey>>,
    types_by_owner: HashMap<EntityKey, HashMap<String, Vec<EntityKey>>>,
    declared_modules: HashMap<DeclarationId, EntityKey>,
    declared_types: HashMap<DeclarationId, EntityKey>,
}

impl RustSyntaxWalker {
    fn new() -> Self {
        Self {
            builder: SyntaxFactBuilder::new(),
            modules: HashMap::new(),
            types_by_owner: HashMap::new(),
            declared_modules: HashMap::new(),
            declared_types: HashMap::new(),
        }
    }
}

// -----------------------------------------------------------------------------
// Fact Passes
// -----------------------------------------------------------------------------

/// Produces facts with three ordered syntax passes.
///
/// Pass one registers modules and types. Pass two registers fields and
/// callables. Both passes add the containment relation of each new entity.
/// Pass three adds local import relations.
fn produce_document_facts(documents: &[ParsedDocument<'_>]) -> Result<SyntaxFacts, AnalysisError> {
    let mut walker = RustSyntaxWalker::new();

    for document in documents {
        walker.register_file_module(document)?;
    }
    for document in documents {
        let module = EntityKey::module(&document.path);
        walker.register_types(
            document,
            &document.syntax.items,
            &document.module_path,
            &module,
        )?;
    }
    for document in documents {
        let module = EntityKey::module(&document.path);
        walker.register_members(
            document,
            &document.syntax.items,
            &document.module_path,
            &module,
            &module,
        )?;
    }
    for document in documents {
        let module = EntityKey::module(&document.path);
        walker.add_import_relations(
            document,
            &document.syntax.items,
            &document.module_path,
            &module,
        )?;
    }

    Ok(walker.builder.finish())
}

impl RustSyntaxWalker {
    /// Registers one file module before item traversal starts.
    fn register_file_module(&mut self, document: &ParsedDocument<'_>) -> Result<(), AnalysisError> {
        let module = self.builder.add_module(&document.path, document.source)?;
        self.modules
            .entry(document.module_path.clone())
            .or_default()
            .push(module);
        Ok(())
    }

    /// Registers inline modules, structs, enums, and traits.
    fn register_types(
        &mut self,
        document: &ParsedDocument<'_>,
        items: &[Item],
        module_path: &ModulePath,
        module: &EntityKey,
    ) -> Result<(), AnalysisError> {
        for item in items {
            match item {
                // Register and enter one inline module.
                Item::Mod(item) if item.content.is_some() => {
                    self.register_inline_module(document, item, module_path, module)?;
                }
                // Register one structure.
                Item::Struct(item) => {
                    self.register_type(
                        document,
                        module,
                        &item.ident,
                        TypeKind::Struct,
                        &item.attrs,
                        item_start(&item.vis, item.struct_token.span),
                        item.span(),
                        fields_body_span(&item.fields),
                    )?;
                }
                // Register one enumeration.
                Item::Enum(item) => {
                    self.register_type(
                        document,
                        module,
                        &item.ident,
                        TypeKind::Enum,
                        &item.attrs,
                        item_start(&item.vis, item.enum_token.span),
                        item.span(),
                        Some((item.brace_token.span.open(), item.brace_token.span.close())),
                    )?;
                }
                // Register one trait.
                Item::Trait(item) => {
                    self.register_type(
                        document,
                        module,
                        &item.ident,
                        TypeKind::Trait,
                        &item.attrs,
                        item_start(&item.vis, item.trait_token.span),
                        item.span(),
                        Some((item.brace_token.span.open(), item.brace_token.span.close())),
                    )?;
                }
                // Ignore syntax without a graph entity.
                _ => {}
            }
        }
        Ok(())
    }

    fn register_inline_module(
        &mut self,
        document: &ParsedDocument<'_>,
        item: &ItemMod,
        module_path: &ModulePath,
        owner: &EntityKey,
    ) -> Result<(), AnalysisError> {
        let Some((brace, items)) = &item.content else {
            return Ok(());
        };
        let evidence = entity_evidence(
            document,
            item.ident.span(),
            item_start(&item.vis, item.mod_token.span),
            item.span(),
            Some((brace.span.open(), brace.span.close())),
        )?;
        let declaration_start = evidence.declarations[0].start;
        let module =
            self.builder
                .add_entity(owner, item.ident.to_string(), EntityKind::Module, evidence);
        self.builder
            .add_relation(owner, &module, RelationKind::Contains);

        let mut nested_path = module_path.clone();
        nested_path.segments.push(item.ident.to_string());
        self.modules
            .entry(nested_path.clone())
            .or_default()
            .push(module.clone());
        self.declared_modules.insert(
            DeclarationId {
                path: document.path.clone(),
                start: declaration_start,
            },
            module.clone(),
        );
        self.register_types(document, items, &nested_path, &module)
    }

    #[allow(
        clippy::too_many_arguments,
        reason = "type registration needs its syntax evidence and graph owner"
    )]
    fn register_type(
        &mut self,
        document: &ParsedDocument<'_>,
        owner: &EntityKey,
        ident: &syn::Ident,
        kind: TypeKind,
        attrs: &[Attribute],
        start: Span,
        end: Span,
        body: Option<(Span, Span)>,
    ) -> Result<(), AnalysisError> {
        let evidence = entity_evidence(document, ident.span(), start, end, body)?;
        let declaration_start = evidence.declarations[0].start;
        let name = ident.to_string();
        let entity = self.builder.add_entity(
            owner,
            name.clone(),
            EntityKind::Type(TypeData {
                kind,
                documentation: documentation(attrs),
            }),
            evidence,
        );
        self.builder
            .add_relation(owner, &entity, RelationKind::Contains);
        self.types_by_owner
            .entry(owner.clone())
            .or_default()
            .entry(name)
            .or_default()
            .push(entity.clone());
        self.declared_types.insert(
            DeclarationId {
                path: document.path.clone(),
                start: declaration_start,
            },
            entity,
        );
        Ok(())
    }

    /// Registers fields, free functions, trait items, and implementation items.
    fn register_members(
        &mut self,
        document: &ParsedDocument<'_>,
        items: &[Item],
        module_path: &ModulePath,
        module: &EntityKey,
        file_module: &EntityKey,
    ) -> Result<(), AnalysisError> {
        for item in items {
            match item {
                // Enter one registered inline module.
                Item::Mod(item) => {
                    if let Some((_, nested_items)) = &item.content {
                        let mut nested_path = module_path.clone();
                        nested_path.segments.push(item.ident.to_string());
                        let nested = self.declared_module(document, &item.ident)?;
                        self.register_members(
                            document,
                            nested_items,
                            &nested_path,
                            &nested,
                            file_module,
                        )?;
                    }
                }
                // Register structure fields.
                Item::Struct(item) => {
                    let owner = self.declared_type(document, &item.ident)?;
                    self.register_fields(document, &item.fields, &owner, None)?;
                }
                // Register enumeration fields.
                Item::Enum(item) => {
                    let owner = self.declared_type(document, &item.ident)?;
                    for variant in &item.variants {
                        self.register_fields(
                            document,
                            &variant.fields,
                            &owner,
                            Some(&variant.ident.to_string()),
                        )?;
                    }
                }
                // Register one free function.
                Item::Fn(item) => {
                    self.register_callable(
                        document,
                        module,
                        Some(&item.vis),
                        &item.attrs,
                        &item.sig,
                        Some(&item.block),
                        item.span(),
                    )?;
                }
                // Register trait functions.
                Item::Trait(item) => {
                    self.register_trait_items(document, item)?;
                }
                // Register implementation functions.
                Item::Impl(item) => {
                    self.register_impl_items(document, item, module_path, module, file_module)?;
                }
                // Ignore syntax without member entities.
                _ => {}
            }
        }
        Ok(())
    }

    fn declared_module(
        &self,
        document: &ParsedDocument<'_>,
        ident: &syn::Ident,
    ) -> Result<EntityKey, AnalysisError> {
        let declaration = source_range(document, ident.span())?;
        self.declared_modules
            .get(&DeclarationId {
                path: document.path.clone(),
                start: declaration.start,
            })
            .cloned()
            .ok_or_else(|| {
                AnalysisError::new(format!("the {ident} Rust module was not registered"))
            })
    }

    fn declared_type(
        &self,
        document: &ParsedDocument<'_>,
        ident: &syn::Ident,
    ) -> Result<EntityKey, AnalysisError> {
        let declaration = source_range(document, ident.span())?;
        self.declared_types
            .get(&DeclarationId {
                path: document.path.clone(),
                start: declaration.start,
            })
            .cloned()
            .ok_or_else(|| AnalysisError::new(format!("the {ident} Rust type was not registered")))
    }

    fn register_fields(
        &mut self,
        document: &ParsedDocument<'_>,
        fields: &Fields,
        owner: &EntityKey,
        variant: Option<&str>,
    ) -> Result<(), AnalysisError> {
        for (index, field) in fields.iter().enumerate() {
            let (name, declaration_span) = if let Some(ident) = &field.ident {
                let name = variant
                    .map_or_else(|| ident.to_string(), |variant| format!("{variant}.{ident}"));
                (name, ident.span())
            } else {
                let name = variant
                    .map_or_else(|| index.to_string(), |variant| format!("{variant}.{index}"));
                (name, field.ty.span())
            };
            let evidence =
                entity_evidence(document, declaration_span, field.span(), field.span(), None)?;
            let entity = self.builder.add_entity(
                owner,
                name,
                EntityKind::Field(FieldData {
                    declared_type: Some(source_text(document, field.ty.span())?.trim().to_owned()),
                }),
                evidence,
            );
            self.builder
                .add_relation(owner, &entity, RelationKind::Contains);
        }
        Ok(())
    }

    fn register_trait_items(
        &mut self,
        document: &ParsedDocument<'_>,
        item: &ItemTrait,
    ) -> Result<(), AnalysisError> {
        let owner = self.declared_type(document, &item.ident)?;
        for trait_item in &item.items {
            if let TraitItem::Fn(function) = trait_item {
                self.register_callable(
                    document,
                    &owner,
                    None,
                    &function.attrs,
                    &function.sig,
                    function.default.as_ref(),
                    function.span(),
                )?;
            }
        }
        Ok(())
    }

    fn register_impl_items(
        &mut self,
        document: &ParsedDocument<'_>,
        item: &ItemImpl,
        module_path: &ModulePath,
        module: &EntityKey,
        file_module: &EntityKey,
    ) -> Result<(), AnalysisError> {
        let owner = simple_type_path(item.self_ty.as_ref())
            .as_deref()
            .and_then(|path| self.resolve_type(module_path, module, path))
            .unwrap_or_else(|| file_module.clone());

        for impl_item in &item.items {
            if let ImplItem::Fn(function) = impl_item {
                self.register_callable(
                    document,
                    &owner,
                    Some(&function.vis),
                    &function.attrs,
                    &function.sig,
                    Some(&function.block),
                    function.span(),
                )?;
            }
        }
        Ok(())
    }

    #[allow(
        clippy::too_many_arguments,
        reason = "callable registration needs syntax evidence, metadata, and graph ownership"
    )]
    fn register_callable(
        &mut self,
        document: &ParsedDocument<'_>,
        owner: &EntityKey,
        visibility: Option<&Visibility>,
        attrs: &[Attribute],
        signature: &Signature,
        body: Option<&Block>,
        end: Span,
    ) -> Result<(), AnalysisError> {
        let kind = if signature
            .inputs
            .iter()
            .any(|argument| matches!(argument, FnArg::Receiver(_)))
        {
            CallableKind::Method
        } else {
            CallableKind::Function
        };
        let start = visibility.map_or_else(
            || signature.span(),
            |visibility| item_start(visibility, signature.span()),
        );
        let signature_text = source_text_between(document, start, signature.span())?
            .trim()
            .to_owned();
        let mut evidence = entity_evidence(
            document,
            signature.ident.span(),
            start,
            end,
            body.map(|block| {
                (
                    block.brace_token.span.open(),
                    block.brace_token.span.close(),
                )
            }),
        )?;
        evidence.type_inputs = callable_type_inputs(document, signature)?;
        evidence.type_outputs = callable_type_outputs(document, signature)?;
        let entity = self.builder.add_entity(
            owner,
            signature.ident.to_string(),
            EntityKind::Callable(CallableData {
                kind,
                signature: Some(signature_text),
                parameters: callable_parameters(document, signature)?,
                return_type: callable_return_type(document, signature)?,
                documentation: documentation(attrs),
            }),
            evidence,
        );
        self.builder
            .add_relation(owner, &entity, RelationKind::Contains);
        Ok(())
    }

    fn resolve_type(
        &self,
        current_path: &ModulePath,
        current_module: &EntityKey,
        path: &[String],
    ) -> Option<EntityKey> {
        let (name, qualifiers) = path.split_last()?;
        let owner = match self.resolve_namespace(current_path, current_module, qualifiers) {
            Lookup::Unique(owner) => owner,
            Lookup::Absent | Lookup::Ambiguous => return None,
        };
        match self.lookup_type(&owner, name) {
            Lookup::Unique(entity) => Some(entity),
            Lookup::Absent | Lookup::Ambiguous => None,
        }
    }

    fn resolve_namespace(
        &self,
        current_path: &ModulePath,
        current_module: &EntityKey,
        path: &[String],
    ) -> Lookup<EntityKey> {
        if path.is_empty() {
            return Lookup::Unique(current_module.clone());
        }
        self.resolve_module(current_path, current_module, path)
    }

    fn resolve_module(
        &self,
        current_path: &ModulePath,
        current_module: &EntityKey,
        path: &[String],
    ) -> Lookup<EntityKey> {
        if path == ["self"] {
            return Lookup::Unique(current_module.clone());
        }
        let Some(path) = relative_module_path(current_path, path) else {
            return Lookup::Absent;
        };
        self.lookup_module(&path)
    }

    fn lookup_module(&self, path: &ModulePath) -> Lookup<EntityKey> {
        unique_entity(self.modules.get(path))
    }

    fn lookup_type(&self, owner: &EntityKey, name: &str) -> Lookup<EntityKey> {
        unique_entity(
            self.types_by_owner
                .get(owner)
                .and_then(|types| types.get(name)),
        )
    }

    fn add_import_relations(
        &mut self,
        document: &ParsedDocument<'_>,
        items: &[Item],
        module_path: &ModulePath,
        module: &EntityKey,
    ) -> Result<(), AnalysisError> {
        for item in items {
            match item {
                // Enter one inline module.
                Item::Mod(item) => {
                    if let Some((_, nested_items)) = &item.content {
                        let mut nested_path = module_path.clone();
                        nested_path.segments.push(item.ident.to_string());
                        let nested = self.declared_module(document, &item.ident)?;
                        self.add_import_relations(document, nested_items, &nested_path, &nested)?;
                    }
                }
                // Resolve direct local use paths.
                Item::Use(item) => {
                    let mut paths = Vec::new();
                    flatten_use_tree(&item.tree, &mut Vec::new(), &mut paths);
                    let observation = SourceSpan {
                        path: document.path.clone(),
                        range: source_range_between(
                            document,
                            item.use_token.span,
                            item.semi_token.span,
                        )?,
                    };
                    for path in paths {
                        if let Some(target) = self.resolve_import(module_path, module, &path) {
                            self.builder.add_observed_relation(
                                module,
                                &target,
                                RelationKind::Imports,
                                vec![observation.clone()],
                            );
                        }
                    }
                }
                // Ignore syntax without nested module imports.
                _ => {}
            }
        }
        Ok(())
    }

    fn resolve_import(
        &self,
        current_path: &ModulePath,
        current_module: &EntityKey,
        path: &[String],
    ) -> Option<EntityKey> {
        match self.resolve_module(current_path, current_module, path) {
            Lookup::Unique(module) => Some(module),
            Lookup::Ambiguous => None,
            Lookup::Absent => self.resolve_type(current_path, current_module, path),
        }
    }
}

fn item_start(visibility: &Visibility, fallback: Span) -> Span {
    match visibility {
        Visibility::Inherited => fallback,
        Visibility::Public(_) | Visibility::Restricted(_) => visibility.span(),
    }
}

fn fields_body_span(fields: &Fields) -> Option<(Span, Span)> {
    match fields {
        Fields::Named(fields) => Some((
            fields.brace_token.span.open(),
            fields.brace_token.span.close(),
        )),
        Fields::Unnamed(fields) => Some((
            fields.paren_token.span.open(),
            fields.paren_token.span.close(),
        )),
        Fields::Unit => None,
    }
}

fn simple_type_path(ty: &Type) -> Option<Vec<String>> {
    let Type::Path(TypePath {
        qself: None, path, ..
    }) = ty
    else {
        return None;
    };
    if path.leading_colon.is_some() || path.segments.is_empty() {
        return None;
    }
    let final_index = path.segments.len() - 1;
    if path.segments.iter().enumerate().any(|(index, segment)| {
        index != final_index && !matches!(segment.arguments, PathArguments::None)
    }) {
        return None;
    }
    Some(
        path.segments
            .iter()
            .map(|segment| segment.ident.to_string())
            .collect(),
    )
}

fn unique_entity(entities: Option<&Vec<EntityKey>>) -> Lookup<EntityKey> {
    match entities.map(Vec::as_slice) {
        None | Some([]) => Lookup::Absent,
        Some([entity]) => Lookup::Unique(entity.clone()),
        Some([_, _, ..]) => Lookup::Ambiguous,
    }
}

fn flatten_use_tree(tree: &UseTree, prefix: &mut Vec<String>, paths: &mut Vec<Vec<String>>) {
    match tree {
        // Add one path prefix.
        UseTree::Path(path) => {
            prefix.push(path.ident.to_string());
            flatten_use_tree(&path.tree, prefix, paths);
            prefix.pop();
        }
        // Add one imported name.
        UseTree::Name(name) => {
            let mut path = prefix.clone();
            path.push(name.ident.to_string());
            paths.push(path);
        }
        // Add one renamed import by its source name.
        UseTree::Rename(rename) => {
            let mut path = prefix.clone();
            path.push(rename.ident.to_string());
            paths.push(path);
        }
        // Expand every import in one group.
        UseTree::Group(group) => {
            for tree in &group.items {
                flatten_use_tree(tree, prefix, paths);
            }
        }
        // Ignore glob imports because they have no unique target.
        UseTree::Glob(_) => {}
    }
}

// -----------------------------------------------------------------------------
// Entity Evidence
// -----------------------------------------------------------------------------

fn entity_evidence(
    document: &ParsedDocument<'_>,
    declaration: Span,
    start: Span,
    end: Span,
    body: Option<(Span, Span)>,
) -> Result<EntityEvidence, AnalysisError> {
    Ok(EntityEvidence {
        path: document.path.clone(),
        declarations: vec![source_range(document, declaration)?],
        scope: Some(source_range_between(document, start, end)?),
        body: body
            .map(|(body_start, body_end)| source_range_between(document, body_start, body_end))
            .transpose()?,
        type_inputs: Vec::new(),
        type_outputs: Vec::new(),
    })
}

fn source_range(document: &ParsedDocument<'_>, span: Span) -> Result<SourceRange, AnalysisError> {
    source_range_between(document, span, span)
}

fn source_range_between(
    document: &ParsedDocument<'_>,
    start: Span,
    end: Span,
) -> Result<SourceRange, AnalysisError> {
    let start = document.lines.byte_offset(document.source, start.start())?;
    let end = document.lines.byte_offset(document.source, end.end())?;
    if start > end {
        return Err(invalid_source_range());
    }
    Ok(SourceRange { start, end })
}

fn source_text<'a>(document: &ParsedDocument<'a>, span: Span) -> Result<&'a str, AnalysisError> {
    source_text_between(document, span, span)
}

fn source_text_between<'a>(
    document: &ParsedDocument<'a>,
    start: Span,
    end: Span,
) -> Result<&'a str, AnalysisError> {
    let range = source_range_between(document, start, end)?;
    document
        .source
        .get(range.start as usize..range.end as usize)
        .ok_or_else(invalid_source_range)
}

fn invalid_source_range() -> AnalysisError {
    AnalysisError::new("a Rust source range is invalid")
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use crate::analysis::{
        fact::{
            CallableData, CallableKind, EntityFact, EntityKind, FieldData, RelationKind,
            SyntaxFacts, TypeData, TypeKind,
        },
        source::{DocumentSnapshot, SourceRange, Sources},
    };

    use super::{produce_syntax_facts, select_sources};

    const REPRESENTATIVE_SOURCE: &str = r"mod nested {
    /// One record.
    pub struct Record {
        pub name: String,
        count: usize,
    }

    pub trait Run {
        fn run(&self, value: usize) -> bool;
    }

    impl Record {
        pub fn new(name: String) -> Self {
            Self { name, count: 0 }
        }

        pub fn count(&self) -> usize {
            self.count
        }
    }
}
";

    #[test]
    #[allow(
        clippy::too_many_lines,
        reason = "one representative fixture verifies all required Rust entity metadata"
    )]
    fn extracts_representative_rust_entities_and_metadata() {
        let facts = facts_for("src/lib.rs", REPRESENTATIVE_SOURCE)
            .expect("the representative source must produce facts");

        assert_eq!(
            entities_of_kind(&facts, |kind| matches!(kind, EntityKind::Module)),
            2
        );

        let root = entity(&facts, "src/lib.rs", |kind| {
            matches!(kind, EntityKind::Module)
        });
        assert_eq!(
            range_text(
                REPRESENTATIVE_SOURCE,
                root.evidence.scope.expect("the file scope must exist")
            ),
            REPRESENTATIVE_SOURCE
        );

        let nested = entity(&facts, "nested", |kind| matches!(kind, EntityKind::Module));
        assert_eq!(declaration_text(REPRESENTATIVE_SOURCE, nested), "nested");
        assert_eq!(nested.key.as_str(), "src/lib.rs::module:nested");

        let record = typed_entity(&facts, "Record", &TypeKind::Struct);
        assert_eq!(declaration_text(REPRESENTATIVE_SOURCE, record), "Record");
        assert_eq!(
            type_data(record).documentation.as_deref(),
            Some("One record.")
        );
        assert_eq!(
            range_text(
                REPRESENTATIVE_SOURCE,
                record.evidence.scope.expect("the struct scope must exist")
            ),
            "pub struct Record {\n        pub name: String,\n        count: usize,\n    }"
        );

        let run = typed_entity(&facts, "Run", &TypeKind::Trait);
        assert_eq!(declaration_text(REPRESENTATIVE_SOURCE, run), "Run");

        let name = field_entity(&facts, "name");
        assert_eq!(field_data(name).declared_type.as_deref(), Some("String"));
        assert_eq!(declaration_text(REPRESENTATIVE_SOURCE, name), "name");

        let count_field = facts
            .entities
            .iter()
            .find(|entity| entity.name == "count" && matches!(entity.kind, EntityKind::Field(_)))
            .expect("the count field must exist");
        assert_eq!(
            field_data(count_field).declared_type.as_deref(),
            Some("usize")
        );
        assert_eq!(
            declaration_text(REPRESENTATIVE_SOURCE, count_field),
            "count"
        );

        let new = callable_entity(&facts, "new");
        let new_data = callable_data(new);
        assert!(matches!(new_data.kind, CallableKind::Function));
        assert_eq!(
            new_data.signature.as_deref(),
            Some("pub fn new(name: String) -> Self")
        );
        assert_eq!(new_data.parameters, ["name: String"]);
        assert_eq!(new_data.return_type.as_deref(), Some("Self"));
        assert_eq!(
            range_text(REPRESENTATIVE_SOURCE, new.evidence.type_inputs[0]),
            "String"
        );
        assert_eq!(
            range_text(REPRESENTATIVE_SOURCE, new.evidence.type_outputs[0]),
            "Self"
        );
        assert_eq!(declaration_text(REPRESENTATIVE_SOURCE, new), "new");
        assert_eq!(
            new.key.as_str(),
            "src/lib.rs::module:nested::type:Record::callable:new"
        );

        let count_method = callable_entity(&facts, "count");
        let count_data = callable_data(count_method);
        assert!(matches!(count_data.kind, CallableKind::Method));
        assert_eq!(
            count_data.signature.as_deref(),
            Some("pub fn count(&self) -> usize")
        );
        assert_eq!(count_data.parameters, Vec::<String>::new());
        assert_eq!(count_data.return_type.as_deref(), Some("usize"));
        assert_eq!(count_method.evidence.type_inputs, Vec::new());
        assert_eq!(
            range_text(REPRESENTATIVE_SOURCE, count_method.evidence.type_outputs[0]),
            "usize"
        );
        assert_eq!(
            range_text(
                REPRESENTATIVE_SOURCE,
                count_method
                    .evidence
                    .body
                    .expect("the count body must exist")
            ),
            "{\n            self.count\n        }"
        );

        let trait_run = callable_entity(&facts, "run");
        let trait_run_data = callable_data(trait_run);
        assert!(matches!(trait_run_data.kind, CallableKind::Method));
        assert_eq!(
            trait_run_data.signature.as_deref(),
            Some("fn run(&self, value: usize) -> bool")
        );
        assert_eq!(trait_run_data.parameters, ["value: usize"]);
        assert_eq!(trait_run_data.return_type.as_deref(), Some("bool"));
        assert_eq!(
            range_text(REPRESENTATIVE_SOURCE, trait_run.evidence.type_inputs[0]),
            "usize"
        );
        assert_eq!(
            range_text(REPRESENTATIVE_SOURCE, trait_run.evidence.type_outputs[0]),
            "bool"
        );
        assert!(trait_run.evidence.body.is_none());
        assert_eq!(
            trait_run.key.as_str(),
            "src/lib.rs::module:nested::type:Run::callable:run"
        );

        assert_contains(&facts, &nested.key, &record.key);
        assert_contains(&facts, &record.key, &new.key);
        assert_contains(&facts, &record.key, &count_method.key);
        assert_contains(&facts, &run.key, &trait_run.key);
    }

    #[test]
    fn extracts_enums_and_tuple_fields() {
        let source = "pub struct Pair(pub String, usize);\npub enum State { Ready, Done }\n";
        let facts = facts_for("models.rs", source).expect("the source must produce facts");

        let pair = typed_entity(&facts, "Pair", &TypeKind::Struct);
        let state = typed_entity(&facts, "State", &TypeKind::Enum);
        assert_eq!(declaration_text(source, state), "State");

        let fields = facts
            .entities
            .iter()
            .filter(|entity| matches!(entity.kind, EntityKind::Field(_)))
            .collect::<Vec<_>>();
        assert_eq!(fields.len(), 2);
        assert_eq!(fields[0].name, "0");
        assert_eq!(
            field_data(fields[0]).declared_type.as_deref(),
            Some("String")
        );
        assert_eq!(declaration_text(source, fields[0]), "String");
        assert_eq!(fields[1].name, "1");
        assert_eq!(
            field_data(fields[1]).declared_type.as_deref(),
            Some("usize")
        );
        assert_eq!(declaration_text(source, fields[1]), "usize");
        assert_contains(&facts, &pair.key, &fields[0].key);
        assert_contains(&facts, &pair.key, &fields[1].key);
    }

    #[test]
    fn converts_unicode_line_columns_to_byte_ranges() {
        let source = "// π🙂\npub struct Café;\n";
        let facts = facts_for("unicode.rs", source).expect("the source must produce facts");

        let declaration = typed_entity(&facts, "Café", &TypeKind::Struct);

        assert_eq!(declaration_text(source, declaration), "Café");
        assert_eq!(
            declaration.evidence.declarations[0],
            expected_range(source, "Café")
        );
    }

    #[test]
    fn keeps_line_one_ranges_exact_after_a_byte_order_mark() {
        let source = "\u{feff}pub struct Alpha;\n";
        let facts = facts_for("bom.rs", source).expect("the source must produce facts");

        let alpha = typed_entity(&facts, "Alpha", &TypeKind::Struct);

        assert_eq!(declaration_text(source, alpha), "Alpha");
        assert_eq!(
            range_text(source, alpha.evidence.scope.expect("the scope must exist")),
            "pub struct Alpha;"
        );
    }

    #[test]
    fn rejects_malformed_rust_source() {
        let Err(error) = facts_for("broken.rs", "pub struct Broken {") else {
            panic!("malformed Rust must fail");
        };

        assert!(error.message.contains("cannot parse Rust source broken.rs"));
    }

    #[test]
    fn attaches_unresolved_implementation_items_to_the_file_module() {
        let source = "impl External {\n    fn run(&self) {}\n}\n";
        let facts = facts_for("orphan.rs", source).expect("the source must produce facts");
        let root = entity(&facts, "orphan.rs", |kind| {
            matches!(kind, EntityKind::Module)
        });
        let run = callable_entity(&facts, "run");

        assert_eq!(run.key.as_str(), "orphan.rs::callable:run");
        assert_contains(&facts, &root.key, &run.key);
        assert!(
            facts
                .entities
                .iter()
                .all(|entity| entity.name != "External")
        );
    }

    #[test]
    fn attaches_generic_implementation_items_to_the_local_type() {
        let source =
            "struct Record<T>(T);\nimpl<T> Record<T> {\n    fn value(&self) -> &T { &self.0 }\n}\n";
        let facts = facts_for("generic.rs", source).expect("the source must produce facts");
        let record = typed_entity(&facts, "Record", &TypeKind::Struct);
        let value = callable_entity(&facts, "value");

        assert_eq!(
            value.key.as_str(),
            "generic.rs::type:Record::callable:value"
        );
        assert_contains(&facts, &record.key, &value.key);
    }

    #[test]
    fn registers_library_and_binary_roots_as_distinct_file_modules() {
        let documents = [
            document("src/lib.rs", "pub struct Library;\n"),
            document("src/main.rs", "pub struct Application;\n"),
        ];
        let facts = facts_for_documents(&documents).expect("both crate roots must produce facts");

        let modules = facts
            .entities
            .iter()
            .filter(|entity| matches!(entity.kind, EntityKind::Module))
            .collect::<Vec<_>>();

        assert_eq!(modules.len(), 2);
        assert_eq!(modules[0].key.as_str(), "src/lib.rs");
        assert_eq!(modules[1].key.as_str(), "src/main.rs");
        assert_eq!(
            typed_entity(&facts, "Library", &TypeKind::Struct)
                .key
                .as_str(),
            "src/lib.rs::type:Library"
        );
        assert_eq!(
            typed_entity(&facts, "Application", &TypeKind::Struct)
                .key
                .as_str(),
            "src/main.rs::type:Application"
        );
    }

    #[test]
    fn resolves_crate_paths_within_each_package() {
        let source = "mod shared { pub struct Item; }\nuse crate::shared::Item;\n";
        let documents = [
            document("src/lib.rs", source),
            document("crates/api/src/lib.rs", source),
        ];
        let facts = facts_for_documents(&documents).expect("both packages must produce facts");
        let root = entity_with_key(&facts, "src/lib.rs");
        let root_item = entity_with_key(&facts, "src/lib.rs::module:shared::type:Item");
        let api = entity_with_key(&facts, "crates/api/src/lib.rs");
        let api_item = entity_with_key(&facts, "crates/api/src/lib.rs::module:shared::type:Item");

        assert_imports(&facts, root, root_item);
        assert_imports(&facts, api, api_item);
        assert!(!has_relation(
            &facts,
            &root.key,
            &api_item.key,
            RelationKind::Imports
        ));
        assert!(!has_relation(
            &facts,
            &api.key,
            &root_item.key,
            RelationKind::Imports
        ));
    }

    #[test]
    fn resolves_library_and_binary_crate_roots_independently() {
        let documents = [
            document(
                "src/lib.rs",
                "pub struct Shared;\nimpl crate::Shared { fn library(&self) {} }\n",
            ),
            document(
                "src/main.rs",
                "pub struct Shared;\nimpl crate::Shared { fn application(&self) {} }\n",
            ),
        ];
        let facts = facts_for_documents(&documents).expect("both crate roots must produce facts");
        let library_type = entity_with_key(&facts, "src/lib.rs::type:Shared");
        let library_method = entity_with_key(&facts, "src/lib.rs::type:Shared::callable:library");
        let application_type = entity_with_key(&facts, "src/main.rs::type:Shared");
        let application_method =
            entity_with_key(&facts, "src/main.rs::type:Shared::callable:application");

        assert_contains(&facts, &library_type.key, &library_method.key);
        assert_contains(&facts, &application_type.key, &application_method.key);
    }

    #[test]
    fn isolates_standalone_binary_crate_paths_from_the_library() {
        let documents = [
            document(
                "src/lib.rs",
                "pub struct Shared;\nimpl crate::Shared { fn library(&self) {} }\n",
            ),
            document(
                "src/bin/tool.rs",
                "pub struct Shared;\nimpl crate::Shared { fn tool(&self) {} }\n",
            ),
        ];
        let facts = facts_for_documents(&documents).expect("both targets must produce facts");
        let library_type = entity_with_key(&facts, "src/lib.rs::type:Shared");
        let library_method = entity_with_key(&facts, "src/lib.rs::type:Shared::callable:library");
        let tool_type = entity_with_key(&facts, "src/bin/tool.rs::type:Shared");
        let tool_method = entity_with_key(&facts, "src/bin/tool.rs::type:Shared::callable:tool");

        assert_contains(&facts, &library_type.key, &library_method.key);
        assert_contains(&facts, &tool_type.key, &tool_method.key);
        assert!(!has_relation(
            &facts,
            &library_type.key,
            &tool_method.key,
            RelationKind::Contains
        ));
    }

    #[test]
    fn isolates_each_standard_standalone_target() {
        for target in ["examples/demo.rs", "benches/demo.rs", "tests/demo.rs"] {
            let documents = [
                document(
                    "src/lib.rs",
                    "pub struct Shared;\nimpl crate::Shared { fn library(&self) {} }\n",
                ),
                document(
                    target,
                    "pub struct Shared;\nimpl crate::Shared { fn target(&self) {} }\n",
                ),
            ];
            let facts = facts_for_documents(&documents).expect("both targets must produce facts");
            let target_type = entity_with_key(&facts, &format!("{target}::type:Shared"));
            let target_method =
                entity_with_key(&facts, &format!("{target}::type:Shared::callable:target"));

            assert_contains(&facts, &target_type.key, &target_method.key);
        }
    }

    #[test]
    fn resolves_directory_form_binary_modules() {
        let documents = [
            document("src/bin/tool/main.rs", "use crate::service;\n"),
            document("src/bin/tool/service.rs", "pub struct Service;\n"),
        ];
        let facts = facts_for_documents(&documents).expect("the binary target must produce facts");
        let root = entity_with_key(&facts, "src/bin/tool/main.rs");
        let service = entity_with_key(&facts, "src/bin/tool/service.rs");

        assert_imports(&facts, root, service);
    }

    #[test]
    fn resolves_directory_form_example_modules() {
        let documents = [
            document("examples/demo/main.rs", "use crate::service;\n"),
            document("examples/demo/service.rs", "pub struct Service;\n"),
        ];
        let facts = facts_for_documents(&documents).expect("the example target must produce facts");
        let root = entity_with_key(&facts, "examples/demo/main.rs");
        let service = entity_with_key(&facts, "examples/demo/service.rs");

        assert_imports(&facts, root, service);
    }

    #[test]
    fn resolves_nested_crate_modules_from_their_source_root() {
        let documents = [
            document("crates/api/src/lib.rs", "use crate::service;\n"),
            document("crates/api/src/service.rs", "pub struct Service;\n"),
        ];
        let facts = facts_for_documents(&documents).expect("the nested crate must produce facts");
        let root = entity_with_key(&facts, "crates/api/src/lib.rs");
        let service = entity_with_key(&facts, "crates/api/src/service.rs");

        assert_imports(&facts, root, service);
    }

    #[test]
    fn does_not_fall_back_to_root_for_unprefixed_nested_paths() {
        let source = "mod target { pub struct Record; }\nmod nested {\n    impl target::Record { fn wrong(&self) {} }\n    use target;\n}\n";
        let facts = facts_for("lib.rs", source).expect("the source must produce facts");
        let root = entity(&facts, "lib.rs", |kind| matches!(kind, EntityKind::Module));
        let wrong = callable_entity(&facts, "wrong");

        assert_eq!(wrong.key.as_str(), "lib.rs::callable:wrong");
        assert_contains(&facts, &root.key, &wrong.key);
        assert!(
            facts
                .relations
                .iter()
                .all(|relation| relation.kind != RelationKind::Imports)
        );
    }

    #[test]
    fn stops_resolution_at_an_ambiguous_relative_namespace() {
        let source = "struct Record;\nmod target {}\nmod nested {\n    #[cfg(first)] struct Record;\n    #[cfg(second)] struct Record;\n    #[cfg(first)] mod target {}\n    #[cfg(second)] mod target {}\n    impl Record { fn ambiguous(&self) {} }\n    use target;\n}\n";
        let facts = facts_for("lib.rs", source).expect("ambiguous syntax must still produce facts");
        let root = entity(&facts, "lib.rs", |kind| matches!(kind, EntityKind::Module));
        let ambiguous = callable_entity(&facts, "ambiguous");

        assert_eq!(ambiguous.key.as_str(), "lib.rs::callable:ambiguous");
        assert_contains(&facts, &root.key, &ambiguous.key);
        assert!(
            facts
                .relations
                .iter()
                .all(|relation| relation.kind != RelationKind::Imports)
        );
    }

    #[test]
    fn records_unambiguous_local_use_relations() {
        let source = "mod alpha {}\nuse crate::alpha;\n";
        let facts = facts_for("lib.rs", source).expect("the source must produce facts");
        let root = entity(&facts, "lib.rs", |kind| matches!(kind, EntityKind::Module));
        let alpha = entity(&facts, "alpha", |kind| matches!(kind, EntityKind::Module));
        let import = facts
            .relations
            .iter()
            .find(|relation| {
                relation.source == root.key
                    && relation.target == alpha.key
                    && relation.kind == RelationKind::Imports
            })
            .expect("the local use must produce an import relation");

        assert_eq!(import.evidence.observations.len(), 1);
        assert_eq!(
            range_text(source, import.evidence.observations[0].range),
            "use crate::alpha;"
        );
    }

    #[test]
    fn selects_only_rust_documents() {
        let documents = [
            document("src/lib.rs", ""),
            document("src/tool.py", ""),
            document("README.md", ""),
        ];

        let sources =
            select_sources(&documents, &["rs".to_owned()]).expect("selection must succeed");

        assert_eq!(sources.len(), 1);
        assert_eq!(sources.documents()[0].path, PathBuf::from("src/lib.rs"));
    }

    fn facts_for(
        path: &str,
        source: &str,
    ) -> Result<SyntaxFacts, crate::analysis::fact::AnalysisError> {
        let documents = [document(path, source)];
        facts_for_documents(&documents)
    }

    fn facts_for_documents(
        documents: &[DocumentSnapshot],
    ) -> Result<SyntaxFacts, crate::analysis::fact::AnalysisError> {
        let sources = Sources::new(documents.iter().collect());
        produce_syntax_facts(&sources)
    }

    fn document(path: &str, source: &str) -> DocumentSnapshot {
        DocumentSnapshot {
            path: PathBuf::from(path),
            content: source.to_owned(),
        }
    }

    fn entities_of_kind(facts: &SyntaxFacts, predicate: impl Fn(&EntityKind) -> bool) -> usize {
        facts
            .entities
            .iter()
            .filter(|entity| predicate(&entity.kind))
            .count()
    }

    fn entity<'a>(
        facts: &'a SyntaxFacts,
        name: &str,
        predicate: impl Fn(&EntityKind) -> bool,
    ) -> &'a EntityFact {
        facts
            .entities
            .iter()
            .find(|entity| entity.name == name && predicate(&entity.kind))
            .unwrap_or_else(|| panic!("the {name} entity must exist"))
    }

    fn entity_with_key<'a>(facts: &'a SyntaxFacts, key: &str) -> &'a EntityFact {
        facts
            .entities
            .iter()
            .find(|entity| entity.key.as_str() == key)
            .unwrap_or_else(|| panic!("the {key} entity must exist"))
    }

    fn typed_entity<'a>(facts: &'a SyntaxFacts, name: &str, expected: &TypeKind) -> &'a EntityFact {
        entity(
            facts,
            name,
            |kind| matches!(kind, EntityKind::Type(data) if same_type_kind(&data.kind, expected)),
        )
    }

    fn field_entity<'a>(facts: &'a SyntaxFacts, name: &str) -> &'a EntityFact {
        entity(facts, name, |kind| matches!(kind, EntityKind::Field(_)))
    }

    fn callable_entity<'a>(facts: &'a SyntaxFacts, name: &str) -> &'a EntityFact {
        entity(facts, name, |kind| matches!(kind, EntityKind::Callable(_)))
    }

    fn type_data(entity: &EntityFact) -> &TypeData {
        let EntityKind::Type(data) = &entity.kind else {
            panic!("the entity must be a type");
        };
        data
    }

    fn field_data(entity: &EntityFact) -> &FieldData {
        let EntityKind::Field(data) = &entity.kind else {
            panic!("the entity must be a field");
        };
        data
    }

    fn callable_data(entity: &EntityFact) -> &CallableData {
        let EntityKind::Callable(data) = &entity.kind else {
            panic!("the entity must be callable");
        };
        data
    }

    fn same_type_kind(left: &TypeKind, right: &TypeKind) -> bool {
        matches!(
            (left, right),
            (TypeKind::Class, TypeKind::Class)
                | (TypeKind::Struct, TypeKind::Struct)
                | (TypeKind::Enum, TypeKind::Enum)
                | (TypeKind::Interface, TypeKind::Interface)
                | (TypeKind::Trait, TypeKind::Trait)
        )
    }

    fn assert_contains(
        facts: &SyntaxFacts,
        owner: &crate::analysis::fact::EntityKey,
        child: &crate::analysis::fact::EntityKey,
    ) {
        assert!(facts.relations.iter().any(|relation| {
            relation.source == *owner
                && relation.target == *child
                && relation.kind == RelationKind::Contains
        }));
    }

    fn assert_imports(facts: &SyntaxFacts, source: &EntityFact, target: &EntityFact) {
        assert!(has_relation(
            facts,
            &source.key,
            &target.key,
            RelationKind::Imports
        ));
    }

    fn has_relation(
        facts: &SyntaxFacts,
        source: &crate::analysis::fact::EntityKey,
        target: &crate::analysis::fact::EntityKey,
        kind: RelationKind,
    ) -> bool {
        facts.relations.iter().any(|relation| {
            relation.source == *source && relation.target == *target && relation.kind == kind
        })
    }

    fn declaration_text<'a>(source: &'a str, entity: &EntityFact) -> &'a str {
        range_text(
            source,
            *entity
                .evidence
                .declarations
                .first()
                .expect("the declaration range must exist"),
        )
    }

    fn range_text(source: &str, range: SourceRange) -> &str {
        &source[range.start as usize..range.end as usize]
    }

    fn expected_range(source: &str, text: &str) -> SourceRange {
        let start = source.find(text).expect("the expected text must exist");
        SourceRange {
            start: u32::try_from(start).expect("the fixture offset must fit"),
            end: u32::try_from(start + text.len()).expect("the fixture offset must fit"),
        }
    }
}
