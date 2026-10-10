//! Flow validation.
//!
//! This module owns the runtime rules that JSON Schema cannot express. The
//! `agent` checks run on one agent output before a freeze. The `artifact` checks
//! run on one stored generation and recompute its derived layers and digests.
//! Both collect every issue; code never repairs a rejected value.
//!
//! # Sections
//! `Limits` holds the one table of text and count limits. `Shared rules` holds
//! the text, cluster, and author rules of both checks.

mod agent;
mod artifact;

use std::{collections::BTreeSet, ops::RangeInclusive};

pub use agent::{CheckedFlow, CheckedStep, PriorContext, check_agent_flow};
pub use artifact::verify_artifact;

use super::{
    models::{Author, ClusterIntent, FlowIssue, FlowPhase, IssueCode},
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
pub(super) const CLUSTER_DESCRIPTION: TextRule = TextRule::block(20, 400);
pub(super) const PHASE_NOTE: TextRule = TextRule::line(3, 200);
pub(super) const STEP_TITLE: TextRule = TextRule::line(3, 96);
pub(super) const STEP_ANNOTATION: TextRule = TextRule::block(1, 1500);
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

/// Checks unique keys, unique labels ignoring case, and the phase note rule.
fn cluster_rules(clusters: &[ClusterIntent], path: &str) -> Vec<FlowIssue> {
    let mut issues = Vec::new();
    let mut keys = BTreeSet::new();
    let mut labels = BTreeSet::new();
    for (index, cluster) in clusters.iter().enumerate() {
        let at = format!("{path}/{index}");
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
