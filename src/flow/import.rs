//! Flow import.
//!
//! This module freezes one agent output as the next generation of one flow.
//! Agent output bytes and a graph artifact of the store are the inputs. One
//! generation file and a moved current pointer are the outputs.
//!
//! An import validates the complete output before any write. It writes nothing
//! when any error issue exists. A dry run does the complete import, including
//! the self-check of the new generation, and writes nothing. The purpose follows
//! from the store: a flow with a lineage gets a `revision`, and any other flow
//! gets an `initial` generation. A revision must use the graph of its parent.
//!
//! # Sections
//! `Import` freezes one agent output. `Rules` holds the parse, prior, freeze, and
//! report steps.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

use serde::Serialize;

use super::{
    FlowError, assign, canonical,
    derive::{self, DeriveError},
    flow_id,
    lineage::{self, StoredGeneration},
    models::{
        AgentFlow, AgentFlowOutput, ArtifactKind, Author, Digests, FLOW_ARTIFACT_SCHEMA_VERSION,
        FactBinding, FactLayer, FlowArtifact, FlowId, FlowIssue, FlowStep, FrozenStatus,
        Generation, GenerationPurpose, IntentLayer, IssueCode, NoFlowReason, NodeKey, Provenance,
        Sha256Digest,
    },
    text,
    validate::{self, CheckedFlow, PriorContext},
};
use crate::store::{
    graph::{ArtifactGraph, check_graph_name, read_graph},
    lock_store,
};

// -----------------------------------------------------------------------------
// Import
// -----------------------------------------------------------------------------

/// One import request. `author` comes from the invocation, never from the agent.
pub struct ImportRequest<'a> {
    pub name: &'a str,
    pub graph_file: &'a str,
    pub input: &'a [u8],
    pub author: Author,
    pub dry_run: bool,
}

/// The result of one successful freeze.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FrozenGeneration {
    pub name: String,
    pub ordinal: u32,
    pub purpose: GenerationPurpose,
    pub generation_digest: Sha256Digest,
    /// The generation path relative to `flows/`.
    pub file: String,
    /// True when the output equals the current generation and nothing was written.
    pub unchanged: bool,
    /// True when the request was a dry run and nothing was written.
    pub dry_run: bool,
    pub warnings: Vec<FlowIssue>,
}

#[derive(Debug)]
pub enum ImportOutcome {
    Frozen(FrozenGeneration),
    NoFlow {
        reason: NoFlowReason,
        explanation: String,
    },
}

/// Freezes one agent output as the next generation of one flow.
///
/// A dry run takes no store lock and returns the generation that a freeze
/// would write.
///
/// # Errors
/// Returns `Rejected` with every issue when the output is invalid; nothing is
/// written then. Returns other errors for store or graph failures.
pub fn import_flow(
    directory: &Path,
    request: ImportRequest<'_>,
) -> Result<ImportOutcome, FlowError> {
    let flow_id = flow_id(request.name)?;
    check_graph_name(request.graph_file)?;
    validate::check_author(&request.author).map_err(|message| {
        FlowError::Rejected(vec![
            FlowIssue::error(IssueCode::InvalidArtifact, message).at("/provenance/author"),
        ])
    })?;
    let flow = match parse_output(request.input)? {
        Ok(flow) => flow,
        Err(outcome) => return Ok(outcome),
    };
    let _lock = if request.dry_run {
        None
    } else {
        Some(lock_store(directory)?)
    };
    let (graph, graph_digest) = read_graph(directory, request.graph_file)?;
    let binding = FactBinding {
        graph_artifact: request.graph_file.to_owned(),
        graph_digest: Sha256Digest::try_from(graph_digest)
            .map_err(|error| FlowError::Derive(DeriveError::Fact(error)))?,
    };
    let parent = lineage::read_current(directory, &flow_id)?;
    let (purpose, prior_ids, mut issues) = prior_rules(parent.as_ref(), &binding);
    let graph_keys: BTreeSet<&str> = graph.nodes.iter().map(|node| node.key.as_str()).collect();
    let context = PriorContext {
        purpose,
        prior_step_ids: prior_ids.iter().map(String::as_str).collect(),
    };
    let (checked, flow_issues) = validate::check_agent_flow(&flow, &context, &graph_keys);
    issues.extend(flow_issues);
    let Some(checked) = checked.filter(|_| !issues.iter().any(FlowIssue::is_error)) else {
        return Err(FlowError::Rejected(issues));
    };
    let warnings: Vec<FlowIssue> = issues
        .into_iter()
        .filter(|issue| !issue.is_error())
        .collect();
    let (facts, intent) = build_intent(&flow_id, checked, &graph, binding, &prior_ids)?;
    let parent = parent.map(|stored| stored.artifact);
    if let Some(parent) = &parent
        && canonical::digest(&intent)? == parent.digests.intent
        && derive::facts_digest(&facts)? == parent.digests.facts
    {
        return Ok(ImportOutcome::Frozen(report(
            parent,
            true,
            request.dry_run,
            warnings,
        )));
    }
    let artifact = freeze(
        flow_id,
        purpose,
        parent.as_ref(),
        facts,
        intent,
        request.author,
    )?;
    if !request.dry_run {
        lineage::write_current(directory, &artifact)?;
    }
    Ok(ImportOutcome::Frozen(report(
        &artifact,
        false,
        request.dry_run,
        warnings,
    )))
}

