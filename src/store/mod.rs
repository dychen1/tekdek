//! Artifact store.
//!
//! This module owns the store folder on disk: its location, its lock, its
//! reads, its atomic writes, its link checks, and the content digest of stored
//! bytes. The `graph` submodule owns the graph artifact. The flow modules read
//! and write through these helpers.
//!
//! # Sections
//!
//! `Project store` finds and creates the `.tekdek` folder of a project.
//! `Reads` opens and reads one regular file without following a link.
//! `Writes` replaces files atomically.
//! `Locks and links` guards one store operation.
//! `Digests` names stored bytes.

pub mod graph;

use std::{
    fmt::Write as _,
    fs::{self, File, Metadata},
    io::{self, Read},
    path::{Path, PathBuf},
};

use sha2::{Digest, Sha256};

// -----------------------------------------------------------------------------
// Project store
// -----------------------------------------------------------------------------

/// The folder name of a project store.
pub const PROJECT_STORE: &str = ".tekdek";

/// The ignore file of a new project store.
///
/// The lock and interrupted writes stay local. An older tekdek wrote the catalog
/// to `index.json`; tekdek now builds it in memory, so the file stays out of Git.
const PROJECT_IGNORE: &str =
    "# Local files of tekdek. Commit the rest of this folder.\n.store.lock\n*.tmp\nindex.json\n";

/// Returns the nearest project store in `start` or one of its parents.
///
/// The search stops at the top folder of a Git work tree, after it examines
/// that folder. It also stops below the home folder: the home folder is
/// examined only when the search starts there. So a stray `~/.tekdek` does not
/// capture the projects under the home folder.
#[must_use]
pub fn find_project_store(start: &Path) -> Option<PathBuf> {
    find_store_below(start, home_directory().as_deref())
}

fn find_store_below(start: &Path, home: Option<&Path>) -> Option<PathBuf> {
    let start = fs::canonicalize(start).unwrap_or_else(|_| start.to_path_buf());
    for directory in start.ancestors() {
        if home == Some(directory) && directory != start {
            return None;
        }
        let store = directory.join(PROJECT_STORE);
        if store.is_dir() {
            return Some(store);
        }
        if directory.join(".git").exists() {
            return None;
        }
    }
    None
}

/// Returns the canonical home folder of the user, if the system names one.
fn home_directory() -> Option<PathBuf> {
    let home = std::env::home_dir()?;
    Some(fs::canonicalize(&home).unwrap_or(home))
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
// Reads
// -----------------------------------------------------------------------------

/// Why `open_regular` cannot open a file.
pub(crate) enum OpenFailure {
    Link,
    Missing(io::Error),
    NotFile,
    Other(io::Error),
}

/// Opens one regular file at `base` joined with `parts`.
///
/// No component of the path, `base` included, can be a symbolic link. The open
/// uses `O_NOFOLLOW` and `O_NONBLOCK`: a link that appears after the walk fails
/// with `ELOOP`, and a pipe does not block the open.
pub(crate) fn open_regular(base: &Path, parts: &[&str]) -> Result<(File, Metadata), OpenFailure> {
    let check = |path: &Path| match fs::symlink_metadata(path) {
        // Refuse a symbolic link at any component.
        Ok(metadata) if metadata.file_type().is_symlink() => Err(OpenFailure::Link),
        // Keep walking when the component exists.
        Ok(_) => Ok(()),
        // Report a component that does not exist.
        Err(error) if error.kind() == io::ErrorKind::NotFound => Err(OpenFailure::Missing(error)),
        // Report any other failure.
        Err(error) => Err(OpenFailure::Other(error)),
    };
    let mut path = base.to_path_buf();
    check(&path)?;
    for part in parts {
        path.push(part);
        check(&path)?;
    }
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;

        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = options.open(&path).map_err(|error| {
        #[cfg(unix)]
        if error.raw_os_error() == Some(libc::ELOOP) {
            return OpenFailure::Link;
        }
        OpenFailure::Other(error)
    })?;
    let metadata = file.metadata().map_err(OpenFailure::Other)?;
    if !metadata.is_file() {
        return Err(OpenFailure::NotFile);
    }
    Ok((file, metadata))
}

/// Reads one regular store file. The error names the path.
///
/// A path that is not a regular file, such as a folder or a pipe, counts as a
/// missing file: its error kind is `NotFound`. A symbolic link is
/// `InvalidInput`. A pipe never blocks the read.
pub(crate) fn read_bytes(path: &Path) -> io::Result<Vec<u8>> {
    let cannot = |kind: io::ErrorKind, reason: &dyn std::fmt::Display| {
        io::Error::new(kind, format!("cannot read {}: {reason}", path.display()))
    };
    let (mut file, _) = open_regular(path, &[]).map_err(|failure| match failure {
        // Refuse a link with the message of the link check.
        OpenFailure::Link => io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("the store path is a symbolic link: {}", path.display()),
        ),
        // Report a folder or a pipe as a missing file.
        OpenFailure::NotFile => cannot(io::ErrorKind::NotFound, &"the path is not a regular file"),
        // Keep the kind and the text of any other failure.
        OpenFailure::Missing(error) | OpenFailure::Other(error) => cannot(error.kind(), &error),
    })?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .map_err(|error| cannot(error.kind(), &error))?;
    Ok(bytes)
}

