//! Diff derivation: the checked agent document and one row of facts per step.
//!
//! Two frozen flows, the agent document, the two graphs, and a reader of file
//! text at each commit are the inputs. The pairs, the notes, and the rows are
//! the output. The module reads files only through the reader.
//!
//! # Sections
//!
//! `Agent document` checks the pairs and the notes.
//! `Rows` derives the kind, the spans, and the line diff of each step.
//! `Spans` cuts one function out of one file text.

use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;

use super::{
    lines::{counts, diff_lines},
    models::{
        AgentDiff, ChangeKind, ClusterNote, DIFF_CONTRACT_VERSION, DiffRow, LineSpan, Side,
        StepPair,
    },
};
use crate::{
    flow::{
        models::{FlowArtifact, FlowStep},
        text,
    },
    source_view::{CutError, cut},
    store::graph::{ArtifactGraph, ArtifactNode},
};

/// The longest cluster note, in characters.
const MAX_NOTE: usize = 240;

/// One problem of an agent document or of a derivation. `path` is a JSON pointer.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct DiffIssue {
    pub path: String,
    pub message: String,
}

impl DiffIssue {
    fn new(path: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            message: message.into(),
        }
    }
}

/// Returns the file text at one side's commit, or `None` when the file is not there.
pub type Reader<'a> = dyn Fn(Side, &str) -> Option<String> + 'a;

// -----------------------------------------------------------------------------
// Agent document
// -----------------------------------------------------------------------------

/// Checks the agent document against the two flows and returns the pairs and notes by ID.
///
/// A step can be in at most one pair. A cluster can have at most one note on each side.
///
/// # Errors
/// Returns every issue of the document.
pub fn check_agent<'a>(
    agent: &AgentDiff,
    before: &'a FlowArtifact,
    after: &'a FlowArtifact,
) -> Result<(Vec<StepPair>, Vec<ClusterNote>), Vec<DiffIssue>> {
    let mut issues = Vec::new();
    if agent.contract_version != DIFF_CONTRACT_VERSION {
        issues.push(DiffIssue::new(
            "/contractVersion",
            format!("use contract version {DIFF_CONTRACT_VERSION}"),
        ));
    }
    let (old, new) = (&before.intent.steps, &after.intent.steps);
    let mut used = (BTreeSet::new(), BTreeSet::new());
    let mut pairs = Vec::new();
    for (index, pair) in agent.pairs.iter().enumerate() {
        let path = format!("/pairs/{index}");
        let step =
            |number: usize, steps: &'a [FlowStep], side: &str| -> Result<&'a FlowStep, DiffIssue> {
                number
                    .checked_sub(1)
                    .and_then(|at| steps.get(at))
                    .ok_or_else(|| {
                        DiffIssue::new(
                            format!("{path}/{side}"),
                            format!(
                                "{side} step {number} does not exist: the flow has steps 1 to {}",
                                steps.len()
                            ),
                        )
                    })
            };
        match (
            step(pair.before, old, "before"),
            step(pair.after, new, "after"),
        ) {
            (Ok(first), Ok(second)) => {
                if !used.0.insert(pair.before) {
                    issues.push(DiffIssue::new(
                        format!("{path}/before"),
                        format!("before step {} is in more than one pair", pair.before),
                    ));
                }
                if !used.1.insert(pair.after) {
                    issues.push(DiffIssue::new(
                        format!("{path}/after"),
                        format!("after step {} is in more than one pair", pair.after),
                    ));
                }
                pairs.push(StepPair {
                    before: first.id.clone(),
                    after: second.id.clone(),
                });
            }
            (first, second) => issues.extend([first.err(), second.err()].into_iter().flatten()),
        }
    }
    let mut notes = Vec::new();
    let mut seen_notes = BTreeSet::new();
    for (index, note) in agent.notes.iter().enumerate() {
        let path = format!("/notes/{index}");
        let flow = match note.side {
            Side::Before => before,
            Side::After => after,
        };
        let key = flow
            .intent
            .clusters
            .iter()
            .find(|cluster| cluster.key.as_str() == note.cluster_key)
            .map(|cluster| cluster.key.clone());
        let Some(key) = key else {
            issues.push(DiffIssue::new(
                format!("{path}/clusterKey"),
                format!(
                    "the {} flow has no cluster {}",
                    side_name(note.side),
                    note.cluster_key
                ),
            ));
            continue;
        };
        if !seen_notes.insert((note.side, key.clone())) {
            issues.push(DiffIssue::new(
                format!("{path}/clusterKey"),
                format!("the cluster {key} has more than one note on this side"),
            ));
        }
        let note_text = text::canonical(&note.text);
        if let Err(fault) = text::check_line(&note_text, 1, MAX_NOTE) {
            issues.push(DiffIssue::new(format!("{path}/text"), fault.to_string()));
        }
        notes.push(ClusterNote {
            side: note.side,
            cluster_key: key,
            text: note_text,
        });
    }
    if issues.is_empty() {
        Ok((pairs, notes))
    } else {
        Err(issues)
    }
}

