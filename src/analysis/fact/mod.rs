//! Fact Models
//!
//! `FactSet` contains codebase facts.
//!
//! `SyntaxFacts` come from parsing source. They define entities and
//! syntax relations with source evidence.
//!
//! `SemanticFact` values come from an LSP server. They report calls,
//! references, and type definitions with symbol anchors and source locations.
//!
//! The `graph` module builds the codebase graph from these facts.
//!
//! # Sections
//!
//! `Fact Set` holds one codebase report.
//! `Facts` defines syntax facts and semantic facts.
//! `Entities` defines entities and source evidence.
//! `Relations` defines relation kinds and observations.
//! `Identity` defines stable entity keys.
//! `Errors` defines analysis errors.

pub mod semantic;
pub mod syntax;

use std::{error::Error, fmt, sync::Arc};

use crate::analysis::{
    language::Language,
    source::{SourceRange, SourceSpan},
};

pub use semantic::{ReportedLocation, ResolutionFallback, SymbolAnchor};
pub use syntax::SyntaxFactBuilder;

// -----------------------------------------------------------------------------
// Fact Set
// -----------------------------------------------------------------------------

pub struct FactSet {
    pub language: Language,
    pub syntax: SyntaxFacts,
    pub semantic: Vec<SemanticFact>,
}

impl FactSet {
    #[must_use]
    pub fn new(language: Language, syntax: SyntaxFacts, semantic: Vec<SemanticFact>) -> Self {
        Self {
            language,
            syntax,
            semantic,
        }
    }
}

// -----------------------------------------------------------------------------
// Facts
// -----------------------------------------------------------------------------

#[derive(Default)]
pub struct SyntaxFacts {
    pub entities: Vec<EntityFact>,
    pub relations: Vec<RelationFact>,
}

pub struct EntityFact {
    pub key: EntityKey,
    pub name: String,
    pub kind: EntityKind,
    pub evidence: EntityEvidence,
}

pub struct RelationFact {
    pub source: EntityKey,
    pub target: EntityKey,
    pub kind: RelationKind,
    pub evidence: RelationEvidence,
}

/// One fact from an LSP server.
#[derive(Debug, Eq, PartialEq)]
pub enum SemanticFact {
    Call {
        caller: SymbolAnchor,
        callee: SymbolAnchor,
        call_sites: Vec<ReportedLocation>,
    },
    Reference {
        target: SymbolAnchor,
        occurrences: Vec<ReportedLocation>,
    },
    TypeDefinition {
        queried_symbol: SymbolAnchor,
        targets: Vec<ReportedLocation>,
    },
}

// -----------------------------------------------------------------------------
// Entities
// -----------------------------------------------------------------------------

#[derive(Eq, PartialEq)]
pub enum EntityKind {
    Module,
    Type(TypeData),
    Callable(CallableData),
    Field(FieldData),
}

impl EntityKind {
    /// Returns the kind name used in keys and artifacts.
    #[must_use]
    pub fn tag(&self) -> &'static str {
        match self {
            Self::Module => "module",
            Self::Type(_) => "type",
            Self::Callable(_) => "callable",
            Self::Field(_) => "field",
        }
    }
}

#[derive(Eq, PartialEq)]
pub struct TypeData {
    pub kind: TypeKind,
    pub documentation: Option<String>,
}

#[derive(Eq, PartialEq)]
pub enum TypeKind {
    Class,
    Struct,
    Enum,
    Interface,
    Trait,
}

#[derive(Eq, PartialEq)]
pub struct CallableData {
    pub kind: CallableKind,
    pub signature: Option<String>,
    pub parameters: Vec<String>,
    pub return_type: Option<String>,
    pub documentation: Option<String>,
}

#[derive(Eq, PartialEq)]
pub enum CallableKind {
    Function,
    Method,
    Constructor,
}

#[derive(Eq, PartialEq)]
pub struct FieldData {
    pub declared_type: Option<String>,
}

/// Source ranges used to match one entity.
///
/// Each range holds bytes in the file that `path` names.
///
/// ```text
/// def run(value: Request) -> Reply:
///     ^^^        ^^^^^^^     ^^^^^
///     |          |           +- type_outputs
///     |          +- type_inputs
///     +- declarations
///
///     return Reply()
///     ^^^^^^^^^^^^^^ body
/// ```
///
/// `scope` covers the complete definition. A parameter default is outside both
/// type lists, because it runs in the enclosing scope.
pub struct EntityEvidence {
    pub path: String,
    pub declarations: Vec<SourceRange>,
    pub scope: Option<SourceRange>,
    pub body: Option<SourceRange>,
    pub type_inputs: Vec<SourceRange>,
    pub type_outputs: Vec<SourceRange>,
}

// -----------------------------------------------------------------------------
// Relations
// -----------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum RelationKind {
    Contains,
    Calls,
    Imports,
    InputType,
    OutputType,
    UsesType,
    UsesValue,
}

pub struct RelationEvidence {
    pub observations: Vec<SourceSpan>,
}

impl RelationEvidence {
    /// Sorts observations and deduplicates them by path and start.
    pub fn normalize_observations(&mut self) {
        self.observations.sort_by(|left, right| {
            (&left.path, left.range.start, left.range.end).cmp(&(
                &right.path,
                right.range.start,
                right.range.end,
            ))
        });
        self.observations.dedup_by(|left, right| {
            left.path == right.path && left.range.start == right.range.start
        });
    }
}

// -----------------------------------------------------------------------------
// Identity
// -----------------------------------------------------------------------------

/// A stable entity identity across builds.
///
/// # Grammar
///
/// `key = path | key "::" segment`
///
/// A segment contains an entity kind, name, and optional sibling ordinal.
/// Entity kinds are `type`, `callable`, and `field`.
///
/// Examples are `pkg/api.py`, `pkg/api.py::type:Router`, and
/// `pkg/api.py::type:Router::callable:__init__[1]`.
///
/// The first sibling of one name keeps no ordinal. A later duplicate does not
/// change an existing key.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct EntityKey(Arc<str>);

impl EntityKey {
    /// Creates the root key for one module from its relative source path.
    #[must_use]
    pub fn module(path: &str) -> Self {
        Self(Arc::from(path))
    }

    /// Creates the key for one entity declared inside this entity.
    ///
    /// `ordinal` numbers the second and later sibling of one kind and name.
    #[must_use]
    pub(crate) fn child(&self, kind: &EntityKind, name: &str, ordinal: u32) -> Self {
        let tag = kind.tag();
        let key = if ordinal == 0 {
            format!("{self}::{tag}:{name}")
        } else {
            format!("{self}::{tag}:{name}[{ordinal}]")
        };
        Self(Arc::from(key))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for EntityKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

// -----------------------------------------------------------------------------
// Errors
// -----------------------------------------------------------------------------

#[derive(Debug)]
pub struct AnalysisError {
    pub message: String,
}

impl AnalysisError {
    #[must_use]
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for AnalysisError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl Error for AnalysisError {}
