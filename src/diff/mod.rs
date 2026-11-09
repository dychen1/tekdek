//! PR diffs.
//!
//! This module freezes one diff: a Before flow on the base commit, an After
//! flow on the head commit, the step pairs and cluster notes that an agent
//! writes, and one row of derived facts per step. The store, git, and the agent
//! document are the inputs. One file `diffs/<name>.json` in the store is the
//! output.
//!
//! A diff exists only when a user asks for one. A flow import never makes one.
//!
//! # Sections
//!
//! `Prepare` checks out and indexes the base commit and lists the flows that fit it.
//! `Import` checks and freezes one agent document.
//! `Verify` derives a frozen diff again from git.
//! `Store` reads and lists frozen diffs.
//! `Errors` defines the failures.

pub mod derive;
pub mod git;
pub mod lines;
pub mod models;

use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet},
    fmt, fs, io,
    path::{Path, PathBuf},
};

use serde::Serialize;

use self::{
    derive::{DiffIssue, Reader, check_agent, derive_rows, rederive_rows},
    models::{
        AgentDiff, ClusterNote, DIFF_ARTIFACT_SCHEMA_VERSION, DiffArtifact, DiffKind, DiffRow,
        DiffSide, Side, StepPair,
    },
};
use crate::{
    catalog::default_artifact,
    flow::{
        canonical::{self, CanonicalError},
        derive::digests,
        lineage::{LineageError, read_current},
        models::{Author, FlowArtifact, FlowId, Provenance, Sha256Digest},
        validate::check_author,
    },
    store::{atomic_write, graph::read_graph, lock_store, read_bytes},
};

/// The JSON Schema of the agent diff document.
pub const AGENT_DIFF_SCHEMA: &str = include_str!("agent-diff.schema.json");

/// The store folder of frozen diffs.
pub const DIFFS: &str = "diffs";

// -----------------------------------------------------------------------------
// Prepare
// -----------------------------------------------------------------------------

/// The checked-out and indexed base of one diff.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Prepared {
    pub base_commit: String,
    pub head_commit: String,
    /// The project folder inside the base worktree.
    pub base_root: PathBuf,
    pub base_store: PathBuf,
    pub base_graph: String,
    /// The flows whose every step function is unchanged on the base.
    pub fitting_flows: Vec<FittingFlow>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FittingFlow {
    pub name: String,
    pub store: PathBuf,
    pub steps: usize,
}

/// Indexes one project folder into its store with one graph file name.
pub type Indexer<'a> = dyn Fn(&Path, &str) -> Result<(), String> + 'a;

/// Checks out the base commit in a worktree, indexes it, and lists the flows that fit it.
///
/// The worktree is in the git folder of the repository, so the project graph
/// does not change. An existing worktree for the same commit is used again. The base
/// graph gets the file name of the project graph.
///
/// # Errors
/// Returns an error when git, the index, or a store read fails.
pub fn prepare(
    store: &Path,
    base: Option<&str>,
    index: &Indexer<'_>,
) -> Result<Prepared, DiffError> {
    let root = project_root(store)?;
    let base_commit = match base {
        Some(base) => git::commit(&root, base)?,
        None => git::default_base(&root)?,
    };
    let head_commit = git::commit(&root, "HEAD")?;
    let (worktree, base_root) = base_paths(&root, &base_commit)?;
    if worktree.exists() {
        if !git::is_worktree(&root, &worktree)? {
            return Err(DiffError::Git(format!(
                "{} is not a worktree of this repository: remove it",
                worktree.display()
            )));
        }
        if git::commit(&worktree, "HEAD")? != base_commit {
            return Err(DiffError::Git(format!(
                "{} holds another commit: remove it",
                worktree.display()
            )));
        }
    } else {
        git::add_worktree(&root, &worktree, &base_commit)?;
    }
    let graph = default_artifact(store)?;
    index(&base_root, graph.trim_end_matches(".json")).map_err(DiffError::Index)?;
    let base_store = base_root.join(crate::store::PROJECT_STORE);
    let (base_graph, _) = read_graph(&base_store, &graph)?;
    let mut fitting_flows = Vec::new();
    for directory in [&base_store, &store.to_path_buf()] {
        for flow_id in crate::flow::lineage::lineage_ids(directory).unwrap_or_default() {
            let Ok(Some(current)) = read_current(directory, &flow_id) else {
                continue;
            };
            if fits(&current.artifact, &base_graph) {
                fitting_flows.push(FittingFlow {
                    name: flow_id.to_string(),
                    store: directory.clone(),
                    steps: current.artifact.intent.steps.len(),
                });
            }
        }
    }
    Ok(Prepared {
        base_commit,
        head_commit,
        base_root,
        base_store,
        base_graph: graph,
        fitting_flows,
    })
}

