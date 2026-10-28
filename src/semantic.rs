//! On-demand semantic edges.
//!
//! This module fills the semantic cache of a graph with the edges of the node
//! keys that a consumer reads. A store, a graph file, and node keys are the
//! input. The cache file of that graph is the output. The `daemon` submodule
//! keeps the language servers of a store warm between commands.

#[cfg(unix)]
pub mod daemon;

use std::{collections::BTreeSet, fs, io, path::Path};

use crate::{
    analysis::{config::configured_profiles, graph::NodeId, resolve::Resolver},
    store::{
        graph::{read_graph, relation_kind},
        semantic::{SemanticCache, SemanticEdge, read_cache, write_cache},
    },
};

/// Resolves the keys that the cache of one graph lacks.
///
/// On Unix, the daemon of the store resolves them, and this call starts the
/// daemon when none runs. On other systems, and with `TEKDEK_NO_DAEMON=1`,
/// this process resolves them as `ensure_in_process` does. Do not call it
/// while you hold the store lock: the cache write takes that lock.
///
/// # Errors
///
/// Returns an error if the graph or its cache cannot be read or written, the
/// source files changed after the graph was built, a server fails, or the
/// daemon fails two times.
pub fn ensure(
    store: &Path,
    graph_file: &str,
    keys: &[String],
    config: Option<&Path>,
) -> io::Result<()> {
    #[cfg(unix)]
    if std::env::var_os("TEKDEK_NO_DAEMON").is_none_or(|value| value != "1") {
        return daemon::ensure(store, graph_file, keys, config);
    }
    ensure_in_process(store, graph_file, keys, config)
}

/// Resolves the keys that the cache of one graph lacks, with language servers that run in this process.
///
/// The project is the folder that holds the store. The servers come from the
/// language profiles of the configuration file `config`, or of the default
/// configuration file, as `index` reads them. A cache that holds every key
/// starts no server.
///
/// # Errors
///
/// Returns an error if the graph or its cache cannot be read or written, the
/// source files changed after the graph was built, or a server fails.
pub fn ensure_in_process(
    store: &Path,
    graph_file: &str,
    keys: &[String],
    config: Option<&Path>,
) -> io::Result<()> {
    if resolved(store, graph_file, keys)? {
        return Ok(());
    }
    let store = fs::canonicalize(store)?;
    let profiles = configured_profiles(config).map_err(io::Error::other)?;
    let mut resolver = Resolver::new(project_root(&store)?, profiles).map_err(io::Error::other)?;
    let filled = fill_cache(&mut resolver, &store, graph_file, keys);
    let stopped = resolver.shutdown().map_err(io::Error::other);
    filled?;
    stopped
}

/// Returns true when the cache of one graph holds every key.
fn resolved(store: &Path, graph_file: &str, keys: &[String]) -> io::Result<bool> {
    let (_, digest) = read_graph(store, graph_file)?;
    let cache = read_cache(store, graph_file, &digest)?;
    Ok(keys.iter().all(|key| cache.resolved.contains(key)))
}

/// Returns the project folder of a canonical store: the folder that holds it.
fn project_root(store: &Path) -> io::Result<&Path> {
    store.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("the store {} has no project folder", store.display()),
        )
    })
}

/// Resolves the keys that the cache of one graph lacks, and adds their edges to the cache.
///
/// Returns the edges that this call added.
///
/// # Errors
///
/// Returns `InvalidData` if the resolver parsed other source files than the
/// graph was built from. Returns an error if the graph or its cache cannot be
/// read or written, or a server fails.
pub fn fill_cache(
    resolver: &mut Resolver,
    store: &Path,
    graph_file: &str,
    keys: &[String],
) -> io::Result<Vec<SemanticEdge>> {
    let (graph, digest) = read_graph(store, graph_file)?;
    let cache = read_cache(store, graph_file, &digest)?;
    let missing = keys
        .iter()
        .filter(|key| !cache.resolved.contains(*key))
        .cloned()
        .collect::<BTreeSet<_>>();
    if missing.is_empty() {
        return Ok(Vec::new());
    }
    if graph.source_hash.as_deref() != Some(resolver.source_hash()) {
        let root = store.parent().unwrap_or(store);
        let name = root.file_name().unwrap_or_default().to_string_lossy();
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "the graph {graph_file} does not match the source files in {}. Only the project graph {name}.json gets semantic edges; a graph of a subfolder or from --output does not. If {graph_file} is the project graph, run `tekdek index` in {}",
                root.display(),
                root.display()
            ),
        ));
    }

    let missing = missing.into_iter().collect::<Vec<_>>();
    let edges = resolver.resolve(&missing).map_err(io::Error::other)?;
    let nodes = &resolver.graph().nodes;
    let key = |id: NodeId| nodes[id.index()].key.as_str().to_owned();
    let edges = edges
        .iter()
        .map(|edge| SemanticEdge {
            source: key(edge.source),
            target: key(edge.target),
            kind: relation_kind(edge.kind),
            weight: edge.weight,
        })
        .collect::<Vec<_>>();
    write_cache(
        store,
        graph_file,
        &SemanticCache {
            resolved: missing.into_iter().collect(),
            edges: edges.clone(),
            ..SemanticCache::new(digest)
        },
    )?;
    Ok(edges)
}

