//! Artifact catalog.
//!
//! This module owns `index.json`, the catalog that the viewer reads. It lists
//! every graph artifact of one store with its digest, and the current
//! generation of every flow lineage under the graph that the flow binds. The
//! store directory is the input. One index document is the output.
//!
//! # Sections
//!
//! `Schema` defines the index document.
//! `Writing` rebuilds and writes the index.
//! `Scanning` lists graph files and flow lineages.

use std::{collections::BTreeMap, ffi::OsStr, fs, io, path::Path};

use serde::Serialize;

use crate::{
    flow::lineage::{FlowListing, lineage_listings, lineage_state},
    store::{digest_of, lock_store, reject_symlink, write_json},
};

// -----------------------------------------------------------------------------
// Schema
// -----------------------------------------------------------------------------

const INDEX_SCHEMA_VERSION: u32 = 3;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ArtifactIndex {
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
// Writing
// -----------------------------------------------------------------------------

/// Rebuilds the index with `default_artifact` as the default.
///
/// The writer rescans after writing and retries if a graph file or a flow
/// pointer changed meanwhile.
///
/// # Errors
///
/// Returns an error if scanning, reading, serialization, or writing fails, or if
/// the default artifact is not in the store.
pub fn write_artifact_index(directory: &Path, default_artifact: &str) -> io::Result<()> {
    let _lock = lock_store(directory)?;
    write_locked(directory, default_artifact)
}

/// Rebuilds the index and keeps the current default artifact.
///
/// A missing or unreadable index falls back to the first artifact file.
///
/// # Errors
///
/// Returns an error if the store has no artifact or writing fails.
pub fn refresh_artifact_index(directory: &Path) -> io::Result<()> {
    let _lock = lock_store(directory)?;
    let default = default_artifact(directory)?;
    write_locked(directory, &default)
}

/// Returns the default graph file of the store.
///
/// The index default wins when it names a graph file of the store. A missing
/// or unreadable index falls back to the first graph file in name order.
///
/// # Errors
///
/// Returns an error if the store has no graph file or cannot be read.
pub fn default_artifact(directory: &Path) -> io::Result<String> {
    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct ExistingIndex {
        default_artifact: String,
    }

    reject_symlink(directory)?;
    let files = artifact_files(directory)?;
    let first_artifact = files.first().cloned().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "the artifact directory is empty",
        )
    })?;
    reject_symlink(&directory.join("index.json"))?;
    let existing = fs::read_to_string(directory.join("index.json"))
        .ok()
        .and_then(|text| serde_json::from_str::<ExistingIndex>(&text).ok())
        .map(|index| index.default_artifact);
    Ok(existing
        .filter(|file| files.contains(file))
        .unwrap_or(first_artifact))
}

fn write_locked(directory: &Path, default_artifact: &str) -> io::Result<()> {
    loop {
        let files = artifact_files(directory)?;
        let lineages = lineage_state(directory)?;
        let index = artifact_index(directory, &files, default_artifact)?;
        if !index
            .artifacts
            .iter()
            .any(|artifact| artifact.file == default_artifact)
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "the default artifact is not in the artifact directory",
            ));
        }

        write_json(&directory.join("index.json"), &index)?;
        if artifact_files(directory)? == files && lineage_state(directory)? == lineages {
            return Ok(());
        }
    }
}

// -----------------------------------------------------------------------------
// Scanning
// -----------------------------------------------------------------------------

/// Lists the graph files of the store in name order: every `*.json` file except `index.json`.
fn artifact_files(directory: &Path) -> io::Result<Vec<String>> {
    let mut files = Vec::new();
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let name = entry.file_name();
        if !entry.file_type()?.is_file() || Path::new(&name).extension() != Some(OsStr::new("json"))
        {
            continue;
        }
        match name.into_string() {
            Ok(name) if name != "index.json" => files.push(name),
            Ok(_) => {}
            Err(_) => eprintln!("warning: cannot list a JSON file: its name is not valid UTF-8"),
        }
    }
    files.sort();
    Ok(files)
}

/// Builds the index document. A flow is stale when its graph digest differs from the file.
fn artifact_index(
    directory: &Path,
    files: &[String],
    default_artifact: &str,
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
        default_artifact: default_artifact.to_owned(),
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

    use super::{refresh_artifact_index, write_artifact_index};
    use crate::{
        flow::fixtures::{self, frozen, import, output},
        store::digest_of,
    };

    fn index(directory: &std::path::Path) -> Value {
        serde_json::from_slice(&fs::read(directory.join("index.json")).expect("index"))
            .expect("index JSON")
    }

    #[test]
    fn indexes_artifacts_by_file_name() {
        let directory = tempfile::tempdir().expect("store");
        fs::write(directory.path().join("zeta.json"), "{}").expect("artifact");
        fs::write(directory.path().join("alpha.json"), "{}").expect("artifact");

        write_artifact_index(directory.path(), "zeta.json").expect("index");

        let document = index(directory.path());
        assert_eq!(document["schemaVersion"], 3);
        assert_eq!(document["defaultArtifact"], "zeta.json");
        assert_eq!(document["artifacts"][0]["label"], "alpha");
        assert_eq!(document["artifacts"][0]["digest"], digest_of(b"{}"));
        assert_eq!(document["artifacts"][1]["file"], "zeta.json");
        assert!(document["artifacts"][0].get("flows").is_none());
    }

    #[test]
    fn lists_a_frozen_flow_under_its_graph_and_marks_it_stale_after_a_change() {
        let directory = fixtures::store();
        let path = directory.path();
        let generation = frozen(import(path, "demo-flow", &output()));

        write_artifact_index(path, "graph.json").expect("index");

        let document = index(path);
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
        fixtures::write_graph(path, "src/extra.rs::callable:another");
        refresh_artifact_index(path).expect("refresh");
        assert_eq!(index(path)["artifacts"][0]["flows"][0]["stale"], true);
    }

    #[test]
    fn refreshes_the_index_and_keeps_the_default() {
        let directory = tempfile::tempdir().expect("store");
        fs::write(directory.path().join("beta.json"), "{}").expect("artifact");
        fs::write(directory.path().join("alpha.json"), "{}").expect("artifact");
        write_artifact_index(directory.path(), "beta.json").expect("index");

        refresh_artifact_index(directory.path()).expect("refresh");

        assert_eq!(index(directory.path())["defaultArtifact"], "beta.json");
    }

    #[test]
    fn rejects_a_default_artifact_outside_the_store() {
        let directory = tempfile::tempdir().expect("store");
        fs::write(directory.path().join("alpha.json"), "{}").expect("artifact");

        let error = write_artifact_index(directory.path(), "missing.json").expect_err("missing");

        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
    }
}