/// True when every node of the flow is in the graph with the same content hash.
fn fits(flow: &FlowArtifact, graph: &crate::store::graph::ArtifactGraph) -> bool {
    let hashes: BTreeMap<&str, Option<&str>> = graph
        .nodes
        .iter()
        .map(|node| (node.key.as_str(), node.content_hash.as_deref()))
        .collect();
    flow.facts.nodes.iter().all(|node| {
        let hash = node.content_hash.as_ref().map(ToString::to_string);
        hash.is_some() && hashes.get(node.key.as_str()).copied().flatten() == hash.as_deref()
    })
}

/// Returns the base worktree and the project folder inside it for one commit.
///
/// The worktree is in the git folder of the repository: only the repository
/// owner can write there, and no index of the project reads it.
fn base_paths(root: &Path, commit: &str) -> Result<(PathBuf, PathBuf), DiffError> {
    let worktree = git::common_dir(root)?
        .join("tekdek-diff")
        .join(&commit[..commit.len().min(12)]);
    let base_root = worktree.join(git::prefix(root)?);
    Ok((worktree, base_root))
}

fn project_root(store: &Path) -> Result<PathBuf, DiffError> {
    let store = fs::canonicalize(store)?;
    store
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| DiffError::Git("the store has no project folder".to_owned()))
}

// -----------------------------------------------------------------------------
// Import
// -----------------------------------------------------------------------------

/// One import request. `author` comes from the invocation, never from the agent.
pub struct ImportRequest<'a> {
    pub name: &'a str,
    pub base: &'a str,
    pub before: &'a str,
    pub after: &'a str,
    pub input: &'a [u8],
    pub author: Author,
    pub dry_run: bool,
}

/// The result of one successful freeze.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FrozenDiff {
    pub name: String,
    pub file: String,
    pub digest: Sha256Digest,
    pub base_commit: String,
    pub head_commit: String,
    pub counts: BTreeMap<String, usize>,
    pub added: u32,
    pub removed: u32,
    pub dry_run: bool,
}

