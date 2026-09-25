//! Graph artifact.
//!
//! This module owns the JSON document of one indexed graph: its schema, its
//! conversion from the analysis graph, its validation, and its reads and
//! writes in the store. An analysis graph is the input. One JSON document is the
//! output.
//!
//! # Determinism
//!
//! Equal graphs produce byte-identical documents. The writer preserves list
//! order and adds no timestamp or machine data. Node and edge order follow graph
//! order. A content hash derives from source text only. Schema declarations
//! define JSON field names and field order.
//!
//! # Compatibility
//!
//! The reader ignores unknown fields. Older schema 1 documents that hold a
//! `clustering` or a `comparison` field stay readable.
//!
//! # Sections
//!
//! `Schema` defines the document shape.
//! `Conversion` maps the analysis graph onto that shape.
//! `Reading and writing` validates, reads, and writes one document.

use std::{collections::BTreeSet, fs, io, path::Path};

use serde::{Deserialize, Serialize};

use super::{digest_of, lock_store, reject_symlink, write_json};
use crate::analysis::{
    fact::{EntityKind, RelationKind},
    graph::{Edge, Graph, Node, NodeId, ProvenanceOrigin},
    source::{SourceRange, SourceSpan},
};

// -----------------------------------------------------------------------------
// Schema
// -----------------------------------------------------------------------------

/// The graph wire-schema version.
///
/// An incompatible schema change requires a new version.
pub const SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GraphArtifact {
    pub schema_version: u32,
    pub graph: ArtifactGraph,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactGraph {
    pub languages: Vec<String>,
    pub nodes: Vec<ArtifactNode>,
    pub edges: Vec<ArtifactEdge>,
}

/// `id` identifies this node inside this document. `key` preserves the entity
/// key supplied by the graph.
///
/// `kind` selects callable, field, or type metadata. Serde omits absent metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtifactNode {
    pub id: u32,
    pub language: String,
    pub key: String,
    pub name: String,
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_hash: Option<String>,
    pub path: String,
    pub origin: String,
    pub declarations: Vec<ArtifactRange>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scope: Option<ArtifactRange>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body: Option<ArtifactRange>,
    /// Optional declaration metadata preserved when reading graph artifacts.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub position: Option<ArtifactPosition>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub parameters: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub return_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub declared_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub documentation: Option<String>,
}

/// `weight` and `observations` preserve the graph edge values.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactEdge {
    pub source: u32,
    pub target: u32,
    pub kind: String,
    pub weight: u32,
    pub origin: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub observations: Vec<ArtifactObservation>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactObservation {
    pub path: String,
    pub start: u32,
    pub end: u32,
}

/// One half-open UTF-8 byte range.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactRange {
    pub start: u32,
    pub end: u32,
}

/// One one-based line and column.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactPosition {
    pub line: u32,
    pub column: u32,
}

// -----------------------------------------------------------------------------
// Conversion
// -----------------------------------------------------------------------------

impl GraphArtifact {
    #[must_use]
    pub fn new(graph: &Graph) -> Self {
        let mut languages = graph
            .nodes
            .iter()
            .map(|node| node.language.as_str().to_owned())
            .collect::<Vec<_>>();
        languages.sort_unstable();
        languages.dedup();

        Self {
            schema_version: SCHEMA_VERSION,
            graph: ArtifactGraph {
                languages,
                nodes: graph
                    .identified_nodes()
                    .map(|(id, node)| artifact_node(id, node))
                    .collect(),
                edges: graph.edges.iter().map(artifact_edge).collect(),
            },
        }
    }
}

