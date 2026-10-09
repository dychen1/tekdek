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
//! `Scanning` lists graph files and flow lineages.

use std::{
    collections::BTreeMap,
    ffi::OsStr,
    fs, io,
    path::{Path, PathBuf},
};

use serde::Serialize;

use crate::{
    flow::lineage::{FlowListing, lineage_listings},
    store::{digest_of, reject_symlink},
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

impl ArtifactIndex {
    /// Returns the graph file that lists the flow, or `None` when no graph lists it.
    #[must_use]
    pub fn graph_of_flow(&self, flow: &str) -> Option<&str> {
        self.artifacts
            .iter()
            .find(|artifact| artifact.flows.iter().any(|listing| listing.name == flow))
            .map(|artifact| artifact.file.as_str())
    }
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
        files.push(format!("flows/{}", lineage.listing.file));
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
pub fn project_graph_file(directory: &Path) -> Option<String> {
    let directory = fs::canonicalize(directory).unwrap_or_else(|_| PathBuf::from(directory));
    let name = directory.parent()?.file_name()?.to_str()?;
    Some(format!("{name}.json"))
}

fn select_default(directory: &Path, files: &[String]) -> io::Result<String> {
    let first = files.first().ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, "the store has no graph file")
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
/// that does not start with a dot, except `index.json`.
///
/// An `index.json` file from an older tekdek is not a graph file, and the catalog ignores it.
/// A symbolic link is not a regular file.
///
/// # Errors
///
/// Returns an error if the store is a symbolic link or cannot be listed.
pub fn artifact_files(directory: &Path) -> io::Result<Vec<String>> {
    reject_symlink(directory)?;
    let mut files = Vec::new();
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let name = entry.file_name();
        if !entry.file_type()?.is_file() || Path::new(&name).extension() != Some(OsStr::new("json"))
        {
            continue;
        }
        match name.into_string() {
            Ok(name) if !name.starts_with('.') && !name.eq_ignore_ascii_case("index.json") => {
                files.push(name);
            }
            Ok(_) => {}
            Err(_) => eprintln!("warning: cannot list a JSON file: its name is not valid UTF-8"),
        }
    }
    files.sort();
    Ok(files)
}

/// Builds the catalog document. A flow is stale when its graph digest differs from the file.
fn artifact_index(
    directory: &Path,
    files: &[String],
    default_artifact: String,
) -> io::Result<ArtifactIndex> {
    let mut listings: BTreeMap<String, Vec<(String, FlowListing)>> = BTreeMap::new();
    for lineage in lineage_listings(directory)? {
        listings
            .entry(lineage.graph_artifact)
            .or_default()
            .push((lineage.graph_digest, lineage.listing));
    }

    let mut artifacts = Vec::new();
    for file in files {
        let Some(label) = file.strip_suffix(".json") else {
            continue;
        };
        let path = directory.join(file);
        let bytes = fs::read(&path).map_err(|error| {
            io::Error::new(
                error.kind(),
                format!("cannot read {}: {error}", path.display()),
            )
        })?;
        let digest = digest_of(&bytes);
        let mut flows: Vec<FlowListing> = listings
            .remove(file.as_str())
            .unwrap_or_default()
            .into_iter()
            .map(|(bound, listing)| FlowListing {
                stale: bound != digest,
                ..listing
            })
            .collect();
        flows.sort_by(|left, right| left.name.cmp(&right.name));
        artifacts.push(ArtifactIndexEntry {
            label: label.to_owned(),
            file: file.clone(),
            digest,
            flows,
        });
    }
    for (graph_artifact, flows) in listings {
        for (_, flow) in flows {
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
// Tests
// -----------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::fs;

    use serde_json::Value;

    use super::{build_catalog, default_artifact, readable_files};
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
            build_catalog(path)
                .expect("catalog")
                .graph_of_flow("demo-flow"),
            Some("graph.json")
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
}
