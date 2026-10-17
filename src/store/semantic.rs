//! Semantic cache.
//!
//! This module owns `<graph>.semantic.json`: the semantic edges that were
//! resolved for one graph file. A graph file and its cache are the input of
//! `read_merged_graph` and `merged_graph_bytes`. One graph with the cached
//! semantic edges is the output.
//!
//! # Binding
//!
//! A cache records the byte digest of its graph file. A cache of another digest
//! or another version is ignored, and the next write replaces it. The cache is
//! not part of flow binding, so a write never makes a flow stale.
//!
//! # Sections
//!
//! `Schema` defines the cache document.
//! `Reading and writing` reads one cache and merges new edges into it.
//! `Merged graph` adds the cached edges to their graph.

use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet, hash_map::Entry},
    io,
    path::Path,
};

use serde::{Deserialize, Serialize};

use super::{
    digest_of,
    graph::{
        ArtifactEdge, ArtifactGraph, GraphArtifact, Origin, RelationKind, check_graph_name,
        read_graph,
    },
    lock_store, read_bytes, reject_symlink, write_bytes,
};

// -----------------------------------------------------------------------------
// Schema
// -----------------------------------------------------------------------------

/// The end of the file name of a semantic cache: `<graph>.semantic.json`.
pub(crate) const SEMANTIC_SUFFIX: &str = ".semantic.json";

/// The cache schema version. A cache of another version reads as empty.
pub const CACHE_VERSION: u32 = 1;

/// The resolved node keys of one graph file and the semantic edges that touch them.
///
/// A key is resolved when its callers, its outgoing calls, the uses of it, the
/// types of its locals, and the imports of it and of its file are in `edges`.
/// Some edges of a key appear once their other end is resolved too: its own
/// signature types (until then a parameter type is a `uses_type` edge), the
/// values and fields that it uses, and calls in macros, function values, or
/// module-level code. Keys and edges are sorted, and one source, target, and
/// kind has one edge.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SemanticCache {
    pub version: u32,
    pub graph_digest: String,
    pub resolved: BTreeSet<String>,
    pub edges: Vec<SemanticEdge>,
}

/// One semantic edge between two node keys.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SemanticEdge {
    pub source: String,
    pub target: String,
    pub kind: RelationKind,
    /// The most observations that one resolution saw: a lower bound of the
    /// index weight, because keys that resolve together can see more.
    pub weight: u32,
}

impl SemanticCache {
    #[must_use]
    pub fn new(graph_digest: String) -> Self {
        Self {
            version: CACHE_VERSION,
            graph_digest,
            resolved: BTreeSet::new(),
            edges: Vec::new(),
        }
    }
}

// -----------------------------------------------------------------------------
// Reading and writing
// -----------------------------------------------------------------------------

/// Reads the cache of one graph file for the current byte digest of that file.
///
/// A missing cache, a cache that is not valid, and a cache of another digest
/// or another version read as an empty cache of `graph_digest`.
///
/// # Errors
///
/// Returns an error for an invalid graph name, a link, or a failed read.
pub fn read_cache(
    directory: &Path,
    graph_file: &str,
    graph_digest: &str,
) -> io::Result<SemanticCache> {
    check_graph_name(graph_file)?;
    reject_symlink(directory)?;
    let empty = || SemanticCache::new(graph_digest.to_owned());
    let bytes = match read_bytes(&directory.join(cache_file(graph_file))) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(empty()),
        Err(error) => return Err(error),
    };
    Ok(serde_json::from_slice::<SemanticCache>(&bytes)
        .ok()
        .filter(|cache| cache.version == CACHE_VERSION && cache.graph_digest == graph_digest)
        .unwrap_or_else(empty))
}

