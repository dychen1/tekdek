//! Function source view.
//!
//! This module cuts the source of the step functions of one flow out of their
//! files for the viewer's code column. A graph, a frozen flow, and a reader of
//! project files are the inputs. One list of function sources is the output.
//! The module reads files only through the reader that the caller gives it.
//!
//! # Sections
//!
//! `Document` defines the JSON answer.
//! `Flow sources` cuts each distinct step function in step order.
//! `Cutting` turns one file text and one graph node into lines.
//! `Lines` maps byte offsets to lines.

use std::collections::{HashMap, HashSet};

use serde::Serialize;

use crate::{
    analysis::source::ContentHash,
    flow::models::FlowArtifact,
    store::graph::{ArtifactGraph, ArtifactNode},
};

// -----------------------------------------------------------------------------
// Document
// -----------------------------------------------------------------------------

/// The most lines that one function shows. The answer counts the rest in `more`.
pub const MAX_LINES: usize = 400;

/// The source of every distinct step function of one flow.
#[derive(Debug, Serialize)]
pub struct SourceDocument {
    pub artifact: String,
    pub flow: String,
    pub functions: Vec<FunctionSource>,
}

/// The source of one step function, or why it has none.
#[derive(Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FunctionSource {
    pub key: String,
    pub status: SourceStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(flatten)]
    pub cut: Option<SourceCut>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceStatus {
    Ok,
    Missing,
    Changed,
    Unreadable,
}

/// The lines of one function. Line numbers are one-based.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceCut {
    pub first_line: usize,
    pub declaration_line: usize,
    pub doc_lines: Option<[usize; 2]>,
    pub signature_lines: [usize; 2],
    pub lines: Vec<String>,
    pub more: usize,
}

impl FunctionSource {
    fn bare(key: &str, status: SourceStatus) -> Self {
        Self {
            key: key.to_owned(),
            status,
            path: None,
            language: None,
            reason: None,
            cut: None,
        }
    }

    fn at(node: &ArtifactNode, status: SourceStatus) -> Self {
        Self {
            path: Some(node.path.clone()),
            language: Some(node.language.clone()),
            ..Self::bare(&node.key, status)
        }
    }
}

// -----------------------------------------------------------------------------
// Flow sources
// -----------------------------------------------------------------------------

/// Cuts the source of each distinct step function of the flow, in step order.
///
/// `read` returns the text of one project file, or the reason it cannot. Each file is read once.
pub fn flow_sources(
    graph: &ArtifactGraph,
    flow: &FlowArtifact,
    mut read: impl FnMut(&str) -> Result<String, String>,
) -> Vec<FunctionSource> {
    let nodes: HashMap<&str, &ArtifactNode> = graph
        .nodes
        .iter()
        .map(|node| (node.key.as_str(), node))
        .collect();
    let mut texts: HashMap<String, Result<String, String>> = HashMap::new();
    let mut seen = HashSet::new();
    let mut functions = Vec::new();
    for step in &flow.intent.steps {
        let key = step.node_key.as_str();
        if !seen.insert(key) {
            continue;
        }
        let Some(node) = nodes.get(key) else {
            functions.push(FunctionSource::bare(key, SourceStatus::Missing));
            continue;
        };
        let text = texts
            .entry(node.path.clone())
            .or_insert_with(|| read(&node.path));
        functions.push(match &*text {
            Err(reason) => FunctionSource {
                reason: Some(reason.clone()),
                ..FunctionSource::at(node, SourceStatus::Unreadable)
            },
            Ok(text) => match cut(text, node) {
                Ok(cut) => FunctionSource {
                    cut: Some(cut),
                    ..FunctionSource::at(node, SourceStatus::Ok)
                },
                Err(CutError::Changed) => FunctionSource::at(node, SourceStatus::Changed),
                Err(CutError::NoSpan) => FunctionSource {
                    reason: Some("the graph has no source range for this function".to_owned()),
                    ..FunctionSource::at(node, SourceStatus::Unreadable)
                },
            },
        });
    }
    functions
}

// -----------------------------------------------------------------------------
// Cutting
// -----------------------------------------------------------------------------

/// Why one node cannot be cut from its file text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CutError {
    /// The node has no scope and no declaration.
    NoSpan,
    /// The text no longer hashes to the node's content hash, or a range leaves the text.
    Changed,
}

