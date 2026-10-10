//! Flow cluster assignment.
//!
//! This module owns the assigner: the `step-labels` rules provider, which copies
//! each agent label, and the `unanimous` membership rule, which turns step labels
//! into function membership. An intent layer is the input. An assignment layer is
//! the output. The same intent always gives the same assignment bytes.

use std::{collections::BTreeMap, fmt};

use serde::Serialize;

use super::{
    canonical::{self, CanonicalError},
    models::{
        AssignedBasis, AssignerProvenance, AssignmentLayer, AssignmentPolicy, ClusterKey,
        ClusterVisitCount, InputScope, IntentLayer, LabelPolicy, MemberDecision, MembershipBasis,
        MembershipRule, NodeKey, Sha256Digest, StepDecision, StepId,
    },
};

/// The only policy: agent labels, unanimous membership.
const POLICY: AssignmentPolicy = AssignmentPolicy {
    label_policy: LabelPolicy::AgentLabels,
    membership_rule: MembershipRule::Unanimous,
};

/// Returns the provenance of the `step-labels` 1.0.0 rules assigner.
#[must_use]
fn step_labels_assigner() -> AssignerProvenance {
    AssignerProvenance::Rules {
        assigner_id: "step-labels".to_owned(),
        version: "1.0.0".to_owned(),
        input_scope: InputScope::LabelsOnly,
    }
}

/// Resolves step decisions and function membership for one intent layer.
///
/// # Errors
/// Returns an error for a step with an unknown cluster, or for a node whose steps
/// carry different clusters.
pub fn assign(intent: &IntentLayer) -> Result<AssignmentLayer, AssignError> {
    let assigner = step_labels_assigner();
    let mut steps = Vec::with_capacity(intent.steps.len());
    // Node key -> its cluster and its step count.
    let mut members: BTreeMap<&NodeKey, (&ClusterKey, u32)> = BTreeMap::new();
    for step in &intent.steps {
        let cluster = &step.cluster_key;
        if !intent.clusters.iter().any(|known| &known.key == cluster) {
            return Err(AssignError::UnknownCluster {
                step: step.id.clone(),
                cluster: cluster.clone(),
            });
        }
        steps.push(StepDecision::Assigned {
            step_id: step.id.clone(),
            cluster_key: cluster.clone(),
            basis: AssignedBasis::AgentLabel,
        });
        let member = members.entry(&step.node_key).or_insert((cluster, 0));
        if member.0 != cluster {
            let mut clusters = vec![member.0.clone(), cluster.clone()];
            clusters.sort();
            return Err(AssignError::NodeLabelConflict {
                node: step.node_key.clone(),
                clusters,
            });
        }
        member.1 += 1;
    }
    let members = members
        .into_iter()
        .map(|(node, (cluster, count))| MemberDecision::Assigned {
            node_key: node.clone(),
            cluster_key: cluster.clone(),
            basis: MembershipBasis::Unanimous,
            visits: vec![ClusterVisitCount {
                cluster_key: cluster.clone(),
                steps: count,
            }],
        })
        .collect();
    Ok(AssignmentLayer {
        policy: POLICY,
        input_digest: input_digest(intent, &assigner)?,
        assigner,
        steps,
        members,
    })
}

/// Returns the digest of exactly the inputs that the `labels_only` scope reads.
///
/// # Errors
/// Returns an error when the inputs cannot have a canonical form.
fn input_digest(
    intent: &IntentLayer,
    assigner: &AssignerProvenance,
) -> Result<Sha256Digest, CanonicalError> {
    #[derive(Serialize)]
    struct StepInput<'a> {
        #[serde(rename = "stepId")]
        step_id: &'a StepId,
        #[serde(rename = "nodeKey")]
        node_key: &'a NodeKey,
        #[serde(rename = "clusterKey")]
        cluster_key: &'a ClusterKey,
    }
    #[derive(Serialize)]
    struct Input<'a> {
        template: &'static str,
        policy: AssignmentPolicy,
        assigner: Sha256Digest,
        clusters: Vec<&'a ClusterKey>,
        steps: Vec<StepInput<'a>>,
    }
    canonical::digest(&Input {
        template: "assignment-input/2",
        policy: POLICY,
        assigner: canonical::digest(assigner)?,
        clusters: intent.clusters.iter().map(|cluster| &cluster.key).collect(),
        steps: intent
            .steps
            .iter()
            .map(|step| StepInput {
                step_id: &step.id,
                node_key: &step.node_key,
                cluster_key: &step.cluster_key,
            })
            .collect(),
    })
}

// -----------------------------------------------------------------------------
// Errors
// -----------------------------------------------------------------------------

#[derive(Debug)]
pub enum AssignError {
    UnknownCluster {
        step: StepId,
        cluster: ClusterKey,
    },
    NodeLabelConflict {
        node: NodeKey,
        clusters: Vec<ClusterKey>,
    },
    Canonical(CanonicalError),
}

impl fmt::Display for AssignError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            // Name the step and its unknown label.
            Self::UnknownCluster { step, cluster } => write!(
                formatter,
                "the step {step} names the unknown cluster {cluster}"
            ),
            // List the conflicting labels of one node.
            Self::NodeLabelConflict { node, clusters } => write!(
                formatter,
                "the steps of {node} carry different clusters: {}",
                clusters
                    .iter()
                    .map(ClusterKey::as_str)
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            // Preserve the serialization error.
            Self::Canonical(error) => write!(formatter, "{error}"),
        }
    }
}

impl std::error::Error for AssignError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Canonical(error) => Some(error),
            _ => None,
        }
    }
}

impl From<CanonicalError> for AssignError {
    fn from(error: CanonicalError) -> Self {
        Self::Canonical(error)
    }
}
