//! Flow validation.
//!
//! This module owns the runtime rules that JSON Schema cannot express. The
//! `agent` checks run on one agent output before a freeze. The `artifact` checks
//! run on one stored generation and recompute its derived layers and digests.
//! Both collect every issue; code never repairs a rejected value.
//!
//! # Sections
//! `Limits` holds the one table of text and count limits. `Shared rules` holds
//! the text, cluster, ledger, mark, and author rules of both checks.

mod agent;
mod artifact;

use std::{collections::BTreeSet, ops::RangeInclusive};

pub use agent::{CheckedFlow, CheckedStep, PriorContext, check_agent_flow};
pub use artifact::verify_artifact;

use super::{
    models::{Author, ClusterIntent, FlowIssue, FlowPhase, IssueCode, StepLedger},
    text,
};

// -----------------------------------------------------------------------------
// Limits
// -----------------------------------------------------------------------------

/// The length limits of one text field. A `block` text can hold tabs and line feeds.
///
/// A test compares each limit with the JSON Schemas.
#[derive(Clone, Copy)]
pub(super) struct TextRule {
    pub min: usize,
    pub max: usize,
    pub block: bool,
}

impl TextRule {
    const fn line(min: usize, max: usize) -> Self {
        Self {
            min,
            max,
            block: false,
        }
    }

    const fn block(min: usize, max: usize) -> Self {
        Self {
            min,
            max,
            block: true,
        }
    }
}

pub(super) const BEHAVIOR: TextRule = TextRule::block(20, 600);
pub(super) const BOUNDARIES: TextRule = TextRule::block(10, 600);
pub(super) const REVISION_SUMMARY: TextRule = TextRule::block(20, 1200);
pub(super) const NO_FLOW_EXPLANATION: TextRule = TextRule::block(20, 1000);
pub(super) const CLUSTER_LABEL: TextRule = TextRule::line(3, 60);
pub(super) const CLUSTER_TYPE: TextRule = TextRule::line(1, 80);
pub(super) const CLUSTER_RULE: TextRule = TextRule::line(10, 200);
pub(super) const PHASE_NOTE: TextRule = TextRule::line(3, 200);
pub(super) const STEP_TITLE: TextRule = TextRule::line(3, 96);
pub(super) const LEDGER_ITEM: TextRule = TextRule::line(2, 200);
pub(super) const CLUSTER_TYPES: RangeInclusive<usize> = 0..=4;
pub(super) const CLUSTER_RULES: RangeInclusive<usize> = 1..=3;
pub(super) const LEDGER_ITEMS: RangeInclusive<usize> = 1..=6;
pub(super) const CLUSTER_COUNT: RangeInclusive<usize> = 1..=8;
pub(super) const STEP_COUNT: RangeInclusive<usize> = 1..=400;

// -----------------------------------------------------------------------------
// Shared rules
// -----------------------------------------------------------------------------

/// Checks one stored text, which must already be canonical.
pub(super) fn stored_text(issues: &mut Vec<FlowIssue>, value: &str, path: &str, rule: TextRule) {
    let TextRule { min, max, block } = rule;
    let result = if block {
        text::check_block(value, min, max)
    } else {
        text::check_line(value, min, max)
    };
    if let Err(fault) = result {
        issues.push(FlowIssue::error(IssueCode::InvalidText, fault.to_string()).at(path));
    }
}

/// The kinds that a mark can name. A mark is a backtick span `kind:name`.
pub const MARK_KINDS: [&str; 5] = ["fn", "type", "value", "file", "field"];

/// Checks the marks of one text: each backtick span is `kind:name` with a known kind.
///
/// With `functions`, a `fn` mark must name a function of the graph. A stored
/// artifact has no graph, so its check passes `None`.
fn mark_rules(
    issues: &mut Vec<FlowIssue>,
    value: &str,
    path: &str,
    functions: Option<&BTreeSet<&str>>,
) {
    let parts: Vec<&str> = value.split('`').collect();
    if parts.len().is_multiple_of(2) {
        issues.push(
            FlowIssue::error(IssueCode::InvalidMark, "a mark has no closing backtick").at(path),
        );
        return;
    }
    for mark in parts.iter().skip(1).step_by(2) {
        let fault = match mark.split_once(':') {
            None => Some(format!(
                "the mark `{mark}` needs a kind, for example `fn:{mark}`"
            )),
            Some((kind, _)) if !MARK_KINDS.contains(&kind) => Some(format!(
                "the mark `{mark}` has an unknown kind; use one of {}",
                MARK_KINDS.join(", ")
            )),
            Some((_, name)) if name.is_empty() || name.trim() != name => Some(format!(
                "the mark `{mark}` needs a name without outer spaces"
            )),
            Some(("fn", name)) if functions.is_some_and(|known| !known.contains(name)) => {
                Some(format!("the mark `{mark}` names no function of the graph"))
            }
            _ => None,
        };
        if let Some(message) = fault {
            issues.push(FlowIssue::error(IssueCode::InvalidMark, message).at(path));
        }
    }
}