// -----------------------------------------------------------------------------
// Rules
// -----------------------------------------------------------------------------

/// Parses the agent output. A `no_flow` answer comes back as a finished outcome.
fn parse_output(input: &[u8]) -> Result<Result<AgentFlow, ImportOutcome>, FlowError> {
    let output: AgentFlowOutput = serde_json::from_slice(input).map_err(|error| {
        FlowError::Rejected(vec![FlowIssue::error(
            IssueCode::InvalidOutput,
            error.to_string(),
        )])
    })?;
    match output {
        AgentFlowOutput::NoFlow {
            contract_version,
            reason,
            explanation,
        } => {
            let explanation = text::canonical(&explanation);
            let mut issues = contract_issues(contract_version);
            validate::stored_text(
                &mut issues,
                &explanation,
                "/explanation",
                validate::NO_FLOW_EXPLANATION,
            );
            if issues.is_empty() {
                Ok(Err(ImportOutcome::NoFlow {
                    reason,
                    explanation,
                }))
            } else {
                Err(FlowError::Rejected(issues))
            }
        }
        AgentFlowOutput::Flow {
            contract_version,
            flow,
        } => {
            let issues = contract_issues(contract_version);
            if issues.is_empty() {
                Ok(Ok(flow))
            } else {
                Err(FlowError::Rejected(issues))
            }
        }
    }
}

fn contract_issues(version: u32) -> Vec<FlowIssue> {
    if version == 1 {
        vec![]
    } else {
        vec![
            FlowIssue::error(
                IssueCode::UnsupportedContract,
                format!("unsupported contract version {version}: expected 1"),
            )
            .at("/contractVersion"),
        ]
    }
}

/// Returns the purpose, the prior step IDs, and the graph binding issues.
fn prior_rules(
    parent: Option<&StoredGeneration>,
    current: &FactBinding,
) -> (GenerationPurpose, Vec<String>, Vec<FlowIssue>) {
    let Some(parent) = parent else {
        return (GenerationPurpose::Initial, vec![], vec![]);
    };
    let mut issues = Vec::new();
    let bound = &parent.artifact.facts.binding;
    if bound.graph_artifact != current.graph_artifact {
        issues.push(FlowIssue::error(
            IssueCode::StaleGraph,
            format!(
                "the flow is bound to {0}: use --graph {0}",
                bound.graph_artifact
            ),
        ));
    } else if bound.graph_digest != current.graph_digest {
        issues.push(FlowIssue::error(
            IssueCode::StaleGraph,
            format!(
                "the graph {} changed since the prior version: import the flow under a new name",
                bound.graph_artifact
            ),
        ));
    }
    let ids = parent
        .artifact
        .intent
        .steps
        .iter()
        .map(|step| step.id.to_string())
        .collect();
    (GenerationPurpose::Revision, ids, issues)
}

