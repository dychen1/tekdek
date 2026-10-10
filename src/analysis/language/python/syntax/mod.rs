//! Python syntax facts.
//!
//! This module selects Python source documents and extracts syntax facts.
//! Source snapshots are the input to selection. Selected sources are the input
//! to fact extraction.
//!
//! Python syntax reports modules, classes, callables, fields, containment,
//! imports, and declared metadata.
//!
//! # Sections
//!
//! `Entry Points` defines source selection and fact extraction.
//! `Source Model` defines parsed documents, scopes, and extraction state.
//! `Document Parsing` derives module names and parses documents.
//! `Fact Construction` registers modules before statement traversal.
//! `Statement Traversal` processes Python suites.
//! `Fields` extracts class and instance fields.
//! `Imports` matches imports to selected modules.
//! `Entity Evidence` computes declaration, scope, and body ranges.
//! `Source Text` reads exact source slices.
//!
//! `imports` derives module names and resolves relative imports. `signature`
//! extracts signatures, parameters, return types, and documentation.

mod imports;
mod signature;

use std::{
    collections::{HashMap, HashSet},
    ffi::OsStr,
};

use rustpython_parser::{
    Mode, Parse, Tok,
    ast::{self, Arguments, ExceptHandler, Expr, ExprContext, Ranged, Stmt, text_size::TextRange},
    lexer,
};

use imports::{import_base, join_module_name, module_name};
use signature::{source_docstring, source_parameters, source_return_type, source_signature};

use crate::analysis::{
    fact::{
        AnalysisError, CallableData, CallableKind, EntityEvidence, EntityKey, EntityKind,
        RelationKind, SyntaxFactBuilder, SyntaxFacts, TypeData, TypeKind,
    },
    language::document_path,
    source::{DocumentSnapshot, SourceRange, SourceSpan, Sources, document_has_extension},
};

// -----------------------------------------------------------------------------
// Entry Points
// -----------------------------------------------------------------------------

/// Selects source documents that match the Python profile.
///
/// The function prefers a source file over its matching stub file.
///
/// # Errors
///
/// Returns an error if a selected document has an invalid path.
pub(super) fn select_sources<'a>(
    documents: &'a [DocumentSnapshot],
    extensions: &[String],
) -> Result<Sources<'a>, AnalysisError> {
    let source_stems = documents
        .iter()
        .filter(|document| document_has_extension(document, extensions))
        .filter(|document| has_extension(document, "py"))
        .map(|document| document.path.with_extension(""))
        .collect::<HashSet<_>>();

    let selected = documents
        .iter()
        .filter(|document| document_has_extension(document, extensions))
        .filter(|document| {
            !has_extension(document, "pyi")
                || !source_stems.contains(&document.path.with_extension(""))
        })
        .collect::<Vec<_>>();

    for document in &selected {
        document_path(document)?;
    }

    Ok(Sources::new(selected))
}

/// Produces syntax facts from selected Python sources.
///
/// # Errors
///
/// Returns an error if an identity, source range, or metadata value is invalid.
pub(super) fn produce_syntax_facts(sources: &Sources<'_>) -> Result<SyntaxFacts, AnalysisError> {
    produce_document_facts(&parse_documents(sources)?)
}

// -----------------------------------------------------------------------------
// Source Model
// -----------------------------------------------------------------------------

struct ParsedDocument<'a> {
    path: String,
    module_name: String,
    is_package_initializer: bool,
    source: &'a str,
    syntax_tree: ast::Suite,
}

struct CallableSyntax<'a> {
    name: &'a str,
    arguments: &'a Arguments,
    returns: Option<&'a Expr>,
    body: &'a [Stmt],
    range: TextRange,
}

impl<'a> CallableSyntax<'a> {
    fn function(definition: &'a ast::StmtFunctionDef) -> Self {
        Self {
            name: definition.name.as_str(),
            arguments: &definition.args,
            returns: definition.returns.as_deref(),
            body: &definition.body,
            range: definition.range,
        }
    }

    fn async_function(definition: &'a ast::StmtAsyncFunctionDef) -> Self {
        Self {
            name: definition.name.as_str(),
            arguments: &definition.args,
            returns: definition.returns.as_deref(),
            body: &definition.body,
            range: definition.range,
        }
    }
}

