//! TypeScript and JavaScript syntax facts.
//!
//! This module selects TypeScript and JavaScript source documents and extracts
//! syntax facts. Source snapshots are the input to selection. Selected sources
//! are the input to fact extraction.
//!
//! The syntax reports modules, namespaces, classes, interfaces, enums,
//! callables, fields, containment, imports, and declared metadata. A function
//! is a callable when a declaration, a class member, or a variable names it.
//! Other functions, such as callbacks, belong to the callable or module that
//! holds them.
//!
//! # Sections
//!
//! `Entry Points` defines source selection and fact extraction.
//! `Source Model` defines scopes, callables, and the syntax walker.
//! `Declarations` adds namespaces, types, and callables.
//! `Fields` adds class, interface, and instance fields.
//! `Imports` records import sites and the imported names to query.
//! `Entity Facts` adds entities with their evidence.
//! `Overloads` merges overload signatures into one callable.
//! `Source Text` converts spans and reads names.
//!
//! `signature` extracts signatures, parameters, type annotations, and
//! documentation.

mod signature;

use std::collections::{HashMap, HashSet};

use oxc_allocator::{Allocator, ArenaVec};
use oxc_ast::{
    Comment,
    ast::{
        AccessorProperty, ArrowFunctionExpression, AssignmentExpression, AssignmentTarget,
        BindingPattern, Class, ClassBody, ClassElement, ClassType, Declaration,
        ExportAllDeclaration, ExportDefaultDeclarationKind, ExportFromDeclaration, Expression,
        FormalParameters, Function, FunctionType, ImportDeclaration, ImportDeclarationSpecifier,
        MethodDefinition, MethodDefinitionKind, PropertyDefinition, PropertyKey, Statement,
        StaticBlock, StringLiteral, TSEnumDeclaration, TSInterfaceDeclaration,
        TSNamespaceDeclaration, TSSignature, TSTypeAnnotation, VariableDeclaration,
        VariableDeclarator,
    },
};
use oxc_ast_visit::{Visit, walk};
use oxc_parser::{ParseOptions, Parser};
use oxc_span::{GetSpan, SourceType, Span};
use oxc_syntax::scope::ScopeFlags;

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

/// Selects source documents that match the profile.
///
/// Declaration files and files in build output folders are absent.
///
/// # Errors
///
/// Returns an error if a selected document has an invalid path.
pub(super) fn select_sources<'a>(
    documents: &'a [DocumentSnapshot],
    extensions: &[String],
) -> Result<Sources<'a>, AnalysisError> {
    let selected = documents
        .iter()
        .filter(|document| document_has_extension(document, extensions))
        .filter(|document| !is_declaration_file(document) && !is_build_output(document))
        .collect::<Vec<_>>();

    for document in &selected {
        document_path(document)?;
    }

    Ok(Sources::new(selected))
}

/// Produces syntax facts from selected sources.
///
/// Pass one registers every module, so module facts come first. Pass two
/// parses and walks each document.
///
/// # Errors
///
/// Returns an error if a module path repeats or a source does not parse.
pub(super) fn produce_syntax_facts(sources: &Sources<'_>) -> Result<SyntaxFacts, AnalysisError> {
    let mut builder = SyntaxFactBuilder::new();
    for document in sources.iter() {
        builder.add_module(document_path(document)?, &document.content)?;
    }
    for document in sources.iter() {
        scan_document(&mut builder, document)?;
    }
    Ok(builder.finish())
}

/// Returns `true` for a declaration file, such as `index.d.ts`.
///
/// shortcut: a declaration file holds no bodies to step through, so its types
/// get no node. Select these files when a flow needs their types.
fn is_declaration_file(document: &DocumentSnapshot) -> bool {
    SourceType::from_path(&document.path).is_ok_and(SourceType::is_typescript_definition)
}

/// Returns `true` for a file in a `dist`, `build`, or `coverage` folder.
///
/// These folders hold generated JavaScript. Other languages keep them, because
/// Python and Rust packages can use these names for source.
fn is_build_output(document: &DocumentSnapshot) -> bool {
    document.path.parent().is_some_and(|folder| {
        folder.components().any(|part| {
            matches!(
                part.as_os_str().to_str(),
                Some("dist" | "build" | "coverage")
            )
        })
    })
}

/// Parses one document and adds its facts.
///
/// JavaScript always allows JSX, because React code often uses `.js`. An
/// extension that names no TypeScript or JavaScript dialect parses as
/// TypeScript with JSX. A script can return outside a function, as Node.js
/// allows in `CommonJS`.
///
/// shortcut: a document with a syntax error keeps only its module, and a
/// warning names it, so one stray script does not stop the index of every
/// language. Report these documents in the index output if users miss them.
///
/// # Errors
///
/// Returns an error if the path is invalid.
fn scan_document(
    builder: &mut SyntaxFactBuilder,
    document: &DocumentSnapshot,
) -> Result<(), AnalysisError> {
    let path = document_path(document)?;
    let allocator = Allocator::default();
    let source_type = SourceType::from_path(path).map_or_else(
        |_| SourceType::tsx(),
        |source_type| source_type.with_jsx(source_type.is_jsx() || source_type.is_javascript()),
    );
    let parsed = Parser::new(&allocator, &document.content, source_type)
        .with_options(ParseOptions {
            allow_return_outside_function: true,
            ..ParseOptions::default()
        })
        .parse();
    if let Some(error) = parsed.diagnostics.errors().next() {
        eprintln!("warning: {path} has no declarations in the graph: cannot parse it: {error}");
        return Ok(());
    }

    let mut walker = SyntaxWalker {
        builder,
        path,
        source: &document.content,
        comments: &parsed.program.comments,
        module: EntityKey::module(path),
        scopes: Vec::new(),
        binding: None,
        overloads: Overloads::default(),
    };
    walker.visit_program(&parsed.program);
    Ok(())
}

