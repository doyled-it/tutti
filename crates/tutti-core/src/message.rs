// SPDX-License-Identifier: AGPL-3.0-or-later
//! The typed artifacts that flow between stages and backends.

use crate::domain::{BranchPlan, Issue, IssueId};
use serde::{Deserialize, Serialize};

/// A stage's role in the loop. Selects which skills the backend activates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Implementer,
    Reviewer,
    FixApplier,
    Planner,
    Greener,
}

/// The resolved role -> skills mapping handed to a backend for one run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RolePlaybook {
    pub role: Role,
    pub skills: Vec<String>,
}

/// One headless task for a backend.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentTask {
    pub playbook: RolePlaybook,
    pub issue: Issue,
    /// The feature branch the agent works on inside the worktree.
    pub worktree_branch: String,
    pub model: String,
    /// The prior `ReviewReport` carried into the fix-applier stage (its findings to act on).
    pub review: Option<ReviewReport>,
    /// MCP servers to wire into the invocation (e.g. codegraph). Empty = none, which is
    /// the default and preserves prior behavior.
    #[serde(default)]
    pub mcp_servers: Vec<crate::mcp::McpServer>,
}

/// Streamed progress from a running agent. Drives logs now, the UI later.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AgentEvent {
    Line(String),
    ToolUse(String),
    Done,
}

/// Terminal status of an agent run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AgentStatus {
    ReadyToShip,
    Blocked,
    Error,
}

/// Token/cost accounting for one run.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
}

/// What an agent run produced.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentOutcome {
    pub status: AgentStatus,
    /// Present when an implement/apply stage is ready to ship.
    pub handoff: Option<Handoff>,
    /// Present when a review stage ran.
    pub review: Option<ReviewReport>,
    /// Present when a Planner stage ran and produced a decision (carried back as a
    /// `.tutti/plan.json` artifact, mirroring the handoff/review protocol).
    pub plan: Option<PlanDecision>,
    pub summary: String,
    pub usage: Usage,
    /// Human-readable reason when `status == Blocked`.
    pub blocked_reason: Option<String>,
}

/// The contract from a creative stage to the mechanical executor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Handoff {
    pub issue: IssueId,
    pub branch: String,
    pub target: BranchPlan,
    pub pr_title: String,
    pub pr_body: String,
    pub labels: Vec<String>,
    pub decision_note: Option<String>,
}

/// Severity of a review finding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Blocking,
    Major,
    Minor,
}

/// One reviewer finding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Finding {
    pub severity: Severity,
    pub file: String,
    pub line: Option<u64>,
    pub claim: String,
}

/// The reviewer's overall call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Verdict {
    Approve,
    RequestChanges,
}

/// Output of the review stage.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewReport {
    pub findings: Vec<Finding>,
    pub verdict: Verdict,
}

impl ReviewReport {
    /// True when at least one finding must be fixed before merge.
    pub fn needs_fixes(&self) -> bool {
        self.verdict == Verdict::RequestChanges
            || self
                .findings
                .iter()
                .any(|f| f.severity == Severity::Blocking)
    }

    /// True when the report has any finding that MUST be resolved before shipping. Minor
    /// findings are deliberately excluded: they are cleared best-effort but never gate the
    /// ship or extend the review loop (the verdict field is advisory; severity is the gate).
    pub fn has_blocking_or_major(&self) -> bool {
        self.findings
            .iter()
            .any(|f| matches!(f.severity, Severity::Blocking | Severity::Major))
    }
}

/// A new issue the planner proposes creating.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewIssue {
    pub title: String,
    pub body: String,
    pub labels: Vec<String>,
    /// Where the planner wants this issue filed, named by title. Titles rather than forge
    /// ids because the planner only ever sees titles (in the tracking snapshot); it has no
    /// way to know a `MilestoneId`. A title the engine cannot resolve is dropped back to
    /// top-level placement, never treated as a reason to skip creating the issue.
    /// `#[serde(default)]` so a planner that omits the field still parses.
    #[serde(default)]
    pub milestone: Option<String>,
    /// As `milestone`, for the epic. The tracking snapshot does not list epics yet (the
    /// forge's `list_epics` is an N+1), so in practice the planner rarely sets this; the
    /// path is here so it works the moment the snapshot does carry them.
    #[serde(default)]
    pub epic: Option<String>,
}

/// What the planner wants to do next. The engine whitelists which are auto-executed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PlanAction {
    NextIssue,
    CreateIssues(Vec<NewIssue>),
    CloseMilestone(String),
    Stop,
}

/// The planner's proposal after an issue ships.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanDecision {
    pub action: PlanAction,
    pub rationale: String,
    pub needs_human: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn review_with_blocking_finding_needs_fixes() {
        let report = ReviewReport {
            findings: vec![Finding {
                severity: Severity::Blocking,
                file: "a.rs".into(),
                line: Some(3),
                claim: "off-by-one".into(),
            }],
            verdict: Verdict::Approve, // even on Approve, a blocking finding forces fixes
        };
        assert!(report.needs_fixes());
    }

    #[test]
    fn clean_approve_needs_no_fixes() {
        let report = ReviewReport {
            findings: vec![],
            verdict: Verdict::Approve,
        };
        assert!(!report.needs_fixes());
    }

    #[test]
    fn major_finding_is_blocking_or_major() {
        let report = ReviewReport {
            findings: vec![Finding {
                severity: Severity::Major,
                file: "a.rs".into(),
                line: Some(3),
                claim: "wrong condition".into(),
            }],
            verdict: Verdict::Approve,
        };
        assert!(report.has_blocking_or_major());
    }

    #[test]
    fn minor_only_finding_is_not_blocking_or_major() {
        let report = ReviewReport {
            findings: vec![Finding {
                severity: Severity::Minor,
                file: "a.rs".into(),
                line: None,
                claim: "missing coverage note".into(),
            }],
            verdict: Verdict::RequestChanges,
        };
        assert!(!report.has_blocking_or_major());
    }

    #[test]
    fn empty_report_is_not_blocking_or_major() {
        let report = ReviewReport {
            findings: vec![],
            verdict: Verdict::Approve,
        };
        assert!(!report.has_blocking_or_major());
    }
}
