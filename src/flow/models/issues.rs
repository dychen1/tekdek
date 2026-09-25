//! Flow validation issues.
//!
//! Errors block a freeze and make an artifact invalid. Warnings do not.

use std::fmt;

use serde::Serialize;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IssueCode {
    InvalidOutput,
    UnsupportedContract,
    InvalidKey,
    InvalidText,
    ClusterLimit,
    StepLimit,
    PhaseNoteRequired,
    PhaseNoteForbidden,
    UnknownNode,
    UnknownCluster,
    UnusedCluster,
    DuplicateClusterKey,
    DuplicateClusterLabel,
    DuplicateStepId,
    UnknownPriorStep,
    PriorStepReused,
    NodeLabelConflict,
    RevisionSummaryRequired,
    RevisionSummaryForbidden,
    StaleGraph,
    InvalidArtifact,
    DerivedMismatch,
    DigestMismatch,
    UnknownGeneration,
    TextTrimmed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Error,
    Warning,
}

/// One problem with an agent output or a stored artifact. `path` is a JSON pointer.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FlowIssue {
    pub code: IssueCode,
    pub severity: Severity,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub step_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cluster_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub node_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

impl FlowIssue {
    #[must_use]
    pub fn error(code: IssueCode, message: impl Into<String>) -> Self {
        Self {
            code,
            severity: Severity::Error,
            message: message.into(),
            step_id: None,
            cluster_key: None,
            node_key: None,
            path: None,
        }
    }

    #[must_use]
    pub fn warning(code: IssueCode, message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Warning,
            ..Self::error(code, message)
        }
    }

    #[must_use]
    pub fn at(mut self, path: impl Into<String>) -> Self {
        self.path = Some(path.into());
        self
    }

    #[must_use]
    pub fn step(mut self, id: impl Into<String>) -> Self {
        self.step_id = Some(id.into());
        self
    }

    #[must_use]
    pub fn cluster(mut self, key: impl Into<String>) -> Self {
        self.cluster_key = Some(key.into());
        self
    }

    #[must_use]
    pub fn node(mut self, key: impl Into<String>) -> Self {
        self.node_key = Some(key.into());
        self
    }

    #[must_use]
    pub fn is_error(&self) -> bool {
        self.severity == Severity::Error
    }
}

impl fmt::Display for FlowIssue {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let code = super::wire_name(&self.code);
        write!(formatter, "{code}: {}", self.message)?;
        if let Some(path) = &self.path {
            write!(formatter, " (at {path})")?;
        }
        Ok(())
    }
}
