//! Rust module paths.
//!
//! This module maps Rust source files to crate roots and module paths, and it
//! resolves qualified `use` paths against the current module.

use std::{
    ffi::OsStr,
    path::{Component, Path, PathBuf},
};

use super::ParsedDocument;

#[derive(Clone, Default, Eq, Hash, PartialEq)]
pub(super) struct ModulePath {
    pub(super) crate_root: String,
    pub(super) segments: Vec<String>,
}

pub(super) struct CrateRoot {
    identity: String,
    source_root: PathBuf,
}

/// Returns the one namespace selected by Rust path qualification.
///
/// Unprefixed paths stay relative. They never retry from the crate root.
pub(super) fn relative_module_path(
    current_module: &ModulePath,
    path: &[String],
) -> Option<ModulePath> {
    let mut path = path.to_vec();
    if path.len() > 1 && path.last().is_some_and(|part| part == "self") {
        path.pop();
    }
    let Some(first) = path.first().map(String::as_str) else {
        return Some(current_module.clone());
    };

    match first {
        "crate" => Some(ModulePath {
            crate_root: current_module.crate_root.clone(),
            segments: path.into_iter().skip(1).collect(),
        }),
        "self" => {
            let mut candidate = current_module.clone();
            candidate.segments.extend(path.into_iter().skip(1));
            Some(candidate)
        }
        "super" => {
            let mut candidate = current_module.clone();
            let mut index = 0;
            while path.get(index).is_some_and(|part| part == "super") {
                candidate.segments.pop()?;
                index += 1;
            }
            candidate.segments.extend(path.into_iter().skip(index));
            Some(candidate)
        }
        _ => {
            let mut relative = current_module.clone();
            relative.segments.extend(path);
            Some(relative)
        }
    }
}

pub(super) fn crate_roots(documents: &[ParsedDocument<'_>]) -> Vec<CrateRoot> {
    documents
        .iter()
        .filter_map(|document| {
            let path = Path::new(&document.path);
            conventional_crate_source_root(path).map(|source_root| CrateRoot {
                identity: document.path.clone(),
                source_root,
            })
        })
        .collect()
}

pub(super) fn conventional_crate_source_root(path: &Path) -> Option<PathBuf> {
    if path.extension().and_then(OsStr::to_str) != Some("rs") {
        return None;
    }
    let parent = path.parent().unwrap_or_else(|| Path::new(""));
    let stem = path.file_stem().and_then(OsStr::to_str)?;

    if matches!(stem, "lib" | "main")
        && (parent.as_os_str().is_empty() || parent.file_name() == Some(OsStr::new("src")))
    {
        return Some(parent.to_owned());
    }
    if target_collection(parent) {
        return Some(path.with_extension(""));
    }
    if stem == "main" && parent.parent().is_some_and(target_collection) {
        return Some(parent.to_owned());
    }

    None
}

pub(super) fn target_collection(path: &Path) -> bool {
    matches!(
        path.file_name().and_then(OsStr::to_str),
        Some("examples" | "benches" | "tests")
    ) || (path.file_name() == Some(OsStr::new("bin"))
        && path.parent().and_then(Path::file_name) == Some(OsStr::new("src")))
}

pub(super) fn module_path(path: &str, crate_roots: &[CrateRoot]) -> ModulePath {
    if let Some(crate_root) = crate_roots
        .iter()
        .find(|crate_root| crate_root.identity == path)
    {
        return ModulePath {
            crate_root: crate_root.identity.clone(),
            segments: Vec::new(),
        };
    }

    let path = Path::new(path);
    let matching_roots = crate_roots
        .iter()
        .filter(|crate_root| path.strip_prefix(&crate_root.source_root).is_ok())
        .collect::<Vec<_>>();
    let max_depth = matching_roots
        .iter()
        .map(|crate_root| crate_root.source_root.components().count())
        .max();
    if let Some(max_depth) = max_depth {
        let nearest_roots = matching_roots
            .into_iter()
            .filter(|crate_root| crate_root.source_root.components().count() == max_depth)
            .collect::<Vec<_>>();
        if let [crate_root] = nearest_roots.as_slice() {
            return ModulePath {
                crate_root: crate_root.identity.clone(),
                segments: module_segments(path, &crate_root.source_root),
            };
        }
    }

    let source_root = inferred_source_root(path);
    ModulePath {
        crate_root: format!("source-root:{}", source_root.display()),
        segments: module_segments(path, &source_root),
    }
}

pub(super) fn inferred_source_root(path: &Path) -> PathBuf {
    let mut current = PathBuf::new();
    let mut source_root = PathBuf::new();

    for component in path.parent().into_iter().flat_map(Path::components) {
        let Component::Normal(part) = component else {
            continue;
        };
        current.push(part);
        if part == "src" {
            source_root.clone_from(&current);
        }
    }

    source_root
}

pub(super) fn module_segments(path: &Path, source_root: &Path) -> Vec<String> {
    let relative = path.strip_prefix(source_root).unwrap_or(path);
    let mut parts = relative
        .parent()
        .into_iter()
        .flat_map(Path::components)
        .filter_map(|component| match component {
            Component::Normal(part) => part.to_str().map(str::to_owned),
            Component::Prefix(_)
            | Component::RootDir
            | Component::CurDir
            | Component::ParentDir => None,
        })
        .collect::<Vec<_>>();

    let stem = relative
        .file_stem()
        .and_then(OsStr::to_str)
        .unwrap_or_default();
    if !matches!(stem, "lib" | "main" | "mod") && !stem.is_empty() {
        parts.push(stem.to_owned());
    }
    parts
}
