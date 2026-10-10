//! Syntax graph index.
//!
//! Matches source positions to the nodes of one syntax graph: declarations,
//! bodies, scopes, signature annotations, constructors, and import sites.
//!
//! Position queries use one index to avoid a full node scan. Owner queries
//! select the narrowest range. Equal widths select the lowest ID.

use std::collections::HashMap;

use super::build::SyntaxGraph;
use crate::analysis::{
    fact::{CallableKind, EntityKind, RelationKind},
    graph::{Node, NodeId},
    language::Language,
    source::SourceRange,
};

/// Position of a type in a callable signature.
#[derive(Clone, Copy)]
pub(super) enum SignatureRole {
    Input,
    Output,
}

impl SignatureRole {
    /// Returns the endpoints and kind for a signature relation.
    pub(super) fn relation(
        self,
        callable: NodeId,
        stated: NodeId,
    ) -> (NodeId, NodeId, RelationKind) {
        match self {
            Self::Input => (stated, callable, RelationKind::InputType),
            Self::Output => (callable, stated, RelationKind::OutputType),
        }
    }
}

pub(super) struct SyntaxGraphIndex<'a> {
    graph: &'a SyntaxGraph,
    paths: HashMap<(Language, &'a str), PathRanges>,
    constructors: HashMap<NodeId, NodeId>,
    import_ranges: HashMap<(Language, &'a str), Vec<SourceRange>>,
}

#[derive(Default)]
struct PathRanges {
    declarations: Vec<(SourceRange, NodeId)>,
    bodies: Vec<(SourceRange, NodeId)>,
    scopes: Vec<(SourceRange, NodeId)>,
    callable_or_module_bodies: Vec<(SourceRange, NodeId)>,
    type_inputs: Vec<(SourceRange, NodeId)>,
    type_outputs: Vec<(SourceRange, NodeId)>,
}

impl<'a> SyntaxGraphIndex<'a> {
    pub(super) fn new(graph: &'a SyntaxGraph) -> Self {
        let mut paths = HashMap::<(Language, &str), PathRanges>::new();

        for (id, node) in graph.identified_nodes() {
            let ranges = paths.entry((node.language, node.path())).or_default();

            for declaration in node.declarations() {
                ranges.declarations.push((*declaration, id));
            }
            if let Some(scope) = node.scope() {
                ranges.scopes.push((scope, id));
            }
            if let Some(body) = node.body() {
                ranges.bodies.push((body, id));
                if matches!(&node.kind, EntityKind::Module | EntityKind::Callable(_)) {
                    ranges.callable_or_module_bodies.push((body, id));
                }
            }
            for annotation in node.type_inputs() {
                ranges.type_inputs.push((*annotation, id));
            }
            for annotation in node.type_outputs() {
                ranges.type_outputs.push((*annotation, id));
            }
        }

        Self {
            graph,
            paths,
            constructors: constructors_by_class(graph),
            import_ranges: import_ranges_by_path(graph),
        }
    }

    pub(super) fn node(&self, id: NodeId) -> Option<&Node> {
        self.graph.nodes.get(id.index())
    }

    /// Returns the node declared at one source byte offset.
    pub(super) fn declaration_at(
        &self,
        language: Language,
        path: &str,
        offset: u32,
    ) -> Option<NodeId> {
        self.narrowest(language, path, offset, |ranges| &ranges.declarations)
    }

    /// Returns the smallest node body that owns one source byte offset.
    pub(super) fn body_owner_at(
        &self,
        language: Language,
        path: &str,
        offset: u32,
    ) -> Option<NodeId> {
        self.narrowest(language, path, offset, |ranges| &ranges.bodies)
    }

    /// Returns the smallest lexical scope that owns one byte offset.
    pub(super) fn scope_owner_at(
        &self,
        language: Language,
        path: &str,
        offset: u32,
    ) -> Option<NodeId> {
        self.narrowest(language, path, offset, |ranges| &ranges.scopes)
    }

    /// Returns the smallest callable or module body at one byte offset.
    pub(super) fn callable_or_module_body_owner_at(
        &self,
        language: Language,
        path: &str,
        offset: u32,
    ) -> Option<NodeId> {
        self.narrowest(language, path, offset, |ranges| {
            &ranges.callable_or_module_bodies
        })
    }

    /// Returns the callable and role for an annotation.
    pub(super) fn signature_role_at(
        &self,
        language: Language,
        path: &str,
        offset: u32,
    ) -> Option<(NodeId, SignatureRole)> {
        if let Some(node) = self.narrowest(language, path, offset, |ranges| &ranges.type_inputs) {
            return Some((node, SignatureRole::Input));
        }

        self.narrowest(language, path, offset, |ranges| &ranges.type_outputs)
            .map(|node| (node, SignatureRole::Output))
    }

    /// Returns the last direct constructor of a class.
    pub(super) fn last_direct_constructor(&self, class: NodeId) -> Option<NodeId> {
        self.constructors.get(&class).copied()
    }

    /// Returns `true` when an import observation contains one byte offset.
    pub(super) fn is_import_position(&self, language: Language, path: &str, offset: u32) -> bool {
        self.import_ranges
            .get(&(language, path))
            .is_some_and(|ranges| ranges.iter().any(|range| contains(*range, offset)))
    }

    /// Returns the narrowest range that contains a byte offset.
    fn narrowest(
        &self,
        language: Language,
        path: &str,
        offset: u32,
        select: impl Fn(&PathRanges) -> &Vec<(SourceRange, NodeId)>,
    ) -> Option<NodeId> {
        select(self.paths.get(&(language, path))?)
            .iter()
            .filter(|(range, _)| contains(*range, offset))
            .min_by_key(|(range, id)| (range_width(*range), id.0))
            .map(|(_, id)| *id)
    }
}

/// Indexes the last direct constructor of each class.
fn constructors_by_class(graph: &SyntaxGraph) -> HashMap<NodeId, NodeId> {
    let mut constructors = HashMap::<NodeId, NodeId>::new();

    for edge in &graph.syntax_edges {
        if edge.kind != RelationKind::Contains {
            continue;
        }
        let Some(node) = graph.nodes.get(edge.target.index()) else {
            continue;
        };
        if !matches!(
            &node.kind,
            EntityKind::Callable(data) if matches!(data.kind, CallableKind::Constructor)
        ) {
            continue;
        }
        constructors
            .entry(edge.source)
            .and_modify(|last| *last = (*last).max(edge.target))
            .or_insert(edge.target);
    }

    constructors
}

/// Returns every import observation range, grouped by source path.
fn import_ranges_by_path(graph: &SyntaxGraph) -> HashMap<(Language, &str), Vec<SourceRange>> {
    let mut import_ranges = HashMap::<(Language, &str), Vec<SourceRange>>::new();

    for edge in &graph.syntax_edges {
        if edge.kind != RelationKind::Imports {
            continue;
        }
        let Some(source) = graph.nodes.get(edge.source.index()) else {
            continue;
        };
        for observation in &edge.provenance.evidence.observations {
            import_ranges
                .entry((source.language, observation.path.as_str()))
                .or_default()
                .push(observation.range);
        }
    }

    import_ranges
}

fn contains(range: SourceRange, offset: u32) -> bool {
    range.start <= offset && offset < range.end
}

fn range_width(range: SourceRange) -> u32 {
    range.end.saturating_sub(range.start)
}

#[cfg(test)]
mod tests {
    use super::SyntaxGraphIndex;
    use crate::analysis::{
        fact::EntityKind,
        graph::{
            NodeId,
            build::SyntaxGraph,
            fixtures::{function, node},
        },
        language::Language,
    };

    #[test]
    fn finds_exact_declarations_and_deepest_body_owners() {
        let graph = SyntaxGraph {
            nodes: vec![
                node(0, "app.py", EntityKind::Module, Vec::new(), Some((0, 100))),
                node(1, "app.py", function(), vec![(10, 15)], Some((20, 90))),
                node(2, "app.py", function(), vec![(30, 35)], Some((40, 80))),
            ],
            syntax_edges: Vec::new(),
        };
        let lookup = SyntaxGraphIndex::new(&graph);

        assert_eq!(
            lookup.declaration_at(Language::Python, "app.py", 32),
            Some(NodeId(2))
        );
        assert_eq!(
            lookup.body_owner_at(Language::Python, "app.py", 50),
            Some(NodeId(2))
        );
        assert_eq!(
            lookup.body_owner_at(Language::Python, "app.py", 32),
            Some(NodeId(1))
        );
        assert_eq!(
            lookup.scope_owner_at(Language::Python, "app.py", 12),
            Some(NodeId(1))
        );
        assert_eq!(
            lookup.callable_or_module_body_owner_at(Language::Python, "app.py", 12),
            Some(NodeId(0))
        );
        assert_eq!(lookup.body_owner_at(Language::Python, "other.py", 50), None);
    }
}
