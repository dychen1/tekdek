//! View derivation.
//!
//! This module derives the view layer: the palette color of each cluster, the
//! visit runs, and the static call transition of each step. The fact, intent,
//! and assignment layers, and the parent view, are the input.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use crate::flow::models::{
    AssignmentLayer, ClusterColor, ClusterIntent, ClusterKey, ClusterView, FactEdge, FactLayer,
    IntentLayer, NodeKey, Palette, RelationKind, StepDecision, StepId, StepView, Transition,
    ViewLayer, VisitRun,
};

/// Derives the view layer.
///
/// A retained cluster keeps its parent color. A new cluster takes the lowest
/// palette slot that no retained cluster uses and that no cluster removed in this
/// generation used. When those slots run out, it takes the lowest slot that no
/// retained cluster uses.
#[must_use]
pub fn view(
    facts: &FactLayer,
    intent: &IntentLayer,
    assignment: &AssignmentLayer,
    parent: Option<&ViewLayer>,
) -> ViewLayer {
    let steps = step_views(facts, intent, assignment);
    let colors = cluster_colors(&intent.clusters, parent);
    let clusters = intent
        .clusters
        .iter()
        .zip(colors)
        .map(|(cluster, color)| ClusterView {
            cluster_key: cluster.key.clone(),
            color,
            member_count: u32::try_from(
                assignment
                    .members
                    .iter()
                    .filter(|member| member.cluster_key() == &cluster.key)
                    .count(),
            )
            .unwrap_or(u32::MAX),
            visits: visit_runs(&steps, &cluster.key),
        })
        .collect();
    ViewLayer {
        palette: Palette::FlowBright1,
        clusters,
        steps,
    }
}

fn step_views(
    facts: &FactLayer,
    intent: &IntentLayer,
    assignment: &AssignmentLayer,
) -> Vec<StepView> {
    let decided: BTreeMap<&StepId, &ClusterKey> = assignment
        .steps
        .iter()
        .map(|decision| match decision {
            StepDecision::Assigned {
                step_id,
                cluster_key,
                ..
            } => (step_id, cluster_key),
        })
        .collect();
    let calls = CallGraph::new(&facts.edges);
    let mut previous: Option<&NodeKey> = None;
    intent
        .steps
        .iter()
        .map(|step| {
            let key = &step.node_key;
            let transition = previous.map_or(Transition::FlowStart, |previous| {
                calls.transition(previous, key)
            });
            previous = Some(key);
            StepView {
                step_id: step.id.clone(),
                cluster_key: decided
                    .get(&step.id)
                    .map_or_else(|| step.cluster_key.clone(), |cluster| (*cluster).clone()),
                transition,
            }
        })
        .collect()
}

fn cluster_colors(clusters: &[ClusterIntent], parent: Option<&ViewLayer>) -> Vec<ClusterColor> {
    let inherited: BTreeMap<&ClusterKey, ClusterColor> = parent
        .map(|view| {
            view.clusters
                .iter()
                .map(|cluster| (&cluster.cluster_key, cluster.color))
                .collect()
        })
        .unwrap_or_default();
    let current: BTreeSet<&ClusterKey> = clusters.iter().map(|cluster| &cluster.key).collect();
    let mut retained: Vec<ClusterColor> = clusters
        .iter()
        .filter_map(|cluster| inherited.get(&cluster.key).copied())
        .collect();
    let removed: Vec<ClusterColor> = inherited
        .iter()
        .filter(|(key, _)| !current.contains(*key))
        .map(|(_, color)| *color)
        .collect();
    clusters
        .iter()
        .map(|cluster| {
            if let Some(color) = inherited.get(&cluster.key) {
                return *color;
            }
            let free = |color: &&ClusterColor| !retained.contains(color);
            let color = ClusterColor::PALETTE
                .iter()
                .filter(free)
                .find(|color| !removed.contains(color))
                .or_else(|| ClusterColor::PALETTE.iter().find(free))
                .copied()
                .unwrap_or(ClusterColor::Violet);
            retained.push(color);
            color
        })
        .collect()
}

fn visit_runs(steps: &[StepView], key: &ClusterKey) -> Vec<VisitRun> {
    let mut runs = Vec::new();
    let mut start = None;
    for (index, step) in (1_u32..).zip(steps) {
        let inside = &step.cluster_key == key;
        match (inside, start) {
            (true, None) => start = Some(index),
            (false, Some(first)) => {
                runs.push(VisitRun {
                    first_step: first,
                    last_step: index - 1,
                });
                start = None;
            }
            _ => {}
        }
    }
    if let Some(first) = start {
        runs.push(VisitRun {
            first_step: first,
            last_step: u32::try_from(steps.len()).unwrap_or(u32::MAX),
        });
    }
    runs
}

/// Static `calls` edges between fact nodes.
struct CallGraph<'a> {
    callees: BTreeMap<&'a NodeKey, BTreeSet<&'a NodeKey>>,
    callers: BTreeMap<&'a NodeKey, BTreeSet<&'a NodeKey>>,
}