/// Adds the resolved keys and the edges of `cache` to the stored cache of one graph file.
///
/// The write holds the store lock and replaces the file atomically. The stored
/// keys and edges of the same digest stay. Two edges with the same source,
/// target, and kind merge into one with the larger weight, and a type use that
/// a signature edge states goes, as the graph semantic stage drops it. So the
/// stored cache does not depend on the order or the grouping of resolutions.
///
/// # Errors
///
/// Returns `InvalidInput` if the digest of the graph file is not
/// `cache.graph_digest`, so edges of an older graph never replace a newer cache.
/// Returns an error for an invalid graph name, a link, or a failed read or write.
pub fn write_cache(directory: &Path, graph_file: &str, cache: &SemanticCache) -> io::Result<()> {
    check_graph_name(graph_file)?;
    let _lock = lock_store(directory)?;
    let current = digest_of(&read_bytes(&directory.join(graph_file))?);
    if current != cache.graph_digest {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("the graph {graph_file} changed after its semantic edges were resolved"),
        ));
    }

    let mut stored = read_cache(directory, graph_file, &current)?;
    stored.resolved.extend(cache.resolved.iter().cloned());
    let edges = stored
        .edges
        .into_iter()
        .chain(cache.edges.iter().cloned())
        .fold(BTreeMap::new(), |mut edges, edge| {
            let weight = edges
                .entry((edge.source, edge.target, edge.kind))
                .or_insert(edge.weight);
            *weight = (*weight).max(edge.weight);
            edges
        });
    let stated = edges
        .keys()
        .filter_map(|(source, target, kind)| match kind {
            RelationKind::InputType => Some((target.clone(), source.clone())),
            RelationKind::OutputType => Some((source.clone(), target.clone())),
            RelationKind::Contains
            | RelationKind::Calls
            | RelationKind::Imports
            | RelationKind::UsesType
            | RelationKind::UsesValue => None,
        })
        .collect::<HashSet<_>>();
    stored.edges = edges
        .into_iter()
        .filter(|((source, target, kind), _)| {
            *kind != RelationKind::UsesType || !stated.contains(&(source.clone(), target.clone()))
        })
        .map(|((source, target, kind), weight)| SemanticEdge {
            source,
            target,
            kind,
            weight,
        })
        .collect();

    let mut document = serde_json::to_vec(&stored).map_err(io::Error::other)?;
    document.push(b'\n');
    write_bytes(&directory.join(cache_file(graph_file)), &document)
}

/// Returns the cache file name of one checked graph file name.
fn cache_file(graph_file: &str) -> String {
    let name = graph_file.strip_suffix(".json").unwrap_or(graph_file);
    format!("{name}{SEMANTIC_SUFFIX}")
}

// -----------------------------------------------------------------------------
// Merged graph
// -----------------------------------------------------------------------------

/// Reads one graph file with its cached semantic edges, and returns the graph and the byte digest of the graph file.
///
/// A cached edge with the source, target, and kind of a graph edge sets the
/// weight of that edge. The other cached edges follow the graph edges, with a
/// semantic origin and no observations. A cached edge that names a key outside
/// the graph is skipped.
///
/// # Errors
///
/// Returns an error if the graph or its cache cannot be read.
pub fn read_merged_graph(
    directory: &Path,
    graph_file: &str,
) -> io::Result<(ArtifactGraph, String)> {
    let (mut graph, digest) = read_graph(directory, graph_file)?;
    let cache = read_cache(directory, graph_file, &digest)?;
    merge(&mut graph, cache);
    Ok((graph, digest))
}

/// Returns the merged graph document of one graph file, and the byte digest of the graph file.
///
/// Without cached edges, the document is the bytes of the graph file, read once
/// and not parsed. `serve` sends this document for the graph file.
///
/// # Errors
///
/// Returns an error if the graph or its cache cannot be read.
pub fn merged_graph_bytes(directory: &Path, graph_file: &str) -> io::Result<(Vec<u8>, String)> {
    check_graph_name(graph_file)?;
    reject_symlink(directory)?;
    let bytes = read_bytes(&directory.join(graph_file))?;
    let digest = digest_of(&bytes);
    let cache = read_cache(directory, graph_file, &digest)?;
    if cache.edges.is_empty() {
        return Ok((bytes, digest));
    }
    let mut artifact = GraphArtifact::read(&bytes)?;
    merge(&mut artifact.graph, cache);
    Ok((artifact.to_bytes()?, digest))
}

/// Adds the edges of one cache to its graph, as `read_merged_graph` documents.
fn merge(graph: &mut ArtifactGraph, cache: SemanticCache) {
    let ids = graph
        .nodes
        .iter()
        .map(|node| (node.key.as_str(), node.id))
        .collect::<HashMap<_, _>>();
    let mut positions = graph
        .edges
        .iter()
        .enumerate()
        .map(|(position, edge)| ((edge.source, edge.target, edge.kind), position))
        .collect::<HashMap<_, _>>();
    for edge in cache.edges {
        let (Some(&source), Some(&target)) =
            (ids.get(edge.source.as_str()), ids.get(edge.target.as_str()))
        else {
            continue;
        };
        match positions.entry((source, target, edge.kind)) {
            Entry::Occupied(position) => graph.edges[*position.get()].weight = edge.weight,
            Entry::Vacant(position) => {
                position.insert(graph.edges.len());
                graph.edges.push(ArtifactEdge {
                    source,
                    target,
                    kind: edge.kind,
                    weight: edge.weight,
                    origin: Origin::Semantic,
                    observations: Vec::new(),
                });
            }
        }
    }
}

