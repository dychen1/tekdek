//! Safe file reads.
//!
//! This module reads the store files and the project files that the viewer
//! server sends. Both readers open through `store::open_regular`: only a
//! regular file, and no symbolic link at any component of the path. A base
//! folder and a relative path are the input. The file bytes, or a reply or a
//! reason for a failure, are the output.

use std::{io::Read, path::Path};

use super::{Reply, not_found, read_failure};
use crate::store::{OpenFailure, open_regular};

/// The largest project file that the source route reads.
const MAX_SOURCE_BYTES: u64 = 2 * 1024 * 1024;

/// Reads one regular store file.
pub(super) fn read_store(store: &Path, parts: &[&str]) -> Result<Vec<u8>, Reply> {
    let (mut file, _) = open_regular(store, parts).map_err(|failure| match failure {
        // Refuse a link.
        OpenFailure::Link => Reply::failure(403, "Symbolic links are not allowed."),
        // Report a missing file.
        OpenFailure::Missing(_) => not_found(),
        // Refuse a folder or a pipe.
        OpenFailure::NotFile => Reply::failure(403, "The path must be a regular file."),
        // Map any other failure.
        OpenFailure::Other(error) => read_failure(&error),
    })?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .map_err(|error| read_failure(&error))?;
    Ok(bytes)
}

/// Reads one project file as UTF-8 text, or returns why it cannot.
pub(super) fn read_project(root: &Path, relative: &str) -> Result<String, String> {
    let cannot = |what: &str| format!("the file cannot be {what}");
    if !project_path(relative) {
        return Err("the path leaves the project".to_owned());
    }
    let parts: Vec<&str> = relative.split('/').collect();
    let (file, metadata) = open_regular(root, &parts).map_err(|failure| match failure {
        // Refuse a link.
        OpenFailure::Link => "the path holds a symbolic link".to_owned(),
        // Report a missing file.
        OpenFailure::Missing(_) => "the file is missing".to_owned(),
        // Refuse a folder or a pipe.
        OpenFailure::NotFile => "the path is not a regular file".to_owned(),
        // Report any other failure.
        OpenFailure::Other(_) => cannot("opened"),
    })?;
    if metadata.len() > MAX_SOURCE_BYTES {
        return Err("the file is larger than 2 MiB".to_owned());
    }
    let mut bytes = Vec::new();
    file.take(MAX_SOURCE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| cannot("read"))?;
    // The file can grow between the metadata call and the read.
    if bytes.len() as u64 > MAX_SOURCE_BYTES {
        return Err("the file is larger than 2 MiB".to_owned());
    }
    String::from_utf8(bytes).map_err(|_| "the file is not UTF-8 text".to_owned())
}

/// Accepts a relative path with no empty, `.`, or `..` component, no backslash, and no colon.
fn project_path(path: &str) -> bool {
    !path.is_empty()
        && !path.starts_with('/')
        && !path.contains(['\\', ':'])
        && path
            .split('/')
            .all(|part| !part.is_empty() && part != "." && part != "..")
}

