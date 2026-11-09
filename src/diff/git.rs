//! Git access for diffs.
//!
//! This module runs `git` in the project folder: it resolves commits, reads
//! file text at a commit, and adds or removes the base worktree.

use std::{
    path::{Path, PathBuf},
    process::Command,
};

use super::DiffError;

/// Runs one git command in `directory` and returns its standard output.
fn git(directory: &Path, arguments: &[&str]) -> Result<Vec<u8>, DiffError> {
    let output = Command::new("git")
        .arg("-C")
        .arg(directory)
        .args(arguments)
        .output()
        .map_err(|error| DiffError::Git(format!("cannot run git: {error}")))?;
    if output.status.success() {
        Ok(output.stdout)
    } else {
        Err(DiffError::Git(format!(
            "git {} failed: {}",
            arguments.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        )))
    }
}

fn git_line(directory: &Path, arguments: &[&str]) -> Result<String, DiffError> {
    Ok(String::from_utf8_lossy(&git(directory, arguments)?)
        .trim()
        .to_owned())
}

/// Returns the full ID of the commit that `revision` names.
///
/// # Errors
/// Returns an error when git cannot resolve the revision.
pub fn commit(directory: &Path, revision: &str) -> Result<String, DiffError> {
    git_line(
        directory,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("{revision}^{{commit}}"),
        ],
    )
    .map_err(|_| DiffError::Git(format!("git cannot find the commit {revision}")))
}

/// Returns the merge-base of `HEAD` and the default branch.
///
/// The default branch is the first that exists of `origin/HEAD`, `origin/main`,
/// `main`, `origin/master`, and `master`.
///
/// # Errors
/// Returns an error when no default branch exists or the merge-base fails.
pub fn default_base(directory: &Path) -> Result<String, DiffError> {
    let branch = [
        "origin/HEAD",
        "origin/main",
        "main",
        "origin/master",
        "master",
    ]
    .into_iter()
    .find(|branch| commit(directory, branch).is_ok())
    .ok_or_else(|| {
        DiffError::Git("no default branch: pass --base <commit or branch>".to_owned())
    })?;
    git_line(directory, &["merge-base", "HEAD", branch])
}

/// Returns the path of `directory` inside its repository, with a trailing slash, or an empty string at the top.
///
/// # Errors
/// Returns an error when `directory` is not in a git work tree.
pub fn prefix(directory: &Path) -> Result<String, DiffError> {
    git_line(directory, &["rev-parse", "--show-prefix"])
}

/// Returns the text of one file at one commit. `path` is relative to `directory`.
///
/// Returns `None` when the file is not in the commit or is not UTF-8.
#[must_use]
pub fn show(directory: &Path, commit: &str, path: &str) -> Option<String> {
    String::from_utf8(git(directory, &["show", &format!("{commit}:./{path}")]).ok()?).ok()
}

/// Checks out `commit` in a new detached worktree at `path`.
///
/// # Errors
/// Returns an error when git cannot add the worktree.
pub fn add_worktree(directory: &Path, path: &Path, commit: &str) -> Result<(), DiffError> {
    let path = path.to_string_lossy();
    git(
        directory,
        &["worktree", "add", "--detach", "--quiet", &path, commit],
    )
    .map(drop)
}

/// Removes the worktree at `path`, with its files.
///
/// # Errors
/// Returns an error when git cannot remove the worktree.
pub fn remove_worktree(directory: &Path, path: &Path) -> Result<(), DiffError> {
    let path = path.to_string_lossy();
    git(directory, &["worktree", "remove", "--force", &path]).map(drop)
}

/// Returns the git folder that all worktrees of the repository share.
///
/// # Errors
/// Returns an error when `directory` is not in a git work tree.
pub fn common_dir(directory: &Path) -> Result<PathBuf, DiffError> {
    let path = PathBuf::from(git_line(directory, &["rev-parse", "--git-common-dir"])?);
    Ok(if path.is_absolute() {
        path
    } else {
        directory.join(path)
    })
}

/// True when the repository lists `path` as one of its worktrees.
///
/// # Errors
/// Returns an error when git cannot list the worktrees.
pub fn is_worktree(directory: &Path, path: &Path) -> Result<bool, DiffError> {
    let listing = String::from_utf8_lossy(&git(directory, &["worktree", "list", "--porcelain"])?)
        .into_owned();
    let wanted = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    Ok(listing
        .lines()
        .filter_map(|line| line.strip_prefix("worktree "))
        .any(|listed| {
            let listed = Path::new(listed);
            listed
                .canonicalize()
                .unwrap_or_else(|_| listed.to_path_buf())
                == wanted
        }))
}

/// Returns the top folder of the work tree that holds `directory`.
///
/// # Errors
/// Returns an error when `directory` is not in a git work tree.
pub fn top_level(directory: &Path) -> Result<PathBuf, DiffError> {
    git_line(directory, &["rev-parse", "--show-toplevel"]).map(PathBuf::from)
}
