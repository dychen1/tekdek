//! Artifact store.
//!
//! This module owns the store folder on disk: its location, its lock, its atomic
//! writes, its link checks, and the content digest of stored bytes. The `graph`
//! submodule owns the graph artifact. The flow and catalog modules write through
//! these helpers.
//!
//! # Sections
//!
//! `Project store` finds and creates the `.tekdek` folder of a project.
//! `Writes` replaces files atomically.
//! `Locks and links` guards one store operation.
//! `Digests` names stored bytes.

pub mod graph;

use std::{
    fmt::Write as _,
    fs, io,
    path::{Path, PathBuf},
};

use serde::Serialize;
use sha2::{Digest, Sha256};

// -----------------------------------------------------------------------------
// Project store
// -----------------------------------------------------------------------------

/// The folder name of a project store.
pub const PROJECT_STORE: &str = ".tekdek";

/// The ignore file of a new project store: the lock and interrupted writes stay local.
const PROJECT_IGNORE: &str =
    "# Local files of tekdek. Commit the rest of this folder.\n.store.lock\n*.tmp\n";

/// Returns the nearest project store in `start` or one of its parents.
#[must_use]
pub fn find_project_store(start: &Path) -> Option<PathBuf> {
    start
        .ancestors()
        .map(|directory| directory.join(PROJECT_STORE))
        .find(|store| store.is_dir())
}

/// Creates a project store with its ignore file. An existing folder stays as it is.
///
/// # Errors
///
/// Returns an error if the folder or its ignore file cannot be written.
pub fn create_project_store(store: &Path) -> io::Result<()> {
    reject_symlink(store)?;
    if store.exists() {
        return Ok(());
    }
    fs::create_dir_all(store)?;
    write_bytes(&store.join(".gitignore"), PROJECT_IGNORE.as_bytes())
}

// -----------------------------------------------------------------------------
// Writes
// -----------------------------------------------------------------------------

/// Writes compact JSON followed by one newline, atomically.
pub(crate) fn write_json(path: &Path, value: &impl Serialize) -> io::Result<()> {
    let mut document = serde_json::to_vec(value).map_err(io::Error::other)?;
    document.push(b'\n');
    write_bytes(path, &document)
}

/// Replaces one file atomically through a synced temporary file in the same directory.
pub(crate) fn write_bytes(path: &Path, document: &[u8]) -> io::Result<()> {
    if let Some(directory) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        reject_symlink(directory)?;
        fs::create_dir_all(directory)?;
    }
    reject_symlink(path)?;
    let temporary = path.with_extension(format!("{}.tmp", temporary_suffix()));
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)?;
    let result = (|| {
        use io::Write;
        file.write_all(document)?;
        file.sync_all()?;
        fs::rename(&temporary, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

/// Returns a suffix that no other temporary file of this process or clock tick uses.
fn temporary_suffix() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    format!(
        "{:x}-{:x}-{:x}",
        jiff::Timestamp::now().as_nanosecond(),
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
}

// -----------------------------------------------------------------------------
// Locks and links
// -----------------------------------------------------------------------------

/// Locks cooperative graph writes, flow imports, and catalog writes in one store.
///
/// The caller must keep the file open through the complete operation.
/// A graph write or a flow import and its later catalog write use separate lock scopes.
pub(crate) fn lock_store(directory: &Path) -> io::Result<fs::File> {
    reject_symlink(directory)?;
    fs::create_dir_all(directory)?;
    let path = directory.join(".store.lock");
    reject_symlink(&path)?;
    let file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)?;
    file.lock()?;
    Ok(file)
}

/// Rejects a store path that is a symbolic link. A missing path passes.
pub(crate) fn reject_symlink(path: &Path) -> io::Result<()> {
    match fs::symlink_metadata(path) {
        // Reject links before a store operation.
        Ok(metadata) if metadata.file_type().is_symlink() => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("the store path is a symbolic link: {}", path.display()),
        )),
        // Accept an existing non-link entry.
        Ok(_) => Ok(()),
        // Permit a later create operation.
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        // Preserve other metadata failures.
        Err(error) => Err(error),
    }
}

// -----------------------------------------------------------------------------
// Digests
// -----------------------------------------------------------------------------

/// Returns the document digest as `sha256:<hex>`.
#[must_use]
pub fn digest_of(bytes: &[u8]) -> String {
    let mut text = String::from("sha256:");
    for byte in Sha256::digest(bytes) {
        let _ = write!(text, "{byte:02x}");
    }
    text
}

// -----------------------------------------------------------------------------
// Tests
// -----------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::fs;

    use super::{PROJECT_STORE, create_project_store, digest_of, find_project_store, write_bytes};

    #[test]
    fn finds_the_nearest_project_store_and_creates_one_with_an_ignore_file() {
        let directory = tempfile::tempdir().expect("project");
        let project = directory.path().join("project");
        let nested = project.join("src").join("cli");
        fs::create_dir_all(&nested).expect("folders");
        assert_eq!(find_project_store(&nested), None);

        let store = project.join(PROJECT_STORE);
        create_project_store(&store).expect("create");
        assert_eq!(find_project_store(&nested), Some(store.clone()));
        assert_eq!(find_project_store(&project), Some(store.clone()));
        let ignore = fs::read_to_string(store.join(".gitignore")).expect("ignore file");
        assert!(ignore.contains(".store.lock"));

        fs::write(store.join(".gitignore"), "kept\n").expect("edit");
        create_project_store(&store).expect("keep");
        assert_eq!(
            fs::read_to_string(store.join(".gitignore")).expect("ignore"),
            "kept\n"
        );
    }

    #[test]
    fn replaces_a_file_and_leaves_no_temporary_file() {
        let directory = tempfile::tempdir().expect("store");
        let path = directory.path().join("nested").join("value.json");
        write_bytes(&path, b"first").expect("first write");
        write_bytes(&path, b"second").expect("second write");

        assert_eq!(fs::read(&path).expect("read"), b"second");
        let names: Vec<_> = fs::read_dir(path.parent().expect("parent"))
            .expect("list")
            .map(|entry| entry.expect("entry").file_name())
            .collect();
        assert_eq!(names, ["value.json"]);
    }

    #[cfg(unix)]
    #[test]
    fn rejects_a_symbolic_link() {
        use super::reject_symlink;

        let directory = tempfile::tempdir().expect("store");
        let target = directory.path().join("target.json");
        let link = directory.path().join("link.json");
        fs::write(&target, b"{}").expect("target");
        std::os::unix::fs::symlink(&target, &link).expect("link");

        assert!(reject_symlink(&link).is_err());
        assert!(write_bytes(&link, b"{}").is_err());
        assert!(reject_symlink(&directory.path().join("missing.json")).is_ok());
    }

    #[test]
    fn names_bytes_with_a_sha256_digest() {
        assert_eq!(
            digest_of(b""),
            "sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }
}