/// Builds the fact layer and the intent layer, with step IDs and attestations.
fn build_intent(
    flow_id: &FlowId,
    checked: CheckedFlow,
    graph: &ArtifactGraph,
    binding: FactBinding,
    prior_ids: &[String],
) -> Result<(FactLayer, IntentLayer), FlowError> {
    let keys: BTreeSet<&str> = checked
        .steps
        .iter()
        .map(|step| step.node_key.as_str())
        .collect();
    let facts = derive::fact_layer(graph, binding, &keys)?;
    let attested: BTreeMap<&NodeKey, &Sha256Digest> = facts
        .nodes
        .iter()
        .map(|node| (&node.key, &node.fact_digest))
        .collect();
    let mut used: BTreeSet<String> = prior_ids.iter().cloned().collect();
    let mut steps = Vec::with_capacity(checked.steps.len());
    for step in checked.steps {
        let id = match step.prior_step_id {
            Some(id) => id,
            None => derive::step_id(flow_id, &step.node_key, &used)?,
        };
        used.insert(id.to_string());
        let attested_fact_digest = attested
            .get(&step.node_key)
            .map(|digest| (*digest).clone())
            .ok_or_else(|| {
                FlowError::Derive(DeriveError::Graph(format!("no fact for {}", step.node_key)))
            })?;
        steps.push(FlowStep {
            id,
            node_key: step.node_key,
            cluster_key: step.cluster_key,
            title: step.title,
            annotation: step.annotation,
            attested_fact_digest,
        });
    }
    let intent = IntentLayer {
        behavior: checked.behavior,
        boundaries: checked.boundaries,
        evidence_basis: checked.evidence_basis,
        revision_summary: checked.revision_summary,
        clusters: checked.clusters,
        steps,
    };
    Ok((facts, intent))
}

/// Derives the code-owned layers and digests, then verifies the new generation.
fn freeze(
    flow_id: FlowId,
    purpose: GenerationPurpose,
    parent: Option<&FlowArtifact>,
    facts: FactLayer,
    intent: IntentLayer,
    author: Author,
) -> Result<FlowArtifact, FlowError> {
    let assignment = assign::assign(&intent)?;
    let view = derive::view(
        &facts,
        &intent,
        &assignment,
        parent.map(|parent| &parent.view),
    );
    let diff = parent.map(|parent| derive::diff(parent, &intent, &assignment));
    let generation = Generation {
        ordinal: parent.map_or(1, |parent| parent.generation.ordinal + 1),
        purpose,
        parent_digest: parent.map(|parent| parent.digests.generation.clone()),
    };
    let placeholder = canonical::digest_bytes(b"");
    let mut artifact = FlowArtifact {
        kind: ArtifactKind::Flow,
        schema_version: FLOW_ARTIFACT_SCHEMA_VERSION,
        flow_id,
        status: FrozenStatus::Frozen,
        generation,
        facts,
        intent,
        assignment,
        view,
        diff,
        digests: Digests {
            facts: placeholder.clone(),
            intent: placeholder.clone(),
            assignment: placeholder.clone(),
            view: placeholder.clone(),
            diff: None,
            generation: placeholder,
        },
        provenance: Provenance {
            frozen_at: jiff::Timestamp::now()
                .strftime("%Y-%m-%dT%H:%M:%SZ")
                .to_string(),
            cli_version: env!("CARGO_PKG_VERSION").to_owned(),
            author,
        },
    };
    artifact.digests = derive::digests(&artifact)?;
    let problems: Vec<_> = validate::verify_artifact(&artifact, parent)
        .into_iter()
        .filter(FlowIssue::is_error)
        .collect();
    if problems.is_empty() {
        Ok(artifact)
    } else {
        Err(FlowError::SelfCheck(problems))
    }
}

