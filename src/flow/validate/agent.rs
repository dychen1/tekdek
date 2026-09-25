//! Agent output checks.
//!
//! This module canonicalizes and checks one agent flow before a freeze: text
//! limits, keys, cluster vocabulary, node and prior step references, and the
//! unanimous label rule. The agent flow, the prior step IDs, and the graph keys
//! are the input. A checked flow and every issue are the output.

use std::collections::{BTreeMap, BTreeSet};

use super::{cluster_rules, stored_text};
use crate::flow::{
    models::{
        AgentFlow, AgentStep, ClusterIntent, ClusterKey, EvidenceBasis, FlowIssue,
        GenerationPurpose, IssueCode, NodeKey, StepId, is_cluster_key, is_node_key, is_step_id,
    },
    text,
};

/// The prior version that an agent output may continue.
pub struct PriorContext<'a> {
    pub purpose: GenerationPurpose,
    /// Step IDs of the parent generation. Empty for an initial generation.
    pub prior_step_ids: BTreeSet<&'a str>,
}

/// One agent step after canonicalization and reference checks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckedStep {
    pub prior_step_id: Option<StepId>,
    pub node_key: NodeKey,
    pub cluster_key: ClusterKey,
    pub title: String,
    pub annotation: String,
}

/// One agent flow after canonicalization and reference checks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckedFlow {
    pub behavior: String,
    pub boundaries: Option<String>,
    pub evidence_basis: EvidenceBasis,
    pub revision_summary: Option<String>,
    pub clusters: Vec<ClusterIntent>,
    pub steps: Vec<CheckedStep>,
}

/// Canonicalizes and checks one agent flow against the graph and the prior version.
///
/// Returns the checked flow only when no error issue exists. Warnings (for
/// example `text_trimmed`) come back in both cases.
#[must_use]
pub fn check_agent_flow(
    flow: &AgentFlow,
    context: &PriorContext<'_>,
    graph_keys: &BTreeSet<&str>,
) -> (Option<CheckedFlow>, Vec<FlowIssue>) {
    let mut issues = Vec::new();
    let behavior = agent_text(
        &mut issues,
        &flow.behavior,
        "/flow/behavior",
        (20, 600),
        true,
    );
    let boundaries = flow
        .boundaries
        .as_deref()
        .map(|value| agent_text(&mut issues, value, "/flow/boundaries", (10, 600), true));
    let revision_summary = flow.revision_summary.as_deref().map(|value| {
        agent_text(
            &mut issues,
            value,
            "/flow/revisionSummary",
            (20, 1200),
            true,
        )
    });
    let clusters = check_clusters(flow, &mut issues);
    if !(1..=400).contains(&flow.steps.len()) {
        issues.push(
            FlowIssue::error(IssueCode::StepLimit, "a flow needs 1 to 400 steps").at("/flow/steps"),
        );
    }
    let known: BTreeSet<&str> = flow
        .clusters
        .iter()
        .map(|cluster| cluster.key.as_str())
        .collect();
    let mut prior_used = BTreeSet::new();
    let steps: Vec<_> = flow
        .steps
        .iter()
        .enumerate()
        .map(|(index, step)| {
            let references = StepReferences {
                context,
                graph_keys,
                known: &known,
            };
            check_step(step, index, &references, &mut prior_used, &mut issues)
        })
        .collect();
    issues.extend(label_conflicts(
        flow.steps
            .iter()
            .map(|step| (step.node_key.as_str(), step.cluster_key.as_str())),
    ));
    issues.extend(revision_summary_rule(
        context.purpose,
        flow.revision_summary.is_some(),
    ));
    let checked = if issues.iter().any(FlowIssue::is_error) {
        None
    } else {
        Some(CheckedFlow {
            behavior,
            boundaries,
            evidence_basis: flow.evidence_basis,
            revision_summary,
            clusters,
            steps: steps.into_iter().flatten().collect(),
        })
    };
    (checked, issues)
}

/// Checks the cluster vocabulary: text, keys, uniqueness, limits, and use.
fn check_clusters(flow: &AgentFlow, issues: &mut Vec<FlowIssue>) -> Vec<ClusterIntent> {
    let mut clusters = Vec::new();
    for (index, cluster) in flow.clusters.iter().enumerate() {
        let path = format!("/flow/clusters/{index}");
        let label = agent_text(
            issues,
            &cluster.label,
            &format!("{path}/label"),
            (3, 60),
            false,
        );
        let description = agent_text(
            issues,
            &cluster.description,
            &format!("{path}/description"),
            (20, 400),
            true,
        );
        let phase_note = cluster
            .phase_note
            .as_deref()
            .map(|value| agent_text(issues, value, &format!("{path}/phaseNote"), (3, 200), false));
        match ClusterKey::try_from(cluster.key.as_str()) {
            Ok(key) => clusters.push(ClusterIntent {
                key,
                label,
                description,
                phase: cluster.phase,
                phase_note,
            }),
            Err(error) => issues.push(
                FlowIssue::error(IssueCode::InvalidKey, error.to_string())
                    .at(format!("{path}/key"))
                    .cluster(&cluster.key),
            ),
        }
    }
    issues.extend(cluster_rules(&clusters, "/flow/clusters"));
    if !(1..=8).contains(&flow.clusters.len()) {
        issues.push(
            FlowIssue::error(IssueCode::ClusterLimit, "a flow needs 1 to 8 clusters")
                .at("/flow/clusters"),
        );
    }
    let used: BTreeSet<&str> = flow
        .steps
        .iter()
        .map(|step| step.cluster_key.as_str())
        .collect();
    for (index, cluster) in flow.clusters.iter().enumerate() {
        if !used.contains(cluster.key.as_str()) {
            issues.push(
                FlowIssue::error(
                    IssueCode::UnusedCluster,
                    format!("no step uses the cluster {}", cluster.key),
                )
                .at(format!("/flow/clusters/{index}"))
                .cluster(&cluster.key),
            );
        }
    }
    clusters
}

