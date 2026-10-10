//! Flow verification.
//!
//! This module checks the current generation of one flow without writing files:
//! it recomputes every derived layer and digest, and it compares the fact layer
//! with the bound graph when that graph is unchanged.

use std::{collections::BTreeSet, path::Path};

use serde::Serialize;

use super::{
    FlowError, current_flow, derive, lineage,
    models::{FlowIssue, GenerationPurpose, IssueCode, Sha256Digest},
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
    let current = current_flow(directory, name)?;
    let artifact = &current.artifact;
    // A parent that cannot be read gives `unknown_generation` in `verify_artifact`.
    let parent = artifact
        .generation
        .parent_digest
        .as_ref()
        .and_then(|digest| {
            lineage::read_generation(
                directory,
                &artifact.flow_id,
                artifact.generation.ordinal.saturating_sub(1),
                digest,
            )
            .ok()
        });
    let mut issues =
        validate::verify_artifact(artifact, parent.as_ref().map(|parent| &parent.artifact));
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
        name: artifact.flow_id.to_string(),
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
        validate::verify_artifact,
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
    fn a_revision_without_its_parent_reports_unknown_generation_once() {
        let directory = fixtures::store();
        let path = directory.path();
        let first = frozen(import(path, "demo-flow", &output()));
        let parent = fixtures::current(path, "demo-flow");
        // A revision that drops the first cluster keeps the second cluster color.
        let mut revised = output();
        revised["flow"]["clusters"]
            .as_array_mut()
            .expect("clusters")
            .remove(0);
        revised["flow"]["revisionSummary"] = "Fold every step into one cluster.".into();
        for (step, stored) in revised["flow"]["steps"]
            .as_array_mut()
            .expect("steps")
            .iter_mut()
            .zip(&parent.intent.steps)
        {
            step["priorStepId"] = stored.id.as_str().into();
            step["clusterKey"] = "write-result".into();
        }
        frozen(import(path, "demo-flow", &revised));
        let revision = fixtures::current(path, "demo-flow");
        std::fs::remove_file(path.join("flows").join(first.file)).expect("remove the parent");

        let report = verify_flow(path, "demo-flow").expect("verify");

        let codes: Vec<_> = report.issues.iter().map(|issue| issue.code).collect();
        assert_eq!(codes, [IssueCode::UnknownGeneration], "{:?}", report.issues);
        let alone = verify_artifact(&revision, None);
        let codes: Vec<_> = alone.iter().map(|issue| issue.code).collect();
        assert_eq!(codes, [IssueCode::UnknownGeneration], "{alone:?}");
    }

    #[test]
    fn verify_names_a_flow_without_a_lineage() {
        let directory = fixtures::store();

        let error = verify_flow(directory.path(), "absent-flow").expect_err("no lineage");

        assert!(matches!(error, FlowError::NoLineage { .. }));
    }
}