/// Builds the result of an import from the generation that is current after it.
fn report(
    artifact: &FlowArtifact,
    unchanged: bool,
    dry_run: bool,
    warnings: Vec<FlowIssue>,
) -> FrozenGeneration {
    FrozenGeneration {
        name: artifact.flow_id.to_string(),
        ordinal: artifact.generation.ordinal,
        purpose: artifact.generation.purpose,
        generation_digest: artifact.digests.generation.clone(),
        file: lineage::generation_file(
            &artifact.flow_id,
            artifact.generation.ordinal,
            &artifact.digests.generation,
        ),
        unchanged,
        dry_run,
        warnings,
    }
}

// -----------------------------------------------------------------------------
// Tests
// -----------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::{collections::BTreeSet, path::Path};

    use serde_json::Value;

    use super::{FlowError, ImportOutcome};
    use crate::flow::{
        derive,
        fixtures::{self, GOLDEN_GENERATION, frozen, import, output, snapshot_files},
        models::{
            ClusterColor, GenerationPurpose, InputCategory, IssueCode, NoFlowReason, Presence,
        },
        verify::verify_flow,
    };

    #[test]
    fn initial_imports_reproduce_the_golden_generation_and_list_the_flow() {
        let directory = fixtures::store();
        let path = directory.path();

        let generation = frozen(import(path, "demo-flow", &output()));

        assert_eq!(
            (generation.ordinal, generation.purpose),
            (1, GenerationPurpose::Initial)
        );
        assert_eq!(generation.warnings, Vec::new());
        assert_eq!(generation.generation_digest.as_str(), GOLDEN_GENERATION);
        let artifact = fixtures::current(path, "demo-flow");
        assert!(artifact.diff.is_none());
        assert_eq!(artifact.intent.steps.len(), 6);
        assert_eq!(artifact.facts.nodes.len(), 5);
        assert_eq!(
            artifact
                .view
                .clusters
                .iter()
                .map(|cluster| cluster.color)
                .collect::<Vec<_>>(),
            [ClusterColor::Cyan, ClusterColor::Red]
        );
        let report = verify_flow(path, "demo-flow").expect("verify");
        assert!(report.issues.is_empty(), "{:?}", report.issues);
        assert!(!report.stale);
    }

    #[test]
    fn initial_imports_derive_step_ids_and_trim_text() {
        let directory = fixtures::store();
        let path = directory.path();
        let mut value = output();
        value["flow"]["behavior"] =
            "  How the demo command parses input and saves a result.  ".into();

        let generation = frozen(import(path, "demo-flow", &value));

        assert!(
            generation
                .warnings
                .iter()
                .any(|issue| issue.code == IssueCode::TextTrimmed)
        );
        let artifact = fixtures::current(path, "demo-flow");
        assert_eq!(
            artifact.intent.behavior,
            "How the demo command parses input and saves a result."
        );
        let first = derive::step_id(
            &artifact.flow_id,
            &artifact.intent.steps[0].node_key,
            &BTreeSet::new(),
        )
        .expect("ID");
        assert_eq!(artifact.intent.steps[0].id, first);
        let ids: BTreeSet<_> = artifact.intent.steps.iter().map(|step| &step.id).collect();
        assert_eq!(ids.len(), artifact.intent.steps.len());
    }

    /// Imports one changed output and checks that it fails with `code` and writes nothing.
    fn assert_rejected(path: &Path, name: &str, value: &Value, code: IssueCode) {
        let before = snapshot_files(path);
        let error = import(path, name, value).expect_err("rejected output");
        assert!(
            error.issues().iter().any(|issue| issue.code == code),
            "{code:?}: {:?}",
            error.issues()
        );
        assert_eq!(snapshot_files(path), before, "{code:?} wrote files");
    }

    #[test]
    fn rejected_outputs_report_every_issue_and_write_nothing() {
        let directory = fixtures::store();
        let path = directory.path();
        for (code, pointer, replacement) in [
            (
                IssueCode::UnknownNode,
                "/flow/steps/0/nodeKey",
                "src/missing.rs::callable:gone".into(),
            ),
            (
                IssueCode::UnknownCluster,
                "/flow/steps/0/clusterKey",
                "not-defined".into(),
            ),
            (
                IssueCode::NodeLabelConflict,
                "/flow/steps/0/clusterKey",
                "write-result".into(),
            ),
            (
                IssueCode::DuplicateClusterKey,
                "/flow/clusters/1/key",
                "parse-input".into(),
            ),
            (
                IssueCode::DuplicateClusterLabel,
                "/flow/clusters/1/label",
                "PARSE THE INPUT".into(),
            ),
            (
                IssueCode::UnknownPriorStep,
                "/flow/steps/0/priorStepId",
                "st-0000000000000000".into(),
            ),
            (
                IssueCode::RevisionSummaryForbidden,
                "/flow/revisionSummary",
                "Summarize a change that has no prior version.".into(),
            ),
            (
                IssueCode::PhaseNoteRequired,
                "/flow/clusters/0/phase",
                "other".into(),
            ),
            (
                IssueCode::PhaseNoteForbidden,
                "/flow/clusters/0/phaseNote",
                "Input handling".into(),
            ),
            (IssueCode::InvalidText, "/flow/steps/0/title", "ab".into()),
            (
                IssueCode::InvalidKey,
                "/flow/clusters/0/key",
                "Parse".into(),
            ),
            (
                IssueCode::InvalidOutput,
                "/flow/evidenceBasis",
                "guesswork".into(),
            ),
            (IssueCode::UnsupportedContract, "/contractVersion", 2.into()),
        ] {
            let mut value = output();
            *value.pointer_mut(pointer).expect(pointer) = replacement;
            assert_rejected(path, "demo-flow", &value, code);
        }
        let mut value = output();
        value["flow"]
            .as_object_mut()
            .expect("flow")
            .remove("boundaries");
        assert_rejected(path, "demo-flow", &value, IssueCode::InvalidOutput);
        let mut value = output();
        value["flow"]["clusters"][0]["color"] = "cyan".into();
        assert_rejected(path, "demo-flow", &value, IssueCode::InvalidOutput);
        let mut value = output();
        let mut spare = value["flow"]["clusters"][0].clone();
        spare["key"] = "spare-cluster".into();
        spare["label"] = "Spare cluster label".into();
        value["flow"]["clusters"]
            .as_array_mut()
            .expect("clusters")
            .push(spare);
        assert_rejected(path, "demo-flow", &value, IssueCode::UnusedCluster);
        assert_rejected(path, "Demo", &output(), IssueCode::InvalidKey);
    }

    /// Builds a revision that moves the entry function into a new cluster and edits one annotation.
    fn revision(parent: &crate::flow::models::FlowArtifact) -> (Value, u32) {
        let mut value = output();
        for (step, stored) in value["flow"]["steps"]
            .as_array_mut()
            .expect("steps")
            .iter_mut()
            .zip(&parent.intent.steps)
        {
            step["priorStepId"] = stored.id.as_str().into();
        }
        let main = value["flow"]["steps"][0]["nodeKey"].clone();
        let mut new_cluster = value["flow"]["clusters"][0].clone();
        new_cluster["key"] = "report-result".into();
        new_cluster["label"] = "Report the command result".into();
        new_cluster["phase"] = "respond".into();
        value["flow"]["clusters"]
            .as_array_mut()
            .expect("clusters")
            .push(new_cluster);
        let mut relabeled = 0;
        for step in value["flow"]["steps"].as_array_mut().expect("steps") {
            if step["nodeKey"] == main {
                step["clusterKey"] = "report-result".into();
                relabeled += 1;
            }
        }
        value["flow"]["steps"][4]["annotation"] = "Write the result under the store lock.".into();
        value["flow"]["revisionSummary"] =
            "Move the entry function into a report cluster and shorten step 5.".into();
        (value, relabeled + 1)
    }

    #[test]
    fn revisions_freeze_a_diff_inherit_colors_and_keep_equal_output() {
        let directory = fixtures::store();
        let path = directory.path();
        frozen(import(path, "demo-flow", &output()));
        let parent = fixtures::current(path, "demo-flow");
        let (value, changed) = revision(&parent);
        let mut missing_summary = value.clone();
        missing_summary["flow"]["revisionSummary"] = Value::Null;
        assert_rejected(
            path,
            "demo-flow",
            &missing_summary,
            IssueCode::RevisionSummaryRequired,
        );
        let mut reused = value.clone();
        reused["flow"]["steps"][1]["priorStepId"] =
            reused["flow"]["steps"][0]["priorStepId"].clone();
        assert_rejected(path, "demo-flow", &reused, IssueCode::PriorStepReused);

        let generation = frozen(import(path, "demo-flow", &value));

        assert_eq!(
            (generation.ordinal, generation.purpose),
            (2, GenerationPurpose::Revision)
        );
        let artifact = fixtures::current(path, "demo-flow");
        assert_eq!(
            artifact.generation.parent_digest.as_ref(),
            Some(&parent.digests.generation)
        );
        let diff = artifact.diff.as_ref().expect("diff");
        assert_eq!(diff.base.generation_digest, parent.digests.generation);
        assert_eq!(diff.summary.clusters_added, 1);
        assert_eq!(diff.summary.members_changed, 1);
        assert_eq!(diff.summary.steps_changed, changed);
        assert_eq!(diff.changed_inputs, vec![InputCategory::Intent]);
        assert!(
            diff.steps
                .iter()
                .all(|step| step.presence == Presence::Retained)
        );
        let colors: Vec<_> = artifact
            .view
            .clusters
            .iter()
            .map(|cluster| cluster.color)
            .collect();
        assert_eq!(
            colors,
            [ClusterColor::Cyan, ClusterColor::Red, ClusterColor::Yellow]
        );
        let report = verify_flow(path, "demo-flow").expect("verify");
        assert!(report.issues.is_empty(), "{:?}", report.issues);
        let again = frozen(import(path, "demo-flow", &value));
        assert!(again.unchanged);
        assert_eq!(again.generation_digest, generation.generation_digest);
    }

    #[test]
    fn a_changed_graph_blocks_a_revision() {
        let directory = fixtures::store();
        let path = directory.path();
        frozen(import(path, "demo-flow", &output()));
        let parent = fixtures::current(path, "demo-flow");
        fixtures::write_graph(path, "src/extra.rs::callable:another");
        let (value, _) = revision(&parent);

        let error = import(path, "demo-flow", &value).expect_err("stale graph");

        assert!(
            error
                .issues()
                .iter()
                .any(|issue| issue.code == IssueCode::StaleGraph)
        );
    }

    #[test]
    fn no_flow_answers_write_nothing() {
        let directory = fixtures::store();
        let before = snapshot_files(directory.path());
        let value = serde_json::json!({
            "contractVersion": 1, "outcome": "no_flow", "reason": "behavior_not_found",
            "explanation": "The index has no handler for webhook retries."
        });

        let outcome = import(directory.path(), "demo-flow", &value).expect("no flow");

        assert!(matches!(
            outcome,
            ImportOutcome::NoFlow {
                reason: NoFlowReason::BehaviorNotFound,
                ..
            }
        ));
        assert_eq!(snapshot_files(directory.path()), before);
    }

    #[test]
    fn a_missing_graph_is_an_operational_error() {
        let directory = fixtures::store();
        let input = serde_json::to_vec(&output()).expect("bytes");

        let error = super::import_flow(
            directory.path(),
            super::ImportRequest {
                name: "demo-flow",
                graph_file: "missing.json",
                input: &input,
                author: fixtures::agent(),
                dry_run: false,
            },
        )
        .expect_err("missing graph");

        assert!(matches!(error, FlowError::Io(_)));
    }
}