/// The references that one agent step may name.
struct StepReferences<'a> {
    context: &'a PriorContext<'a>,
    graph_keys: &'a BTreeSet<&'a str>,
    known: &'a BTreeSet<&'a str>,
}

/// Checks the text and the references of one agent step.
fn check_step<'a>(
    step: &'a AgentStep,
    index: usize,
    references: &StepReferences<'_>,
    prior_used: &mut BTreeSet<&'a str>,
    issues: &mut Vec<FlowIssue>,
) -> Option<CheckedStep> {
    let path = format!("/flow/steps/{index}");
    let title = agent_text(
        issues,
        &step.title,
        &format!("{path}/title"),
        (3, 96),
        false,
    );
    let annotation = agent_text(
        issues,
        &step.annotation,
        &format!("{path}/annotation"),
        (1, 1500),
        true,
    );
    let before = issues.iter().filter(|issue| issue.is_error()).count();
    let prior_step_id = step.prior_step_id.as_deref().and_then(|id| {
        let at = format!("{path}/priorStepId");
        if !is_step_id(id) {
            issues.push(
                FlowIssue::error(IssueCode::InvalidKey, format!("invalid step ID {id:?}")).at(at),
            );
        } else if !references.context.prior_step_ids.contains(id) {
            issues.push(
                FlowIssue::error(
                    IssueCode::UnknownPriorStep,
                    format!("the prior version has no step {id}"),
                )
                .at(at)
                .step(id),
            );
        } else if !prior_used.insert(id) {
            issues.push(
                FlowIssue::error(
                    IssueCode::PriorStepReused,
                    format!("the prior step {id} is used twice"),
                )
                .at(at)
                .step(id),
            );
        }
        StepId::try_from(id).ok()
    });
    let node_key = if !is_node_key(&step.node_key) {
        issues.push(
            FlowIssue::error(IssueCode::InvalidKey, "invalid node key")
                .at(format!("{path}/nodeKey"))
                .node(&step.node_key),
        );
        None
    } else if references.graph_keys.contains(step.node_key.as_str()) {
        NodeKey::try_from(step.node_key.as_str()).ok()
    } else {
        issues.push(
            FlowIssue::error(
                IssueCode::UnknownNode,
                format!("the graph does not contain the node {}", step.node_key),
            )
            .at(format!("{path}/nodeKey"))
            .node(&step.node_key),
        );
        None
    };
    let cluster_key = if !is_cluster_key(&step.cluster_key) {
        issues.push(
            FlowIssue::error(IssueCode::InvalidKey, "invalid cluster key")
                .at(format!("{path}/clusterKey"))
                .cluster(&step.cluster_key),
        );
        None
    } else if references.known.contains(step.cluster_key.as_str()) {
        ClusterKey::try_from(step.cluster_key.as_str()).ok()
    } else {
        issues.push(
            FlowIssue::error(
                IssueCode::UnknownCluster,
                format!("the output defines no cluster {}", step.cluster_key),
            )
            .at(format!("{path}/clusterKey"))
            .cluster(&step.cluster_key),
        );
        None
    };
    let clean = issues.iter().filter(|issue| issue.is_error()).count() == before;
    match (node_key, cluster_key) {
        (Some(node_key), Some(cluster_key)) if clean => Some(CheckedStep {
            prior_step_id,
            node_key,
            cluster_key,
            title,
            annotation,
        }),
        _ => None,
    }
}

/// Canonicalizes one agent text, reports a trim, and checks the result.
fn agent_text(
    issues: &mut Vec<FlowIssue>,
    value: &str,
    path: &str,
    limits: (usize, usize),
    block: bool,
) -> String {
    let canonical = text::canonical(value);
    if canonical != value {
        issues.push(
            FlowIssue::warning(
                IssueCode::TextTrimmed,
                "the text was normalized and trimmed",
            )
            .at(path),
        );
    }
    stored_text(issues, &canonical, path, limits, block);
    canonical
}

/// Reports each node whose steps carry more than one cluster.
fn label_conflicts<'a>(steps: impl Iterator<Item = (&'a str, &'a str)>) -> Vec<FlowIssue> {
    let mut labels: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    for (node, cluster) in steps {
        labels.entry(node).or_default().insert(cluster);
    }
    labels
        .into_iter()
        .filter(|(_, clusters)| clusters.len() > 1)
        .map(|(node, clusters)| {
            FlowIssue::error(
                IssueCode::NodeLabelConflict,
                format!(
                    "the steps of {node} carry different clusters: {}",
                    clusters.into_iter().collect::<Vec<_>>().join(", ")
                ),
            )
            .node(node)
        })
        .collect()
}

/// A revision summary is required for a revision and forbidden for an initial generation.
fn revision_summary_rule(purpose: GenerationPurpose, present: bool) -> Option<FlowIssue> {
    match (purpose, present) {
        (GenerationPurpose::Revision, false) => Some(
            FlowIssue::error(
                IssueCode::RevisionSummaryRequired,
                "a flow with a prior version needs a revision summary",
            )
            .at("/flow/revisionSummary"),
        ),
        (GenerationPurpose::Initial, true) => Some(
            FlowIssue::error(
                IssueCode::RevisionSummaryForbidden,
                "a new flow has no prior version to summarize",
            )
            .at("/flow/revisionSummary"),
        ),
        _ => None,
    }
}
