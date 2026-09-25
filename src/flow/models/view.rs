//! View layer: colors, visit runs, and static transitions.
//!
//! Code derives this layer from the facts, intent, and assignment layers, and
//! from the parent view for color inheritance.

use serde::{Deserialize, Serialize};

use super::{ClusterKey, StepId};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ViewLayer {
    pub palette: Palette,
    /// Intent order.
    pub clusters: Vec<ClusterView>,
    /// Step order.
    pub steps: Vec<StepView>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Palette {
    #[serde(rename = "flow-bright/1")]
    FlowBright1,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClusterView {
    pub cluster_key: ClusterKey,
    pub color: ClusterColor,
    pub member_count: u32,
    /// One-based, inclusive runs of consecutive steps that extract the cluster.
    pub visits: Vec<VisitRun>,
}

/// Palette slots in order. A new cluster takes the lowest free slot.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClusterColor {
    Cyan,
    Red,
    Yellow,
    Magenta,
    Lime,
    Orange,
    Blue,
    Violet,
}

impl ClusterColor {
    /// The `flow-bright/1` slots in palette order.
    pub const PALETTE: [Self; 8] = [
        Self::Cyan,
        Self::Red,
        Self::Yellow,
        Self::Magenta,
        Self::Lime,
        Self::Orange,
        Self::Blue,
        Self::Violet,
    ];
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct VisitRun {
    pub first_step: u32,
    pub last_step: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StepView {
    pub step_id: StepId,
    pub cluster_key: ClusterKey,
    pub transition: Transition,
}

/// The static call relation from the previous step node. Not a runtime observation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Transition {
    FlowStart,
    SameFunction,
    Calls,
    ReturnsToCaller,
    SiblingCall,
    CallsIndirectly,
    ReturnsToAncestor,
    NoStaticEdge,
}