/// Checks one agent document and freezes the diff.
///
/// The Before flow comes from the base store when it is there, and otherwise
/// from the project store. The After flow comes from the project store and must
/// be fresh. A freeze removes the base worktree.
///
/// # Errors
/// Returns `Rejected` with every issue when the document or a derivation is
/// invalid; nothing is written then.
pub fn import(store: &Path, request: &ImportRequest<'_>) -> Result<FrozenDiff, DiffError> {
    let name = FlowId::try_from(request.name).map_err(|error| reject("/name", error))?;
    check_author(&request.author).map_err(|message| reject("/provenance/author", message))?;
    let agent: AgentDiff =
        serde_json::from_slice(request.input).map_err(|error| reject("", error))?;
    let root = project_root(store)?;
    let base_commit = git::commit(&root, request.base)?;
    let head_commit = git::commit(&root, "HEAD")?;
    let (worktree, base_root) = base_paths(&root, &base_commit)?;
    let base_store = base_root.join(crate::store::PROJECT_STORE);
    if !base_store.is_dir() {
        return Err(DiffError::Git(format!(
            "the base {base_commit} is not prepared: run tekdek diff prepare --base {}",
            request.base
        )));
    }
    let before =
        read_flow(&base_store, request.before).or_else(|_| read_flow(store, request.before))?;
    let after = read_flow(store, request.after)?;
    let binding = &after.facts.binding;
    let (head_graph, digest) = read_graph(store, &binding.graph_artifact)?;
    if digest != binding.graph_digest.as_str() {
        return Err(reject(
            "/after",
            format!(
                "the After flow {} is stale: freeze it again on the current graph",
                request.after
            ),
        ));
    }
    let (base_graph, _) = read_graph(&base_store, &default_artifact(&base_store)?)?;
    let (pairs, notes) = check_agent(&agent, &before, &after).map_err(DiffError::Rejected)?;
    let read = git_reader(&root, &base_commit, &head_commit);
    let rows = derive_rows(&before, &after, &pairs, [&base_graph, &head_graph], &read)
        .map_err(DiffError::Rejected)?;
    let mut artifact = DiffArtifact {
        kind: DiffKind::Diff,
        schema_version: DIFF_ARTIFACT_SCHEMA_VERSION,
        name,
        base: DiffSide {
            commit: base_commit.clone(),
            flow: before,
        },
        head: DiffSide {
            commit: head_commit.clone(),
            flow: after,
        },
        pairs,
        notes,
        rows,
        digest: Sha256Digest::of_bytes(b""),
        provenance: Provenance {
            frozen_at: jiff::Timestamp::now()
                .strftime("%Y-%m-%dT%H:%M:%SZ")
                .to_string(),
            cli_version: env!("CARGO_PKG_VERSION").to_owned(),
            author: request.author.clone(),
        },
    };
    artifact.digest = body_digest(&artifact)?;
    let problems = check_artifact(&artifact, &read);
    if !problems.is_empty() {
        return Err(DiffError::SelfCheck(problems));
    }
    let file = format!("{DIFFS}/{}.json", artifact.name);
    if !request.dry_run {
        let _lock = lock_store(store)?;
        fs::create_dir_all(store.join(DIFFS))?;
        let mut bytes = serde_json::to_vec_pretty(&artifact)?;
        bytes.push(b'\n');
        atomic_write(&store.join(&file), &bytes)?;
        if let Err(error) = git::remove_worktree(&root, &worktree) {
            eprintln!("warning: cannot remove the base worktree: {error}");
        }
    }
    let mut counts = BTreeMap::new();
    for row in &artifact.rows {
        *counts.entry(models_kind(row)).or_insert(0) += 1;
    }
    Ok(FrozenDiff {
        name: artifact.name.to_string(),
        file,
        digest: artifact.digest.clone(),
        base_commit: artifact.base.commit.clone(),
        head_commit: artifact.head.commit.clone(),
        counts,
        added: artifact.rows.iter().map(|row| row.added).sum(),
        removed: artifact.rows.iter().map(|row| row.removed).sum(),
        dry_run: request.dry_run,
    })
}

fn models_kind(row: &DiffRow) -> String {
    serde_json::to_value(row.kind)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_default()
}

fn read_flow(store: &Path, name: &str) -> Result<FlowArtifact, DiffError> {
    let flow_id = FlowId::try_from(name).map_err(|error| reject("/flow", error))?;
    read_current(store, &flow_id)?
        .map(|stored| stored.artifact)
        .ok_or_else(|| DiffError::UnknownFlow(name.to_owned()))
}

