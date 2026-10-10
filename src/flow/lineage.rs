//! Flow lineages in the store.
//!
//! This module owns the store layout of frozen flows, and their reads and atomic
//! writes. The store directory is the input.
//!
//! Layout:
//! `flows/<flowId>/generations/<ordinal>-<digest12>.json` holds one frozen
//! generation. `flows/<flowId>/current.json` names the current generation.
//!
//! # Sections
//! `Reads` reads the current or a named generation, and lists the lineages.
//! `Writes` freezes one generation and moves the pointer.
//! `Errors` reports failures.

use std::{
    fmt, fs, io,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use super::models::{FLOW_ARTIFACT_SCHEMA_VERSION, FlowArtifact, FlowId, Sha256Digest};
use crate::store::{read_bytes, reject_symlink, write_bytes};

/// Names the current generation of one flow.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CurrentPointer {
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
/// A pointer that is not a regular file counts as no lineage.
///
/// # Errors
/// Returns an error for a link, an unreadable file, an invalid document, a
/// generation of another flow, or a pointer that names another generation.
pub fn read_current(
    directory: &Path,
    flow_id: &FlowId,
) -> Result<Option<StoredGeneration>, LineageError> {
    let lineage = lineage_path(directory, flow_id)?;
    let bytes = match read_bytes(&lineage.join("current.json")) {
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
/// Returns an error for a link, an unreadable file, an invalid document, or a
/// generation that states another flow than the lineage folder.
pub fn read_generation(
    directory: &Path,
    flow_id: &FlowId,
    ordinal: u32,
    digest: &Sha256Digest,
) -> Result<StoredGeneration, LineageError> {
    let file = generation_file(flow_id, ordinal, digest);
    let path = flows_path(directory)?.join(&file);
    for folder in [path.parent().and_then(Path::parent), path.parent()]
        .into_iter()
        .flatten()
    {
        reject_symlink(folder)?;
    }
    let artifact = parse_artifact(&read_bytes(&path)?)?;
    if artifact.flow_id != *flow_id {
        return Err(LineageError::FlowMismatch {
            folder: flow_id.to_string(),
            stated: artifact.flow_id.to_string(),
        });
    }
    Ok(StoredGeneration { artifact, file })
}

/// Parses one artifact after its schema header check.
///
/// # Errors
/// Returns `UnknownSchema` for another schema version and a JSON error for any
/// other shape problem.
fn parse_artifact(bytes: &[u8]) -> Result<FlowArtifact, LineageError> {
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

// -----------------------------------------------------------------------------
// Writes
// -----------------------------------------------------------------------------

/// Writes one generation and moves the current pointer to it.
///
/// The generation file is written first, so a reader never sees a pointer to a
/// missing file. An identical existing generation file is kept. An entry that
/// is not a regular file counts as missing.
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
    let mut bytes = serde_json::to_vec_pretty(artifact)?;
    bytes.push(b'\n');
    match read_bytes(&path) {
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
    FlowMismatch { folder: String, stated: String },
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
            // Identify a generation that belongs to another flow.
            Self::FlowMismatch { folder, stated } => write!(
                formatter,
                "flow_mismatch: the lineage {folder} holds a generation of the flow {stated}"
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

// -----------------------------------------------------------------------------
// Tests
// -----------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::{fs, path::Path};

    use super::{LineageError, read_current, write_current};
    use crate::flow::{
        FlowError,
        fixtures::{self, frozen, import, output},
        models::FlowId,
        verify::verify_flow,
    };

    fn id(name: &str) -> FlowId {
        FlowId::try_from(name).expect("flow ID")
    }

    /// Freezes one initial generation and one revision of `name`.
    fn freeze_with_revision(path: &Path, name: &str) {
        frozen(import(path, name, &output()));
        let initial = fixtures::current(path, name);
        let mut revised = output();
        for (step, stored) in revised["flow"]["steps"]
            .as_array_mut()
            .expect("steps")
            .iter_mut()
            .zip(&initial.intent.steps)
        {
            step["priorStepId"] = stored.id.as_str().into();
        }
        revised["flow"]["steps"][5]["title"] = "Hold the store lock".into();
        revised["flow"]["revisionSummary"] = "Rename the last step to name the lock.".into();
        frozen(import(path, name, &revised));
    }

    /// Copies the pointer and the generation files of one lineage into another folder.
    fn copy_lineage(path: &Path, from: &str, to: &str) {
        let source = path.join("flows").join(from);
        let target = path.join("flows").join(to);
        fs::create_dir_all(target.join("generations")).expect("target folder");
        fs::copy(source.join("current.json"), target.join("current.json")).expect("pointer");
        for entry in fs::read_dir(source.join("generations")).expect("generations") {
            let file = entry.expect("entry").file_name();
            fs::copy(
                source.join("generations").join(&file),
                target.join("generations").join(&file),
            )
            .expect("generation");
        }
    }

    fn is_flow_mismatch(error: &LineageError) -> bool {
        matches!(error, LineageError::FlowMismatch { .. })
    }

    #[test]
    fn a_revision_copied_under_another_flow_is_rejected() {
        let directory = fixtures::store();
        let path = directory.path();
        freeze_with_revision(path, "flow-b");
        copy_lineage(path, "flow-b", "flow-a");

        let error = read_current(path, &id("flow-a")).expect_err("wrong flow");

        assert!(is_flow_mismatch(&error), "{error}");
        assert!(matches!(
            verify_flow(path, "flow-a"),
            Err(FlowError::Lineage(LineageError::FlowMismatch { .. }))
        ));
        assert!(
            read_current(path, &id("flow-b"))
                .expect("own flow")
                .is_some()
        );
    }

    #[test]
    fn an_initial_generation_copied_under_another_flow_is_rejected() {
        let directory = fixtures::store();
        let path = directory.path();
        frozen(import(path, "flow-b", &output()));
        copy_lineage(path, "flow-b", "flow-a");

        let error = read_current(path, &id("flow-a")).expect_err("wrong flow");

        assert!(is_flow_mismatch(&error), "{error}");
    }

    #[cfg(unix)]
    #[test]
    fn a_pipe_at_the_generation_path_does_not_block_a_write() {
        let directory = fixtures::store();
        let path = directory.path().to_owned();
        let generation = frozen(import(&path, "flow-a", &output()));
        let artifact = fixtures::current(&path, "flow-a");
        let file = path.join("flows").join(&generation.file);
        let bytes = fs::read(&file).expect("generation");
        fs::remove_file(&file).expect("remove generation");
        let made = std::process::Command::new("mkfifo")
            .arg(&file)
            .status()
            .expect("mkfifo");
        assert!(made.success(), "mkfifo must create the pipe");

        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = sender.send(write_current(&path, &artifact).map_err(|error| error.to_string()));
        });
        let written = receiver
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("the write must not block on the pipe");

        assert_eq!(written, Ok(generation.file));
        assert_eq!(fs::read(&file).expect("generation"), bytes);
    }

    #[test]
    fn a_renamed_lineage_folder_is_rejected() {
        let directory = fixtures::store();
        let path = directory.path();
        freeze_with_revision(path, "flow-b");
        fs::rename(path.join("flows/flow-b"), path.join("flows/flow-a")).expect("rename");

        let error = read_current(path, &id("flow-a")).expect_err("renamed folder");

        assert!(is_flow_mismatch(&error), "{error}");
    }
}