// -----------------------------------------------------------------------------
// Tests
// -----------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::fs;

    use super::{MAX_SOURCE_BYTES, project_path, read_project, read_store};

    #[cfg(unix)]
    #[test]
    fn read_store_refuses_links_folders_pipes_and_missing_files() {
        let store = tempfile::tempdir().expect("store");
        let outside = tempfile::tempdir().expect("outside");
        let (store, outside) = (store.path(), outside.path());
        fs::create_dir(store.join("flows")).expect("flows");
        fs::write(store.join("graph.json"), "{}").expect("graph");
        fs::write(outside.join("secret.json"), "{}").expect("secret");
        std::os::unix::fs::symlink(outside.join("secret.json"), store.join("linked.json"))
            .expect("file link");
        std::os::unix::fs::symlink(outside, store.join("away")).expect("folder link");
        let made = std::process::Command::new("mkfifo")
            .arg(store.join("pipe.json"))
            .status()
            .expect("mkfifo");
        assert!(made.success(), "mkfifo must create the pipe");
        let status = |parts: &[&str]| read_store(store, parts).map_err(|reply| reply.status);
        assert_eq!(status(&["graph.json"]), Ok(b"{}".to_vec()));
        for (parts, code) in [
            (&["linked.json"][..], 403),
            (&["away", "secret.json"], 403),
            (&["flows"], 403),
            (&["pipe.json"], 403),
            (&["missing.json"], 404),
            (&["nowhere", "missing.json"], 404),
        ] {
            assert_eq!(status(parts), Err(code), "{parts:?}");
        }
    }

    #[test]
    fn project_path_accepts_only_plain_relative_paths() {
        for path in [
            "main.rs",
            "src/main.rs",
            "a/b/c.rs",
            "src/.hidden/a.rs",
            "src/...rs",
        ] {
            assert!(project_path(path), "{path} must be accepted");
        }
        for path in [
            "",
            "/src/main.rs",
            "src//main.rs",
            "src/",
            ".",
            "./main.rs",
            "src/./main.rs",
            "..",
            "../outside.rs",
            "src/../main.rs",
            "src\\main.rs",
            "C:/main.rs",
            "src/a:b.rs",
        ] {
            assert!(!project_path(path), "{path:?} must be refused");
        }
    }

    #[test]
    fn read_project_returns_the_text_or_why_it_cannot_read_a_file() {
        let root = tempfile::tempdir().expect("root");
        let root = root.path();
        fs::create_dir(root.join("src")).expect("src");
        fs::write(root.join("src/main.rs"), "fn main() {}\n").expect("main");
        let limit = usize::try_from(MAX_SOURCE_BYTES).expect("limit");
        fs::write(root.join("limit.rs"), vec![b'a'; limit]).expect("limit file");
        fs::write(root.join("large.rs"), vec![b'a'; limit + 1]).expect("large file");
        fs::write(root.join("binary.rs"), [0xff, 0xfe, b'a']).expect("binary file");
        assert_eq!(
            read_project(root, "src/main.rs"),
            Ok("fn main() {}\n".to_owned())
        );
        assert_eq!(
            read_project(root, "limit.rs").map(|text| text.len()),
            Ok(limit)
        );
        assert_eq!(
            read_project(root, "large.rs"),
            Err("the file is larger than 2 MiB".to_owned())
        );
        assert_eq!(
            read_project(root, "binary.rs"),
            Err("the file is not UTF-8 text".to_owned())
        );
        for path in ["missing.rs", "src/missing.rs", "nowhere/missing.rs"] {
            assert_eq!(
                read_project(root, path),
                Err("the file is missing".to_owned()),
                "{path}"
            );
        }
        for path in [
            "",
            "/main.rs",
            "src//main.rs",
            "./src/main.rs",
            "../main.rs",
            "src\\main.rs",
            "C:/main.rs",
        ] {
            assert_eq!(
                read_project(root, path),
                Err("the path leaves the project".to_owned()),
                "{path:?}"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn read_project_refuses_folders_links_and_pipes() {
        let root = tempfile::tempdir().expect("root");
        let outside = tempfile::tempdir().expect("outside");
        let (root, outside) = (root.path(), outside.path());
        fs::create_dir(root.join("src")).expect("src");
        fs::write(root.join("src/main.rs"), "fn main() {}\n").expect("main");
        fs::write(outside.join("secret.rs"), "fn secret() {}\n").expect("secret");
        std::os::unix::fs::symlink(root.join("src/main.rs"), root.join("inside.rs"))
            .expect("inside link");
        std::os::unix::fs::symlink(outside.join("secret.rs"), root.join("outside.rs"))
            .expect("outside link");
        std::os::unix::fs::symlink(outside, root.join("away")).expect("folder link");
        let made = std::process::Command::new("mkfifo")
            .arg(root.join("pipe.rs"))
            .status()
            .expect("mkfifo");
        assert!(made.success(), "mkfifo must create the pipe");
        assert_eq!(
            read_project(root, "src"),
            Err("the path is not a regular file".to_owned())
        );
        assert_eq!(
            read_project(root, "pipe.rs"),
            Err("the path is not a regular file".to_owned())
        );
        for path in ["inside.rs", "outside.rs", "away/secret.rs"] {
            assert_eq!(
                read_project(root, path),
                Err("the path holds a symbolic link".to_owned()),
                "{path}"
            );
        }
    }
}