// -----------------------------------------------------------------------------
// Tests
// -----------------------------------------------------------------------------

#[cfg(all(test, feature = "python"))]
mod tests {
    use std::{fs, io, path::Path};

    use super::{ensure_in_process, fill_cache};
    use crate::{
        analysis::{
            self, PipelineConfig,
            language::{Language, LanguageProfile, ServerCommand, builtin_profiles},
            lsp::fake_server::FakeServer,
            resolve::Resolver,
        },
        store::{
            digest_of,
            graph::{GraphArtifact, RelationKind},
            semantic::read_cache,
        },
    };

    pub(super) const RUN: &str = "lib.py::callable:run";
    pub(super) const HELPER: &str = "lib.py::callable:helper";

    /// `run` calls `helper` on line 1.
    const SCRIPT: &str = concat!(
        r#"textDocument/prepareCallHierarchy "position":{"character":4,"line":0}&/lib.py"} "#,
        r#"[{"name":"run","kind":12,"uri":"{root}/lib.py","range":RUN,"selectionRange":RUN}]"#,
        "\n",
        r#"callHierarchy/outgoingCalls "name":"run" [{"to":{"name":"helper","kind":12,"#,
        r#""uri":"{root}/lib.py","range":HELPER,"selectionRange":HELPER},"fromRanges":[CALL]}]"#,
    );

    #[test]
    fn records_the_resolved_keys_and_resolves_each_key_once() {
        let project = project();
        let store = project.path().join(".tekdek");
        let server = FakeServer::new();
        let mut resolver = resolver(project.path(), &server);
        let keys = [RUN.to_owned()];

        let edges = fill_cache(&mut resolver, &store, "graph.json", &keys).expect("first fill");
        let requests = server.requests().len();
        let again = fill_cache(&mut resolver, &store, "graph.json", &keys).expect("second fill");
        ensure_in_process(&store, "graph.json", &keys, None)
            .expect("a cached key starts no server");
        resolver.shutdown().expect("the server should stop");

        assert_eq!(
            edges
                .iter()
                .map(|edge| (edge.source.as_str(), edge.target.as_str(), edge.kind))
                .collect::<Vec<_>>(),
            [(RUN, HELPER, RelationKind::Calls)]
        );
        assert_eq!(again, []);
        assert_eq!(server.requests().len(), requests);
        let digest = digest_of(&fs::read(store.join("graph.json")).expect("graph bytes"));
        let cache = read_cache(&store, "graph.json", &digest).expect("cache");
        assert_eq!(cache.resolved.iter().collect::<Vec<_>>(), [RUN]);
        assert_eq!(cache.edges, edges);
    }