/// Cuts one node out of its file text.
///
/// The cut starts at the Rust doc comments and attributes, or the Python
/// decorators, right above the scope, and ends at the scope's last line.
///
/// # Errors
///
/// Returns an error if the node has no source range, or if the text changed
/// since the graph was built.
pub fn cut(text: &str, node: &ArtifactNode) -> Result<SourceCut, CutError> {
    let (start, end) = scope_range(node).ok_or(CutError::NoSpan)?;
    if !holds(text, start, end) || !hash_matches(text, node) {
        return Err(CutError::Changed);
    }
    let lines = Lines::new(text);
    let scope_first = lines.line_of(start);
    let scope_last = lines.line_of(end.saturating_sub(1).max(start));
    let first = scope_first - leading(&lines, scope_first, &node.language);
    let declaration = node
        .declarations
        .iter()
        .map(|range| range.start as usize)
        .min()
        .filter(|&offset| holds(text, offset, offset))
        .map_or(scope_first, |offset| lines.line_of(offset));
    let signature_end = match &node.body {
        // A Rust body opens on the signature's last line; a Python body starts below it.
        Some(body) if holds(text, body.start as usize, body.end as usize) => {
            let line = lines.line_of(body.start as usize);
            if text[body.start as usize..].starts_with('{') {
                line
            } else {
                line.saturating_sub(1).max(declaration)
            }
        }
        _ => declaration,
    };
    let docs: Vec<usize> = (first..scope_first)
        .filter(|&line| lines.text(line).trim_start().starts_with("///"))
        .collect();
    let total = scope_last - first + 1;
    let shown = total.min(MAX_LINES);
    Ok(SourceCut {
        first_line: first + 1,
        declaration_line: declaration + 1,
        doc_lines: docs.first().zip(docs.last()).map(|(a, b)| [a + 1, b + 1]),
        signature_lines: [declaration + 1, signature_end + 1],
        lines: (first..first + shown)
            .map(|line| lines.text(line).to_owned())
            .collect(),
        more: total - shown,
    })
}

/// The scope, or the span of the declarations when the graph has no scope.
fn scope_range(node: &ArtifactNode) -> Option<(usize, usize)> {
    if let Some(scope) = &node.scope {
        return Some((scope.start as usize, scope.end as usize));
    }
    let start = node.declarations.iter().map(|range| range.start).min()?;
    let end = node.declarations.iter().map(|range| range.end).max()?;
    Some((start as usize, end as usize))
}

/// True when the half-open byte range lies inside the text on character boundaries.
fn holds(text: &str, start: usize, end: usize) -> bool {
    start <= end && end <= text.len() && text.is_char_boundary(start) && text.is_char_boundary(end)
}

/// True when the node has no content hash, or the text still hashes to it.
///
/// The hashed range is the indexer's: the first declaration to the body end.
fn hash_matches(text: &str, node: &ArtifactNode) -> bool {
    let Some(expected) = &node.content_hash else {
        return true;
    };
    let start = node.declarations.iter().map(|range| range.start).min();
    let end = node.declarations.iter().map(|range| range.end).max();
    let range = match (&node.body, start, end) {
        (Some(body), _, _) => Some((start.unwrap_or(body.start), body.end)),
        (None, Some(start), Some(end)) => Some((start, end)),
        (None, _, _) => node.scope.as_ref().map(|scope| (scope.start, scope.end)),
    };
    range
        .and_then(|(start, end)| text.get(start as usize..end as usize))
        .is_some_and(|slice| ContentHash::of(slice).to_string() == *expected)
}

/// Counts the lines right above the scope that belong to it.
fn leading(lines: &Lines<'_>, scope_first: usize, language: &str) -> usize {
    let prefixes: &[&str] = match language {
        "rust" => &["///", "#["],
        "python" => &["@"],
        _ => &[],
    };
    (0..scope_first)
        .rev()
        .take_while(|&line| {
            let text = lines.text(line).trim_start();
            prefixes.iter().any(|prefix| text.starts_with(prefix))
        })
        .count()
}

// -----------------------------------------------------------------------------
// Lines
// -----------------------------------------------------------------------------

/// The line starts of one text.
struct Lines<'a> {
    text: &'a str,
    starts: Vec<usize>,
}

impl<'a> Lines<'a> {
    fn new(text: &'a str) -> Self {
        let mut starts = vec![0];
        starts.extend(text.match_indices('\n').map(|(index, _)| index + 1));
        Self { text, starts }
    }

    /// Returns the zero-based line that holds the byte offset.
    fn line_of(&self, offset: usize) -> usize {
        self.starts.partition_point(|&start| start <= offset) - 1
    }

    /// Returns one zero-based line without its line ending.
    fn text(&self, line: usize) -> &'a str {
        let start = self.starts[line];
        let end = self
            .starts
            .get(line + 1)
            .map_or(self.text.len(), |next| next - 1);
        self.text[start..end].trim_end_matches('\r')
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{CutError, MAX_LINES, SourceStatus, cut, flow_sources};
    use crate::{
        analysis::source::ContentHash,
        flow::fixtures::{current, frozen, import, output, write_graph},
        store::graph::{ArtifactNode, ArtifactRange},
    };