/// Returns the names that a `fn` mark may use for one graph key: the callable name and its last segment.
///
/// A `[n]` suffix, which separates two declarations, is not part of the name.
fn function_names(key: &str) -> impl Iterator<Item = &str> {
    let name = key
        .rsplit_once("::callable:")
        .map(|(_, name)| name.split_once('[').map_or(name, |(name, _)| name));
    let last = name.and_then(|name| name.rsplit(['.', ':', '#']).next());
    name.into_iter()
        .chain(last.filter(|last| Some(*last) != name))
}

/// Collects the function names of the graph for the `fn` mark check.
pub(super) fn graph_functions<'a>(keys: &BTreeSet<&'a str>) -> BTreeSet<&'a str> {
    keys.iter().flat_map(|key| function_names(key)).collect()
}

/// Checks the slot counts of one step ledger and the marks of its items.
///
/// The caller checks the item texts. At least one slot must be used, and a used
/// slot holds 1 to 6 items.
fn ledger_rules(
    issues: &mut Vec<FlowIssue>,
    ledger: &StepLedger,
    path: &str,
    functions: Option<&BTreeSet<&str>>,
) {
    let slots = ledger.slots();
    if slots.iter().all(|(_, items)| items.is_none()) {
        issues.push(
            FlowIssue::error(
                IssueCode::InvalidLedger,
                "a step ledger needs at least one slot",
            )
            .at(path),
        );
    }
    for (name, items) in slots {
        let Some(items) = items else { continue };
        if !LEDGER_ITEMS.contains(&items.len()) {
            issues.push(
                FlowIssue::error(
                    IssueCode::InvalidLedger,
                    format!(
                        "a used slot needs {} to {} items; use null for an unused slot",
                        LEDGER_ITEMS.start(),
                        LEDGER_ITEMS.end()
                    ),
                )
                .at(format!("{path}/{name}")),
            );
        }
        for (index, item) in items.iter().enumerate() {
            mark_rules(issues, item, &format!("{path}/{name}/{index}"), functions);
        }
    }
}

/// Checks the type and rule counts of one cluster, the type names, and the rule marks.
fn cluster_ledger_rules(
    issues: &mut Vec<FlowIssue>,
    cluster: &ClusterIntent,
    path: &str,
    functions: Option<&BTreeSet<&str>>,
) {
    let count =
        |issues: &mut Vec<FlowIssue>, len: usize, range: &RangeInclusive<usize>, field: &str| {
            if !range.contains(&len) {
                issues.push(
                    FlowIssue::error(
                        IssueCode::InvalidLedger,
                        format!("{field} needs {} to {} items", range.start(), range.end()),
                    )
                    .at(format!("{path}/{field}"))
                    .cluster(cluster.key.as_str()),
                );
            }
        };
    count(issues, cluster.input.len(), &CLUSTER_TYPES, "input");
    count(issues, cluster.output.len(), &CLUSTER_TYPES, "output");
    count(issues, cluster.rules.len(), &CLUSTER_RULES, "rules");
    for (field, types) in [("input", &cluster.input), ("output", &cluster.output)] {
        for (index, name) in types.iter().enumerate() {
            if name.contains('`') {
                issues.push(
                    FlowIssue::error(IssueCode::InvalidMark, "a type name takes no marks")
                        .at(format!("{path}/{field}/{index}"))
                        .cluster(cluster.key.as_str()),
                );
            }
        }
    }
    for (index, rule) in cluster.rules.iter().enumerate() {
        mark_rules(issues, rule, &format!("{path}/rules/{index}"), functions);
    }
}