    #[test]
    fn writes_the_same_cache_for_keys_that_resolve_together_or_alone() {
        const CONFIG: &str = "lib.py::type:Config";
        const LOAD: &str = "lib.py::callable:load";
        let range = |line, start, end| {
            format!(
                r#"{{"start":{{"line":{line},"character":{start}}},"end":{{"line":{line},"character":{end}}}}}"#
            )
        };
        let symbol = |name, kind, line, start, end| {
            format!(
                r#"{{"name":"{name}","kind":{kind},"range":{},"selectionRange":{}}}"#,
                range(line, 0, 30),
                range(line, start, end)
            )
        };
        // `load` takes a `Config`: its parameter `c` has that type, and the annotation references it.
        let script = [
            format!(
                r#"textDocument/references "position":{{"character":6,"line":0}}&/lib.py"}} [{{"uri":"{{root}}/lib.py","range":{}}}]"#,
                range(2, 12, 18)
            ),
            format!(
                r#"textDocument/documentSymbol /lib.py"}} [{},{},{}]"#,
                symbol("Config", 5, 0, 6, 12),
                symbol("load", 12, 2, 4, 8),
                symbol("c", 13, 2, 9, 10)
            ),
            format!(
                r#"textDocument/typeDefinition "position":{{"character":9,"line":2}}&/lib.py"}} {{"uri":"{{root}}/lib.py","range":{}}}"#,
                range(0, 6, 12)
            ),
        ]
        .join("\n");
        let server = FakeServer::new();
        let command = server.scripted(&script);
        let cache = |steps: &[&[&str]]| {
            let project = project_with("class Config:\n    pass\ndef load(c: Config):\n    pass\n");
            let store = project.path().join(".tekdek");
            let mut resolver = Resolver::new(project.path(), with_server(&command))
                .expect("the project should parse");
            for keys in steps {
                let keys = keys.iter().map(|key| (*key).to_owned()).collect::<Vec<_>>();
                fill_cache(&mut resolver, &store, "graph.json", &keys).expect("fill");
            }
            resolver.shutdown().expect("the server should stop");
            fs::read_to_string(store.join("graph.semantic.json")).expect("cache")
        };

        let together = cache(&[&[LOAD, CONFIG]]);
        let alone = cache(&[&[LOAD], &[CONFIG]]);

        assert_eq!(together, alone);
        assert!(together.contains(r#""kind":"input_type""#), "{together}");
        assert!(!together.contains(r#""kind":"uses_type""#), "{together}");
    }

    #[test]
    fn refuses_a_graph_of_other_source_files() {
        let project = project();
        let store = project.path().join(".tekdek");
        fs::write(project.path().join("lib.py"), "def run():\n    pass\n").expect("source");
        let server = FakeServer::new();
        let mut resolver = resolver(project.path(), &server);

        let error = fill_cache(&mut resolver, &store, "graph.json", &[RUN.to_owned()])
            .expect_err("the graph is older than the source");

        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        let name = project.path().file_name().expect("name").to_string_lossy();
        assert!(
            error.to_string().contains(&format!(
                "Only the project graph {name}.json gets semantic edges"
            )),
            "{error}"
        );
        assert_eq!(server.requests().len(), 0);
    }

    /// Writes a one-file Python project and its graph in a `.tekdek` store.
    pub(super) fn project() -> tempfile::TempDir {
        project_with("def run():\n    helper()\ndef helper():\n    pass\n")
    }

    /// Writes a Python project with one `lib.py` and its graph in a `.tekdek` store.
    fn project_with(source: &str) -> tempfile::TempDir {
        let project = tempfile::tempdir().expect("the project folder should be created");
        fs::write(project.path().join("lib.py"), source).expect("the source should be written");
        index(project.path());
        project
    }

    /// Indexes a project into the `graph.json` file of its `.tekdek` store.
    pub(super) fn index(project: &Path) {
        let indexed = analysis::run(&PipelineConfig::new(project.to_path_buf()))
            .expect("the project should index");
        GraphArtifact::new(&indexed.graph)
            .with_source_hash(indexed.source_hash)
            .write(&project.join(".tekdek/graph.json"))
            .expect("the graph should be written");
    }

    /// Returns a command that starts the fake server with the script of the project.
    pub(super) fn scripted(server: &FakeServer) -> ServerCommand {
        let range = |line, start, end| {
            format!(
                r#"{{"start":{{"line":{line},"character":{start}}},"end":{{"line":{line},"character":{end}}}}}"#
            )
        };
        server.scripted(
            &SCRIPT
                .replace("RUN", &range(0, 4, 7))
                .replace("HELPER", &range(2, 4, 10))
                .replace("CALL", &range(1, 4, 10)),
        )
    }

    /// Returns the built-in profiles with `server` as the Python server.
    pub(super) fn with_server(server: &ServerCommand) -> Vec<LanguageProfile> {
        let mut profiles = builtin_profiles();
        for profile in &mut profiles {
            if profile.language == Language::Python {
                profile.server = server.clone();
            }
        }
        profiles
    }

    fn resolver(project: &Path, server: &FakeServer) -> Resolver {
        Resolver::new(project, with_server(&scripted(server))).expect("the project should parse")
    }
}
