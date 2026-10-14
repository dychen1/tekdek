//! Source documents.
//!
//! This module owns source snapshots, source coordinates, and file selection.
//! It accepts a root, file extensions, and excluded directory names. It
//! produces ordered source snapshots.
//!
//! # Sections
//!
//! `Documents` defines one source snapshot and one selected view.
//! `Source Coordinates` defines byte ranges and their source paths.
//! `Content Hashes` identifies the text of one source range or of all documents.
//! `Loading` reads matching documents from a codebase root.

use std::{ffi::OsStr, fmt, fs, io, path::Path, path::PathBuf};

// -----------------------------------------------------------------------------
// Documents
// -----------------------------------------------------------------------------

/// The relative path identifies the document across analysis stages.
pub struct DocumentSnapshot {
    pub path: PathBuf,
    pub content: String,
}

impl DocumentSnapshot {
    /// Returns the text of one range, or `None` when it lies outside the
    /// document.
    #[must_use]
    pub fn text(&self, range: SourceRange) -> Option<&str> {
        self.content
            .get(usize::try_from(range.start).ok()?..usize::try_from(range.end).ok()?)
    }
}

pub struct Sources<'a> {
    documents: Vec<&'a DocumentSnapshot>,
}

impl<'a> Sources<'a> {
    /// Preserves the input document order.
    #[must_use]
    pub fn new(documents: Vec<&'a DocumentSnapshot>) -> Self {
        Self { documents }
    }

    /// Returns selected documents in input order.
    #[must_use]
    pub fn documents(&self) -> &[&'a DocumentSnapshot] {
        &self.documents
    }

    /// Iterates over selected documents in input order.
    pub fn iter(&self) -> impl Iterator<Item = &'a DocumentSnapshot> + '_ {
        self.documents.iter().copied()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.documents.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.documents.is_empty()
    }
}

// -----------------------------------------------------------------------------
// Source Coordinates
// -----------------------------------------------------------------------------

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct SourceSpan {
    pub path: String,
    pub range: SourceRange,
}

/// A half-open UTF-8 byte range.
///
/// `start` belongs to the range. `end` does not belong to the range.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct SourceRange {
    pub start: u32,
    pub end: u32,
}

// -----------------------------------------------------------------------------
// Content Hashes
// -----------------------------------------------------------------------------

/// Identifies the text of one source range.
///
/// The value is a 64-bit FNV-1a hash defined here, so it stays identical on
/// every machine and every toolchain.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ContentHash(u64);

impl ContentHash {
    /// Hashes one text slice.
    #[must_use]
    pub fn of(text: &str) -> Self {
        Self(fold(OFFSET_BASIS, text.as_bytes()))
    }
}

/// Hashes the path and text of every document, in path order.
///
/// A changed, added, removed, or renamed document changes the hash.
#[must_use]
pub fn sources_hash(documents: &[DocumentSnapshot]) -> ContentHash {
    let mut ordered = documents.iter().collect::<Vec<_>>();
    ordered.sort_by(|left, right| left.path.cmp(&right.path));
    let mut hash = OFFSET_BASIS;
    for document in ordered {
        let path = document.path.to_string_lossy();
        for part in [path.as_bytes(), document.content.as_bytes()] {
            hash = fold(hash, &part.len().to_le_bytes());
            hash = fold(hash, part);
        }
    }
    ContentHash(hash)
}

/// Adds bytes to an FNV-1a hash.
fn fold(mut hash: u64, bytes: &[u8]) -> u64 {
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(PRIME);
    }
    hash
}

impl fmt::Display for ContentHash {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{:016x}", self.0)
    }
}

const OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
const PRIME: u64 = 0x0000_0100_0000_01b3;

/// Returns the byte range whose text identifies one entity.
///
/// The range runs from the first declaration to the body end, so a nested
/// change marks every ancestor of its owner. Without a body, the range covers
/// the declarations. Without a body and declarations, it is the scope. The
/// indexer hashes this range, and the viewer source cut checks the same range.
#[must_use]
pub fn content_span(
    declarations: &[SourceRange],
    body: Option<SourceRange>,
    scope: Option<SourceRange>,
) -> Option<SourceRange> {
    let start = declarations.iter().map(|range| range.start).min();
    let end = declarations.iter().map(|range| range.end).max();

    if let Some(body) = body {
        return Some(SourceRange {
            start: start.unwrap_or(body.start),
            end: body.end,
        });
    }

    match (start, end) {
        (Some(start), Some(end)) => Some(SourceRange { start, end }),
        _ => scope,
    }
}