/// Returns a reader of file text at the base and head commits. It reads each file once.
fn git_reader<'a>(
    root: &'a Path,
    base: &'a str,
    head: &'a str,
) -> impl Fn(Side, &str) -> Option<String> + 'a {
    let cache: RefCell<BTreeMap<(Side, String), Option<String>>> = RefCell::default();
    move |side, path| {
        let key = (side, path.to_owned());
        if let Some(text) = cache.borrow().get(&key) {
            return text.clone();
        }
        let commit = match side {
            Side::Before => base,
            Side::After => head,
        };
        let text = git::show(root, commit, path);
        cache.borrow_mut().insert(key, text.clone());
        text
    }
}

/// Returns the digest of every field except `digest` and `provenance`.
fn body_digest(artifact: &DiffArtifact) -> Result<Sha256Digest, CanonicalError> {
    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    struct Body<'a> {
        kind: DiffKind,
        schema_version: u32,
        name: &'a FlowId,
        base: &'a DiffSide,
        head: &'a DiffSide,
        pairs: &'a [StepPair],
        notes: &'a [ClusterNote],
        rows: &'a [DiffRow],
    }
    canonical::digest(&Body {
        kind: artifact.kind,
        schema_version: artifact.schema_version,
        name: &artifact.name,
        base: &artifact.base,
        head: &artifact.head,
        pairs: &artifact.pairs,
        notes: &artifact.notes,
        rows: &artifact.rows,
    })
}

// -----------------------------------------------------------------------------
// Verify
// -----------------------------------------------------------------------------

/// The result of one verify.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VerifyReport {
    pub name: String,
    pub valid: bool,
    /// False when git no longer has a commit of the diff. The diff is then not invalid.
    pub verifiable: bool,
    pub issues: Vec<DiffIssue>,
}

/// Checks the digests of one frozen diff and derives its rows again from git.
///
/// # Errors
/// Returns an error when the diff cannot be read.
pub fn verify(store: &Path, name: &str) -> Result<VerifyReport, DiffError> {
    let artifact = read_diff(store, name)?;
    let root = project_root(store)?;
    let commits = [&artifact.base.commit, &artifact.head.commit];
    if let Some(missing) = commits
        .into_iter()
        .find(|commit| git::commit(&root, commit).is_err())
    {
        return Ok(VerifyReport {
            name: name.to_owned(),
            valid: false,
            verifiable: false,
            issues: vec![DiffIssue {
                path: "/base/commit".to_owned(),
                message: format!("git has no commit {missing}: the diff cannot be verified"),
            }],
        });
    }
    let read = git_reader(&root, &artifact.base.commit, &artifact.head.commit);
    let issues = check_artifact(&artifact, &read);
    Ok(VerifyReport {
        name: name.to_owned(),
        valid: issues.is_empty(),
        verifiable: true,
        issues,
    })
}

/// Returns every problem of one diff: digests, references, and rows that git does not give again.
fn check_artifact(artifact: &DiffArtifact, read: &Reader<'_>) -> Vec<DiffIssue> {
    let mut issues = Vec::new();
    let mut issue = |path: &str, message: String| {
        issues.push(DiffIssue {
            path: path.to_owned(),
            message,
        });
    };
    if artifact.schema_version != DIFF_ARTIFACT_SCHEMA_VERSION {
        issue(
            "/schemaVersion",
            format!("use schema version {DIFF_ARTIFACT_SCHEMA_VERSION}"),
        );
    }
    match body_digest(artifact) {
        Ok(digest) if digest == artifact.digest => {}
        _ => issue("/digest", "the digest does not match the diff".to_owned()),
    }
    for (path, flow) in [
        ("/base/flow", &artifact.base.flow),
        ("/head/flow", &artifact.head.flow),
    ] {
        if digests(flow).ok().as_ref() != Some(&flow.digests) {
            issue(path, "the flow digests do not match the flow".to_owned());
        }
    }
    issues.extend(check_references(artifact));
    let (before, after) = (&artifact.base.flow, &artifact.head.flow);
    let mut issue = |path: &str, message: String| {
        issues.push(DiffIssue {
            path: path.to_owned(),
            message,
        });
    };
    let spans: Vec<_> = artifact
        .rows
        .iter()
        .map(|row| (row.base_span.clone(), row.head_span.clone()))
        .collect();
    let rows = rederive_rows(before, after, &artifact.pairs, &spans, read);
    if rows.len() != artifact.rows.len() {
        issue("/rows", format!("expected {} rows", rows.len()));
    }
    for (index, (stored, derived)) in artifact.rows.iter().zip(&rows).enumerate() {
        if stored != derived {
            issue(
                &format!("/rows/{index}"),
                "the row does not match git: the commits or the flows changed".to_owned(),
            );
        }
    }
    issues
}