/// The signature annotation ranges of one callable.
struct CallableAnnotations {
    inputs: Vec<SourceRange>,
    outputs: Vec<SourceRange>,
}

#[derive(Clone)]
enum Scope {
    Module {
        node: EntityKey,
    },
    Class {
        node: EntityKey,
        module: EntityKey,
    },
    Callable {
        node: EntityKey,
        module: EntityKey,
        instance_class: Option<EntityKey>,
    },
}

impl Scope {
    fn owner(&self) -> &EntityKey {
        match self {
            Self::Module { node } | Self::Class { node, .. } | Self::Callable { node, .. } => node,
        }
    }

    fn module(&self) -> EntityKey {
        match self {
            Self::Module { node } => node.clone(),
            Self::Class { module, .. } | Self::Callable { module, .. } => module.clone(),
        }
    }

    fn direct_class(&self) -> Option<EntityKey> {
        match self {
            Self::Class { node, .. } => Some(node.clone()),
            Self::Module { .. } | Self::Callable { .. } => None,
        }
    }

    fn instance_class(&self) -> Option<EntityKey> {
        match self {
            Self::Callable { instance_class, .. } => instance_class.clone(),
            Self::Module { .. } | Self::Class { .. } => None,
        }
    }
}

struct SyntaxWalker {
    builder: SyntaxFactBuilder,
    module_keys: HashMap<String, EntityKey>,
}

// -----------------------------------------------------------------------------
// Document Parsing
// -----------------------------------------------------------------------------

/// Parses every selected Python source document.
///
/// # Errors
///
/// Returns an error if one selected document contains invalid Python.
fn parse_documents<'a>(sources: &Sources<'a>) -> Result<Vec<ParsedDocument<'a>>, AnalysisError> {
    sources.iter().map(parse_document).collect()
}

fn has_extension(document: &DocumentSnapshot, extension: &str) -> bool {
    document.path.extension().and_then(OsStr::to_str) == Some(extension)
}

/// Parses one selected Python document.
///
/// # Errors
///
/// Returns an error if the path or Python source is invalid.
fn parse_document(document: &DocumentSnapshot) -> Result<ParsedDocument<'_>, AnalysisError> {
    let path = document_path(document)?;
    let syntax_tree =
        ast::Suite::parse(&document.content, path).map_err(|error| AnalysisError {
            message: format!("cannot parse {path}: {error}"),
        })?;
    let (module_name, is_package_initializer) = module_name(&document.path)?;

    Ok(ParsedDocument {
        path: path.to_owned(),
        module_name,
        is_package_initializer,
        source: &document.content,
        syntax_tree,
    })
}

// -----------------------------------------------------------------------------
// Fact Construction
// -----------------------------------------------------------------------------