fn artifact_node(id: NodeId, node: &Node) -> ArtifactNode {
    let evidence = &node.provenance.evidence;
    let (signature, parameters, return_type, declared_type, documentation) = match &node.kind {
        // Emit no kind-specific metadata for modules.
        EntityKind::Module => (None, Vec::new(), None, None, None),
        // Preserve type documentation.
        EntityKind::Type(data) => (None, Vec::new(), None, None, data.documentation.clone()),
        // Preserve callable signature metadata.
        EntityKind::Callable(data) => (
            data.signature.clone(),
            data.parameters.clone(),
            data.return_type.clone(),
            None,
            data.documentation.clone(),
        ),
        // Preserve field type metadata.
        EntityKind::Field(data) => (None, Vec::new(), None, data.declared_type.clone(), None),
    };

    ArtifactNode {
        id: id.0,
        language: node.language.as_str().to_owned(),
        key: node.key.to_string(),
        name: node.name.clone(),
        kind: node.kind.tag().to_owned(),
        content_hash: node.content_hash.map(|hash| hash.to_string()),
        path: evidence.path.clone(),
        origin: origin_name(node.provenance.origin).to_owned(),
        declarations: evidence
            .declarations
            .iter()
            .copied()
            .map(artifact_range)
            .collect(),
        scope: evidence.scope.map(artifact_range),
        body: evidence.body.map(artifact_range),
        position: None,
        signature,
        parameters,
        return_type,
        declared_type,
        documentation,
    }
}

fn artifact_edge(edge: &Edge) -> ArtifactEdge {
    ArtifactEdge {
        source: edge.source.0,
        target: edge.target.0,
        kind: relation_kind_name(edge.kind).to_owned(),
        weight: edge.weight,
        origin: origin_name(edge.provenance.origin).to_owned(),
        observations: edge
            .provenance
            .evidence
            .observations
            .iter()
            .map(artifact_observation)
            .collect(),
    }
}

fn artifact_observation(span: &SourceSpan) -> ArtifactObservation {
    ArtifactObservation {
        path: span.path.clone(),
        start: span.range.start,
        end: span.range.end,
    }
}

fn artifact_range(range: SourceRange) -> ArtifactRange {
    ArtifactRange {
        start: range.start,
        end: range.end,
    }
}

fn relation_kind_name(kind: RelationKind) -> &'static str {
    match kind {
        // Encode containment.
        RelationKind::Contains => "contains",
        // Encode direct calls.
        RelationKind::Calls => "calls",
        // Encode imports.
        RelationKind::Imports => "imports",
        // Encode an accepted signature type.
        RelationKind::InputType => "input_type",
        // Encode a returned signature type.
        RelationKind::OutputType => "output_type",
        // Encode type use.
        RelationKind::UsesType => "uses_type",
        // Encode value use.
        RelationKind::UsesValue => "uses_value",
    }
}

fn origin_name(origin: ProvenanceOrigin) -> &'static str {
    match origin {
        ProvenanceOrigin::Syntax => "syntax",
        ProvenanceOrigin::Semantic => "semantic",
        ProvenanceOrigin::Derived => "derived",
    }
}

// -----------------------------------------------------------------------------
// Reading and writing
// -----------------------------------------------------------------------------

impl GraphArtifact {
    /// Reads one supported artifact and checks its graph references.
    ///
    /// # Errors
    /// Returns an error for invalid JSON, an unsupported schema, or invalid graph data.
    pub fn read(bytes: &[u8]) -> io::Result<Self> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Header {
            schema_version: u32,
        }
        let header: Header = serde_json::from_slice(bytes).map_err(io::Error::other)?;
        if header.schema_version != SCHEMA_VERSION {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "unsupported graph schema {}: expected {SCHEMA_VERSION}",
                    header.schema_version
                ),
            ));
        }
        let artifact: Self = serde_json::from_slice(bytes).map_err(io::Error::other)?;
        artifact.graph.validate()?;
        Ok(artifact)
    }

    /// Writes compact JSON followed by one newline, under the store lock.
    ///
    /// # Errors
    ///
    /// Returns an error if directory creation, serialization, or writing fails.
    pub fn write(&self, path: &Path) -> io::Result<()> {
        let directory = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        let _lock = lock_store(directory)?;
        write_json(path, self)
    }
}

