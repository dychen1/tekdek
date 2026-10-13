//! Artifact catalog.
//!
//! This module builds the catalog that the viewer reads as `index.json`. It
//! lists every graph file of one store with its digest, and the current
//! generation of every flow lineage under the graph that the flow binds. The
//! store directory is the input. One catalog document in memory is the output.
//! No command stores the catalog: the graph files and the flow lineages are the
//! only source, so two branches that add flows never conflict on it.
//!
//! # Sections
//!
//! `Schema` defines the catalog document.
//! `Building` builds the catalog and selects the default graph.
//! `Scanning` lists graph files.
//! `Flow listings` lists the current generation of every flow and decides its freshness.

use std::{
    collections::BTreeMap,
    ffi::OsStr,
    fs, io,
    path::{Path, PathBuf},
};

use serde::Serialize;

use crate::{
    flow::{
        lineage::{lineage_ids, read_current},
        models::FLOW_ARTIFACT_SCHEMA_VERSION,
    },
    store::{digest_of, graph::check_graph_name, read_bytes, reject_symlink},
};

// -----------------------------------------------------------------------------
// Schema
// -----------------------------------------------------------------------------

const INDEX_SCHEMA_VERSION: u32 = 3;

/// The catalog document of one store.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtifactIndex {
    schema_version: u32,
    default_artifact: String,
    artifacts: Vec<ArtifactIndexEntry>,
}

#[derive(Serialize)]
struct ArtifactIndexEntry {
    label: String,
    file: String,
    digest: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    flows: Vec<FlowListing>,
}

// -----------------------------------------------------------------------------
// Building
// -----------------------------------------------------------------------------

/// Builds the catalog of the store from its graph files and flow lineages.
///
/// # Errors
///
/// Returns an error if the store has no graph file, or if scanning or reading fails.
pub fn build_catalog(directory: &Path) -> io::Result<ArtifactIndex> {
    let files = artifact_files(directory)?;
    let default_artifact = select_default(directory, &files)?;
    artifact_index(directory, &files, default_artifact)
}

/// Returns the store paths that the viewer can read: the graph files and the current generations.
///
/// A generation path starts with `flows/`. This listing reads no graph bytes.
///
/// # Errors
///
/// Returns an error if scanning fails.
pub fn readable_files(directory: &Path) -> io::Result<Vec<String>> {
    let mut files = artifact_files(directory)?;
    for lineage in lineage_listings(directory)? {
        files.push(format!("flows/{}", lineage.file));
    }
    Ok(files)
}

/// Returns the default graph file of the store.
///
/// The only graph file is the default. With more graph files, the graph named
/// after the project folder wins, and then the first graph file in name order.
/// The project folder is the folder that holds the store.
///
/// # Errors
///
/// Returns an error if the store has no graph file or cannot be read.
pub fn default_artifact(directory: &Path) -> io::Result<String> {
    let files = artifact_files(directory)?;
    select_default(directory, &files)
}

/// Returns the graph file named after the project folder of the store, if the folder has a UTF-8 name.
#[must_use]
fn project_graph_file(directory: &Path) -> Option<String> {
    let directory = fs::canonicalize(directory).unwrap_or_else(|_| PathBuf::from(directory));
    let name = directory.parent()?.file_name()?.to_str()?;
    Some(format!("{name}.json"))
}

fn select_default(directory: &Path, files: &[String]) -> io::Result<String> {
    let first = files.first().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("the store {} has no graph file", directory.display()),
        )
    })?;
    if files.len() == 1 {
        return Ok(first.clone());
    }
    Ok(project_graph_file(directory)
        .filter(|file| files.contains(file))
        .unwrap_or_else(|| first.clone()))
}

// -----------------------------------------------------------------------------
// Scanning
// -----------------------------------------------------------------------------

/// Lists the graph files of the store in name order: every regular `*.json` file
/// whose name passes `check_graph_name`.
///
/// An `index.json` file from an older tekdek is not a graph file, and the catalog ignores it.
/// A symbolic link is not a regular file.
///
/// # Errors
///
/// Returns an error if the store is a symbolic link or cannot be listed.
fn artifact_files(directory: &Path) -> io::Result<Vec<String>> {
    reject_symlink(directory)?;
    let mut files = Vec::new();
    let entries = fs::read_dir(directory).map_err(|error| {
        io::Error::new(
            error.kind(),
            format!("cannot read {}: {error}", directory.display()),
        )
    })?;
    for entry in entries {
        let entry = entry?;
        let name = entry.file_name();
        if !entry.file_type()?.is_file() || Path::new(&name).extension() != Some(OsStr::new("json"))
        {
            continue;
        }
        match name.into_string() {
            Ok(name) if check_graph_name(&name).is_ok() => {
                files.push(name);
            }
            Ok(_) => {}
            Err(_) => eprintln!("warning: cannot list a JSON file: its name is not valid UTF-8"),
        }
    }
    files.sort();
    Ok(files)
}

