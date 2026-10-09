//! Flow lineages in the store.
//!
//! This module owns the store layout of frozen flows, their reads and atomic
//! writes, and their listings. The store directory is the input.
//!
//! Layout:
//! `flows/<flowId>/generations/<ordinal>-<digest12>.json` holds one frozen
//! generation. `flows/<flowId>/current.json` names the current generation.
//!
//! # Sections
//! `Reads` reads the current or a named generation.
//! `Writes` freezes one generation and moves the pointer.
//! `Listings` lists lineages for the catalog and the CLI.
//! `Errors` reports failures.

use std::{
    fmt, fs, io,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use super::models::{FLOW_ARTIFACT_SCHEMA_VERSION, FlowArtifact, FlowId, Sha256Digest};
use crate::store::{
    graph::{check_graph_name, read_graph},
    reject_symlink, write_bytes,
};

/// Names the current generation of one flow.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CurrentPointer {
    pub generation_digest: Sha256Digest,
    pub ordinal: u32,
}

/// One stored generation and its path relative to `flows/`.
#[derive(Clone, Debug)]
pub struct StoredGeneration {
    pub artifact: FlowArtifact,
    pub file: String,
}

/// Returns the generation path relative to `flows/`.
#[must_use]
pub fn generation_file(flow_id: &FlowId, ordinal: u32, digest: &Sha256Digest) -> String {
    format!(
        "{flow_id}/generations/{ordinal}-{}.json",
        &digest.hex()[..12]
    )
}

// -----------------------------------------------------------------------------
// Reads
// -----------------------------------------------------------------------------

/// Reads the current generation of one flow, or `None` when the flow has no lineage.
///
/// # Errors
/// Returns an error for a link, an unreadable file, an invalid document, or a
/// pointer that names another generation.
pub fn read_current(
    directory: &Path,
    flow_id: &FlowId,
) -> Result<Option<StoredGeneration>, LineageError> {
    let lineage = lineage_path(directory, flow_id)?;
    let pointer_path = lineage.join("current.json");
    reject_symlink(&pointer_path)?;
    let bytes = match fs::read(&pointer_path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let pointer: CurrentPointer = serde_json::from_slice(&bytes)?;
    let stored = read_generation(
        directory,
        flow_id,
        pointer.ordinal,
        &pointer.generation_digest,
    )?;
    if stored.artifact.digests.generation != pointer.generation_digest {
        return Err(LineageError::PointerMismatch {
            flow: flow_id.to_string(),
        });
    }
    Ok(Some(stored))
}

/// Reads one generation by ordinal and digest.
///
/// # Errors
/// Returns an error for a link, an unreadable file, or an invalid document.
pub fn read_generation(
    directory: &Path,
    flow_id: &FlowId,
    ordinal: u32,
    digest: &Sha256Digest,
) -> Result<StoredGeneration, LineageError> {
    let file = generation_file(flow_id, ordinal, digest);
    let path = flows_path(directory)?.join(&file);
    for step in [
        path.parent().and_then(Path::parent),
        path.parent(),
        Some(path.as_path()),
    ]
    .into_iter()
    .flatten()
    {
        reject_symlink(step)?;
    }
    let bytes = fs::read(&path).map_err(|error| {
        io::Error::new(
            error.kind(),
            format!("cannot read {}: {error}", path.display()),
        )
    })?;
    Ok(StoredGeneration {
        artifact: parse_artifact(&bytes)?,
        file,
    })
}

/// Parses one artifact after its schema header check.
///
/// # Errors
/// Returns `UnknownSchema` for another schema version and a JSON error for any
/// other shape problem.
pub fn parse_artifact(bytes: &[u8]) -> Result<FlowArtifact, LineageError> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Header {
        schema_version: u32,
    }
    let header: Header = serde_json::from_slice(bytes)?;
    if header.schema_version != FLOW_ARTIFACT_SCHEMA_VERSION {
        return Err(LineageError::UnknownSchema {
            stated: header.schema_version,
        });
    }
    Ok(serde_json::from_slice(bytes)?)
}

// -----------------------------------------------------------------------------
// Writes
// -----------------------------------------------------------------------------

/// Writes one generation and moves the current pointer to it.
///
/// The generation file is written first, so a reader never sees a pointer to a
/// missing file. An identical existing generation file is kept.
///
/// # Errors
/// Returns an error for a link, a file failure, or a different file at the same path.
pub fn write_current(directory: &Path, artifact: &FlowArtifact) -> Result<String, LineageError> {
    let file = generation_file(
        &artifact.flow_id,
        artifact.generation.ordinal,
        &artifact.digests.generation,
    );
    let flows = flows_path(directory)?;
    let lineage = lineage_path(directory, &artifact.flow_id)?;
    let generations = lineage.join("generations");
    reject_symlink(&generations)?;
    fs::create_dir_all(&generations)?;
    let path = flows.join(&file);
    reject_symlink(&path)?;
    let mut bytes = serde_json::to_vec_pretty(artifact)?;
    bytes.push(b'\n');
    match fs::read(&path) {
        Ok(existing) if existing == bytes => {}
        Ok(_) => return Err(LineageError::Conflict { file }),
        Err(error) if error.kind() == io::ErrorKind::NotFound => write_bytes(&path, &bytes)?,
        Err(error) => return Err(error.into()),
    }
    let pointer = CurrentPointer {
        generation_digest: artifact.digests.generation.clone(),
        ordinal: artifact.generation.ordinal,
    };
    let mut pointer_bytes = serde_json::to_vec_pretty(&pointer)?;
    pointer_bytes.push(b'\n');
    write_bytes(&lineage.join("current.json"), &pointer_bytes)?;
    Ok(file)
}

