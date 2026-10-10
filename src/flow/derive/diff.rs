//! Diff derivation.
//!
//! This module compares one revision with its parent on the same graph. The
//! parent artifact and the new intent and assignment layers are the input. The
//! diff overlay is the output.

use std::collections::{BTreeMap, BTreeSet};

use crate::flow::models::{
    AssignmentLayer, ClusterDiff, ClusterField, ClusterIntent, ClusterKey, DiffBase, DiffOverlay,
    DiffSummary, FlowArtifact, FlowStep, InputCategory, IntentLayer, MemberDiff, MembershipChange,
    NodeKey, Presence, StepDiff, StepField, StepId,
};

/// Compares a revision with its parent.
///
/// Position changes use the longest common subsequence of retained IDs. A tie keeps
/// the earlier current items.
#[must_use]
pub fn diff(
    parent: &FlowArtifact,
    intent: &IntentLayer,
    assignment: &AssignmentLayer,
) -> DiffOverlay {
    let mut summary = DiffSummary::default();
    let steps = step_diffs(&parent.intent.steps, &intent.steps, &mut summary);
    let base_members = membership(&parent.assignment);
    let current_members = membership(assignment);
    let clusters = cluster_diffs(
        &parent.intent.clusters,
        &intent.clusters,
        &base_members,
        &current_members,
        &mut summary,
    );
    let base_labels = node_labels(&parent.intent.steps);
    let current_labels = node_labels(&intent.steps);
    let nodes: BTreeSet<&NodeKey> = base_members
        .keys()
        .chain(current_members.keys())
        .copied()
        .collect();
    let members = nodes
        .into_iter()
        .filter_map(|key| {
            let membership = match (base_members.get(key), current_members.get(key)) {
                (Some(from), Some(to)) if from != to => MembershipChange::Moved {
                    from_cluster: (*from).clone(),
                    to_cluster: (*to).clone(),
                },
                (Some(_), Some(cluster)) => MembershipChange::Unchanged {
                    cluster_key: (*cluster).clone(),
                },
                (None, Some(to)) => MembershipChange::Joined {
                    to_cluster: (*to).clone(),
                },
                (Some(from), None) => MembershipChange::Left {
                    from_cluster: (*from).clone(),
                },
                // The key union holds only keys of one side or both.
                (None, None) => return None,
            };
            if !matches!(membership, MembershipChange::Unchanged { .. }) {
                summary.members_changed += 1;
            }
            let causes = if base_labels.get(key) == current_labels.get(key) {
                vec![]
            } else {
                vec![InputCategory::Intent]
            };
            Some(MemberDiff {
                node_key: key.clone(),
                membership,
                causes,
            })
        })
        .collect();
    let changed_inputs = if parent.assignment.input_digest == assignment.input_digest {
        vec![]
    } else {
        vec![InputCategory::Intent]
    };
    DiffOverlay {
        base: DiffBase {
            generation_digest: parent.digests.generation.clone(),
            ordinal: parent.generation.ordinal,
        },
        changed_inputs,
        summary,
        steps,
        clusters,
        members,
    }
}

fn step_diffs(base: &[FlowStep], current: &[FlowStep], summary: &mut DiffSummary) -> Vec<StepDiff> {
    keyed_changes(base, current, |step| &step.id)
        .into_iter()
        .map(|change| {
            let mut changed_fields = Vec::new();
            if let (Some(before), step) = (change.before, change.item) {
                if before.node_key != step.node_key {
                    changed_fields.push(StepField::Node);
                }
                if before.cluster_key != step.cluster_key {
                    changed_fields.push(StepField::Cluster);
                }
                if before.title != step.title {
                    changed_fields.push(StepField::Title);
                }
                if before.annotation != step.annotation {
                    changed_fields.push(StepField::Annotation);
                }
                if !change.in_place {
                    changed_fields.push(StepField::Position);
                }
            }
            match change.presence {
                Presence::Added => summary.steps_added += 1,
                Presence::Removed => summary.steps_removed += 1,
                Presence::Retained => {
                    summary.steps_changed += u32::from(!changed_fields.is_empty());
                }
            }
            StepDiff {
                step_id: change.item.id.clone(),
                presence: change.presence,
                changed_fields,
            }
        })
        .collect()
}

fn cluster_diffs(
    base: &[ClusterIntent],
    current: &[ClusterIntent],
    base_members: &BTreeMap<&NodeKey, &ClusterKey>,
    current_members: &BTreeMap<&NodeKey, &ClusterKey>,
    summary: &mut DiffSummary,
) -> Vec<ClusterDiff> {
    let members_of =
        |members: &BTreeMap<&NodeKey, &ClusterKey>, key: &ClusterKey| -> BTreeSet<NodeKey> {
            members
                .iter()
                .filter(|(_, cluster)| **cluster == key)
                .map(|(node, _)| (*node).clone())
                .collect()
        };
    keyed_changes(base, current, |cluster| &cluster.key)
        .into_iter()
        .map(|change| {
            let mut changed_fields = Vec::new();
            if let (Some(before), cluster) = (change.before, change.item) {
                if before.label != cluster.label {
                    changed_fields.push(ClusterField::Label);
                }
                if before.description != cluster.description {
                    changed_fields.push(ClusterField::Description);
                }
                if before.phase != cluster.phase || before.phase_note != cluster.phase_note {
                    changed_fields.push(ClusterField::Phase);
                }
                if members_of(base_members, &cluster.key)
                    != members_of(current_members, &cluster.key)
                {
                    changed_fields.push(ClusterField::Members);
                }
                if !change.in_place {
                    changed_fields.push(ClusterField::Position);
                }
            }
            match change.presence {
                Presence::Added => summary.clusters_added += 1,
                Presence::Removed => summary.clusters_removed += 1,
                Presence::Retained => {
                    summary.clusters_changed += u32::from(!changed_fields.is_empty());
                }
            }
            ClusterDiff {
                cluster_key: change.item.key.clone(),
                presence: change.presence,
                changed_fields,
            }
        })
        .collect()
}