/// Builds the catalog document.
fn artifact_index(
    directory: &Path,
    files: &[String],
    default_artifact: String,
) -> io::Result<ArtifactIndex> {
    let digests = graph_digests(directory, files)?;
    let mut listings: BTreeMap<String, Vec<FlowListing>> = BTreeMap::new();
    for lineage in lineage_listings(directory)? {
        listings
            .entry(lineage.graph_artifact.clone())
            .or_default()
            .push(lineage.into_listing(&digests));
    }

    let mut artifacts = Vec::new();
    for (file, digest) in digests {
        let Some(label) = file.strip_suffix(".json").map(str::to_owned) else {
            continue;
        };
        let mut flows = listings.remove(&file).unwrap_or_default();
        flows.sort_by(|left, right| left.name.cmp(&right.name));
        artifacts.push(ArtifactIndexEntry {
            label,
            file,
            digest,
            flows,
        });
    }
    for (graph_artifact, flows) in listings {
        for flow in flows {
            eprintln!(
                "warning: the flow {} names the missing artifact {graph_artifact}",
                flow.name
            );
        }
    }

    Ok(ArtifactIndex {
        schema_version: INDEX_SCHEMA_VERSION,
        default_artifact,
        artifacts,
    })
}

// -----------------------------------------------------------------------------
// Flow listings
// -----------------------------------------------------------------------------

/// One flow listing. `file` is relative to `flows/`.
///
/// A flow is stale when its bound graph file is missing or its byte digest
/// differs from the bound digest.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FlowListing {
    pub name: String,
    pub file: String,
    pub schema_version: u32,
    pub behavior: String,
    pub step_count: usize,
    pub cluster_count: usize,
    pub stale: bool,
}

/// Lists flows in name order, with an optional graph filter.
///
/// # Errors
/// Returns an error for an invalid graph name, a link, or a directory failure.
pub fn list_flows(directory: &Path, graph_file: Option<&str>) -> io::Result<Vec<FlowListing>> {
    if let Some(file) = graph_file {
        check_graph_name(file)?;
    }
    let digests = graph_digests(directory, &artifact_files(directory)?)?;
    let mut result: Vec<FlowListing> = lineage_listings(directory)?
        .into_iter()
        .filter(|lineage| graph_file.is_none_or(|file| file == lineage.graph_artifact))
        .map(|lineage| lineage.into_listing(&digests))
        .collect();
    result.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(result)
}

/// The current generation of one flow and the graph binding that decides its freshness.
struct LineageListing {
    name: String,
    file: String,
    behavior: String,
    step_count: usize,
    cluster_count: usize,
    graph_artifact: String,
    graph_digest: String,
}

impl LineageListing {
    /// Applies the staleness rule with the byte digests of the graph files.
    fn into_listing(self, digests: &BTreeMap<String, String>) -> FlowListing {
        FlowListing {
            stale: digests.get(&self.graph_artifact) != Some(&self.graph_digest),
            name: self.name,
            file: self.file,
            schema_version: FLOW_ARTIFACT_SCHEMA_VERSION,
            behavior: self.behavior,
            step_count: self.step_count,
            cluster_count: self.cluster_count,
        }
    }
}

/// Lists the current generation of every lineage. An invalid lineage produces a warning.
///
/// # Errors
/// Returns an error for a link or a directory failure.
fn lineage_listings(directory: &Path) -> io::Result<Vec<LineageListing>> {
    let mut listings = Vec::new();
    for id in lineage_ids(directory)? {
        let current = match read_current(directory, &id) {
            Ok(Some(current)) => current,
            Ok(None) => continue,
            Err(error) => {
                eprintln!("warning: skipping the flow {id}: {error}");
                continue;
            }
        };
        let artifact = current.artifact;
        listings.push(LineageListing {
            name: id.to_string(),
            file: current.file,
            behavior: artifact.intent.behavior,
            step_count: artifact.intent.steps.len(),
            cluster_count: artifact.intent.clusters.len(),
            graph_artifact: artifact.facts.binding.graph_artifact,
            graph_digest: artifact.facts.binding.graph_digest.to_string(),
        });
    }
    Ok(listings)
}

/// Hashes each graph file once and returns the byte digest of each file by name.
fn graph_digests(directory: &Path, files: &[String]) -> io::Result<BTreeMap<String, String>> {
    let mut digests = BTreeMap::new();
    for file in files {
        let bytes = read_bytes(&directory.join(file))?;
        digests.insert(file.clone(), digest_of(&bytes));
    }
    Ok(digests)
}

