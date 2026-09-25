//! Python module names and imports.
//!
//! This module derives the dotted module name of one source path and resolves
//! the base module of a relative `from` import.

use std::{
    ffi::OsStr,
    path::{Component, Path},
};

use rustpython_parser::ast;

use super::ParsedDocument;
use crate::analysis::fact::AnalysisError;

/// Returns the import module name and package status for one Python path.
///
/// - `service.py` becomes `("service", false)`.
/// - `package/service.py` becomes `("package.service", false)`.
/// - `package/__init__.py` becomes `("package", true)`.
/// - `__init__.py` becomes `("", true)`.
///
/// # Errors
///
/// Returns an error if the path has no valid file stem.
pub(super) fn module_name(path: &Path) -> Result<(String, bool), AnalysisError> {
    let stem = path
        .file_stem()
        .and_then(OsStr::to_str)
        .ok_or_else(|| AnalysisError {
            message: format!(
                "the Python document has no valid file stem: {}",
                path.display()
            ),
        })?;
    let is_package_initializer = stem == "__init__";
    let mut parts = path
        .parent()
        .into_iter()
        .flat_map(Path::components)
        .filter_map(|component| match component {
            Component::Normal(part) => part.to_str(),
            Component::Prefix(_)
            | Component::RootDir
            | Component::CurDir
            | Component::ParentDir => None,
        })
        .map(str::to_owned)
        .collect::<Vec<_>>();

    if !is_package_initializer {
        parts.push(stem.to_owned());
    }

    Ok((parts.join("."), is_package_initializer))
}

/// Returns the absolute base name for one `from` import statement.
///
/// Returns `None` when a relative import moves above the selected package root.
pub(super) fn import_base(
    document: &ParsedDocument<'_>,
    import: &ast::StmtImportFrom,
) -> Option<String> {
    let level = import.level.map_or(0, |level| level.to_usize());

    // Keep an absolute import unchanged.
    if level == 0 {
        return import.module.as_ref().map(ToString::to_string);
    }

    let mut parts = document
        .module_name
        .split('.')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    // Start from the containing package for a module file.
    if !document.is_package_initializer {
        parts.pop();
    }

    let parents = level - 1;
    // Reject an import above the selected package root.
    if parents > parts.len() {
        return None;
    }
    parts.truncate(parts.len() - parents);

    // Append an explicit relative module path.
    if let Some(module) = &import.module {
        parts.extend(module.as_str().split('.'));
    }

    Some(parts.join("."))
}

pub(super) fn join_module_name(base: &str, child: &str) -> String {
    if base.is_empty() {
        child.to_owned()
    } else {
        format!("{base}.{child}")
    }
}