    const RUST: &str = "use std::fs;\n\n/// Reads the file.\n/// Returns its text.\n#[must_use]\npub fn read(path: &str)\n    -> String {\n    fs::read_to_string(path).unwrap()\n}\n";
    const PYTHON: &str = "import os\n\n@cache\ndef load(path):\n    \"\"\"Load it.\"\"\"\n    return os.path.basename(path)\n";

    fn at(text: &str, needle: &str) -> u32 {
        u32::try_from(text.find(needle).expect(needle)).expect("offset")
    }

    fn end(text: &str, needle: &str) -> u32 {
        u32::try_from(text.rfind(needle).expect(needle) + needle.len()).expect("offset")
    }

    /// A callable node of `text` with one declaration, a scope, and a body, hashed as the indexer hashes it.
    fn node(
        text: &str,
        language: &str,
        name: (u32, u32),
        scope: (u32, u32),
        body: (u32, u32),
    ) -> ArtifactNode {
        let range = |(start, end): (u32, u32)| ArtifactRange { start, end };
        ArtifactNode {
            id: 0,
            language: language.to_owned(),
            key: "src/a::callable:f".to_owned(),
            name: "f".to_owned(),
            kind: "callable".to_owned(),
            content_hash: Some(
                ContentHash::of(&text[name.0 as usize..body.1 as usize]).to_string(),
            ),
            path: "src/a".to_owned(),
            origin: "syntax".to_owned(),
            declarations: vec![range(name)],
            scope: Some(range(scope)),
            body: Some(range(body)),
            position: None,
            signature: None,
            parameters: vec![],
            return_type: None,
            declared_type: None,
            documentation: None,
        }
    }

    fn rust_node(text: &str) -> ArtifactNode {
        let name = at(text, "read(");
        node(
            text,
            "rust",
            (name, name + 4),
            (at(text, "pub fn"), end(text, "}")),
            (at(text, "{"), end(text, "}")),
        )
    }

    #[test]
    fn cuts_a_rust_function_with_its_doc_comment_and_attributes() {
        let cut = cut(RUST, &rust_node(RUST)).expect("cut");
        assert_eq!(cut.first_line, 3);
        assert_eq!(cut.declaration_line, 6);
        assert_eq!(cut.doc_lines, Some([3, 4]));
        assert_eq!(cut.signature_lines, [6, 7]);
        assert_eq!(
            cut.lines.first().map(String::as_str),
            Some("/// Reads the file.")
        );
        assert_eq!(cut.lines.last().map(String::as_str), Some("}"));
        assert_eq!((cut.lines.len(), cut.more), (7, 0));
    }

    #[test]
    fn cuts_a_python_function_with_its_decorator_and_no_doc_mark() {
        let name = at(PYTHON, "load(");
        let body = (at(PYTHON, "\"\"\""), end(PYTHON, "(path)"));
        let cut = cut(
            PYTHON,
            &node(
                PYTHON,
                "python",
                (name, name + 4),
                (at(PYTHON, "def"), body.1),
                body,
            ),
        )
        .expect("cut");
        assert_eq!((cut.first_line, cut.declaration_line), (3, 4));
        assert_eq!(cut.doc_lines, None);
        assert_eq!(cut.signature_lines, [4, 4]);
        assert_eq!(
            cut.lines,
            [
                "@cache",
                "def load(path):",
                "    \"\"\"Load it.\"\"\"",
                "    return os.path.basename(path)"
            ]
        );
    }

    #[test]
    fn sends_doc_lines_as_null_when_a_cut_has_none() {
        let name = at(PYTHON, "load(");
        let body = (at(PYTHON, "\"\"\""), end(PYTHON, "(path)"));
        let node = node(
            PYTHON,
            "python",
            (name, name + 4),
            (at(PYTHON, "def"), body.1),
            body,
        );
        let cut = cut(PYTHON, &node).expect("cut");
        assert_eq!(cut.doc_lines, None);
        let json = serde_json::to_value(&cut).expect("json");
        assert_eq!(json.get("docLines"), Some(&serde_json::Value::Null));
    }

    #[test]
    fn drops_carriage_returns_and_keeps_multi_byte_text_whole() {
        let text = RUST
            .replace("Reads the file.", "Reads the café file.")
            .replace('\n', "\r\n");
        let cut = cut(&text, &rust_node(&text)).expect("cut");
        assert_eq!(cut.lines[0], "/// Reads the café file.");
        assert!(cut.lines.iter().all(|line| !line.ends_with('\r')));
    }