impl<'a> CallGraph<'a> {
    fn new(edges: &'a [FactEdge]) -> Self {
        let mut outgoing: BTreeMap<_, BTreeSet<_>> = BTreeMap::new();
        let mut incoming: BTreeMap<_, BTreeSet<_>> = BTreeMap::new();
        for edge in edges.iter().filter(|edge| edge.kind == RelationKind::Calls) {
            outgoing
                .entry(&edge.source)
                .or_default()
                .insert(&edge.target);
            incoming
                .entry(&edge.target)
                .or_default()
                .insert(&edge.source);
        }
        Self {
            callees: outgoing,
            callers: incoming,
        }
    }

    /// Classifies the move from one step node to the next, in the documented check order.
    fn transition(&self, previous: &NodeKey, key: &NodeKey) -> Transition {
        let calls = |from: &NodeKey, to: &NodeKey| {
            self.callees.get(from).is_some_and(|set| set.contains(to))
        };
        if previous == key {
            Transition::SameFunction
        } else if calls(previous, key) {
            Transition::Calls
        } else if calls(key, previous) {
            Transition::ReturnsToCaller
        } else if self.shares_caller(previous, key) {
            Transition::SiblingCall
        } else if self.reaches(previous, key) {
            Transition::CallsIndirectly
        } else if self.reaches(key, previous) {
            Transition::ReturnsToAncestor
        } else {
            Transition::NoStaticEdge
        }
    }

    fn shares_caller(&self, left: &NodeKey, right: &NodeKey) -> bool {
        match (self.callers.get(left), self.callers.get(right)) {
            (Some(left), Some(right)) => !left.is_disjoint(right),
            _ => false,
        }
    }

    fn reaches(&self, start: &NodeKey, goal: &NodeKey) -> bool {
        let mut seen = BTreeSet::from([start]);
        let mut queue = VecDeque::from([start]);
        while let Some(node) = queue.pop_front() {
            for next in self.callees.get(node).into_iter().flatten() {
                if *next == goal {
                    return true;
                }
                if seen.insert(next) {
                    queue.push_back(next);
                }
            }
        }
        false
    }
}

// -----------------------------------------------------------------------------
// Tests
// -----------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::{CallGraph, cluster_colors};
    use crate::flow::models::{
        ClusterColor, ClusterIntent, ClusterKey, ClusterView, FactEdge, FlowPhase, NodeKey,
        Palette, RelationKind, Transition, ViewLayer,
    };

    fn key(text: &str) -> NodeKey {
        NodeKey::try_from(text).expect("node key")
    }

    fn cluster(name: &str) -> ClusterIntent {
        ClusterIntent {
            key: ClusterKey::try_from(name).expect("cluster key"),
            label: name.to_owned(),
            description: "A cluster that the tests use.".to_owned(),
            phase: FlowPhase::Transform,
            phase_note: None,
        }
    }

    #[test]
    fn classifies_static_transitions_in_the_documented_order() {
        let edge = |source: &str, target: &str| FactEdge {
            source: key(source),
            target: key(target),
            kind: RelationKind::Calls,
        };
        let edges = [edge("a", "b"), edge("a", "c"), edge("b", "d")];
        let calls = CallGraph::new(&edges);

        assert_eq!(
            calls.transition(&key("a"), &key("a")),
            Transition::SameFunction
        );
        assert_eq!(calls.transition(&key("a"), &key("b")), Transition::Calls);
        assert_eq!(
            calls.transition(&key("b"), &key("a")),
            Transition::ReturnsToCaller
        );
        assert_eq!(
            calls.transition(&key("b"), &key("c")),
            Transition::SiblingCall
        );
        assert_eq!(
            calls.transition(&key("a"), &key("d")),
            Transition::CallsIndirectly
        );
        assert_eq!(
            calls.transition(&key("d"), &key("a")),
            Transition::ReturnsToAncestor
        );
        assert_eq!(
            calls.transition(&key("c"), &key("d")),
            Transition::NoStaticEdge
        );
    }

    #[test]
    fn keeps_parent_colors_and_skips_colors_of_removed_clusters() {
        let parent = ViewLayer {
            palette: Palette::FlowBright1,
            clusters: [("kept", ClusterColor::Red), ("gone", ClusterColor::Cyan)]
                .into_iter()
                .map(|(name, color)| ClusterView {
                    cluster_key: ClusterKey::try_from(name).expect("cluster key"),
                    color,
                    member_count: 1,
                    visits: vec![],
                })
                .collect(),
            steps: vec![],
        };

        let colors = cluster_colors(&[cluster("fresh"), cluster("kept")], Some(&parent));

        assert_eq!(colors, [ClusterColor::Yellow, ClusterColor::Red]);
        assert_eq!(
            cluster_colors(&[cluster("first"), cluster("second")], None),
            [ClusterColor::Cyan, ClusterColor::Red]
        );
    }
}
