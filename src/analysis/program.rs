//! Program lookup.
//!
//! This module finds the executable file of a language server or a helper
//! program, as the operating system would run it. A program name or path and
//! the search path are the inputs. One executable path is the output.

use std::{
    env, fs,
    path::{Path, PathBuf},
};

/// The extensions that Windows runs without an explicit extension, in lookup order.
///
/// npm installs `.cmd` wrappers, for example `pyright-langserver.cmd`, next to a
/// shell script without an extension that Windows cannot start.
const WINDOWS_EXTENSIONS: [&str; 3] = [".exe", ".cmd", ".bat"];

/// Returns the first executable file for a program name or path.
///
/// A name with one component is looked up in each `PATH` folder in order. On
/// Windows, a name without an extension matches `<name>.exe`, `<name>.cmd`, and
/// `<name>.bat`, in that order, and never the name alone.
#[must_use]
pub fn find_program(program: &str) -> Option<PathBuf> {
    let search_paths = env::var_os("PATH")
        .map(|value| env::split_paths(&value).collect::<Vec<_>>())
        .unwrap_or_default();
    program_candidates(program, &search_paths, executable_extensions())
        .into_iter()
        .find(|candidate| is_executable_file(candidate))
}

/// Returns the executable file named `name` in the folder of `program`.
#[must_use]
pub fn find_sibling_program(program: &Path, name: &str) -> Option<PathBuf> {
    let folder = program.parent()?.to_path_buf();
    program_candidates(name, &[folder], executable_extensions())
        .into_iter()
        .find(|candidate| is_executable_file(candidate))
}

fn program_candidates(
    program: &str,
    search_paths: &[PathBuf],
    extensions: &[&str],
) -> Vec<PathBuf> {
    let path = Path::new(program);
    let base_candidates = if path.components().count() > 1 {
        vec![path.to_path_buf()]
    } else {
        search_paths
            .iter()
            .map(|directory| directory.join(path))
            .collect()
    };

    base_candidates
        .into_iter()
        .flat_map(|candidate| {
            if extensions.is_empty() || candidate.extension().is_some() {
                return vec![candidate];
            }
            extensions
                .iter()
                .map(|extension| {
                    let mut path = candidate.as_os_str().to_os_string();
                    path.push(extension);
                    PathBuf::from(path)
                })
                .collect()
        })
        .collect()
}

const fn executable_extensions() -> &'static [&'static str] {
    if cfg!(windows) {
        &WINDOWS_EXTENSIONS
    } else {
        &[]
    }
}

fn is_executable_file(path: &Path) -> bool {
    let Ok(metadata) = fs::metadata(path) else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

// -----------------------------------------------------------------------------
// Tests
// -----------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::{WINDOWS_EXTENSIONS, program_candidates};

    #[test]
    fn builds_path_candidates_with_the_windows_extensions_only() {
        let search_paths = [PathBuf::from("first"), PathBuf::from("second")];

        let candidates = program_candidates("server", &search_paths, &WINDOWS_EXTENSIONS);

        assert_eq!(
            candidates,
            [
                PathBuf::from("first/server.exe"),
                PathBuf::from("first/server.cmd"),
                PathBuf::from("first/server.bat"),
                PathBuf::from("second/server.exe"),
                PathBuf::from("second/server.cmd"),
                PathBuf::from("second/server.bat"),
            ]
        );
    }

    #[test]
    fn keeps_an_explicit_extension_and_a_unix_name_as_they_are() {
        assert_eq!(
            program_candidates("tools/server.cmd", &[], &WINDOWS_EXTENSIONS),
            [PathBuf::from("tools/server.cmd")]
        );
        assert_eq!(
            program_candidates("server", &[PathBuf::from("bin")], &[]),
            [PathBuf::from("bin/server")]
        );
    }

    #[cfg(unix)]
    #[test]
    fn finds_an_executable_sibling() {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempfile::tempdir().expect("folder");
        let server = directory.path().join("pyright-langserver");
        let sibling = directory.path().join("pyright");
        for path in [&server, &sibling] {
            std::fs::write(path, "#!/bin/sh\n").expect("script");
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
                .expect("permissions");
        }

        assert_eq!(
            super::find_sibling_program(&server, "pyright"),
            Some(sibling)
        );
        assert_eq!(super::find_sibling_program(&server, "missing"), None);
    }
}