/// Returns every pair, note, and span that does not name a step, cluster, or file of the two flows.
fn check_references(artifact: &DiffArtifact) -> Vec<DiffIssue> {
    let mut issues = Vec::new();
    let mut issue = |path: &str, message: String| {
        issues.push(DiffIssue {
            path: path.to_owned(),
            message,
        });
    };
    let (before, after) = (&artifact.base.flow, &artifact.head.flow);
    let ids = |flow: &FlowArtifact| -> BTreeSet<String> {
        flow.intent
            .steps
            .iter()
            .map(|step| step.id.to_string())
            .collect()
    };
    let (old, new) = (ids(before), ids(after));
    let mut seen = (BTreeSet::new(), BTreeSet::new());
    for (index, pair) in artifact.pairs.iter().enumerate() {
        if !old.contains(pair.before.as_str()) || !seen.0.insert(pair.before.as_str()) {
            issue(
                &format!("/pairs/{index}/before"),
                "unknown or repeated step".to_owned(),
            );
        }
        if !new.contains(pair.after.as_str()) || !seen.1.insert(pair.after.as_str()) {
            issue(
                &format!("/pairs/{index}/after"),
                "unknown or repeated step".to_owned(),
            );
        }
    }
    for (index, note) in artifact.notes.iter().enumerate() {
        let flow = if note.side == Side::Before {
            before
        } else {
            after
        };
        if !flow
            .intent
            .clusters
            .iter()
            .any(|cluster| cluster.key == note.cluster_key)
        {
            issue(
                &format!("/notes/{index}/clusterKey"),
                "unknown cluster".to_owned(),
            );
        }
    }
    for (index, row) in artifact.rows.iter().enumerate() {
        let path = |id: &Option<crate::flow::models::StepId>, flow: &FlowArtifact| {
            let step = flow
                .intent
                .steps
                .iter()
                .find(|step| Some(&step.id) == id.as_ref())?;
            let node = flow
                .facts
                .nodes
                .iter()
                .find(|node| node.key == step.node_key)?;
            Some(node.path.clone())
        };
        // A span lies in the file of its own step, or of the other step for a new or removed step.
        let base = path(&row.before_step, before).or_else(|| path(&row.after_step, after));
        let head = path(&row.after_step, after).or_else(|| path(&row.before_step, before));
        for (name, span, expected) in [
            ("baseSpan", &row.base_span, base),
            ("headSpan", &row.head_span, head),
        ] {
            if let Some(span) = span
                && Some(&span.path) != expected.as_ref()
            {
                issue(
                    &format!("/rows/{index}/{name}"),
                    "the span is not in the file of its step".to_owned(),
                );
            }
        }
    }
    issues
}

// -----------------------------------------------------------------------------
// Store
// -----------------------------------------------------------------------------

/// Reads one frozen diff.
///
/// # Errors
/// Returns an error for an invalid name, a missing file, or an invalid document.
pub fn read_diff(store: &Path, name: &str) -> Result<DiffArtifact, DiffError> {
    let name = FlowId::try_from(name).map_err(|error| reject("/name", error))?;
    let bytes = read_bytes(&store.join(DIFFS).join(format!("{name}.json"))).map_err(|error| {
        if error.kind() == io::ErrorKind::NotFound {
            DiffError::UnknownDiff(name.to_string())
        } else {
            DiffError::Io(error)
        }
    })?;
    Ok(serde_json::from_slice(&bytes)?)
}