// -----------------------------------------------------------------------------
// Loading
// -----------------------------------------------------------------------------

pub const DEFAULT_EXCLUDED_DIRECTORIES: &[&str] = &["node_modules", "target", "__pycache__"];

/// Loads matching source documents from one codebase root.
///
/// Returned paths are relative to `root`. The walk visits sorted directory
/// entries depth-first. It skips hidden entries and excluded directories.
///
/// # Errors
///
/// Returns an error if the walk cannot read a directory or source file.
pub fn load_documents(
    root: &Path,
    extensions: &[String],
    excluded_directories: &[&str],
) -> io::Result<Vec<DocumentSnapshot>> {
    let mut documents = Vec::new();
    load_directory(root, root, extensions, excluded_directories, &mut documents)?;
    Ok(documents)
}

/// # Errors
///
/// Returns an error that names the directory or source file that the walk
/// cannot read. A source file that is not UTF-8 is an error.
fn load_directory(
    root: &Path,
    directory: &Path,
    extensions: &[String],
    excluded_directories: &[&str],
    documents: &mut Vec<DocumentSnapshot>,
) -> io::Result<()> {
    let mut entries = fs::read_dir(directory)
        .and_then(Iterator::collect::<io::Result<Vec<_>>>)
        .map_err(|error| read_error(directory, &error))?;
    entries.sort_by_key(fs::DirEntry::path);

    for entry in entries {
        let path = entry.path();
        let file_type = entry
            .file_type()
            .map_err(|error| read_error(&path, &error))?;

        if is_hidden(&path) {
            continue;
        }

        if file_type.is_dir() && !is_excluded(&path, excluded_directories) {
            load_directory(root, &path, extensions, excluded_directories, documents)?;
        }

        if !file_type.is_file() || !has_extension(&path, extensions) {
            continue;
        }

        let relative_path = path
            .strip_prefix(root)
            .map_err(io::Error::other)?
            .to_path_buf();

        let content = fs::read_to_string(&path).map_err(|error| read_error(&path, &error))?;
        documents.push(DocumentSnapshot {
            path: relative_path,
            content,
        });
    }

    Ok(())
}

fn read_error(path: &Path, error: &io::Error) -> io::Error {
    io::Error::new(
        error.kind(),
        format!("cannot read {}: {error}", path.display()),
    )
}

fn is_hidden(path: &Path) -> bool {
    path.file_name()
        .and_then(OsStr::to_str)
        .is_some_and(|name| name.starts_with('.'))
}

fn is_excluded(path: &Path, excluded_directories: &[&str]) -> bool {
    path.file_name()
        .and_then(OsStr::to_str)
        .is_some_and(|name| excluded_directories.contains(&name))
}

pub(crate) fn document_has_extension(document: &DocumentSnapshot, extensions: &[String]) -> bool {
    has_extension(&document.path, extensions)
}

fn has_extension(path: &Path, extensions: &[String]) -> bool {
    path.extension()
        .and_then(OsStr::to_str)
        .is_some_and(|extension| extensions.iter().any(|candidate| candidate == extension))
}