// -----------------------------------------------------------------------------
// Listings
// -----------------------------------------------------------------------------

/// One flow listing. `file` is relative to `flows/`.
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

/// One listing and the graph binding that decides its freshness.
pub struct LineageListing {
    pub graph_artifact: String,
    pub graph_digest: String,
    pub listing: FlowListing,
}

/// Lists the current generation of every lineage. An invalid lineage produces a warning.
///
/// `stale` stays false: the caller compares `graph_digest` with the graph it holds.
///
/// # Errors
/// Returns an error for a link or a directory failure.
pub fn lineage_listings(directory: &Path) -> io::Result<Vec<LineageListing>> {
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
            graph_artifact: artifact.facts.binding.graph_artifact.clone(),
            graph_digest: artifact.facts.binding.graph_digest.to_string(),
            listing: FlowListing {
                name: id.to_string(),
                file: current.file,
                schema_version: FLOW_ARTIFACT_SCHEMA_VERSION,
                behavior: artifact.intent.behavior,
                step_count: artifact.intent.steps.len(),
                cluster_count: artifact.intent.clusters.len(),
                stale: false,
            },
        });
    }
    Ok(listings)
}

/// Lists flows in name order, with an optional graph filter.
///
/// A flow is stale when its bound graph changed or is missing.
///
/// # Errors
/// Returns an error for an invalid graph name, a link, or a directory failure.
pub fn list_flows(directory: &Path, graph_file: Option<&str>) -> io::Result<Vec<FlowListing>> {
    if let Some(file) = graph_file {
        check_graph_name(file)?;
    }
    let mut result = Vec::new();
    for lineage in lineage_listings(directory)? {
        if graph_file.is_some_and(|file| file != lineage.graph_artifact) {
            continue;
        }
        let stale = read_graph(directory, &lineage.graph_artifact)
            .map_or(true, |(_, digest)| digest != lineage.graph_digest);
        result.push(FlowListing {
            stale,
            ..lineage.listing
        });
    }
    result.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(result)
}

/// Lists the flow IDs that have a current pointer, in name order.
///
/// # Errors
/// Returns an error for a link or a directory failure.
pub fn lineage_ids(directory: &Path) -> io::Result<Vec<FlowId>> {
    let flows = directory.join("flows");
    reject_symlink(&flows)?;
    let entries = match fs::read_dir(&flows) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(vec![]),
        Err(error) => return Err(error),
    };
    let mut ids = Vec::new();
    for entry in entries {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let Ok(name) = entry.file_name().into_string() else {
            continue;
        };
        let Ok(id) = FlowId::try_from(name) else {
            continue;
        };
        if entry.path().join("current.json").is_file() {
            ids.push(id);
        }
    }
    ids.sort();
    Ok(ids)
}

fn flows_path(directory: &Path) -> io::Result<PathBuf> {
    reject_symlink(directory)?;
    let flows = directory.join("flows");
    reject_symlink(&flows)?;
    Ok(flows)
}

fn lineage_path(directory: &Path, flow_id: &FlowId) -> io::Result<PathBuf> {
    let lineage = flows_path(directory)?.join(flow_id.as_str());
    reject_symlink(&lineage)?;
    Ok(lineage)
}

// -----------------------------------------------------------------------------
// Errors
// -----------------------------------------------------------------------------

#[derive(Debug)]
pub enum LineageError {
    Io(io::Error),
    Json(serde_json::Error),
    UnknownSchema { stated: u32 },
    PointerMismatch { flow: String },
    Conflict { file: String },
}

impl fmt::Display for LineageError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            // Preserve the file error.
            Self::Io(error) => write!(formatter, "{error}"),
            // Identify invalid JSON or an invalid artifact shape.
            Self::Json(error) => write!(formatter, "invalid flow artifact: {error}"),
            // Identify an unsupported schema.
            Self::UnknownSchema { stated } => write!(
                formatter,
                "unsupported flow artifact schema {stated}: expected {FLOW_ARTIFACT_SCHEMA_VERSION}"
            ),
            // Identify a pointer that names another generation.
            Self::PointerMismatch { flow } => write!(
                formatter,
                "pointer_mismatch: the current pointer of {flow} names a different generation"
            ),
            // Refuse to replace a frozen file.
            Self::Conflict { file } => write!(
                formatter,
                "the frozen generation {file} already exists with different content"
            ),
        }
    }
}

impl std::error::Error for LineageError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Json(error) => Some(error),
            _ => None,
        }
    }
}

impl From<io::Error> for LineageError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<serde_json::Error> for LineageError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}