/// Checks unique keys, unique labels ignoring case, the phase note rule, and the cluster ledger.
fn cluster_rules(
    clusters: &[ClusterIntent],
    path: &str,
    functions: Option<&BTreeSet<&str>>,
) -> Vec<FlowIssue> {
    let mut issues = Vec::new();
    let mut keys = BTreeSet::new();
    let mut labels = BTreeSet::new();
    for (index, cluster) in clusters.iter().enumerate() {
        let at = format!("{path}/{index}");
        cluster_ledger_rules(&mut issues, cluster, &at, functions);
        if !keys.insert(&cluster.key) {
            issues.push(
                FlowIssue::error(
                    IssueCode::DuplicateClusterKey,
                    format!("the cluster key {} is used twice", cluster.key),
                )
                .at(&at)
                .cluster(cluster.key.as_str()),
            );
        }
        if !labels.insert(cluster.label.to_lowercase()) {
            issues.push(
                FlowIssue::error(
                    IssueCode::DuplicateClusterLabel,
                    format!("the cluster label {:?} is used twice", cluster.label),
                )
                .at(&at)
                .cluster(cluster.key.as_str()),
            );
        }
        match (
            cluster.phase == FlowPhase::Other,
            cluster.phase_note.is_some(),
        ) {
            (true, false) => issues.push(
                FlowIssue::error(
                    IssueCode::PhaseNoteRequired,
                    "the phase other needs a phase note",
                )
                .at(format!("{at}/phaseNote"))
                .cluster(cluster.key.as_str()),
            ),
            (false, true) => issues.push(
                FlowIssue::error(
                    IssueCode::PhaseNoteForbidden,
                    "only the phase other takes a phase note",
                )
                .at(format!("{at}/phaseNote"))
                .cluster(cluster.key.as_str()),
            ),
            _ => {}
        }
    }
    issues
}

/// Reports each cluster that no step uses.
///
/// `keys` are the cluster keys in list order, so a path keeps the list index.
/// `used` holds the cluster key of every step.
fn unused_clusters<'a>(
    keys: impl Iterator<Item = &'a str>,
    used: &BTreeSet<&str>,
    path: &str,
) -> Vec<FlowIssue> {
    keys.enumerate()
        .filter(|(_, key)| !used.contains(key))
        .map(|(index, key)| {
            FlowIssue::error(
                IssueCode::UnusedCluster,
                format!("no step uses the cluster {key}"),
            )
            .at(format!("{path}/{index}"))
            .cluster(key)
        })
        .collect()
}

/// Checks the author record that the CLI writes into provenance.
///
/// # Errors
/// Returns a message for a name or model ID that breaks its pattern.
pub fn check_author(author: &Author) -> Result<(), String> {
    match author {
        Author::Agent { name, model_id } => {
            let bytes = name.as_bytes();
            let name_ok = (2..=64).contains(&bytes.len())
                && bytes.first().is_some_and(u8::is_ascii_lowercase)
                && bytes.iter().all(|byte| {
                    byte.is_ascii_lowercase() || byte.is_ascii_digit() || *byte == b'-'
                });
            let model_ok = model_id.as_deref().is_none_or(|model| {
                let bytes = model.as_bytes();
                (1..=200).contains(&bytes.len())
                    && bytes.first().is_some_and(u8::is_ascii_alphanumeric)
                    && bytes
                        .iter()
                        .all(|byte| byte.is_ascii_alphanumeric() || b"._:/@+-".contains(byte))
            });
            if !name_ok {
                return Err(format!(
                    "invalid agent name {name:?}: use lowercase letters, digits, and hyphens"
                ));
            }
            if !model_ok {
                return Err("invalid model ID".to_owned());
            }
            Ok(())
        }
        Author::Human { name } => {
            if (1..=120).contains(&name.chars().count()) {
                Ok(())
            } else {
                Err("a human author name needs 1 to 120 characters".to_owned())
            }
        }
        Author::System => Ok(()),
    }
}

// -----------------------------------------------------------------------------
// Tests
// -----------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::{graph_functions, mark_rules};
    use crate::flow::models::IssueCode;

    fn mark_issues(text: &str) -> Vec<IssueCode> {
        let keys = BTreeSet::from([
            "src/a.rs::callable:run",
            "src/cli/flow.rs::type:Command::callable:parse[2]",
            "src/a.rs::type:Plan",
        ]);
        let functions = graph_functions(&keys);
        let mut issues = Vec::new();
        mark_rules(&mut issues, text, "/x", Some(&functions));
        issues.into_iter().map(|issue| issue.code).collect()
    }

    #[test]
    fn function_marks_match_callable_names_without_the_declaration_suffix() {
        assert_eq!(
            mark_issues("`fn:run` calls `fn:parse` and returns `type:Plan`."),
            []
        );
        assert_eq!(mark_issues("`fn:Plan`"), [IssueCode::InvalidMark]);
    }

    #[test]
    fn marks_need_a_known_kind_a_name_and_a_closing_backtick() {
        for text in ["`run`", "`size:run`", "`fn:`", "`fn: run`", "calls `fn:run"] {
            assert_eq!(mark_issues(text), [IssueCode::InvalidMark], "{text}");
        }
        assert_eq!(mark_issues("no marks here"), []);
    }
}