    #[test]
    fn reports_a_changed_file_and_a_range_outside_the_text() {
        let node = rust_node(RUST);
        assert_eq!(
            cut(&RUST.replace("unwrap()", "expect(\"x\")"), &node),
            Err(CutError::Changed)
        );
        assert_eq!(cut("fn", &node), Err(CutError::Changed));
    }

    #[test]
    fn reports_a_node_without_a_source_range() {
        let mut node = rust_node(RUST);
        node.scope = None;
        node.declarations.clear();
        assert_eq!(cut(RUST, &node), Err(CutError::NoSpan));
    }

    #[test]
    fn shows_at_most_the_line_cap_and_counts_the_rest() {
        let text = format!("fn big() {{\n{}}}\n", "    x();\n".repeat(450));
        let name = at(&text, "big");
        let cut = cut(
            &text,
            &node(
                &text,
                "rust",
                (name, name + 3),
                (at(&text, "fn"), end(&text, "}")),
                (at(&text, "{"), end(&text, "}")),
            ),
        )
        .expect("cut");
        assert_eq!((cut.lines.len(), cut.more), (MAX_LINES, 452 - MAX_LINES));
    }

    #[test]
    fn cuts_each_distinct_step_function_once_and_reports_why_others_have_no_source() {
        const MAIN: &str = "src/main.rs::callable:main";
        const PARSE: &str = "src/parse.rs::callable:parse_args";
        const READ: &str = "src/parse.rs::callable:read_input";
        const WRITE: &str = "src/store.rs::callable:write_result";
        const LOCK: &str = "src/store.rs::callable:lock_store";

        let store = tempfile::tempdir().expect("store");
        let mut artifact = write_graph(store.path(), "src/extra.rs::callable:helper");
        frozen(import(store.path(), "demo-flow", &output()));
        let flow = current(store.path(), "demo-flow");
        // The flow visits main twice. The fixture nodes have no source range.
        let cut_node = rust_node(RUST);
        for node in &mut artifact.graph.nodes {
            match node.key.as_str() {
                // Gets a real range over the text that the reader returns.
                MAIN => {
                    node.declarations = cut_node.declarations.clone();
                    node.scope = cut_node.scope.clone();
                    node.body = cut_node.body.clone();
                    node.content_hash = cut_node.content_hash.clone();
                }
                // Leaves the graph, while the flow keeps its key.
                PARSE => node.key = "src/parse.rs::callable:renamed".to_owned(),
                _ => {}
            }
        }

        let mut reads = Vec::new();
        let functions = flow_sources(&artifact.graph, &flow, |path| {
            reads.push(path.to_owned());
            match path {
                "src/main.rs" => Ok(RUST.to_owned()),
                "src/parse.rs" => Ok("fn read_input() {}\n".to_owned()),
                _ => Err("the file is missing".to_owned()),
            }
        });

        let steps: Vec<_> = functions
            .iter()
            .map(|function| (function.key.as_str(), function.status))
            .collect();
        assert_eq!(
            steps,
            [
                (MAIN, SourceStatus::Ok),
                (PARSE, SourceStatus::Missing),
                (READ, SourceStatus::Unreadable),
                (WRITE, SourceStatus::Unreadable),
                (LOCK, SourceStatus::Unreadable),
            ]
        );
        // Two functions share store.rs. A missing function reads no file.
        reads.sort();
        assert_eq!(reads, ["src/main.rs", "src/parse.rs", "src/store.rs"]);
        assert_eq!(
            functions[2].reason.as_deref(),
            Some("the graph has no source range for this function")
        );
        assert_eq!(functions[3].reason.as_deref(), Some("the file is missing"));
        assert_eq!(functions[4].reason, functions[3].reason);

        assert_eq!(
            serde_json::to_value(&functions[0]).expect("json"),
            json!({
                "key": MAIN, "status": "ok", "path": "src/main.rs", "language": "rust",
                "firstLine": 3, "declarationLine": 6, "docLines": [3, 4],
                "signatureLines": [6, 7], "more": 0,
                "lines": [
                    "/// Reads the file.", "/// Returns its text.", "#[must_use]",
                    "pub fn read(path: &str)", "    -> String {",
                    "    fs::read_to_string(path).unwrap()", "}"
                ]
            })
        );
        assert_eq!(
            serde_json::to_value(&functions[1]).expect("json"),
            json!({"key": PARSE, "status": "missing"})
        );
        assert_eq!(
            serde_json::to_value(&functions[3]).expect("json"),
            json!({
                "key": WRITE, "status": "unreadable", "path": "src/store.rs",
                "language": "rust", "reason": "the file is missing"
            })
        );
    }
}