/// Produces every syntax fact from already parsed Python documents.
///
/// Pass one registers every module, so a forward import can resolve. Pass two
/// walks each document and emits its entities and relations.
///
/// # Errors
///
/// Returns an error if a module name repeats or a source range is invalid.
fn produce_document_facts(documents: &[ParsedDocument<'_>]) -> Result<SyntaxFacts, AnalysisError> {
    let mut walker = SyntaxWalker::new();

    for document in documents {
        walker.add_module(document)?;
    }
    for document in documents {
        walker.scan_statements(
            document,
            &document.syntax_tree,
            &Scope::Module {
                node: EntityKey::module(&document.path),
            },
        )?;
    }

    Ok(walker.builder.finish())
}

impl SyntaxWalker {
    fn new() -> Self {
        Self {
            builder: SyntaxFactBuilder::new(),
            module_keys: HashMap::new(),
        }
    }

    /// Registers one module before syntax traversal starts.
    ///
    /// # Errors
    ///
    /// Returns an error if the module name repeats or the source is too large.
    fn add_module(&mut self, document: &ParsedDocument<'_>) -> Result<(), AnalysisError> {
        let module = self.builder.add_module(&document.path, document.source)?;
        if self.module_keys.contains_key(&document.module_name) {
            return Err(AnalysisError::new(format!(
                "duplicate source unit: {}",
                document.module_name
            )));
        }
        self.module_keys
            .insert(document.module_name.clone(), module);
        Ok(())
    }
}

// -----------------------------------------------------------------------------
// Statement Traversal
// -----------------------------------------------------------------------------
//
// Statement traversal creates entity facts and syntax-derived relation facts.
// Control-flow suites keep their current module, class, or callable owner.

// scan_statements
//     → class → scan_statements with a class scope
//     → callable → scan_statements with a callable scope
//     → assignment → add or update a field
//     → import → add or update an import edge
//     → control flow → scan_statements with the same scope

impl SyntaxWalker {
    /// Adds facts from one Python statement list.
    ///
    /// Control-flow statements preserve the current lexical scope.
    ///
    /// # Errors
    ///
    /// Returns an error if a syntax identity or source range is invalid.
    fn scan_statements(
        &mut self,
        document: &ParsedDocument<'_>,
        statements: &[Stmt],
        scope: &Scope,
    ) -> Result<(), AnalysisError> {
        for statement in statements {
            match statement {
                // Enter a class scope.
                Stmt::ClassDef(definition) => {
                    self.add_class(document, definition, scope)?;
                }
                // Enter a synchronous callable scope.
                Stmt::FunctionDef(definition) => {
                    self.add_callable(document, &CallableSyntax::function(definition), scope)?;
                }
                // Enter an asynchronous callable scope.
                Stmt::AsyncFunctionDef(definition) => {
                    self.add_callable(
                        document,
                        &CallableSyntax::async_function(definition),
                        scope,
                    )?;
                }
                // Read assigned fields.
                Stmt::Assign(assignment) => {
                    for target in &assignment.targets {
                        self.add_field_target(
                            document,
                            target,
                            scope,
                            assignment.type_comment.clone(),
                        )?;
                    }
                }
                // Read one annotated field.
                Stmt::AnnAssign(assignment) => {
                    let declared_type = Some(
                        source_text(document.source, assignment.annotation.range())?
                            .trim()
                            .to_owned(),
                    );
                    self.add_field_target(document, &assignment.target, scope, declared_type)?;
                }
                // Read one augmented field.
                Stmt::AugAssign(assignment) => {
                    self.add_field_target(document, &assignment.target, scope, None)?;
                }
                // Read direct imports.
                Stmt::Import(import) => self.add_imports(document, import, scope),
                // Read from imports.
                Stmt::ImportFrom(import) => self.add_from_imports(document, import, scope),
                // Scan suites that keep this scope.
                _ => self.scan_control_flow(document, statement, scope)?,
            }
        }

        Ok(())
    }

    /// Adds one class and scans its body with a class scope.
    ///
    /// # Errors
    ///
    /// Returns an error if a source range or nested fact is invalid.
    fn add_class(
        &mut self,
        document: &ParsedDocument<'_>,
        definition: &ast::StmtClassDef,
        scope: &Scope,
    ) -> Result<(), AnalysisError> {
        let evidence = class_or_callable_evidence(
            &document.path,
            document.source,
            definition.range,
            definition.name.as_str(),
            &definition.body,
        )?;
        let node = self.builder.add_entity(
            scope.owner(),
            definition.name.to_string(),
            EntityKind::Type(TypeData {
                kind: TypeKind::Class,
                documentation: source_docstring(&definition.body),
            }),
            evidence,
        );
        self.builder
            .add_relation(scope.owner(), &node, RelationKind::Contains);
        self.scan_statements(
            document,
            &definition.body,
            &Scope::Class {
                node,
                module: scope.module(),
            },
        )
    }

    /// Adds one callable and scans its body with a callable scope.
    ///
    /// # Errors
    ///
    /// Returns an error if a callable source range or metadata value is invalid.
    fn add_callable(
        &mut self,
        document: &ParsedDocument<'_>,
        callable: &CallableSyntax<'_>,
        scope: &Scope,
    ) -> Result<(), AnalysisError> {
        let class = scope.direct_class();
        let kind = match class {
            Some(_) if callable.name == "__init__" => CallableKind::Constructor,
            Some(_) => CallableKind::Method,
            None => CallableKind::Function,
        };
        let mut evidence = class_or_callable_evidence(
            &document.path,
            document.source,
            callable.range,
            callable.name,
            callable.body,
        )?;
        let annotations = callable_annotations(callable);
        evidence.type_inputs = annotations.inputs;
        evidence.type_outputs = annotations.outputs;

        let node = self.builder.add_entity(
            scope.owner(),
            callable.name.to_owned(),
            EntityKind::Callable(CallableData {
                kind,
                signature: Some(source_signature(document.source, callable)?),
                parameters: source_parameters(document.source, callable.arguments)?,
                return_type: source_return_type(document.source, callable.returns)?,
                documentation: source_docstring(callable.body),
            }),
            evidence,
        );
        self.builder
            .add_relation(scope.owner(), &node, RelationKind::Contains);

        let instance_class = class.filter(|_| Self::first_parameter_is_self(callable.arguments));
        self.scan_statements(
            document,
            callable.body,
            &Scope::Callable {
                node,
                module: scope.module(),
                instance_class,
            },
        )
    }

    /// Returns `true` when the first positional parameter is `self`.
    fn first_parameter_is_self(arguments: &Arguments) -> bool {
        arguments
            .posonlyargs
            .first()
            .or_else(|| arguments.args.first())
            .is_some_and(|argument| argument.def.arg.as_str() == "self")
    }

    /// Scans nested suites that preserve the current lexical scope.
    ///
    /// # Errors
    ///
    /// Returns an error if a nested source fact is invalid.
    fn scan_control_flow(
        &mut self,
        document: &ParsedDocument<'_>,
        statement: &Stmt,
        scope: &Scope,
    ) -> Result<(), AnalysisError> {
        match statement {
            // Scan synchronous loop suites.
            Stmt::For(statement) => {
                self.scan_suites(document, &[&statement.body, &statement.orelse], scope)
            }
            // Scan asynchronous loop suites.
            Stmt::AsyncFor(statement) => {
                self.scan_suites(document, &[&statement.body, &statement.orelse], scope)
            }
            // Scan while-loop suites.
            Stmt::While(statement) => {
                self.scan_suites(document, &[&statement.body, &statement.orelse], scope)
            }
            // Scan conditional suites.
            Stmt::If(statement) => {
                self.scan_suites(document, &[&statement.body, &statement.orelse], scope)
            }
            // Scan a synchronous context body.
            Stmt::With(statement) => self.scan_suites(document, &[&statement.body], scope),
            // Scan an asynchronous context body.
            Stmt::AsyncWith(statement) => self.scan_suites(document, &[&statement.body], scope),
            // Scan each match case.
            Stmt::Match(statement) => {
                for case in &statement.cases {
                    self.scan_statements(document, &case.body, scope)?;
                }
                Ok(())
            }
            // Scan standard try suites.
            Stmt::Try(statement) => self.scan_try_suites(
                document,
                &statement.body,
                &statement.handlers,
                &statement.orelse,
                &statement.finalbody,
                scope,
            ),
            // Scan exception-group try suites.
            Stmt::TryStar(statement) => self.scan_try_suites(
                document,
                &statement.body,
                &statement.handlers,
                &statement.orelse,
                &statement.finalbody,
                scope,
            ),
            // Ignore statements without suites.
            _ => Ok(()),
        }
    }

    /// Scans each suite with the same scope.
    ///
    /// # Errors
    ///
    /// Returns an error if a nested source fact is invalid.
    fn scan_suites(
        &mut self,
        document: &ParsedDocument<'_>,
        suites: &[&[Stmt]],
        scope: &Scope,
    ) -> Result<(), AnalysisError> {
        for suite in suites {
            self.scan_statements(document, suite, scope)?;
        }

        Ok(())
    }

    /// Scans each suite in a `try` or `try-star` statement.
    ///
    /// # Errors
    ///
    /// Returns an error if a nested source fact is invalid.
    fn scan_try_suites(
        &mut self,
        document: &ParsedDocument<'_>,
        body: &[Stmt],
        handlers: &[ExceptHandler],
        orelse: &[Stmt],
        finalbody: &[Stmt],
        scope: &Scope,
    ) -> Result<(), AnalysisError> {
        self.scan_statements(document, body, scope)?;
        for handler in handlers {
            let ExceptHandler::ExceptHandler(handler) = handler;
            self.scan_statements(document, &handler.body, scope)?;
        }
        self.scan_suites(document, &[orelse, finalbody], scope)
    }

    // -----------------------------------------------------------------------------
    // Fields
    // -----------------------------------------------------------------------------
    //
    // Field extraction accepts direct class assignments and direct `self`
    // assignments. Repeated declarations share one field entity.

    /// Adds fields represented by one assignment target.
    ///
    /// The function accepts class names and direct `self` attributes.
    ///
    /// # Errors
    ///
    /// Returns an error if an instance-field range is invalid.
    fn add_field_target(
        &mut self,
        document: &ParsedDocument<'_>,
        target: &Expr,
        scope: &Scope,
        declared_type: Option<String>,
    ) -> Result<(), AnalysisError> {
        match target {
            // Record a direct class field. The parser limits a name range to the name.
            Expr::Name(target) => {
                if let Some(class) = scope.direct_class() {
                    self.builder.add_or_extend_field(
                        &class,
                        target.id.as_str(),
                        &document.path,
                        declared_type,
                        text_range(target.range),
                    );
                }
            }
            // Record a direct instance field.
            Expr::Attribute(target)
                if matches!(target.ctx, ExprContext::Store)
                    && matches!(target.value.as_ref(), Expr::Name(owner) if owner.id.as_str() == "self") =>
            {
                if let Some(class) = scope.instance_class() {
                    self.builder.add_or_extend_field(
                        &class,
                        target.attr.as_str(),
                        &document.path,
                        declared_type,
                        instance_field_name_range(
                            document.source,
                            target.range,
                            target.attr.as_str(),
                        )?,
                    );
                }
            }
            // Expand a tuple target.
            Expr::Tuple(targets) => {
                for target in &targets.elts {
                    self.add_field_target(document, target, scope, declared_type.clone())?;
                }
            }
            // Expand a list target.
            Expr::List(targets) => {
                for target in &targets.elts {
                    self.add_field_target(document, target, scope, declared_type.clone())?;
                }
            }
            // Unwrap a starred target.
            Expr::Starred(target) => {
                self.add_field_target(document, &target.value, scope, declared_type)?;
            }
            // Ignore targets that cannot declare fields.
            _ => {}
        }

        Ok(())
    }

    // -----------------------------------------------------------------------------
    // Imports
    // -----------------------------------------------------------------------------
    //
    // Import resolution maps Python module names to selected documents. Each
    // resolved import statement contributes one relation observation.

    /// Adds each selected module from one `import` statement.
    fn add_imports(
        &mut self,
        document: &ParsedDocument<'_>,
        import: &ast::StmtImport,
        scope: &Scope,
    ) {
        let observation = source_span(&document.path, import.range);
        for alias in &import.names {
            if let Some(target) = self.module_keys.get(alias.name.as_str()).cloned() {
                self.builder.add_observed_relation(
                    scope.owner(),
                    &target,
                    RelationKind::Imports,
                    vec![observation.clone()],
                );
            }
        }
    }

    /// Adds each selected module once for one `from` import statement.
    ///
    /// A selected child module takes precedence over its selected base module.
    fn add_from_imports(
        &mut self,
        document: &ParsedDocument<'_>,
        import: &ast::StmtImportFrom,
        scope: &Scope,
    ) {
        let Some(base) = import_base(document, import) else {
            return;
        };
        let observation = source_span(&document.path, import.range);
        let mut seen = HashSet::new();

        for alias in &import.names {
            let child = join_module_name(&base, alias.name.as_str());
            let target = self
                .module_keys
                .get(&child)
                .or_else(|| self.module_keys.get(&base))
                .cloned();

            if let Some(target) = target
                && seen.insert(target.clone())
            {
                self.builder.add_observed_relation(
                    scope.owner(),
                    &target,
                    RelationKind::Imports,
                    vec![observation.clone()],
                );
            }
        }
    }
}

fn source_span(path: &str, range: TextRange) -> SourceSpan {
    SourceSpan {
        path: path.to_owned(),
        range: text_range(range),
    }
}

// -----------------------------------------------------------------------------
// Entity Evidence
// -----------------------------------------------------------------------------
//
// These functions convert `RustPython` ranges into fact ranges. Declaration
// ranges support exact matching. Body ranges support ownership.

/// Returns source evidence for one class or callable.
///
/// The declaration range covers only the declared name. The body range covers
/// the statement suite, not the definition header.
fn class_or_callable_evidence(
    path: &str,
    source: &str,
    definition_range: TextRange,
    name: &str,
    body: &[Stmt],
) -> Result<EntityEvidence, AnalysisError> {
    Ok(EntityEvidence {
        path: path.to_owned(),
        declarations: vec![class_or_callable_name_range(
            source,
            definition_range,
            name,
        )?],
        scope: Some(text_range(definition_range)),
        body: owned_suite_range(source, body)?,
        type_inputs: Vec::new(),
        type_outputs: Vec::new(),
    })
}

/// Returns the signature annotation ranges of one callable.
///
/// A default expression is absent. It runs in the enclosing scope.
fn callable_annotations(callable: &CallableSyntax<'_>) -> CallableAnnotations {
    let arguments = callable.arguments;
    let mut inputs = Vec::new();

    for argument in arguments.posonlyargs.iter().chain(&arguments.args) {
        push_annotation_range(&mut inputs, argument.def.annotation.as_deref());
    }
    if let Some(argument) = &arguments.vararg {
        push_annotation_range(&mut inputs, argument.annotation.as_deref());
    }
    for argument in &arguments.kwonlyargs {
        push_annotation_range(&mut inputs, argument.def.annotation.as_deref());
    }
    if let Some(argument) = &arguments.kwarg {
        push_annotation_range(&mut inputs, argument.annotation.as_deref());
    }

    let mut outputs = Vec::new();
    push_annotation_range(&mut outputs, callable.returns);

    CallableAnnotations { inputs, outputs }
}

/// Adds one annotation range when the annotation exists.
fn push_annotation_range(ranges: &mut Vec<SourceRange>, annotation: Option<&Expr>) {
    if let Some(annotation) = annotation {
        ranges.push(text_range(annotation.range()));
    }
}

/// Returns the field name in one direct instance-field assignment.
///
/// The syntax range covers the complete `self.name` expression. This function
/// selects only `name`.
fn instance_field_name_range(
    source: &str,
    attribute_range: TextRange,
    name: &str,
) -> Result<SourceRange, AnalysisError> {
    let expression = source_text(source, attribute_range)?;
    let mut found = None;

    for token in lexer::lex(expression, Mode::Expression) {
        let (token, token_range) = token.map_err(|error| token_error(&error))?;
        if matches!(token, Tok::Name { name: token_name } if token_name == name) {
            found = Some(token_range_in_document(attribute_range, token_range)?);
        }
    }

    found.ok_or_else(|| AnalysisError {
        message: format!("cannot find the {name} attribute name"),
    })
}

fn text_range(range: TextRange) -> SourceRange {
    SourceRange {
        start: range.start().to_u32(),
        end: range.end().to_u32(),
    }
}

/// Returns the exact name token after one `class` or `def` keyword.
///
/// `RustPython` stores the complete definition range but no separate name
/// range. The returned range supports exact declaration matching.
fn class_or_callable_name_range(
    source: &str,
    definition_range: TextRange,
    name: &str,
) -> Result<SourceRange, AnalysisError> {
    let definition = source_text(source, definition_range)?;
    let mut expects_name = false;

    for token in lexer::lex(definition, Mode::Module) {
        let (token, token_range) = token.map_err(|error| token_error(&error))?;
        match token {
            Tok::Class | Tok::Def => expects_name = true,
            Tok::Name { name: token_name } if expects_name && token_name == name => {
                return token_range_in_document(definition_range, token_range);
            }
            _ => {}
        }
    }

    Err(AnalysisError {
        message: format!("cannot find the {name} declaration name"),
    })
}

/// Returns the body range owned by one class or callable.
///
/// The range starts at the first body statement and ends after the last one.
/// Decorators on the first nested definition belong to this enclosing body.
fn owned_suite_range(source: &str, body: &[Stmt]) -> Result<Option<SourceRange>, AnalysisError> {
    let Some(first) = body.first() else {
        return Ok(None);
    };
    let Some(last) = body.last() else {
        return Ok(None);
    };

    Ok(Some(SourceRange {
        start: first_owned_statement_start(source, first)?,
        end: last.range().end().to_u32(),
    }))
}

/// Returns the first byte owned by an enclosing statement suite.
///
/// If the first statement has decorators, ownership starts at the first `@`.
/// Those decorators execute before the nested class or callable body starts.
fn first_owned_statement_start(source: &str, statement: &Stmt) -> Result<u32, AnalysisError> {
    let decorators = match statement {
        // Read class decorators.
        Stmt::ClassDef(statement) => &statement.decorator_list,
        // Read synchronous function decorators.
        Stmt::FunctionDef(statement) => &statement.decorator_list,
        // Read asynchronous function decorators.
        Stmt::AsyncFunctionDef(statement) => &statement.decorator_list,
        // Keep the syntax start for other statements.
        _ => return Ok(statement.range().start().to_u32()),
    };
    let Some(decorator) = decorators.first() else {
        return Ok(statement.range().start().to_u32());
    };

    let expression_start = decorator.range().start().to_usize();
    let prefix = source
        .get(..expression_start)
        .ok_or_else(invalid_source_range)?;
    let line_start = prefix.rfind('\n').map_or(0, |offset| offset + 1);
    let decorator_prefix = source
        .get(line_start..expression_start)
        .ok_or_else(invalid_source_range)?;
    let marker = decorator_prefix
        .rfind('@')
        .ok_or_else(invalid_source_range)?;

    u32::try_from(line_start + marker).map_err(|_| invalid_source_range())
}

/// Converts a fragment-relative token range into a document byte range.
///
/// The lexer reports token offsets relative to the syntax fragment it receives.
/// Fact ranges require offsets relative to the complete source document.
fn token_range_in_document(
    fragment: TextRange,
    token: TextRange,
) -> Result<SourceRange, AnalysisError> {
    let base = fragment.start().to_u32();
    Ok(SourceRange {
        start: base
            .checked_add(token.start().to_u32())
            .ok_or_else(invalid_source_range)?,
        end: base
            .checked_add(token.end().to_u32())
            .ok_or_else(invalid_source_range)?,
    })
}

// -----------------------------------------------------------------------------
// Source Text
// -----------------------------------------------------------------------------

/// Returns the source text inside one byte range.
///
/// # Errors
///
/// Returns an error if the range is outside the source or splits a character.
fn source_text(source: &str, range: TextRange) -> Result<&str, AnalysisError> {
    source
        .get(range.start().to_usize()..range.end().to_usize())
        .ok_or_else(invalid_source_range)
}

fn token_error(error: &rustpython_parser::lexer::LexicalError) -> AnalysisError {
    AnalysisError {
        message: format!("cannot tokenize Python source: {error:?}"),
    }
}

fn invalid_source_range() -> AnalysisError {
    AnalysisError {
        message: "a Python source range is invalid".to_owned(),
    }
}

// -----------------------------------------------------------------------------
// Tests
// -----------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use rustpython_parser::{Parse, ast};

    use crate::analysis::{
        fact::{FactSet, RelationKind},
        graph::build::build_graph,
        language::Language,
        source::{DocumentSnapshot, SourceRange},
    };

    use super::{
        CallableSyntax, ParsedDocument, callable_annotations, class_or_callable_evidence,
        instance_field_name_range, produce_document_facts, select_sources,
    };

    #[test]
    fn ignores_unselected_sources_when_preferring_sources_over_stubs() {
        let documents = [
            DocumentSnapshot {
                path: PathBuf::from("service.py"),
                content: "value = 1\n".to_owned(),
            },
            DocumentSnapshot {
                path: PathBuf::from("service.pyi"),
                content: "value: int\n".to_owned(),
            },
        ];

        let sources = select_sources(&documents, &["pyi".to_owned()])
            .expect("the selected stub should be valid");

        assert_eq!(sources.len(), 1);
        assert_eq!(sources.documents()[0].path, PathBuf::from("service.pyi"));
    }

    #[test]
    fn locates_declarations_and_owned_bodies() {
        let source = "def outer():\n    @decorate\n    async def inner(value=default()):\n        self.field = value\n";
        let syntax = ast::Suite::parse(source, "example.py").expect("the fixture must parse");
        let ast::Stmt::FunctionDef(outer) = &syntax[0] else {
            panic!("the first statement must be a function");
        };
        let outer_evidence = class_or_callable_evidence(
            "example.py",
            source,
            outer.range,
            outer.name.as_str(),
            &outer.body,
        )
        .expect("the outer evidence must be valid");

        assert_eq!(range_text(source, outer_evidence.declarations[0]), "outer");
        assert!(
            range_text(
                source,
                outer_evidence.scope.expect("the outer scope must exist")
            )
            .starts_with("def outer")
        );
        assert!(
            range_text(
                source,
                outer_evidence.body.expect("the outer body must exist")
            )
            .starts_with('@')
        );

        let ast::Stmt::AsyncFunctionDef(inner) = &outer.body[0] else {
            panic!("the outer body must contain an asynchronous function");
        };
        let inner_evidence = class_or_callable_evidence(
            "example.py",
            source,
            inner.range,
            inner.name.as_str(),
            &inner.body,
        )
        .expect("the inner evidence must be valid");

        assert_eq!(range_text(source, inner_evidence.declarations[0]), "inner");
        assert_eq!(
            range_text(
                source,
                inner_evidence.body.expect("the inner body must exist")
            )
            .trim(),
            "self.field = value"
        );

        let ast::Stmt::Assign(assignment) = &inner.body[0] else {
            panic!("the inner body must contain an assignment");
        };
        let ast::Expr::Attribute(field) = &assignment.targets[0] else {
            panic!("the assignment target must be an attribute");
        };
        let field_range = instance_field_name_range(source, field.range, field.attr.as_str())
            .expect("the field range must be valid");

        assert_eq!(range_text(source, field_range), "field");
    }

    #[test]
    fn records_distinct_import_observations_on_one_edge() {
        let library_source = "";
        let application_source = "import library\nimport library\n";
        let documents = vec![
            parsed_document("library.py", "library", library_source),
            parsed_document("application.py", "application", application_source),
        ];
        let facts = produce_document_facts(&documents).expect("syntax production must succeed");
        let graph = build_graph(vec![FactSet::new(Language::Python, facts, Vec::new())], &[])
            .expect("graph construction must succeed");
        let import = graph
            .edges
            .iter()
            .find(|edge| edge.kind == RelationKind::Imports)
            .expect("the import edge must exist");

        assert_eq!(import.weight, 2);
        assert_eq!(import.provenance.evidence.observations.len(), 2);
        assert_eq!(
            import.provenance.evidence.observations[0].path,
            "application.py"
        );
        assert_eq!(
            import
                .provenance
                .evidence
                .observations
                .iter()
                .map(|observation| range_text(application_source, observation.range))
                .collect::<Vec<_>>(),
            vec!["import library", "import library"]
        );
    }

    #[test]
    fn separates_accepted_and_returned_signature_annotations() {
        let source = concat!(
            "def run(\n",
            "    first: Request,\n",
            "    /,\n",
            "    second: list[Option],\n",
            "    *rest: Extra,\n",
            "    named: Named = default(),\n",
            "    **options: Options,\n",
            ") -> Response:\n",
            "    return execute()\n",
        );
        let syntax = ast::Suite::parse(source, "example.py").expect("the fixture must parse");
        let ast::Stmt::FunctionDef(definition) = &syntax[0] else {
            panic!("the first statement must be a function");
        };

        let annotations = callable_annotations(&CallableSyntax::function(definition));

        assert_eq!(
            annotations
                .inputs
                .iter()
                .map(|range| range_text(source, *range))
                .collect::<Vec<_>>(),
            vec!["Request", "list[Option]", "Extra", "Named", "Options"]
        );
        assert_eq!(
            annotations
                .outputs
                .iter()
                .map(|range| range_text(source, *range))
                .collect::<Vec<_>>(),
            vec!["Response"]
        );
    }

    fn parsed_document<'a>(path: &str, module_name: &str, source: &'a str) -> ParsedDocument<'a> {
        ParsedDocument {
            path: path.to_owned(),
            module_name: module_name.to_owned(),
            is_package_initializer: false,
            source,
            syntax_tree: ast::Suite::parse(source, path).expect("the fixture must parse"),
        }
    }

    fn range_text(source: &str, range: SourceRange) -> &str {
        &source[range.start as usize..range.end as usize]
    }
}
