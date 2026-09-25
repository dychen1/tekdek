//! Derived flow layers.
//!
//! This module owns the pure functions that code applies to build and check a
//! generation: the fact projection, step IDs, the view layer, the diff overlay,
//! and the digests. Equal inputs always give equal outputs.
//!
//! # Modules
//! `facts` projects graph nodes. `view` derives colors, runs, and transitions.
//! `diff` compares a revision with its parent. `digests` hashes the layers.
//!
//! # Sections
//! `Steps` derives step IDs. `Errors` reports failures.

mod diff;
mod digests;
mod facts;
mod view;

use std::collections::BTreeSet;

pub use diff::diff;
pub use digests::{digests, facts_digest};
pub use facts::{fact_digest, fact_layer};
pub use view::view;

use super::{
    canonical::{self, CanonicalError},
    models::{FlowId, InvalidIdentifier, NodeKey, StepId},
};

// -----------------------------------------------------------------------------
// Steps
// -----------------------------------------------------------------------------

/// Derives the next unused `st-` step ID for one node.
///
/// `n` is the smallest integer from 0 that gives an ID outside `used`.
///
/// # Errors
/// Returns an error when the ID input cannot have a canonical form.
pub fn step_id(
    flow_id: &FlowId,
    node_key: &NodeKey,
    used: &BTreeSet<String>,
) -> Result<StepId, DeriveError> {
    for n in 0_u64.. {
        let hex = canonical::hex_digest(&("step", flow_id, node_key, n))?;
        let candidate = format!("st-{}", &hex[..16]);
        if !used.contains(&candidate) {
            return StepId::try_from(candidate).map_err(DeriveError::Fact);
        }
    }
    Err(DeriveError::Graph("no free step ID".to_owned()))
}

// -----------------------------------------------------------------------------
// Errors
// -----------------------------------------------------------------------------

#[derive(Debug)]
pub enum DeriveError {
    Fact(InvalidIdentifier),
    Graph(String),
    Canonical(CanonicalError),
}

impl std::fmt::Display for DeriveError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Fact(error) => write!(
                formatter,
                "the graph has a value that a flow cannot hold: {error}"
            ),
            Self::Graph(message) => write!(formatter, "{message}"),
            Self::Canonical(error) => write!(formatter, "{error}"),
        }
    }
}

impl std::error::Error for DeriveError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Fact(error) => Some(error),
            Self::Canonical(error) => Some(error),
            Self::Graph(_) => None,
        }
    }
}

impl From<CanonicalError> for DeriveError {
    fn from(error: CanonicalError) -> Self {
        Self::Canonical(error)
    }
}

// -----------------------------------------------------------------------------
// Tests
// -----------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::step_id;
    use crate::flow::models::{FlowId, NodeKey};

    #[test]
    fn derives_the_documented_step_id_and_skips_used_ids() {
        let flow = FlowId::try_from("flow-generation").expect("flow ID");
        let node = NodeKey::try_from("src/main.rs::callable:main").expect("key");
        let first = step_id(&flow, &node, &BTreeSet::new()).expect("step ID");
        assert_eq!(first.as_str(), "st-67244b2b65163f51");

        let second = step_id(&flow, &node, &BTreeSet::from([first.to_string()])).expect("step ID");
        assert_ne!(second, first);
        assert!(second.as_str().starts_with("st-"));
    }
}