// -----------------------------------------------------------------------------
// Source Model
// -----------------------------------------------------------------------------

/// One lexical owner of declarations.
enum Scope {
    /// A namespace.
    Module(EntityKey),
    /// A class or interface body.
    Type { node: EntityKey, class: bool },
    /// A callable body. `instance` is the class that `this` names in it.
    Callable {
        node: EntityKey,
        instance: Option<EntityKey>,
    },
}

impl Scope {
    fn node(&self) -> &EntityKey {
        match self {
            Self::Module(node) | Self::Type { node, .. } | Self::Callable { node, .. } => node,
        }
    }
}

/// The parts of one callable that its facts read.
pub(super) struct CallableSyntax<'s, 'a> {
    name: String,
    declaration: Span,
    kind: CallableKind,
    /// The complete definition.
    scope: Span,
    /// The first byte of the signature text.
    header_start: u32,
    /// A keyword that the signature text starts with, such as `const`.
    keyword: Option<&'static str>,
    /// The first byte of the declaration that a `JSDoc` comment documents.
    documented_from: u32,
    parameters: &'s FormalParameters<'a>,
    return_type: Option<&'s TSTypeAnnotation<'a>>,
    body: Option<Span>,
}

/// The variable declaration that the walker is in.
#[derive(Clone, Copy)]
struct Binding {
    keyword: &'static str,
    start: u32,
    first_declarator: u32,
}

/// A function that a variable or a class property names.
enum BoundFunction<'s, 'a> {
    Arrow(&'s ArrowFunctionExpression<'a>),
    Function(&'s Function<'a>),
}

impl<'s, 'a> BoundFunction<'s, 'a> {
    /// Returns the function that one initializer holds, through parentheses and type assertions.
    fn of(expression: &'s Expression<'a>) -> Option<Self> {
        match expression.get_inner_expression() {
            Expression::ArrowFunctionExpression(arrow) => Some(Self::Arrow(arrow)),
            Expression::FunctionExpression(function) => Some(Self::Function(function)),
            _ => None,
        }
    }

    fn parameters(&self) -> &'s FormalParameters<'a> {
        match self {
            Self::Arrow(arrow) => &arrow.params,
            Self::Function(function) => &function.params,
        }
    }

    fn return_type(&self) -> Option<&'s TSTypeAnnotation<'a>> {
        match self {
            Self::Arrow(arrow) => arrow.return_type.as_deref(),
            Self::Function(function) => function.return_type.as_deref(),
        }
    }

    fn body(&self) -> Option<Span> {
        match self {
            Self::Arrow(arrow) => Some(arrow.body.span()),
            Self::Function(function) => function.body.as_ref().map(|body| body.span),
        }
    }

    /// Walks the parameters and the body.
    fn walk(&self, walker: &mut SyntaxWalker<'_>) {
        match self {
            Self::Arrow(arrow) => walk::walk_arrow_function_expression(walker, arrow),
            Self::Function(function) => walk::walk_function(walker, function, ScopeFlags::Function),
        }
    }
}

struct SyntaxWalker<'s> {
    builder: &'s mut SyntaxFactBuilder,
    path: &'s str,
    source: &'s str,
    comments: &'s [Comment],
    module: EntityKey,
    scopes: Vec<Scope>,
    binding: Option<Binding>,
    overloads: Overloads,
}

impl SyntaxWalker<'_> {
    fn owner(&self) -> &EntityKey {
        self.scopes.last().map_or(&self.module, Scope::node)
    }

    /// Returns the nearest namespace, callable, or module.
    ///
    /// Code in a class body outside its members, such as a field initializer
    /// or a static block, declares nothing on the class.
    fn body_owner(&self) -> EntityKey {
        self.scopes
            .iter()
            .rev()
            .find(|scope| !matches!(scope, Scope::Type { .. }))
            .map_or(&self.module, Scope::node)
            .clone()
    }

    /// Returns the class whose body directly holds the walker.
    fn class_scope(&self) -> Option<EntityKey> {
        match self.scopes.last() {
            Some(Scope::Type { node, class: true }) => Some(node.clone()),
            _ => None,
        }
    }

    /// Returns the class that `this` names in the current callable.
    fn this_class(&self) -> Option<EntityKey> {
        match self.scopes.last() {
            Some(Scope::Callable { instance, .. }) => instance.clone(),
            _ => None,
        }
    }

    /// Walks with one more scope.
    fn enter(&mut self, scope: Scope, walk: impl FnOnce(&mut Self)) {
        self.scopes.push(scope);
        walk(self);
        self.scopes.pop();
    }

    /// Merges the getter and the setter of one property into one callable,
    /// held by the first of the two.
    ///
    /// shortcut: a named function inside the second accessor joins the class,
    /// not the holder; track the holder key if that shows up in real code.
    fn merge_accessor_pairs(&mut self, body: &ClassBody<'_>) {
        let mut first = HashMap::new();
        for method in body.body.iter().filter_map(|element| match element {
            ClassElement::MethodDefinition(method)
                if matches!(
                    method.kind,
                    MethodDefinitionKind::Get | MethodDefinitionKind::Set
                ) =>
            {
                Some(method)
            }
            _ => None,
        }) {
            let Some(member) =
                Member::of_key(self.source, &method.key, method.computed, method.span)
            else {
                continue;
            };
            let key = (member.name.clone(), method.r#static);
            match first.remove(&key) {
                Some(holder) => self.overloads.merge(vec![member], Some(holder)),
                None => {
                    first.insert(key, member);
                }
            }
        }
    }
}

// -----------------------------------------------------------------------------
// Declarations
// -----------------------------------------------------------------------------
//
// The walker visits every node. A declaration hook adds an entity, then walks
// its children in a new scope. Every other node keeps the current scope.