// -----------------------------------------------------------------------------
// Tests
// -----------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::{
        fs, io,
        path::{Path, PathBuf},
    };

    use super::{
        ContentHash, DEFAULT_EXCLUDED_DIRECTORIES, DocumentSnapshot, SourceRange,
        document_has_extension, has_extension, is_excluded, is_hidden, load_documents,
        sources_hash,
    };

    fn document(content: &str) -> DocumentSnapshot {
        DocumentSnapshot {
            path: PathBuf::from("app.py"),
            content: content.to_owned(),
        }
    }

    #[test]
    fn hashes_all_documents_in_path_order() {
        let at = |path: &str, content: &str| DocumentSnapshot {
            path: PathBuf::from(path),
            content: content.to_owned(),
        };
        let hash = |documents: &[DocumentSnapshot]| sources_hash(documents).to_string();
        let base = hash(&[at("a.rs", "fn a() {}"), at("b.rs", "fn b() {}")]);

        assert_eq!(
            hash(&[at("b.rs", "fn b() {}"), at("a.rs", "fn a() {}")]),
            base
        );
        assert_ne!(
            hash(&[at("a.rs", "fn a() {}"), at("b.rs", "fn c() {}")]),
            base
        );
        assert_ne!(
            hash(&[at("a.rs", "fn a() {}"), at("c.rs", "fn b() {}")]),
            base
        );
        assert_ne!(hash(&[at("a.rs", "fn a() {}")]), base);
        // A path and a text that join to the same bytes still differ.
        assert_ne!(hash(&[at("ab", "c")]), hash(&[at("a", "bc")]));
    }

    #[test]
    fn matches_the_published_fnv_vectors() {
        assert_eq!(ContentHash::of("").to_string(), "cbf29ce484222325");
        assert_eq!(ContentHash::of("a").to_string(), "af63dc4c8601ec8c");
    }

    #[test]
    fn distinguishes_equal_and_different_text() {
        assert_eq!(ContentHash::of("def run()"), ContentHash::of("def run()"));
        assert_ne!(ContentHash::of("def run()"), ContentHash::of("def runx"));
    }

    #[test]
    fn returns_the_text_of_one_range() {
        let snapshot = document("value = 1\n");

        assert_eq!(
            snapshot.text(SourceRange { start: 0, end: 5 }),
            Some("value")
        );
    }

    #[test]
    fn rejects_a_range_outside_the_document() {
        let snapshot = document("value = 1\n");

        assert_eq!(snapshot.text(SourceRange { start: 0, end: 100 }), None);
    }

    #[test]
    fn rejects_a_range_that_splits_a_character() {
        let snapshot = document("\u{e9} = 1\n");

        assert_eq!(snapshot.text(SourceRange { start: 0, end: 1 }), None);
    }

    #[test]
    fn skips_every_hidden_entry() {
        assert!(is_hidden(Path::new("src/.venv")));
        assert!(is_hidden(Path::new("src/.archived")));
        assert!(is_hidden(Path::new(".git")));
        assert!(is_hidden(Path::new("src/.hidden.py")));

        assert!(!is_hidden(Path::new("src/app.py")));
        assert!(!is_hidden(Path::new("src/__init__.py")));
    }

    #[test]
    fn skips_every_named_directory() {
        assert!(is_excluded(
            Path::new("a/node_modules"),
            DEFAULT_EXCLUDED_DIRECTORIES
        ));
        assert!(is_excluded(
            Path::new("a/__pycache__"),
            DEFAULT_EXCLUDED_DIRECTORIES
        ));
        assert!(!is_excluded(
            Path::new("a/src"),
            DEFAULT_EXCLUDED_DIRECTORIES
        ));
    }

    #[test]
    fn keeps_only_requested_extensions() {
        let extensions = ["py".to_owned(), "pyi".to_owned()];

        assert!(has_extension(Path::new("a/app.py"), &extensions));
        assert!(has_extension(Path::new("a/app.pyi"), &extensions));
        assert!(!has_extension(Path::new("a/app.rs"), &extensions));
        assert!(!has_extension(Path::new("a/Makefile"), &extensions));
    }

    #[test]
    fn matches_document_extensions_from_profiles() {
        let document = DocumentSnapshot {
            path: PathBuf::from("src/lib.custom"),
            content: String::new(),
        };

        assert!(document_has_extension(&document, &["custom".to_owned()]));
        assert!(!document_has_extension(&document, &["rs".to_owned()]));
    }

    #[test]
    fn names_a_source_file_that_is_not_utf8() {
        let directory = tempfile::tempdir().expect("the test folder should be created");
        let path = directory.path().join("latin1.py");
        fs::write(&path, b"name = '\xe9'\n").expect("the source file should be written");

        let Err(error) = load_documents(directory.path(), &["py".to_owned()], &[]) else {
            panic!("a file that is not UTF-8 should stop the load");
        };

        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(
            error.to_string().contains(&path.display().to_string()),
            "the error should name the file: {error}"
        );
    }
}