// -----------------------------------------------------------------------------
// Writes
// -----------------------------------------------------------------------------

/// Replaces one store file atomically. The file and its folder must not be symbolic links.
pub(crate) fn write_bytes(path: &Path, document: &[u8]) -> io::Result<()> {
    if let Some(directory) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        reject_symlink(directory)?;
    }
    reject_symlink(path)?;
    atomic_write(path, document)
}

/// Replaces one file atomically through a synced temporary file in the same folder.
///
/// This write takes no store lock and checks no link. It creates a missing folder.
///
/// # Errors
///
/// Returns an error if the folder, the temporary file, or the rename fails.
pub fn atomic_write(path: &Path, document: &[u8]) -> io::Result<()> {
    if let Some(directory) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(directory)?;
    }
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

/// Locks cooperative graph writes and flow imports in one store.
///
/// The caller must keep the file open through the complete operation.
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

    use super::{
        PROJECT_STORE, create_project_store, digest_of, find_project_store, find_store_below,
        write_bytes,
    };

    #[test]
    fn finds_the_nearest_project_store_and_creates_one_with_an_ignore_file() {
        let directory = tempfile::tempdir().expect("project");
        let project = fs::canonicalize(directory.path())
            .expect("canonical folder")
            .join("project");
        let nested = project.join("src").join("cli");
        fs::create_dir_all(&nested).expect("folders");
        assert_eq!(find_project_store(&nested), None);

        let store = project.join(PROJECT_STORE);
        create_project_store(&store).expect("create");
        assert_eq!(find_project_store(&nested), Some(store.clone()));
        assert_eq!(find_project_store(&project), Some(store.clone()));
        let ignore = fs::read_to_string(store.join(".gitignore")).expect("ignore file");
        assert!(ignore.contains(".store.lock"));
        assert!(ignore.lines().any(|line| line == "index.json"));

        fs::write(store.join(".gitignore"), "kept\n").expect("edit");
        create_project_store(&store).expect("keep");
        assert_eq!(
            fs::read_to_string(store.join(".gitignore")).expect("ignore"),
            "kept\n"
        );
    }

    #[test]
    fn stops_the_search_at_the_git_top_folder_and_below_the_home_folder() {
        let directory = tempfile::tempdir().expect("home");
        let home = fs::canonicalize(directory.path()).expect("canonical home");
        fs::create_dir_all(home.join(PROJECT_STORE)).expect("stray store");
        let repository = home.join("code").join("acme");
        let nested = repository.join("src");
        fs::create_dir_all(&nested).expect("folders");
        let loose = home.join("notes").join("draft");
        fs::create_dir_all(&loose).expect("folders");

        assert_eq!(find_store_below(&loose, Some(&home)), None);
        assert_eq!(
            find_store_below(&home, Some(&home)),
            Some(home.join(PROJECT_STORE))
        );

        fs::create_dir(repository.join(".git")).expect("git folder");
        assert_eq!(find_store_below(&nested, None), None);
        fs::create_dir(repository.join(PROJECT_STORE)).expect("project store");
        assert_eq!(
            find_store_below(&nested, Some(&home)),
            Some(repository.join(PROJECT_STORE))
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
