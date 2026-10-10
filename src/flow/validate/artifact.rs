//! Stored artifact checks.
//!
//! This module verifies one stored generation: its generation rules, its stored
//! text, its fact projection, and every derived layer and digest, which it
//! recomputes. The artifact and its parent are the input. Every issue is the
//! output.

use std::collections::BTreeSet;

use super::{
    BEHAVIOR, BOUNDARIES, CLUSTER_COUNT, CLUSTER_DESCRIPTION, CLUSTER_LABEL, PHASE_NOTE,
    REVISION_SUMMARY, STEP_ANNOTATION, STEP_COUNT, STEP_TITLE, check_author, cluster_rules,
    stored_text,
};
use crate::flow::{
    assign, derive,
    models::{
        FLOW_ARTIFACT_SCHEMA_VERSION, FlowArtifact, FlowIssue, GenerationPurpose, IntentLayer,
        IssueCode, NodeKey, wire_name,
    },
};

/// Verifies one stored generation by recomputing its derived layers and digests.
///
/// `parent` must be the generation that `generation.parentDigest` names. It is
/// `None` for an initial generation, and for a revision whose parent the caller
/// could not read. The caller reports that fault. The checks that need the
/// parent view or the parent intent do not run then.
#[must_use]
pub fn verify_artifact(artifact: &FlowArtifact, parent: Option<&FlowArtifact>) -> Vec<FlowIssue> {
    let mut issues = Vec::new();
    issues.extend(generation_rules(artifact, parent));
    issues.extend(intent_rules(&artifact.intent));
    issues.extend(fact_rules(artifact));
    match assign::assign(&artifact.intent) {
        Ok(assignment) if assignment == artifact.assignment => {}
        Ok(_) => issues.push(
            FlowIssue::error(
                IssueCode::DerivedMismatch,
                "the assignment layer does not match the intent",
            )
            .at("/assignment"),
        ),
        Err(error) => issues.push(
            FlowIssue::error(IssueCode::DerivedMismatch, error.to_string()).at("/assignment"),
        ),
    }
    let view = derive::view(
        &artifact.facts,
        &artifact.intent,
        &artifact.assignment,
        parent.map(|parent| &parent.view),
    );
    let parent_missing =
        artifact.generation.purpose == GenerationPurpose::Revision && parent.is_none();
    if !parent_missing && view != artifact.view {
        issues.push(
            FlowIssue::error(
                IssueCode::DerivedMismatch,
                "the view layer does not match the other layers",
            )
            .at("/view"),
        );
    }
    if let (Some(parent), Some(diff)) = (parent, &artifact.diff)
        && derive::diff(parent, &artifact.intent, &artifact.assignment) != *diff
    {
        issues.push(
            FlowIssue::error(
                IssueCode::DerivedMismatch,
                "the diff overlay does not match the parent",
            )
            .at("/diff"),
        );
    }
    match derive::digests(artifact) {
        Ok(digests) if digests == artifact.digests => {}
        Ok(_) => issues.push(
            FlowIssue::error(
                IssueCode::DigestMismatch,
                "the stored digests do not match the content",
            )
            .at("/digests"),
        ),
        Err(error) => issues
            .push(FlowIssue::error(IssueCode::DigestMismatch, error.to_string()).at("/digests")),
    }
    if let Err(message) = check_author(&artifact.provenance.author) {
        issues.push(FlowIssue::error(IssueCode::InvalidArtifact, message).at("/provenance/author"));
    }
    if !is_timestamp(&artifact.provenance.frozen_at) || !is_semver(&artifact.provenance.cli_version)
    {
        issues.push(
            FlowIssue::error(
                IssueCode::InvalidArtifact,
                "invalid provenance time or version",
            )
            .at("/provenance"),
        );
    }
    issues
}

/// Checks the ordinal, the parent link, and the diff presence of each purpose.
fn generation_rules(artifact: &FlowArtifact, parent: Option<&FlowArtifact>) -> Vec<FlowIssue> {
    let mut issues = Vec::new();
    let invalid =
        |message: &str| FlowIssue::error(IssueCode::InvalidArtifact, message).at("/generation");
    let generation = &artifact.generation;
    if artifact.schema_version != FLOW_ARTIFACT_SCHEMA_VERSION {
        issues.push(
            FlowIssue::error(IssueCode::InvalidArtifact, "the schema version must be 3")
                .at("/schemaVersion"),
        );
    }
    match generation.purpose {
        // A first generation starts a lineage.
        GenerationPurpose::Initial => {
            if generation.ordinal != 1 || generation.parent_digest.is_some() {
                issues.push(invalid("an initial generation has ordinal 1 and no parent"));
            }
            if artifact.diff.is_some() {
                issues.push(
                    FlowIssue::error(
                        IssueCode::InvalidArtifact,
                        "an initial generation has no diff",
                    )
                    .at("/diff"),
                );
            }
        }
        // A revision names its parent and carries a diff against it.
        GenerationPurpose::Revision => {
            if generation.ordinal < 2 || generation.parent_digest.is_none() {
                issues.push(invalid(
                    "a revision has an ordinal of 2 or more and a parent",
                ));
            }
            if artifact.diff.is_none() {
                issues.push(
                    FlowIssue::error(IssueCode::InvalidArtifact, "a revision has a diff")
                        .at("/diff"),
                );
            }
            match parent {
                Some(parent)
                    if Some(&parent.digests.generation) == generation.parent_digest.as_ref()
                        && parent.flow_id == artifact.flow_id
                        && parent.generation.ordinal + 1 == generation.ordinal => {}
                Some(_) => issues.push(invalid("the parent generation does not match")),
                None => {}
            }
        }
    }
    issues
}

