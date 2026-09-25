//! Flow verification.
//!
//! This module checks the current generation of one flow without writing files:
//! it recomputes every derived layer and digest, and it compares the fact layer
//! with the bound graph when that graph is unchanged.

use std::{collections::BTreeSet, path::Path};

use serde::Serialize;

use super::{
    FlowError, derive, lineage,
    models::{FlowId, FlowIssue, GenerationPurpose, IssueCode, Sha256Digest},
    validate,
};
use crate::store::graph::read_graph;

/// The result of one verification. `issues` is empty for a valid generation.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VerifyReport {
    pub name: String,
    pub file: String,
    pub ordinal: u32,
    pub purpose: GenerationPurpose,
    pub generation_digest: Sha256Digest,
    /// True when the bound graph changed or is missing. A stale flow can still be valid.
    pub stale: bool,
    pub issues: Vec<FlowIssue>,
}

/// Verifies the current generation of one flow without writing files.
///
/// # Errors
/// Returns an error when the flow has no lineage or its files cannot be read.
pub fn verify_flow(directory: &Path, name: &str) -> Result<VerifyReport, FlowError> {
    let flow_id = FlowId::try_from(name).map_err(|error| {
        FlowError::Rejected(vec![FlowIssue::error(
            IssueCode::InvalidKey,
            error.to_string(),
        )])
    })?;
    let current =
        lineage::read_current(directory, &flow_id)?.ok_or_else(|| FlowError::NoLineage {
            name: name.to_owned(),
        })?;
    let artifact = &current.artifact;
    let mut issues = Vec::new();
    let parent = match &artifact.generation.parent_digest {
        Some(digest) => match lineage::read_generation(
            directory,
            &flow_id,
            artifact.generation.ordinal.saturating_sub(1),
            digest,
        ) {
            Ok(parent) => Some(parent.artifact),
            Err(error) => {
                issues.push(FlowIssue::error(
                    IssueCode::UnknownGeneration,
                    format!("cannot read the parent generation: {error}"),
                ));
                None
            }
        },
        None => None,
    };
    issues.extend(validate::verify_artifact(artifact, parent.as_ref()));
    let binding = &artifact.facts.binding;
    let stale = match read_graph(directory, &binding.graph_artifact) {
        Ok((graph, digest)) if digest == binding.graph_digest.as_str() => {
            let keys: BTreeSet<&str> = artifact
                .facts
                .nodes
                .iter()
                .map(|node| node.key.as_str())
                .collect();
            if derive::fact_layer(&graph, binding.clone(), &keys)
                .ok()
                .as_ref()
                != Some(&artifact.facts)
            {
                issues.push(
                    FlowIssue::error(
                        IssueCode::DerivedMismatch,
                        "the fact layer does not match the bound graph",
                    )
                    .at("/facts"),
                );
            }
            false
        }
        _ => true,
    };
    Ok(VerifyReport {
        name: flow_id.to_string(),
        file: current.file,
        ordinal: artifact.generation.ordinal,
        purpose: artifact.generation.purpose,
        generation_digest: artifact.digests.generation.clone(),
        stale,
        issues,
    })
}

// -----------------------------------------------------------------------------
// Tests
// -----------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::verify_flow;
    use crate::flow::{
        FlowError,
        fixtures::{self, frozen, import, output},
        models::IssueCode,
    };

    #[test]
    fn the_committed_demo_flow_verifies_against_its_graph() {
        let store = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("demo");

        let report = verify_flow(&store, "flow-generation").expect("verify the demo flow");

        assert!(report.issues.is_empty(), "{:?}", report.issues);
        assert!(
            !report.stale,
            "reindex the demo store and import the flow again"
        );
    }

    #[test]
    fn verify_reports_an_edited_frozen_generation() {
        let directory = fixtures::store();
        let path = directory.path();
        let generation = frozen(import(path, "demo-flow", &output()));
        let file = path.join("flows").join(&generation.file);
        let text = std::fs::read_to_string(&file).expect("generation");
        std::fs::write(
            &file,
            text.replacen("Start the command", "Start the CLI command", 1),
        )
        .expect("edit");

        let report = verify_flow(path, "demo-flow").expect("verify");

        assert!(
            report
                .issues
                .iter()
                .any(|issue| issue.code == IssueCode::DigestMismatch)
        );
    }

    #[test]
    fn verify_marks_a_changed_graph_stale_but_valid() {
        let directory = fixtures::store();
        let path = directory.path();
        frozen(import(path, "demo-flow", &output()));
        fixtures::write_graph(path, "src/extra.rs::callable:another");

        let report = verify_flow(path, "demo-flow").expect("verify");

        assert!(
            report.stale && report.issues.is_empty(),
            "{:?}",
            report.issues
        );
    }

    #[test]
    fn verify_names_a_flow_without_a_lineage() {
        let directory = fixtures::store();

        let error = verify_flow(directory.path(), "absent-flow").expect_err("no lineage");

        assert!(matches!(error, FlowError::NoLineage { .. }));
    }
}