fn side_name(side: Side) -> &'static str {
    match side {
        Side::Before => "Before",
        Side::After => "After",
    }
}

// -----------------------------------------------------------------------------
// Rows
// -----------------------------------------------------------------------------

/// Derives one row for each pair, each new After step, and each removed Before step.
///
/// The spans come from the graph of each commit. A new step compares with the
/// node of the same key on the base, and a removed step with the node on the head.
///
/// # Errors
/// Returns every step whose node is not in its graph or whose file text does not match the graph.
pub fn derive_rows(
    before: &FlowArtifact,
    after: &FlowArtifact,
    pairs: &[StepPair],
    graphs: [&ArtifactGraph; 2],
    read: &Reader<'_>,
) -> Result<Vec<DiffRow>, Vec<DiffIssue>> {
    let nodes = graphs.map(|graph| {
        graph
            .nodes
            .iter()
            .map(|node| (node.key.as_str(), node))
            .collect::<BTreeMap<_, _>>()
    });
    let mut issues = Vec::new();
    let mut locate = |side: Side, key: &str, required: bool| -> Option<LineSpan> {
        let index = usize::from(side == Side::After);
        let Some(node) = nodes[index].get(key) else {
            if required {
                issues.push(DiffIssue::new(
                    "/rows",
                    format!("the {} graph has no node {key}", side_name(side)),
                ));
            }
            return None;
        };
        match function_span(side, node, read) {
            Ok(span) => span,
            Err(message) => {
                issues.push(DiffIssue::new("/rows", message));
                None
            }
        }
    };
    let mut rows = Vec::new();
    for_each_row(before, after, pairs, |old, new| {
        let base_key = old.or(new).map(|step| step.node_key.as_str());
        let head_key = new.or(old).map(|step| step.node_key.as_str());
        let base_span = base_key.and_then(|key| locate(Side::Before, key, old.is_some()));
        let head_span = head_key.and_then(|key| locate(Side::After, key, new.is_some()));
        rows.push(row(before, after, old, new, base_span, head_span, read));
    });
    if issues.is_empty() {
        Ok(rows)
    } else {
        Err(issues)
    }
}

/// Derives the rows again from their recorded spans and the file text of each commit.
///
/// Verify uses this: it needs no graph.
#[must_use]
pub fn rederive_rows(
    before: &FlowArtifact,
    after: &FlowArtifact,
    pairs: &[StepPair],
    spans: &[(Option<LineSpan>, Option<LineSpan>)],
    read: &Reader<'_>,
) -> Vec<DiffRow> {
    let mut rows = Vec::new();
    let mut spans = spans.iter();
    for_each_row(before, after, pairs, |old, new| {
        let (base_span, head_span) = spans.next().cloned().unwrap_or_default();
        rows.push(row(before, after, old, new, base_span, head_span, read));
    });
    rows
}