/// One frozen diff in a listing.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiffListing {
    pub name: String,
    /// The path relative to the store.
    pub file: String,
    pub base_commit: String,
    pub head_commit: String,
    pub behavior: String,
}

/// Returns the store paths of the diff files, `diffs/<name>.json`, in name order. It parses no file.
///
/// # Errors
/// Returns an error when the folder cannot be read.
pub fn diff_files(store: &Path) -> io::Result<Vec<String>> {
    let entries = match fs::read_dir(store.join(DIFFS)) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    let mut files = Vec::new();
    for entry in entries {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name.to_str().and_then(|file| file.strip_suffix(".json")) else {
            continue;
        };
        if entry.file_type()?.is_file() && FlowId::try_from(name).is_ok() {
            files.push(format!("{DIFFS}/{name}.json"));
        }
    }
    files.sort();
    Ok(files)
}

/// Lists the frozen diffs of a store in name order. An unreadable diff is left out.
///
/// # Errors
/// Returns an error when the folder cannot be read.
pub fn list_diffs(store: &Path) -> io::Result<Vec<DiffListing>> {
    let mut listings = Vec::new();
    for file in diff_files(store)? {
        let name = file
            .trim_start_matches("diffs/")
            .trim_end_matches(".json")
            .to_owned();
        if let Ok(artifact) = read_diff(store, &name) {
            listings.push(DiffListing {
                file,
                name,
                base_commit: artifact.base.commit,
                head_commit: artifact.head.commit,
                behavior: artifact.head.flow.intent.behavior,
            });
        }
    }
    Ok(listings)
}

// -----------------------------------------------------------------------------
// Errors
// -----------------------------------------------------------------------------

#[derive(Debug)]
pub enum DiffError {
    Rejected(Vec<DiffIssue>),
    SelfCheck(Vec<DiffIssue>),
    UnknownFlow(String),
    UnknownDiff(String),
    Git(String),
    Index(String),
    Lineage(LineageError),
    Canonical(CanonicalError),
    Json(serde_json::Error),
    Io(io::Error),
}

impl DiffError {
    /// Returns the issues of a rejected document.
    #[must_use]
    pub fn issues(&self) -> &[DiffIssue] {
        match self {
            Self::Rejected(issues) | Self::SelfCheck(issues) => issues,
            _ => &[],
        }
    }
}

fn reject(path: &str, message: impl fmt::Display) -> DiffError {
    DiffError::Rejected(vec![DiffIssue {
        path: path.to_owned(),
        message: message.to_string(),
    }])
}

impl fmt::Display for DiffError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            // Summarize a rejected document; the issues carry the details.
            Self::Rejected(issues) => write!(
                formatter,
                "validation_failed: the diff document has {} errors, and nothing was written",
                issues.len()
            ),
            // Report an internal consistency fault before any write.
            Self::SelfCheck(issues) => write!(
                formatter,
                "the new diff failed its own verification with {} errors, and nothing was written",
                issues.len()
            ),
            // Name the missing flow.
            Self::UnknownFlow(name) => {
                write!(
                    formatter,
                    "unknown_flow: the flow {name} has no frozen generation"
                )
            }
            // Name the missing diff.
            Self::UnknownDiff(name) => {
                write!(formatter, "unknown_diff: the diff {name} does not exist")
            }
            // Preserve the git or index message.
            Self::Git(message) | Self::Index(message) => write!(formatter, "{message}"),
            // Preserve the lineage error.
            Self::Lineage(error) => write!(formatter, "{error}"),
            // Preserve the serialization error.
            Self::Canonical(error) => write!(formatter, "{error}"),
            // Preserve the document error.
            Self::Json(error) => write!(formatter, "{error}"),
            // Preserve the file error.
            Self::Io(error) => write!(formatter, "{error}"),
        }
    }
}