// -----------------------------------------------------------------------------
// Tests
// -----------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::fs;

    use serde_json::Value;

    use super::{build_catalog, default_artifact, list_flows, readable_files};
    use crate::{
        flow::fixtures::{self, frozen, import, output},
        store::digest_of,
    };

    fn catalog(directory: &std::path::Path) -> Value {
        serde_json::to_value(build_catalog(directory).expect("catalog")).expect("catalog JSON")
    }

    #[test]
    fn lists_graph_files_by_name_and_ignores_an_old_index_file() {
        let directory = tempfile::tempdir().expect("store");
        fs::write(directory.path().join("zeta.json"), "{}").expect("artifact");
        fs::write(directory.path().join("alpha.json"), "{}").expect("artifact");
        fs::write(directory.path().join("index.json"), "<<<<<<< HEAD").expect("old index");

        let document = catalog(directory.path());

        assert_eq!(document["schemaVersion"], 3);
        assert_eq!(document["defaultArtifact"], "alpha.json");
        assert_eq!(document["artifacts"][0]["label"], "alpha");
        assert_eq!(document["artifacts"][0]["digest"], digest_of(b"{}"));
        assert_eq!(document["artifacts"][1]["file"], "zeta.json");
        assert_eq!(document["artifacts"].as_array().map(Vec::len), Some(2));
        assert!(document["artifacts"][0].get("flows").is_none());
    }

    #[test]
    fn lists_only_graph_files_that_the_store_can_read() {
        let directory = tempfile::tempdir().expect("store");
        for file in ["app.json", "a:b.json", " app.json", ".hidden.json"] {
            fs::write(directory.path().join(file), "{}").expect("artifact");
        }

        assert_eq!(
            readable_files(directory.path()).expect("files"),
            ["app.json"]
        );
    }

    #[test]
    fn lists_a_frozen_flow_under_its_graph_and_marks_it_stale_after_a_change() {
        let directory = fixtures::store();
        let path = directory.path();
        let generation = frozen(import(path, "demo-flow", &output()));

        let document = catalog(path);
        let flow = &document["artifacts"][0]["flows"][0];
        assert_eq!(
            *flow,
            serde_json::json!({
                "name": "demo-flow",
                "file": generation.file,
                "schemaVersion": 3,
                "behavior": "How the demo command parses input and saves a result.",
                "stepCount": 6,
                "clusterCount": 2,
                "stale": false
            })
        );
        assert_eq!(
            readable_files(path).expect("files"),
            [
                "graph.json".to_owned(),
                format!("flows/{}", generation.file)
            ]
        );
        fixtures::write_graph(path, "src/extra.rs::callable:another");
        assert_eq!(catalog(path)["artifacts"][0]["flows"][0]["stale"], true);
    }

    #[test]
    fn selects_the_project_graph_as_the_default_and_then_the_first_graph() {
        let directory = tempfile::tempdir().expect("project");
        let project = directory.path().join("acme");
        let store = project.join(".tekdek");
        fs::create_dir_all(&store).expect("store");
        fs::write(store.join("worker.json"), "{}").expect("artifact");
        assert_eq!(default_artifact(&store).expect("default"), "worker.json");

        fs::write(store.join("api.json"), "{}").expect("artifact");
        assert_eq!(default_artifact(&store).expect("default"), "api.json");

        fs::write(store.join("acme.json"), "{}").expect("artifact");
        assert_eq!(default_artifact(&store).expect("default"), "acme.json");
    }

    #[test]
    fn rejects_a_store_without_a_graph_file() {
        let directory = tempfile::tempdir().expect("store");

        let error = build_catalog(directory.path()).err().expect("no graph");

        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
    }

    #[test]
    fn flow_list_and_the_catalog_agree_on_freshness() {
        let directory = fixtures::store();
        let path = directory.path();
        frozen(import(path, "demo-flow", &output()));
        let listed = |path: &std::path::Path| {
            serde_json::to_value(list_flows(path, None).expect("list")).expect("list JSON")
        };

        assert_eq!(listed(path), catalog(path)["artifacts"][0]["flows"]);
        assert_eq!(listed(path)[0]["stale"], false);
        fixtures::write_graph(path, "src/extra.rs::callable:another");
        assert_eq!(listed(path), catalog(path)["artifacts"][0]["flows"]);
        assert_eq!(listed(path)[0]["stale"], true);
        fs::remove_file(path.join("graph.json")).expect("remove the graph");
        assert_eq!(listed(path)[0]["stale"], true);
        assert!(
            list_flows(path, Some("other.json"))
                .expect("list")
                .is_empty()
        );
    }

    #[test]
    fn flow_list_names_a_store_folder_that_does_not_exist() {
        let directory = tempfile::tempdir().expect("project");
        let missing = directory.path().join("no-store");

        let error = list_flows(&missing, None).expect_err("missing store");

        assert_eq!(error.kind(), std::io::ErrorKind::NotFound);
        assert!(
            error.to_string().contains(&missing.display().to_string()),
            "{error}"
        );
    }
}