// -----------------------------------------------------------------------------
// Tests
// -----------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::{collections::BTreeSet, fs, path::Path};

    use super::{
        SemanticCache, SemanticEdge, merged_graph_bytes, read_cache, read_merged_graph, write_cache,
    };
    use crate::{
        flow::fixtures::write_graph,
        store::{
            digest_of,
            graph::{GraphArtifact, Origin, RelationKind, read_graph},
        },
    };

    const MAIN: &str = "src/main.rs::callable:main";
    const PARSE: &str = "src/parse.rs::callable:parse_args";
    const READ: &str = "src/parse.rs::callable:read_input";

    fn edge(source: &str, target: &str, kind: RelationKind, weight: u32) -> SemanticEdge {
        SemanticEdge {
            source: source.to_owned(),
            target: target.to_owned(),
            kind,
            weight,
        }
    }

    fn cache(digest: &str, keys: &[&str], edges: Vec<SemanticEdge>) -> SemanticCache {
        SemanticCache {
            resolved: keys.iter().map(|key| (*key).to_owned()).collect(),
            edges,
            ..SemanticCache::new(digest.to_owned())
        }
    }

    fn graph_digest(store: &Path) -> String {
        digest_of(&fs::read(store.join("graph.json")).expect("graph bytes"))
    }

    #[test]
    fn merges_cached_edges_into_the_graph_and_keeps_the_graph_digest() {
        let store = tempfile::tempdir().expect("store");
        write_graph(store.path(), "src/extra.rs::callable:helper");
        let digest = graph_digest(store.path());
        let (plain, _) = read_graph(store.path(), "graph.json").expect("graph");
        let file = fs::read(store.path().join("graph.json")).expect("graph bytes");
        assert_eq!(
            merged_graph_bytes(store.path(), "graph.json").expect("document"),
            (file, digest.clone()),
            "without cached edges, the document is the graph file"
        );
        write_cache(
            store.path(),
            "graph.json",
            &cache(
                &digest,
                &[MAIN],
                vec![
                    edge(MAIN, PARSE, RelationKind::Calls, 3),
                    edge(MAIN, READ, RelationKind::UsesValue, 2),
                    edge(MAIN, "src/gone.rs::callable:gone", RelationKind::Calls, 1),
                ],
            ),
        )
        .expect("write cache");

        let (merged, merged_digest) = read_merged_graph(store.path(), "graph.json").expect("read");

        assert_eq!(merged_digest, digest);
        // The fixture graph has main (0) -> parse_args (1) as its first calls edge.
        assert_eq!(merged.edges.len(), plain.edges.len() + 1);
        assert_eq!((merged.edges[0].target, merged.edges[0].weight), (1, 3));
        let added = merged.edges.last().expect("added edge");
        assert_eq!(
            (
                added.source,
                added.target,
                added.kind,
                added.weight,
                added.origin
            ),
            (0, 2, RelationKind::UsesValue, 2, Origin::Semantic)
        );
        let (document, document_digest) =
            merged_graph_bytes(store.path(), "graph.json").expect("document");
        assert_eq!(document_digest, digest);
        let edges = GraphArtifact::read(&document)
            .expect("graph document")
            .graph
            .edges;
        assert_eq!(
            serde_json::to_value(edges).expect("edges"),
            serde_json::to_value(&merged.edges).expect("edges")
        );
    }

    #[test]
    fn ignores_and_replaces_the_cache_of_another_graph_digest() {
        let store = tempfile::tempdir().expect("store");
        write_graph(store.path(), "src/extra.rs::callable:helper");
        let old = graph_digest(store.path());
        write_cache(
            store.path(),
            "graph.json",
            &cache(
                &old,
                &[MAIN],
                vec![edge(MAIN, READ, RelationKind::UsesValue, 1)],
            ),
        )
        .expect("write cache");
        write_graph(store.path(), "src/extra.rs::callable:other");
        let new = graph_digest(store.path());

        let (merged, _) = read_merged_graph(store.path(), "graph.json").expect("read");
        assert!(
            merged
                .edges
                .iter()
                .all(|edge| edge.kind == RelationKind::Calls)
        );

        write_cache(store.path(), "graph.json", &cache(&new, &[PARSE], vec![]))
            .expect("replace cache");
        let stored = read_cache(store.path(), "graph.json", &new).expect("cache");
        assert_eq!(stored, cache(&new, &[PARSE], vec![]));
    }

    #[test]
    fn refuses_edges_of_a_graph_that_changed_and_keeps_the_newer_cache() {
        let store = tempfile::tempdir().expect("store");
        write_graph(store.path(), "src/extra.rs::callable:helper");
        let old = graph_digest(store.path());
        write_graph(store.path(), "src/extra.rs::callable:other");
        let new = graph_digest(store.path());
        write_cache(store.path(), "graph.json", &cache(&new, &[PARSE], vec![]))
            .expect("write cache");

        let error = write_cache(store.path(), "graph.json", &cache(&old, &[MAIN], vec![]))
            .expect_err("an older graph digest");

        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
        let stored = read_cache(store.path(), "graph.json", &new).expect("cache");
        assert_eq!(stored.resolved, BTreeSet::from([PARSE.to_owned()]));
    }

    #[test]
    fn adds_keys_and_edges_in_sorted_order_and_keeps_the_larger_weight_of_an_edge() {
        let store = tempfile::tempdir().expect("store");
        write_graph(store.path(), "src/extra.rs::callable:helper");
        let digest = graph_digest(store.path());
        write_cache(
            store.path(),
            "graph.json",
            &cache(
                &digest,
                &[PARSE],
                vec![
                    edge(PARSE, READ, RelationKind::Calls, 1),
                    edge(MAIN, PARSE, RelationKind::Calls, 1),
                ],
            ),
        )
        .expect("first write");
        write_cache(
            store.path(),
            "graph.json",
            &cache(
                &digest,
                &[MAIN],
                vec![edge(MAIN, PARSE, RelationKind::Calls, 4)],
            ),
        )
        .expect("second write");

        let stored = read_cache(store.path(), "graph.json", &digest).expect("cache");

        assert_eq!(
            stored,
            cache(
                &digest,
                &[MAIN, PARSE],
                vec![
                    edge(MAIN, PARSE, RelationKind::Calls, 4),
                    edge(PARSE, READ, RelationKind::Calls, 1),
                ],
            )
        );
        let mut names = fs::read_dir(store.path())
            .expect("list")
            .map(|entry| {
                entry
                    .expect("entry")
                    .file_name()
                    .into_string()
                    .expect("name")
            })
            .collect::<Vec<_>>();
        names.sort();
        assert_eq!(names, [".store.lock", "graph.json", "graph.semantic.json"]);
    }

    #[test]
    fn writes_the_same_cache_for_any_resolution_order() {
        let first = cache(
            "",
            &[MAIN],
            vec![
                edge(MAIN, PARSE, RelationKind::Calls, 2),
                edge(MAIN, READ, RelationKind::UsesValue, 1),
            ],
        );
        let second = cache(
            "",
            &[PARSE],
            vec![
                edge(MAIN, PARSE, RelationKind::Calls, 1),
                edge(PARSE, READ, RelationKind::Calls, 1),
            ],
        );
        let stored = |order: [&SemanticCache; 2]| {
            let store = tempfile::tempdir().expect("store");
            write_graph(store.path(), "src/extra.rs::callable:helper");
            let digest = graph_digest(store.path());
            for each in order {
                let each = SemanticCache {
                    graph_digest: digest.clone(),
                    ..each.clone()
                };
                write_cache(store.path(), "graph.json", &each).expect("write cache");
            }
            fs::read(store.path().join("graph.semantic.json")).expect("cache bytes")
        };

        assert_eq!(stored([&first, &second]), stored([&second, &first]));
    }

    #[test]
    fn drops_a_type_use_that_a_signature_edge_of_another_write_states() {
        let store = tempfile::tempdir().expect("store");
        write_graph(store.path(), "src/extra.rs::callable:helper");
        let digest = graph_digest(store.path());
        let write = |keys: &[&str], edges| {
            write_cache(store.path(), "graph.json", &cache(&digest, keys, edges)).expect("write");
        };
        write(&[MAIN], vec![edge(MAIN, PARSE, RelationKind::UsesType, 1)]);
        write(
            &[PARSE],
            vec![edge(PARSE, MAIN, RelationKind::InputType, 1)],
        );

        let stored = read_cache(store.path(), "graph.json", &digest).expect("cache");

        assert_eq!(
            stored.edges,
            [edge(PARSE, MAIN, RelationKind::InputType, 1)]
        );
    }

    #[test]
    fn reads_a_cache_of_another_version_as_empty() {
        let store = tempfile::tempdir().expect("store");
        write_graph(store.path(), "src/extra.rs::callable:helper");
        let digest = graph_digest(store.path());
        let mut other = cache(&digest, &[MAIN], vec![]);
        other.version = 2;
        fs::write(
            store.path().join("graph.semantic.json"),
            serde_json::to_vec(&other).expect("cache JSON"),
        )
        .expect("cache of another version");

        let stored = read_cache(store.path(), "graph.json", &digest).expect("cache");

        assert_eq!(stored, SemanticCache::new(digest));
    }

    #[test]
    fn reads_a_cache_that_is_not_valid_as_empty() {
        let store = tempfile::tempdir().expect("store");
        write_graph(store.path(), "src/extra.rs::callable:helper");
        let digest = graph_digest(store.path());
        fs::write(store.path().join("graph.semantic.json"), "{").expect("broken cache");

        let stored = read_cache(store.path(), "graph.json", &digest).expect("cache");

        assert_eq!(stored, SemanticCache::new(digest));
    }
}