impl std::error::Error for DiffError {}

impl From<LineageError> for DiffError {
    fn from(error: LineageError) -> Self {
        Self::Lineage(error)
    }
}

impl From<CanonicalError> for DiffError {
    fn from(error: CanonicalError) -> Self {
        Self::Canonical(error)
    }
}

impl From<serde_json::Error> for DiffError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

impl From<io::Error> for DiffError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::flow::{fixtures, models::Author};

    /// Builds a valid diff of the fixture flow with itself, where the first step's function changed.
    fn artifact(read: &Reader<'_>) -> DiffArtifact {
        let store = fixtures::store();
        fixtures::frozen(fixtures::import(store.path(), "demo", &fixtures::output()));
        let flow = fixtures::current(store.path(), "demo");
        let pairs: Vec<StepPair> = flow
            .intent
            .steps
            .iter()
            .map(|step| StepPair {
                before: step.id.clone(),
                after: step.id.clone(),
            })
            .collect();
        let spans: Vec<_> = flow
            .intent
            .steps
            .iter()
            .map(|step| {
                let node = flow
                    .facts
                    .nodes
                    .iter()
                    .find(|node| node.key == step.node_key);
                let span = node.map(|node| models::LineSpan {
                    path: node.path.clone(),
                    start: 1,
                    end: 2,
                });
                (span.clone(), span)
            })
            .collect();
        let rows = rederive_rows(&flow, &flow, &pairs, &spans, read);
        let mut artifact = DiffArtifact {
            kind: DiffKind::Diff,
            schema_version: DIFF_ARTIFACT_SCHEMA_VERSION,
            name: FlowId::try_from("demo-diff").expect("name"),
            base: DiffSide {
                commit: "a".repeat(40),
                flow: flow.clone(),
            },
            head: DiffSide {
                commit: "b".repeat(40),
                flow,
            },
            pairs,
            notes: Vec::new(),
            rows,
            digest: Sha256Digest::of_bytes(b""),
            provenance: Provenance {
                frozen_at: "2026-11-08T00:00:00Z".to_owned(),
                cli_version: "0.0.0".to_owned(),
                author: Author::System,
            },
        };
        artifact.digest = body_digest(&artifact).expect("digest");
        artifact
    }

    #[expect(
        clippy::unnecessary_wraps,
        reason = "a reader returns None for a missing file"
    )]
    fn read(side: Side, _: &str) -> Option<String> {
        Some(match side {
            Side::Before => "fn main() {\n    old();\n}\n".to_owned(),
            Side::After => "fn main() {\n    new();\n}\n".to_owned(),
        })
    }

    #[test]
    fn a_diff_checks_clean_against_the_commits_it_came_from() {
        let artifact = artifact(&read);
        assert_eq!(check_artifact(&artifact, &read), []);
        assert_eq!(artifact.rows[0].kind, models::ChangeKind::Changed);
    }

    #[test]
    fn a_changed_count_or_changed_commit_text_fails_the_check() {
        let mut tampered = artifact(&read);
        tampered.rows[0].added = 9;
        let paths: Vec<_> = check_artifact(&tampered, &read)
            .into_iter()
            .map(|issue| issue.path)
            .collect();
        assert_eq!(paths, ["/digest", "/rows/0"]);

        let mut moved = artifact(&read);
        moved.rows[0].head_span.as_mut().expect("span").path = "src/other.rs".to_owned();
        let paths: Vec<_> = check_artifact(&moved, &read)
            .into_iter()
            .map(|issue| issue.path)
            .collect();
        assert!(paths.contains(&"/rows/0/headSpan".to_owned()), "{paths:?}");

        let other = |_: Side, _: &str| Some("fn main() {}\n".to_owned());
        assert_ne!(check_artifact(&artifact(&read), &other), []);
    }
}