/// One item of a keyed walk. `item` is a current item, or a removed base item.
struct Change<'a, T> {
    presence: Presence,
    item: &'a T,
    /// The base item of a retained item.
    before: Option<&'a T>,
    /// True when a retained item is in a longest common subsequence of the retained keys.
    in_place: bool,
}

/// Walks two lists by key: current items in order, then removed base items in base order.
fn keyed_changes<'a, T, K: Ord>(
    base: &'a [T],
    current: &'a [T],
    key: fn(&T) -> &K,
) -> Vec<Change<'a, T>> {
    let base_by_key: BTreeMap<&K, &T> = base.iter().map(|item| (key(item), item)).collect();
    let current_keys: BTreeSet<&K> = current.iter().map(key).collect();
    let retained_base: Vec<&K> = base
        .iter()
        .map(key)
        .filter(|key| current_keys.contains(key))
        .collect();
    let retained_current: Vec<&K> = current
        .iter()
        .map(key)
        .filter(|key| base_by_key.contains_key(key))
        .collect();
    let in_place = common_subsequence(&retained_base, &retained_current);
    let mut changes: Vec<Change<'a, T>> = current
        .iter()
        .map(|item| {
            let before = base_by_key.get(key(item)).copied();
            Change {
                presence: if before.is_some() {
                    Presence::Retained
                } else {
                    Presence::Added
                },
                item,
                before,
                in_place: in_place.contains(key(item)),
            }
        })
        .collect();
    changes.extend(
        base.iter()
            .filter(|item| !current_keys.contains(key(item)))
            .map(|item| Change {
                presence: Presence::Removed,
                item,
                before: None,
                in_place: false,
            }),
    );
    changes
}

fn membership(assignment: &AssignmentLayer) -> BTreeMap<&NodeKey, &ClusterKey> {
    assignment
        .members
        .iter()
        .map(|member| (member.node_key(), member.cluster_key()))
        .collect()
}

fn node_labels(steps: &[FlowStep]) -> BTreeMap<&NodeKey, Vec<(&StepId, &ClusterKey)>> {
    let mut labels: BTreeMap<_, Vec<_>> = BTreeMap::new();
    for step in steps {
        labels
            .entry(&step.node_key)
            .or_default()
            .push((&step.id, &step.cluster_key));
    }
    labels
}

/// Returns the items of `current` inside one longest common subsequence with `base`.
///
/// A tie keeps the earlier current items.
fn common_subsequence<'a, T: Ord>(base: &[&'a T], current: &[&'a T]) -> BTreeSet<&'a T> {
    let (rows, columns) = (current.len(), base.len());
    // lengths[i][j] is the LCS length of current[i..] and base[j..].
    let mut lengths = vec![vec![0_usize; columns + 1]; rows + 1];
    for i in (0..rows).rev() {
        for j in (0..columns).rev() {
            lengths[i][j] = if current[i] == base[j] {
                lengths[i + 1][j + 1] + 1
            } else {
                lengths[i + 1][j].max(lengths[i][j + 1])
            };
        }
    }
    let mut kept = BTreeSet::new();
    let (mut i, mut j) = (0, 0);
    while i < rows && j < columns {
        if current[i] == base[j] {
            kept.insert(current[i]);
            i += 1;
            j += 1;
        } else if lengths[i][j + 1] >= lengths[i + 1][j] {
            j += 1;
        } else {
            i += 1;
        }
    }
    kept
}

// -----------------------------------------------------------------------------
// Tests
// -----------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::{common_subsequence, keyed_changes};
    use crate::flow::models::Presence;

    #[test]
    fn common_subsequence_marks_moved_items() {
        let (a, b, c, d) = ("a", "b", "c", "d");
        let kept = common_subsequence(&[&a, &b, &c, &d], &[&b, &c, &a, &d]);
        assert_eq!(kept, BTreeSet::from([&b, &c, &d]));
    }

    #[test]
    fn keyed_changes_list_current_items_then_removed_base_items() {
        let changes = keyed_changes(&["a", "b", "c", "d"], &["b", "e", "a", "d"], |key| key);

        let labels: Vec<String> = changes
            .iter()
            .map(|change| match change.presence {
                Presence::Added => format!("added {}", change.item),
                Presence::Retained if change.in_place => {
                    format!("retained {} in place", change.item)
                }
                Presence::Retained => format!("retained {} moved", change.item),
                Presence::Removed => format!("removed {}", change.item),
            })
            .collect();
        assert_eq!(
            labels,
            [
                "retained b in place",
                "added e",
                "retained a moved",
                "retained d in place",
                "removed c"
            ]
        );
    }
}