/// Checks the stored text limits, the cluster rules, step IDs, and cluster use.
fn intent_rules(intent: &IntentLayer) -> Vec<FlowIssue> {
    let mut issues = Vec::new();
    stored_text(&mut issues, &intent.behavior, "/intent/behavior", BEHAVIOR);
    if let Some(value) = &intent.boundaries {
        stored_text(&mut issues, value, "/intent/boundaries", BOUNDARIES);
    }
    if let Some(value) = &intent.revision_summary {
        stored_text(
            &mut issues,
            value,
            "/intent/revisionSummary",
            REVISION_SUMMARY,
        );
    }
    for (index, cluster) in intent.clusters.iter().enumerate() {
        stored_text(
            &mut issues,
            &cluster.label,
            &format!("/intent/clusters/{index}/label"),
            CLUSTER_LABEL,
        );
        stored_text(
            &mut issues,
            &cluster.description,
            &format!("/intent/clusters/{index}/description"),
            CLUSTER_DESCRIPTION,
        );
        if let Some(note) = &cluster.phase_note {
            stored_text(
                &mut issues,
                note,
                &format!("/intent/clusters/{index}/phaseNote"),
                PHASE_NOTE,
            );
        }
    }
    for (index, step) in intent.steps.iter().enumerate() {
        stored_text(
            &mut issues,
            &step.title,
            &format!("/intent/steps/{index}/title"),
            STEP_TITLE,
        );
        stored_text(
            &mut issues,
            &step.annotation,
            &format!("/intent/steps/{index}/annotation"),
            STEP_ANNOTATION,
        );
    }
    let used: BTreeSet<&str> = intent
        .steps
        .iter()
        .map(|step| step.cluster_key.as_str())
        .collect();
    issues.extend(cluster_rules(&intent.clusters, &used, "/intent/clusters"));
    if !CLUSTER_COUNT.contains(&intent.clusters.len()) || !STEP_COUNT.contains(&intent.steps.len())
    {
        issues.push(
            FlowIssue::error(
                IssueCode::InvalidArtifact,
                format!(
                    "a flow needs {} to {} clusters and {} to {} steps",
                    CLUSTER_COUNT.start(),
                    CLUSTER_COUNT.end(),
                    STEP_COUNT.start(),
                    STEP_COUNT.end()
                ),
            )
            .at("/intent"),
        );
    }
    let mut ids = BTreeSet::new();
    for step in &intent.steps {
        if !ids.insert(&step.id) {
            issues.push(
                FlowIssue::error(
                    IssueCode::DuplicateStepId,
                    format!("the step ID {} is used twice", step.id),
                )
                .step(step.id.as_str()),
            );
        }
    }
    issues
}

/// Checks that the fact nodes are the sorted step nodes and that edges join them.
fn fact_rules(artifact: &FlowArtifact) -> Vec<FlowIssue> {
    let mut issues = Vec::new();
    let facts = &artifact.facts;
    let invalid =
        |message: String| FlowIssue::error(IssueCode::InvalidArtifact, message).at("/facts");
    let keys: Vec<&NodeKey> = facts.nodes.iter().map(|node| &node.key).collect();
    let sorted = keys.windows(2).all(|pair| pair[0] < pair[1]);
    let expected: BTreeSet<&NodeKey> = artifact
        .intent
        .steps
        .iter()
        .map(|step| &step.node_key)
        .collect();
    if !sorted || keys.iter().copied().collect::<BTreeSet<_>>() != expected {
        issues.push(invalid(
            "the fact nodes must be the sorted, distinct step nodes".to_owned(),
        ));
    }
    for node in &facts.nodes {
        if derive::fact_digest(node).ok().as_ref() != Some(&node.fact_digest) {
            issues.push(
                FlowIssue::error(
                    IssueCode::DigestMismatch,
                    format!("the fact digest of {} does not match", node.key),
                )
                .node(node.key.as_str()),
            );
        }
    }
    let present: BTreeSet<&NodeKey> = keys.iter().copied().collect();
    let ordered = facts.edges.windows(2).all(|pair| {
        (
            pair[0].source.as_str(),
            pair[0].target.as_str(),
            wire_name(&pair[0].kind),
        ) < (
            pair[1].source.as_str(),
            pair[1].target.as_str(),
            wire_name(&pair[1].kind),
        )
    });
    if !ordered
        || facts
            .edges
            .iter()
            .any(|edge| !present.contains(&edge.source) || !present.contains(&edge.target))
    {
        issues.push(invalid(
            "the fact edges must be sorted, distinct, and join fact nodes".to_owned(),
        ));
    }
    issues
}

fn is_timestamp(text: &str) -> bool {
    text.ends_with('Z') && text.parse::<jiff::Timestamp>().is_ok()
}

fn is_semver(text: &str) -> bool {
    let parts: Vec<_> = text.split('.').collect();
    parts.len() == 3
        && parts.iter().all(|part| {
            !part.is_empty()
                && part.bytes().all(|byte| byte.is_ascii_digit())
                && (part.len() == 1 || !part.starts_with('0'))
        })
}