impl ArtifactGraph {
    /// Checks node identities, edge endpoints, and source locations.
    ///
    /// # Errors
    /// Returns an error for duplicate identities or invalid graph data.
    pub fn validate(&self) -> io::Result<()> {
        let invalid = |message| io::Error::new(io::ErrorKind::InvalidData, message);
        let mut ids = BTreeSet::new();
        let mut keys = BTreeSet::new();
        let languages: BTreeSet<_> = self.languages.iter().collect();
        if languages.len() != self.languages.len()
            || languages.iter().any(|name| name.trim().is_empty())
        {
            return Err(invalid("invalid graph languages"));
        }
        for node in &self.nodes {
            if !ids.insert(node.id) || !keys.insert(&node.key) || node.key.trim().is_empty() {
                return Err(invalid("invalid or duplicate graph node identity"));
            }
            if !languages.contains(&node.language)
                || !matches!(node.kind.as_str(), "module" | "type" | "callable" | "field")
                || !matches!(node.origin.as_str(), "syntax" | "semantic" | "derived")
                || node
                    .declarations
                    .iter()
                    .chain(&node.scope)
                    .chain(&node.body)
                    .any(|range| range.start > range.end)
                || node
                    .position
                    .as_ref()
                    .is_some_and(|position| position.line == 0 || position.column == 0)
            {
                return Err(invalid("invalid graph node metadata"));
            }
        }
        for edge in &self.edges {
            if !ids.contains(&edge.source) || !ids.contains(&edge.target) {
                return Err(invalid("a graph edge names an unknown node"));
            }
            if !matches!(
                edge.kind.as_str(),
                "contains"
                    | "calls"
                    | "imports"
                    | "input_type"
                    | "output_type"
                    | "uses_type"
                    | "uses_value"
            ) || !matches!(edge.origin.as_str(), "syntax" | "semantic" | "derived")
                || edge.observations.iter().any(|span| span.start > span.end)
            {
                return Err(invalid("invalid graph edge metadata"));
            }
        }
        Ok(())
    }
}

/// Checks that a graph file name is one store entry that ends with `.json`.
///
/// # Errors
/// Returns `InvalidInput` for a path, a control character, outer whitespace,
/// another extension, or the reserved name `index.json`.
pub fn check_graph_name(file: &str) -> io::Result<()> {
    let invalid = |message: String| io::Error::new(io::ErrorKind::InvalidInput, message);
    if file.is_empty()
        || file == "."
        || file == ".."
        || file
            .chars()
            .any(|ch| ch.is_control() || matches!(ch, '/' | '\\' | ':'))
        || file.trim() != file
    {
        return Err(invalid(format!(
            "invalid store name {file:?}: use one path component"
        )));
    }
    if file.strip_suffix(".json").is_none() || file.eq_ignore_ascii_case("index.json") {
        return Err(invalid(
            "the graph file must end with .json and must not be index.json".to_owned(),
        ));
    }
    Ok(())
}

/// Reads one graph artifact of the store and returns its graph and byte digest.
///
/// # Errors
/// Returns an error for an invalid name, a link, an unreadable file, or an invalid artifact.
pub fn read_graph(directory: &Path, file: &str) -> io::Result<(ArtifactGraph, String)> {
    check_graph_name(file)?;
    reject_symlink(directory)?;
    let path = directory.join(file);
    reject_symlink(&path)?;
    let bytes = fs::read(&path).map_err(|error| {
        io::Error::new(
            error.kind(),
            format!("cannot read {}: {error}", path.display()),
        )
    })?;
    let artifact = GraphArtifact::read(&bytes)?;
    Ok((artifact.graph, digest_of(&bytes)))
}