impl<'a> Visit<'a> for SyntaxWalker<'_> {
    fn visit_statements(&mut self, statements: &ArenaVec<'a, Statement<'a>>) {
        let members = statements
            .iter()
            .filter_map(statement_function)
            .filter_map(Member::of_function)
            .collect();
        self.overloads.plan(members);
        walk::walk_statements(self, statements);
    }

    fn visit_function(&mut self, function: &Function<'a>, flags: ScopeFlags) {
        let Some(id) = function.id.as_ref().filter(|_| {
            matches!(
                function.r#type,
                FunctionType::FunctionDeclaration | FunctionType::TSDeclareFunction
            )
        }) else {
            // A function expression has its own `this`, so it adds no instance fields.
            let owner = self.body_owner();
            return self.enter(
                Scope::Callable {
                    node: owner,
                    instance: None,
                },
                |walker| walk::walk_function(walker, function, flags),
            );
        };
        let callable = CallableSyntax {
            name: id.name.to_string(),
            declaration: id.span,
            kind: CallableKind::Function,
            scope: function.span,
            header_start: function.span.start,
            keyword: None,
            documented_from: function.span.start,
            parameters: &function.params,
            return_type: function.return_type.as_deref(),
            body: function.body.as_ref().map(|body| body.span),
        };
        if let Some(node) = self.add_callable(callable) {
            self.enter(
                Scope::Callable {
                    node,
                    instance: None,
                },
                |walker| {
                    walk::walk_function(walker, function, flags);
                },
            );
        }
    }

    fn visit_variable_declaration(&mut self, declaration: &VariableDeclaration<'a>) {
        let outer = self.binding.replace(Binding {
            keyword: declaration.kind.as_str(),
            start: declaration.span.start,
            first_declarator: declaration
                .declarations
                .first()
                .map_or(declaration.span.start, |declarator| declarator.span.start),
        });
        walk::walk_variable_declaration(self, declaration);
        self.binding = outer;
    }

    fn visit_variable_declarator(&mut self, declarator: &VariableDeclarator<'a>) {
        let (BindingPattern::BindingIdentifier(id), Some(function), Some(binding)) = (
            &declarator.id,
            declarator.init.as_ref().and_then(BoundFunction::of),
            self.binding,
        ) else {
            return walk::walk_variable_declarator(self, declarator);
        };
        // An arrow function keeps the `this` of its enclosing callable.
        let instance = match function {
            BoundFunction::Arrow(_) => self.this_class(),
            BoundFunction::Function(_) => None,
        };
        let callable = CallableSyntax {
            name: id.name.to_string(),
            declaration: id.span,
            kind: CallableKind::Function,
            scope: declarator.span,
            header_start: declarator.span.start,
            keyword: Some(binding.keyword),
            documented_from: if declarator.span.start == binding.first_declarator {
                binding.start
            } else {
                declarator.span.start
            },
            parameters: function.parameters(),
            return_type: function.return_type(),
            body: function.body(),
        };
        if let Some(node) = self.add_callable(callable) {
            self.enter(Scope::Callable { node, instance }, |walker| {
                function.walk(walker);
            });
        }
    }

    fn visit_class(&mut self, class: &Class<'a>) {
        let Some(id) = class
            .id
            .as_ref()
            .filter(|_| matches!(class.r#type, ClassType::ClassDeclaration))
        else {
            // The members of a class expression join no enclosing class.
            let owner = self.body_owner();
            return self.enter(
                Scope::Callable {
                    node: owner,
                    instance: None,
                },
                |walker| walk::walk_class(walker, class),
            );
        };
        let node = self.add_type(
            id.name.as_str(),
            id.span,
            TypeKind::Class,
            class.span,
            class.body.span,
        );
        self.enter(Scope::Type { node, class: true }, |walker| {
            walk::walk_class(walker, class);
        });
    }

    fn visit_class_body(&mut self, body: &ClassBody<'a>) {
        let members = body
            .body
            .iter()
            .filter_map(|element| match element {
                ClassElement::MethodDefinition(method) => {
                    Member::of_key(self.source, &method.key, method.computed, method.span)
                        .map(|member| member.with_body(method.value.body.is_some()))
                }
                _ => None,
            })
            .collect();
        self.overloads.plan(members);
        self.merge_accessor_pairs(body);
        walk::walk_class_body(self, body);
    }

    fn visit_static_block(&mut self, block: &StaticBlock<'a>) {
        let owner = self.body_owner();
        self.enter(
            Scope::Callable {
                node: owner,
                instance: None,
            },
            |walker| walk::walk_static_block(walker, block),
        );
    }

    fn visit_method_definition(&mut self, method: &MethodDefinition<'a>) {
        let (Some(class), Some((name, declaration))) = (
            self.class_scope(),
            property_name(self.source, &method.key, method.computed),
        ) else {
            return walk::walk_method_definition(self, method);
        };
        let kind = if matches!(method.kind, MethodDefinitionKind::Constructor) {
            self.add_parameter_properties(&class, &method.value.params);
            CallableKind::Constructor
        } else {
            CallableKind::Method
        };
        let callable = CallableSyntax {
            name,
            declaration,
            kind,
            scope: method.span,
            header_start: method
                .decorators
                .last()
                .map_or(method.span.start, |decorator| decorator.span.end),
            keyword: None,
            documented_from: method.span.start,
            parameters: &method.value.params,
            return_type: method.value.return_type.as_deref(),
            body: method.value.body.as_ref().map(|body| body.span),
        };
        let instance = (!method.r#static).then_some(class.clone());
        // A merged accessor still walks its body, so its `this` fields count.
        let node = self.add_callable(callable).unwrap_or(class);
        self.enter(Scope::Callable { node, instance }, |walker| {
            walk::walk_function(walker, &method.value, ScopeFlags::Function);
        });
    }

    fn visit_property_definition(&mut self, property: &PropertyDefinition<'a>) {
        let (Some(class), Some((name, declaration))) = (
            self.class_scope(),
            property_name(self.source, &property.key, property.computed),
        ) else {
            return walk::walk_property_definition(self, property);
        };
        let Some(function) = property.value.as_ref().and_then(BoundFunction::of) else {
            self.builder.add_or_extend_field(
                &class,
                &name,
                self.path,
                signature::type_text(self.source, property.type_annotation.as_deref()),
                range(declaration),
            );
            // An initializer runs with the instance as `this`, but declares nothing on the class.
            let instance = (!property.r#static).then_some(class);
            let owner = self.body_owner();
            return self.enter(
                Scope::Callable {
                    node: owner,
                    instance,
                },
                |walker| {
                    walk::walk_property_definition(walker, property);
                },
            );
        };
        let callable = CallableSyntax {
            name,
            declaration,
            kind: CallableKind::Method,
            scope: property.span,
            header_start: property
                .decorators
                .last()
                .map_or(property.span.start, |decorator| decorator.span.end),
            keyword: None,
            documented_from: property.span.start,
            parameters: function.parameters(),
            return_type: function.return_type(),
            body: function.body(),
        };
        let instance = (!property.r#static).then_some(class);
        if let Some(node) = self.add_callable(callable) {
            self.enter(Scope::Callable { node, instance }, |walker| {
                function.walk(walker);
            });
        }
    }

    fn visit_accessor_property(&mut self, property: &AccessorProperty<'a>) {
        if let (Some(class), Some((name, declaration))) = (
            self.class_scope(),
            property_name(self.source, &property.key, property.computed),
        ) {
            self.builder.add_or_extend_field(
                &class,
                &name,
                self.path,
                signature::type_text(self.source, property.type_annotation.as_deref()),
                range(declaration),
            );
        }
        walk::walk_accessor_property(self, property);
    }

    /// Adds one interface with its property fields and method signatures.
    fn visit_ts_interface_declaration(&mut self, interface: &TSInterfaceDeclaration<'a>) {
        let node = self.add_type(
            interface.id.name.as_str(),
            interface.id.span,
            TypeKind::Trait,
            interface.span,
            interface.body.span,
        );
        let members = interface
            .body
            .body
            .iter()
            .filter_map(|signature| match signature {
                TSSignature::TSMethodSignature(method) => {
                    Member::of_key(self.source, &method.key, method.computed, method.span)
                }
                _ => None,
            })
            .collect();
        self.overloads.plan(members);

        self.enter(
            Scope::Type {
                node: node.clone(),
                class: false,
            },
            |walker| {
                for signature in &interface.body.body {
                    walker.add_interface_member(&node, signature);
                }
            },
        );
    }

    /// Adds one enum. Its members are values, not fields.
    fn visit_ts_enum_declaration(&mut self, declaration: &TSEnumDeclaration<'a>) {
        self.add_type(
            declaration.id.name.as_str(),
            declaration.id.span,
            TypeKind::Enum,
            declaration.span,
            declaration.body.span,
        );
    }

    /// Adds one namespace as a module inside its owner.
    ///
    /// `namespace A.B {}` adds `B` inside `A`.
    fn visit_ts_namespace_declaration(&mut self, namespace: &TSNamespaceDeclaration<'a>) {
        let owner = self.owner().clone();
        let node = self.builder.add_entity(
            &owner,
            namespace.id.name.to_string(),
            EntityKind::Module,
            EntityEvidence {
                path: self.path.to_owned(),
                declarations: vec![range(namespace.id.span)],
                scope: Some(range(namespace.span)),
                body: Some(range(namespace.body.span())),
                type_inputs: Vec::new(),
                type_outputs: Vec::new(),
            },
        );
        self.builder
            .add_relation(&owner, &node, RelationKind::Contains);
        self.enter(Scope::Module(node), |walker| {
            walk::walk_ts_namespace_declaration(walker, namespace);
        });
    }

    // -------------------------------------------------------------------------
    // Fields
    // -------------------------------------------------------------------------

    /// Adds a field for `this.name = value` in a callable whose `this` is an instance.
    fn visit_assignment_expression(&mut self, assignment: &AssignmentExpression<'a>) {
        if let AssignmentTarget::StaticMemberExpression(member) = &assignment.left
            && matches!(member.object, Expression::ThisExpression(_))
            && let Some(class) = self.this_class()
        {
            self.builder.add_or_extend_field(
                &class,
                member.property.name.as_str(),
                self.path,
                None,
                range(member.property.span),
            );
        }
        walk::walk_assignment_expression(self, assignment);
    }

    // -------------------------------------------------------------------------
    // Imports
    // -------------------------------------------------------------------------
    //
    // Each import or re-export statement becomes one import site. The language
    // server resolves each name later. A statement without names, a namespace
    // import, and `export *` query the module specifier.

    fn visit_import_declaration(&mut self, import: &ImportDeclaration<'a>) {
        let names = match &import.specifiers {
            Some(specifiers) if !specifiers.is_empty() => specifiers
                .iter()
                .map(|specifier| match specifier {
                    ImportDeclarationSpecifier::ImportSpecifier(specifier) => {
                        range(specifier.imported.span())
                    }
                    ImportDeclarationSpecifier::ImportDefaultSpecifier(specifier) => {
                        range(specifier.local.span)
                    }
                    ImportDeclarationSpecifier::ImportNamespaceSpecifier(_) => {
                        module_specifier(&import.source)
                    }
                })
                .collect(),
            _ => vec![module_specifier(&import.source)],
        };
        self.add_import_site(import.span, names);
    }

    fn visit_export_from_declaration(&mut self, export: &ExportFromDeclaration<'a>) {
        let names = if export.specifiers.is_empty() {
            vec![module_specifier(&export.source)]
        } else {
            export
                .specifiers
                .iter()
                .map(|specifier| range(specifier.local.span()))
                .collect()
        };
        self.add_import_site(export.span, names);
    }

    fn visit_export_all_declaration(&mut self, export: &ExportAllDeclaration<'a>) {
        self.add_import_site(export.span, vec![module_specifier(&export.source)]);
    }
}

impl SyntaxWalker<'_> {
    /// Adds the constructor parameters that declare fields, such as `private store: Store`.
    fn add_parameter_properties(&mut self, class: &EntityKey, parameters: &FormalParameters<'_>) {
        for parameter in &parameters.items {
            let declares_field =
                parameter.accessibility.is_some() || parameter.readonly || parameter.r#override;
            if let (true, BindingPattern::BindingIdentifier(id)) =
                (declares_field, &parameter.pattern)
            {
                self.builder.add_or_extend_field(
                    class,
                    id.name.as_str(),
                    self.path,
                    signature::type_text(self.source, parameter.type_annotation.as_deref()),
                    range(id.span),
                );
            }
        }
    }

    /// Adds one interface property as a field, or one method signature as a callable.
    fn add_interface_member(&mut self, interface: &EntityKey, member: &TSSignature<'_>) {
        match member {
            TSSignature::TSPropertySignature(property) => {
                if let Some((name, declaration)) =
                    property_name(self.source, &property.key, property.computed)
                {
                    self.builder.add_or_extend_field(
                        interface,
                        &name,
                        self.path,
                        signature::type_text(self.source, property.type_annotation.as_deref()),
                        range(declaration),
                    );
                }
            }
            TSSignature::TSMethodSignature(method) => {
                if let Some((name, declaration)) =
                    property_name(self.source, &method.key, method.computed)
                {
                    self.add_callable(CallableSyntax {
                        name,
                        declaration,
                        kind: CallableKind::Method,
                        scope: method.span,
                        header_start: method.span.start,
                        keyword: None,
                        documented_from: method.span.start,
                        parameters: &method.params,
                        return_type: method.return_type.as_deref(),
                        body: None,
                    });
                }
            }
            _ => {}
        }
    }

    fn add_import_site(&mut self, statement: Span, names: Vec<SourceRange>) {
        let owner = self.owner().clone();
        self.builder.add_import_site(
            &owner,
            SourceSpan {
                path: self.path.to_owned(),
                range: range(statement),
            },
            names,
        );
    }

    // -------------------------------------------------------------------------
    // Entity Facts
    // -------------------------------------------------------------------------

    /// Adds one class, interface, or enum inside the current owner.
    fn add_type(
        &mut self,
        name: &str,
        declaration: Span,
        kind: TypeKind,
        scope: Span,
        body: Span,
    ) -> EntityKey {
        let owner = self.owner().clone();
        let node = self.builder.add_entity(
            &owner,
            name.to_owned(),
            EntityKind::Type(TypeData {
                kind,
                documentation: signature::documentation(self.source, self.comments, scope.start),
            }),
            EntityEvidence {
                path: self.path.to_owned(),
                declarations: vec![range(declaration)],
                scope: Some(range(scope)),
                body: Some(range(body)),
                type_inputs: Vec::new(),
                type_outputs: Vec::new(),
            },
        );
        self.builder
            .add_relation(&owner, &node, RelationKind::Contains);
        node
    }

    /// Adds one callable inside the current owner.
    ///
    /// Returns `None` for an overload signature, because the callable of its
    /// group holds its declaration.
    fn add_callable(&mut self, callable: CallableSyntax<'_, '_>) -> Option<EntityKey> {
        let mut declarations = vec![range(callable.declaration)];
        let mut scope = range(callable.scope);
        let mut documented_from = callable.documented_from;
        match self.overloads.role(callable.scope.start) {
            Role::Signature => return None,
            Role::Holder(group) => {
                declarations.clone_from(&group.declarations);
                scope.start = scope.start.min(group.start);
                scope.end = scope.end.max(group.end);
                documented_from = documented_from.min(group.start);
            }
            Role::Alone => {}
        }
        let (type_inputs, type_outputs) =
            signature::annotations(callable.parameters, callable.return_type);
        let data = CallableData {
            signature: Some(signature::signature(self.source, &callable)),
            parameters: signature::parameters(self.source, callable.parameters),
            return_type: signature::type_text(self.source, callable.return_type),
            documentation: signature::documentation(self.source, self.comments, documented_from),
            kind: callable.kind,
        };
        let owner = self.owner().clone();
        let node = self.builder.add_entity(
            &owner,
            callable.name,
            EntityKind::Callable(data),
            EntityEvidence {
                path: self.path.to_owned(),
                declarations,
                scope: Some(scope),
                body: callable.body.map(range),
                type_inputs,
                type_outputs,
            },
        );
        self.builder
            .add_relation(&owner, &node, RelationKind::Contains);
        Some(node)
    }
}

// -----------------------------------------------------------------------------
// Overloads
// -----------------------------------------------------------------------------
//
// TypeScript declares an overloaded function as bodiless signatures followed
// by one implementation. The language server can report any of them, so one
// callable holds every declaration of its group.

/// One function or method in a list of siblings.
struct Member {
    name: String,
    scope: Span,
    declaration: Span,
    has_body: bool,
}

impl Member {
    fn of_function(function: &Function<'_>) -> Option<Self> {
        function.id.as_ref().map(|id| Self {
            name: id.name.to_string(),
            scope: function.span,
            declaration: id.span,
            has_body: function.body.is_some(),
        })
    }

    fn of_key(source: &str, key: &PropertyKey<'_>, computed: bool, scope: Span) -> Option<Self> {
        property_name(source, key, computed).map(|(name, declaration)| Self {
            name,
            scope,
            declaration,
            has_body: false,
        })
    }

    fn with_body(self, has_body: bool) -> Self {
        Self { has_body, ..self }
    }
}

/// The declarations and source extent of one overload group.
struct Group {
    start: u32,
    end: u32,
    declarations: Vec<SourceRange>,
}

enum Role<'g> {
    /// The member has no overloads.
    Alone,
    /// The member is a signature that another member holds.
    Signature,
    /// The member holds the declarations of its group.
    Holder(&'g Group),
}

/// The overload groups of one document, by the scope start of each member.
#[derive(Default)]
struct Overloads {
    holders: HashMap<u32, Group>,
    signatures: HashSet<u32>,
}

impl Overloads {
    /// Plans the overload groups of one list of siblings.
    ///
    /// A run of bodiless members with one name merges into the next member
    /// with that name and a body. A run without that member merges into its
    /// first signature.
    fn plan(&mut self, members: Vec<Member>) {
        let mut run: Vec<Member> = Vec::new();
        for member in members {
            if run.last().is_some_and(|last| last.name != member.name) {
                self.merge(std::mem::take(&mut run), None);
            }
            if member.has_body {
                self.merge(std::mem::take(&mut run), Some(member));
            } else {
                run.push(member);
            }
        }
        self.merge(run, None);
    }

    /// Records one group when it has more than one member.
    fn merge(&mut self, mut run: Vec<Member>, implementation: Option<Member>) {
        let holder = match implementation {
            Some(member) => member,
            None if run.len() > 1 => run.remove(0),
            None => return,
        };
        if run.is_empty() {
            return;
        }
        let mut declarations = run
            .iter()
            .chain([&holder])
            .map(|member| range(member.declaration))
            .collect::<Vec<_>>();
        declarations.sort_by_key(|declaration| declaration.start);
        let group = Group {
            start: run
                .iter()
                .map(|member| member.scope.start)
                .fold(holder.scope.start, u32::min),
            end: run
                .iter()
                .map(|member| member.scope.end)
                .fold(holder.scope.end, u32::max),
            declarations,
        };
        self.signatures
            .extend(run.iter().map(|member| member.scope.start));
        self.holders.insert(holder.scope.start, group);
    }

    fn role(&self, scope_start: u32) -> Role<'_> {
        if self.signatures.contains(&scope_start) {
            return Role::Signature;
        }
        self.holders
            .get(&scope_start)
            .map_or(Role::Alone, Role::Holder)
    }
}

/// Returns the function that one statement declares, also when it is exported.
fn statement_function<'s, 'a>(statement: &'s Statement<'a>) -> Option<&'s Function<'a>> {
    match statement {
        Statement::FunctionDeclaration(function) => Some(function),
        Statement::ExportDeclaration(export) => match &export.declaration {
            Declaration::FunctionDeclaration(function) => Some(function),
            _ => None,
        },
        Statement::ExportDefaultDeclaration(export) => match &export.declaration {
            ExportDefaultDeclarationKind::FunctionDeclaration(function) => Some(function),
            _ => None,
        },
        _ => None,
    }
}

// -----------------------------------------------------------------------------
// Source Text
// -----------------------------------------------------------------------------

pub(super) fn range(span: Span) -> SourceRange {
    SourceRange {
        start: span.start,
        end: span.end,
    }
}

/// Returns the text between the quotes of a module specifier.
fn module_specifier(literal: &StringLiteral<'_>) -> SourceRange {
    SourceRange {
        start: literal.span.start.saturating_add(1).min(literal.span.end),
        end: literal.span.end.saturating_sub(1).max(literal.span.start),
    }
}

/// Returns the name and the name span of one property key.
///
/// A computed key has a name only when it is a string or number literal.
fn property_name(source: &str, key: &PropertyKey<'_>, computed: bool) -> Option<(String, Span)> {
    match key {
        PropertyKey::StaticIdentifier(identifier) if !computed => {
            Some((identifier.name.to_string(), identifier.span))
        }
        PropertyKey::PrivateIdentifier(identifier) => {
            Some((format!("#{}", identifier.name), identifier.span))
        }
        PropertyKey::StringLiteral(literal) => Some((literal.value.to_string(), literal.span)),
        PropertyKey::NumericLiteral(literal) => {
            Some((literal.span.source_text(source).to_owned(), literal.span))
        }
        _ => None,
    }
}

// -----------------------------------------------------------------------------
// Tests
// -----------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use crate::analysis::{
        fact::{CallableKind, EntityFact, EntityKind, SyntaxFacts, TypeKind},
        source::{DocumentSnapshot, SourceRange, Sources},
    };

    use super::{produce_syntax_facts, select_sources};

    const APP: &str = concat!(
        "import { helper as help, other } from \"./b\";\n",
        "import Default from \"./c\";\n",
        "import * as ns from \"./d\";\n",
        "import \"./side\";\n",
        "export { thing } from \"./e\";\n",
        "export * from \"./f\";\n",
        "\n",
        "/** Adds one. */\n",
        "export function add(a: number, b?: Value, ...rest: Item[]): Result {\n",
        "  const inner = (x: Item) => help(x);\n",
        "  [1].map((y) => inner(y));\n",
        "  return a + 1;\n",
        "}\n",
        "\n",
        "/** Picks one. */\n",
        "export function over(a: string): string;\n",
        "export function over(a: number): number;\n",
        "export function over(a: unknown): unknown {\n",
        "  return a;\n",
        "}\n",
        "\n",
        "export const twice = async (value: number): Promise<number> => value * 2;\n",
        "\n",
        "/**\n",
        " * A box.\n",
        " */\n",
        "export class Box<T> extends Base implements Shape {\n",
        "  #secret = 1;\n",
        "  label: string;\n",
        "  static count = 0;\n",
        "  handler = (event: Event) => this.run(event);\n",
        "\n",
        "  constructor(private readonly store: Store, public size = 1) {\n",
        "    super();\n",
        "    this.extra = store;\n",
        "  }\n",
        "\n",
        "  @log()\n",
        "  run(event: Event): void {}\n",
        "\n",
        "  get value(): T { return this.inner; }\n",
        "  set value(next: T) { this.inner = next; }\n",
        "}\n",
        "\n",
        "interface Shape {\n",
        "  area: number;\n",
        "  scale(by: number): Shape;\n",
        "  scale(by: string): Shape;\n",
        "}\n",
        "\n",
        "enum Color { Red, Green }\n",
        "\n",
        "namespace Outer.Inner {\n",
        "  export function nested() {}\n",
        "}\n",
        "\n",
        "const anonymous = class { method() {} };\n",
        "export default () => anonymous;\n",
    );

    #[test]
    fn reports_named_callables_types_and_fields_and_folds_anonymous_functions() {
        let facts = facts("app.ts", APP);

        assert_eq!(
            facts
                .entities
                .iter()
                .map(|entity| entity.key.as_str())
                .collect::<Vec<_>>(),
            [
                "app.ts",
                "app.ts::callable:add",
                "app.ts::callable:add::callable:inner",
                "app.ts::callable:over",
                "app.ts::callable:twice",
                "app.ts::type:Box",
                "app.ts::type:Box::field:#secret",
                "app.ts::type:Box::field:label",
                "app.ts::type:Box::field:count",
                "app.ts::type:Box::callable:handler",
                "app.ts::type:Box::field:store",
                "app.ts::type:Box::field:size",
                "app.ts::type:Box::callable:constructor",
                "app.ts::type:Box::field:extra",
                "app.ts::type:Box::callable:run",
                "app.ts::type:Box::callable:value",
                "app.ts::type:Box::field:inner",
                "app.ts::type:Shape",
                "app.ts::type:Shape::field:area",
                "app.ts::type:Shape::callable:scale",
                "app.ts::type:Color",
                "app.ts::module:Outer",
                "app.ts::module:Outer::module:Inner",
                "app.ts::module:Outer::module:Inner::callable:nested",
            ]
        );
        assert!(matches!(
            entity(&facts, "app.ts::type:Box::callable:constructor").kind,
            EntityKind::Callable(ref data) if data.kind == CallableKind::Constructor
        ));
        assert!(matches!(
            entity(&facts, "app.ts::type:Box::callable:handler").kind,
            EntityKind::Callable(ref data) if data.kind == CallableKind::Method
        ));
        assert!(matches!(
            entity(&facts, "app.ts::type:Shape").kind,
            EntityKind::Type(ref data) if data.kind == TypeKind::Trait
        ));
        assert!(matches!(
            entity(&facts, "app.ts::type:Color").kind,
            EntityKind::Type(ref data) if data.kind == TypeKind::Enum
        ));
        assert_eq!(
            facts.relations.len(),
            facts.entities.len() - 1,
            "every entity but the file module has one owner"
        );
    }

    #[test]
    fn reads_callable_evidence_signatures_and_documentation() {
        let facts = facts("app.ts", APP);

        let add = entity(&facts, "app.ts::callable:add");
        let EntityKind::Callable(data) = &add.kind else {
            panic!("add must be a callable");
        };
        assert_eq!(texts(&add.evidence.declarations), ["add"]);
        assert!(text(scope(add)).starts_with("function add("));
        assert!(text(body(add)).starts_with("{\n  const inner"));
        assert_eq!(
            texts(&add.evidence.type_inputs),
            ["number", "Value", "Item[]"]
        );
        assert_eq!(texts(&add.evidence.type_outputs), ["Result"]);
        assert_eq!(
            data.signature.as_deref(),
            Some("function add(a: number, b?: Value, ...rest: Item[]): Result")
        );
        assert_eq!(
            data.parameters,
            ["a: number", "b?: Value", "...rest: Item[]"]
        );
        assert_eq!(data.return_type.as_deref(), Some("Result"));
        assert_eq!(data.documentation.as_deref(), Some("Adds one."));

        let twice = entity(&facts, "app.ts::callable:twice");
        let EntityKind::Callable(data) = &twice.kind else {
            panic!("twice must be a callable");
        };
        assert_eq!(
            data.signature.as_deref(),
            Some("const twice = async (value: number): Promise<number>")
        );
        assert_eq!(text(body(twice)), "value * 2");

        let run = entity(&facts, "app.ts::type:Box::callable:run");
        let EntityKind::Callable(data) = &run.kind else {
            panic!("run must be a callable");
        };
        assert_eq!(data.signature.as_deref(), Some("run(event: Event): void"));

        let class = entity(&facts, "app.ts::type:Box");
        assert!(matches!(
            &class.kind,
            EntityKind::Type(data) if data.documentation.as_deref() == Some("A box.")
        ));
        assert_eq!(
            texts(
                &entity(&facts, "app.ts::type:Box::field:store")
                    .evidence
                    .declarations
            ),
            ["store"]
        );
    }

    #[test]
    fn merges_overload_signatures_into_one_callable() {
        let facts = facts("app.ts", APP);

        let over = entity(&facts, "app.ts::callable:over");
        assert_eq!(texts(&over.evidence.declarations), ["over", "over", "over"]);
        assert!(text(scope(over)).starts_with("function over(a: string): string;"));
        assert!(matches!(
            &over.kind,
            EntityKind::Callable(data)
                if data.signature.as_deref() == Some("function over(a: unknown): unknown")
                    && data.documentation.as_deref() == Some("Picks one.")
        ));

        let scale = entity(&facts, "app.ts::type:Shape::callable:scale");
        assert_eq!(texts(&scale.evidence.declarations), ["scale", "scale"]);
        assert_eq!(scale.evidence.body, None);
    }

    #[test]
    fn records_import_sites_with_the_names_to_query() {
        let facts = facts("app.ts", APP);

        assert_eq!(
            facts
                .import_sites
                .iter()
                .map(|site| (
                    site.owner.as_str(),
                    text(site.statement.range),
                    texts(&site.names)
                ))
                .collect::<Vec<_>>(),
            [
                (
                    "app.ts",
                    "import { helper as help, other } from \"./b\";",
                    vec!["helper", "other"]
                ),
                ("app.ts", "import Default from \"./c\";", vec!["Default"]),
                ("app.ts", "import * as ns from \"./d\";", vec!["./d"]),
                ("app.ts", "import \"./side\";", vec!["./side"]),
                ("app.ts", "export { thing } from \"./e\";", vec!["thing"]),
                ("app.ts", "export * from \"./f\";", vec!["./f"]),
            ]
        );
    }

    #[test]
    fn reads_javascript_classes_functions_and_instance_fields() {
        let source = concat!(
            "const { helper } = require(\"./b\");\n",
            "function legacy(v) { return helper(v); }\n",
            "class View {\n",
            "  constructor(el) { this.el = el; }\n",
            "  render = () => <div>{this.el}</div>;\n",
            "}\n",
            "module.exports = { legacy, View };\n",
        );
        let facts = facts("lib/view.js", source);

        assert_eq!(
            facts
                .entities
                .iter()
                .map(|entity| entity.key.as_str())
                .collect::<Vec<_>>(),
            [
                "lib/view.js",
                "lib/view.js::callable:legacy",
                "lib/view.js::type:View",
                "lib/view.js::type:View::callable:constructor",
                "lib/view.js::type:View::field:el",
                "lib/view.js::type:View::callable:render",
            ]
        );
        assert!(facts.import_sites.is_empty());
    }

    #[test]
    fn selects_sources_without_declaration_files_or_build_output() {
        let documents = [
            "index.d.ts",
            "index.ts",
            "types.d.mts",
            "app.mjs",
            "notes.md",
            "dist/app.js",
            "web/build/x.ts",
            "build.ts",
        ]
        .map(|path| DocumentSnapshot {
            path: PathBuf::from(path),
            content: String::new(),
        });
        let extensions = ["ts", "mts", "mjs"].map(str::to_owned);

        let sources = select_sources(&documents, &extensions).expect("the paths are valid");

        assert_eq!(
            sources
                .iter()
                .map(|document| document.path.to_string_lossy().into_owned())
                .collect::<Vec<_>>(),
            ["index.ts", "app.mjs", "build.ts"]
        );
    }

    #[test]
    fn keeps_only_the_module_of_a_source_with_a_syntax_error() {
        let facts = facts("broken.ts", "function (\n");

        assert_eq!(
            facts
                .entities
                .iter()
                .map(|entity| entity.key.as_str())
                .collect::<Vec<_>>(),
            ["broken.ts"]
        );
    }

    #[test]
    fn keeps_class_expression_members_and_callback_fields_out_of_the_enclosing_class() {
        let source = concat!(
            "class A {\n",
            "  static B = class { run() { this.x = 1; } };\n",
            "  start() { setTimeout(function () { this.ready = true; }); }\n",
            "  later() { setTimeout(() => { this.done = true; }); }\n",
            "}\n",
            "if (!process.argv[2]) return;\n",
        );

        assert_eq!(
            facts("bin/a.js", source)
                .entities
                .iter()
                .map(|entity| entity.key.as_str())
                .collect::<Vec<_>>(),
            [
                "bin/a.js",
                "bin/a.js::type:A",
                "bin/a.js::type:A::field:B",
                "bin/a.js::type:A::callable:start",
                "bin/a.js::type:A::callable:later",
                "bin/a.js::type:A::field:done",
            ]
        );
    }

    #[test]
    fn keeps_static_blocks_and_initializers_off_the_class_and_merges_accessor_pairs() {
        let source = concat!(
            "class A {\n",
            "  static { const boot = () => go(); }\n",
            "  field = wrap(function () { const deep = () => 1; });\n",
            "  get value() { return this._v; }\n",
            "  set value(v) { this._v = v; }\n",
            "  static get value() { return 1; }\n",
            "}\n",
        );
        let facts = facts("a.ts", source);

        assert_eq!(
            facts
                .entities
                .iter()
                .map(|entity| entity.key.as_str())
                .collect::<Vec<_>>(),
            [
                "a.ts",
                "a.ts::type:A",
                "a.ts::callable:boot",
                "a.ts::type:A::field:field",
                "a.ts::callable:deep",
                "a.ts::type:A::callable:value",
                "a.ts::type:A::field:_v",
                "a.ts::type:A::callable:value[1]",
            ]
        );
        let value = entity(&facts, "a.ts::type:A::callable:value");
        assert_eq!(value.evidence.declarations.len(), 2);
    }

    fn facts(path: &str, source: &str) -> SyntaxFacts {
        let document = DocumentSnapshot {
            path: PathBuf::from(path),
            content: source.to_owned(),
        };
        produce_syntax_facts(&Sources::new(vec![&document])).expect("the fixture must parse")
    }

    fn entity<'f>(facts: &'f SyntaxFacts, key: &str) -> &'f EntityFact {
        facts
            .entities
            .iter()
            .find(|entity| entity.key.as_str() == key)
            .unwrap_or_else(|| panic!("{key} must exist"))
    }

    fn scope(entity: &EntityFact) -> SourceRange {
        entity.evidence.scope.expect("the entity must have a scope")
    }

    fn body(entity: &EntityFact) -> SourceRange {
        entity.evidence.body.expect("the entity must have a body")
    }

    /// Reads one range of `APP`.
    fn text(range: SourceRange) -> &'static str {
        &APP[range.start as usize..range.end as usize]
    }

    fn texts(ranges: &[SourceRange]) -> Vec<&'static str> {
        ranges.iter().map(|range| text(*range)).collect()
    }
}