/// Calls `visit` for each row in row order: the After steps, then the removed Before steps.
fn for_each_row<'a>(
    before: &'a FlowArtifact,
    after: &'a FlowArtifact,
    pairs: &[StepPair],
    mut visit: impl FnMut(Option<&'a FlowStep>, Option<&'a FlowStep>),
) {
    let partner: BTreeMap<&str, &str> = pairs
        .iter()
        .map(|pair| (pair.after.as_str(), pair.before.as_str()))
        .collect();
    let paired: BTreeSet<&str> = partner.values().copied().collect();
    let by_id: BTreeMap<&str, &FlowStep> = before
        .intent
        .steps
        .iter()
        .map(|step| (step.id.as_str(), step))
        .collect();
    for step in &after.intent.steps {
        let old = partner
            .get(step.id.as_str())
            .and_then(|id| by_id.get(id).copied());
        visit(old, Some(step));
    }
    for step in &before.intent.steps {
        if !paired.contains(step.id.as_str()) {
            visit(Some(step), None);
        }
    }
}

/// Builds one row. The kind of a pair comes from the node paths of the two frozen flows.
fn row(
    before: &FlowArtifact,
    after: &FlowArtifact,
    old: Option<&FlowStep>,
    new: Option<&FlowStep>,
    base_span: Option<LineSpan>,
    head_span: Option<LineSpan>,
    read: &Reader<'_>,
) -> DiffRow {
    let text = |side, span: &Option<LineSpan>| {
        span.as_ref()
            .map(|span| span_lines(side, span, read))
            .unwrap_or_default()
    };
    let lines = diff_lines(
        &text(Side::Before, &base_span),
        &text(Side::After, &head_span),
    );
    let (added, removed) = counts(&lines);
    let kind = match (old, new) {
        // A pair whose function is in another file moved.
        (Some(old), Some(new)) if node_path(before, old) != node_path(after, new) => {
            ChangeKind::Moved
        }
        // A pair with a line change changed.
        (Some(_), Some(_)) if added + removed > 0 => ChangeKind::Changed,
        // A pair with no line change is unchanged.
        (Some(_), Some(_)) => ChangeKind::Unchanged,
        // A step that only the head has is new.
        (None, _) => ChangeKind::New,
        // A step that only the base has is removed.
        (Some(_), None) => ChangeKind::Removed,
    };
    DiffRow {
        kind,
        before_step: old.map(|step| step.id.clone()),
        after_step: new.map(|step| step.id.clone()),
        base_span,
        head_span,
        added,
        removed,
        lines,
    }
}

fn node_path<'a>(flow: &'a FlowArtifact, step: &FlowStep) -> Option<&'a str> {
    flow.facts
        .nodes
        .iter()
        .find(|node| node.key == step.node_key)
        .map(|node| node.path.as_str())
}

// -----------------------------------------------------------------------------
// Spans
// -----------------------------------------------------------------------------

/// Returns the lines of one function on one commit, with its docs and attributes.
///
/// # Errors
/// Returns a message when the file is missing or no longer matches the graph node.
fn function_span(
    side: Side,
    node: &ArtifactNode,
    read: &Reader<'_>,
) -> Result<Option<LineSpan>, String> {
    let stale = || {
        format!(
            "{} of the {} commit does not match its graph: index that commit again",
            node.path,
            side_name(side)
        )
    };
    let text = read(side, &node.path).ok_or_else(stale)?;
    match cut(&text, node) {
        Ok(source) => {
            let total = source.lines.len() + source.more;
            let line = |number: usize| u32::try_from(number).map_err(|_| stale());
            Ok(Some(LineSpan {
                path: node.path.clone(),
                start: line(source.first_line)?,
                end: line(source.first_line + total.max(1) - 1)?,
            }))
        }
        Err(CutError::NoSpan) => Ok(None),
        Err(CutError::Changed) => Err(stale()),
    }
}