// -----------------------------------------------------------------------------
// Tests
// -----------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::{GraphArtifact, check_graph_name, read_graph};
    use crate::analysis::{
        fact::{
            CallableData, CallableKind, EntityEvidence, EntityKey, EntityKind, RelationEvidence,
        },
        graph::{Edge, Graph, Node, NodeId, Provenance, ProvenanceOrigin},
        language::Language,
        source::{ContentHash, SourceRange, SourceSpan},
    };

    #[test]
    fn reads_writer_output_without_metadata_loss_and_rejects_unknown_schema() {
        let artifact = GraphArtifact::new(&two_node_graph());
        let bytes = serde_json::to_vec(&artifact).expect("serialize");
        let read = GraphArtifact::read(&bytes).expect("read artifact");
        assert_eq!(serde_json::to_vec(&read).expect("serialize"), bytes);
        let mut value = serde_json::to_value(&artifact).expect("serialize");
        value["schemaVersion"] = serde_json::json!(2);
        assert!(GraphArtifact::read(&serde_json::to_vec(&value).expect("serialize")).is_err());
        value["schemaVersion"] = serde_json::json!(1);
        value["graph"]["edges"][0]["target"] = serde_json::json!(999);
        assert!(GraphArtifact::read(&serde_json::to_vec(&value).expect("serialize")).is_err());
    }

    #[test]
    fn reads_an_older_graph_with_clustering_and_comparison_fields() {
        let mut value = serde_json::to_value(GraphArtifact::new(&two_node_graph())).expect("value");
        value["clustering"] =
            serde_json::json!({"method": "file_path", "clusters": [], "memberships": []});
        value["comparison"] =
            serde_json::json!({"base": "b1", "target": "b2", "targetKind": "commit"});
        value["graph"]["nodes"][0]["change"] = "unchanged".into();
        value["graph"]["edges"][0]["previousWeight"] = 3.into();

        let read = GraphArtifact::read(&serde_json::to_vec(&value).expect("bytes"))
            .expect("an older graph should stay readable");
        let written = serde_json::to_value(read).expect("value");

        assert!(written.get("clustering").is_none());
        assert!(written.get("comparison").is_none());
        assert!(written["graph"]["nodes"][0].get("change").is_none());
    }

    #[test]
    fn reads_a_stored_graph_with_its_digest_and_rejects_invalid_names() {
        let directory = tempfile::tempdir().expect("store");
        let artifact = GraphArtifact::new(&two_node_graph());
        artifact
            .write(&directory.path().join("app.json"))
            .expect("write graph");
        let bytes = std::fs::read(directory.path().join("app.json")).expect("bytes");

        let (graph, digest) = read_graph(directory.path(), "app.json").expect("read graph");

        assert_eq!(graph.nodes.len(), 2);
        assert_eq!(digest, crate::store::digest_of(&bytes));
        for name in ["index.json", "../app.json", "app.txt", " app.json", ""] {
            assert!(check_graph_name(name).is_err(), "{name:?}");
        }
    }

    #[test]
    fn writes_the_documented_wire_shape() {
        let artifact = GraphArtifact::new(&two_node_graph());
        let document =
            serde_json::to_value(&artifact).expect("the artifact should serialize to JSON");
        let rust_node = &document["graph"]["nodes"][1];

        assert_eq!(document["schemaVersion"], 1);
        assert_eq!(
            document["graph"]["languages"],
            serde_json::json!(["python", "rust"])
        );
        assert_eq!(document["graph"]["nodes"][0]["language"], "python");
        assert_eq!(rust_node["language"], "rust");
        assert_eq!(document["graph"]["nodes"][0]["kind"], "module");
        assert_eq!(rust_node["kind"], "callable");
        assert_eq!(rust_node["path"], "lib.rs");
        assert_eq!(rust_node["signature"], "fn run(value: i32)");
        assert_eq!(rust_node["parameters"][0], "value");
        assert_eq!(document["graph"]["edges"][0]["kind"], "calls");
        assert_eq!(document["graph"]["edges"][0]["origin"], "semantic");
        assert_eq!(
            document["graph"]["edges"][0]["observations"][0]["start"],
            20
        );
        assert_eq!(
            document
                .as_object()
                .expect("object")
                .keys()
                .collect::<Vec<_>>(),
            ["graph", "schemaVersion"]
        );
    }

    #[test]
    fn omits_absent_metadata_fields() {
        let graph = Graph {
            nodes: vec![module_node(None)],
            edges: Vec::new(),
        };

        let document = serde_json::to_value(GraphArtifact::new(&graph))
            .expect("the artifact should serialize to JSON");
        let node = &document["graph"]["nodes"][0];

        for field in [
            "scope",
            "body",
            "signature",
            "parameters",
            "documentation",
            "contentHash",
            "position",
        ] {
            assert!(node.get(field).is_none(), "{field}");
        }
    }

    #[test]
    fn carries_the_content_hash_of_a_node() {
        let graph = Graph {
            nodes: vec![module_node(Some(ContentHash::of("content")))],
            edges: Vec::new(),
        };

        let document = serde_json::to_value(GraphArtifact::new(&graph))
            .expect("the artifact should serialize to JSON");

        assert_eq!(
            document["graph"]["nodes"][0]["contentHash"],
            ContentHash::of("content").to_string()
        );
    }

    fn module_node(content_hash: Option<ContentHash>) -> Node {
        Node {
            language: Language::Python,
            key: EntityKey::module("app.py"),
            name: "app.py".to_owned(),
            kind: EntityKind::Module,
            content_hash,
            provenance: Provenance {
                origin: ProvenanceOrigin::Syntax,
                evidence: EntityEvidence {
                    path: "app.py".to_owned(),
                    declarations: Vec::new(),
                    scope: None,
                    body: None,
                    type_inputs: Vec::new(),
                    type_outputs: Vec::new(),
                },
            },
        }
    }

    /// Builds two nodes that carry every documented wire field.
    fn two_node_graph() -> Graph {
        let callable_key = EntityKey::module("lib.rs").child(
            &EntityKind::Callable(CallableData {
                kind: CallableKind::Function,
                signature: None,
                parameters: Vec::new(),
                return_type: None,
                documentation: None,
            }),
            "run",
            0,
        );
        let mut module = module_node(None);
        module.provenance.evidence.scope = Some(SourceRange { start: 0, end: 40 });
        module.provenance.evidence.body = Some(SourceRange { start: 0, end: 40 });

        Graph {
            nodes: vec![
                module,
                Node {
                    language: Language::Rust,
                    key: callable_key,
                    name: "run".to_owned(),
                    kind: EntityKind::Callable(CallableData {
                        kind: CallableKind::Function,
                        signature: Some("fn run(value: i32)".to_owned()),
                        parameters: vec!["value".to_owned()],
                        return_type: None,
                        documentation: None,
                    }),
                    content_hash: None,
                    provenance: Provenance {
                        origin: ProvenanceOrigin::Syntax,
                        evidence: EntityEvidence {
                            path: "lib.rs".to_owned(),
                            declarations: vec![SourceRange { start: 3, end: 6 }],
                            scope: Some(SourceRange { start: 0, end: 40 }),
                            body: Some(SourceRange { start: 10, end: 40 }),
                            type_inputs: Vec::new(),
                            type_outputs: Vec::new(),
                        },
                    },
                },
            ],
            edges: vec![Edge {
                source: NodeId(1),
                target: NodeId(1),
                kind: crate::analysis::fact::RelationKind::Calls,
                weight: 1,
                provenance: Provenance {
                    origin: ProvenanceOrigin::Semantic,
                    evidence: RelationEvidence {
                        observations: vec![SourceSpan {
                            path: "lib.rs".to_owned(),
                            range: SourceRange { start: 20, end: 23 },
                        }],
                    },
                },
            }],
        }
    }
}