/// Returns the lines of one span. A missing file or line gives fewer lines.
fn span_lines(side: Side, span: &LineSpan, read: &Reader<'_>) -> Vec<String> {
    let Some(text) = read(side, &span.path) else {
        return Vec::new();
    };
    let skip = span.start.saturating_sub(1) as usize;
    let take = (span.end + 1).saturating_sub(span.start) as usize;
    text.lines()
        .skip(skip)
        .take(take)
        .map(|line| line.trim_end_matches('\r').to_owned())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff::models::{AgentNote, AgentPair, LineOp};
    use crate::flow::fixtures;

    /// Freezes the fixture output and returns its generation.
    fn flow() -> FlowArtifact {
        let store = fixtures::store();
        fixtures::frozen(fixtures::import(store.path(), "demo", &fixtures::output()));
        fixtures::current(store.path(), "demo")
    }

    #[test]
    fn check_agent_reports_a_missing_step_and_a_step_in_two_pairs() {
        let flow = flow();
        let agent = AgentDiff {
            contract_version: 1,
            pairs: vec![
                AgentPair {
                    before: 1,
                    after: 1,
                },
                AgentPair {
                    before: 1,
                    after: 2,
                },
                AgentPair {
                    before: 99,
                    after: 1,
                },
            ],
            notes: vec![AgentNote {
                side: Side::After,
                cluster_key: "missing".to_owned(),
                text: "Now it reads twice.".to_owned(),
            }],
        };
        let issues = check_agent(&agent, &flow, &flow).expect_err("rejected");
        let paths: Vec<_> = issues.iter().map(|issue| issue.path.as_str()).collect();
        assert_eq!(
            paths,
            ["/pairs/1/before", "/pairs/2/before", "/notes/0/clusterKey"]
        );
    }

    #[test]
    fn rows_list_after_steps_then_removed_before_steps_with_their_kind() {
        let before = flow();
        let after = flow();
        let steps = &before.intent.steps;
        assert!(steps.len() >= 2, "the fixture has two steps");
        // Pair before 1 with after 2 only: after 1 is new, before 2 is removed.
        let pairs = vec![StepPair {
            before: steps[0].id.clone(),
            after: after.intent.steps[1].id.clone(),
        }];
        let read = |_: Side, _: &str| None;
        let rows = rederive_rows(&before, &after, &pairs, &[], &read);
        let kinds: Vec<_> = rows.iter().map(|row| row.kind).collect();
        assert_eq!(kinds[0], ChangeKind::New);
        assert!(matches!(
            kinds[1],
            ChangeKind::Unchanged | ChangeKind::Moved
        ));
        assert_eq!(kinds.last(), Some(&ChangeKind::Removed));
        assert_eq!(rows.len(), after.intent.steps.len() + steps.len() - 1);
    }

    #[test]
    fn rederived_rows_diff_the_function_lines_of_each_commit() {
        let flow = flow();
        let step = flow.intent.steps[0].id.clone();
        let pairs = vec![StepPair {
            before: step.clone(),
            after: step,
        }];
        let span = |path: &str| LineSpan {
            path: path.to_owned(),
            start: 2,
            end: 4,
        };
        let read = |side: Side, _: &str| {
            Some(match side {
                Side::Before => "x\nfn a() {\n  old();\n}\n".to_owned(),
                Side::After => "x\nfn a() {\n  new();\n}\n".to_owned(),
            })
        };
        let rows = rederive_rows(
            &flow,
            &flow,
            &pairs,
            &[(Some(span("a.rs")), Some(span("a.rs")))],
            &read,
        );
        assert_eq!(rows[0].kind, ChangeKind::Changed);
        assert_eq!((rows[0].added, rows[0].removed), (1, 1));
        assert_eq!(rows[0].lines[0].op, LineOp::Context);
        assert_eq!(rows[0].lines[0].text, "fn a() {");
    }
}
