// SPDX-License-Identifier: AGPL-3.0-or-later
//! The forge decomposer: a `BacklogPlan` data model (the milestone/epic/issue tree a Score
//! design proposes), a deterministic human-readable renderer for the propose -> review step,
//! and an idempotent seeder that creates the tree through `tutti-core`'s `Forge` seam.
//!
//! Generating a `BacklogPlan` from design artifacts (the story map, the Decompose movement)
//! is out of scope here; this module starts from an already-built plan.

use serde::{Deserialize, Serialize};

/// A proposed issue in the backlog. `acceptance` holds EARS-style lines; `deps` names
/// prerequisite issues by title (advisory ordering, rendered into the body).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProposedIssue {
    pub title: String,
    pub body: String,
    #[serde(default)]
    pub labels: Vec<String>,
    #[serde(default)]
    pub acceptance: Vec<String>,
    #[serde(default)]
    pub deps: Vec<String>,
}

/// A proposed epic grouping issues.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProposedEpic {
    pub title: String,
    pub body: String,
    pub issues: Vec<ProposedIssue>,
}

/// A proposed milestone.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProposedMilestone {
    pub title: String,
    #[serde(default)]
    pub due: Option<String>,
    #[serde(default)]
    pub description: String,
}

/// The whole proposed backlog. `milestone` is an optional title+due; epics group issues;
/// `loose_issues` are issues with no epic.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BacklogPlan {
    #[serde(default)]
    pub milestone: Option<ProposedMilestone>,
    #[serde(default)]
    pub epics: Vec<ProposedEpic>,
    #[serde(default)]
    pub loose_issues: Vec<ProposedIssue>,
}

impl BacklogPlan {
    /// Total number of issues across epics and loose issues.
    pub fn issue_count(&self) -> usize {
        self.epics.iter().map(|e| e.issues.len()).sum::<usize>() + self.loose_issues.len()
    }
}

/// A stable idempotency marker for a proposed issue, embedded in the seeded body so a
/// re-run can recognize an already-created issue. Derived deterministically from the title
/// (lowercased, non-alphanumerics collapsed to single hyphens, trimmed).
pub fn issue_marker(title: &str) -> String {
    let mut slug = String::with_capacity(title.len());
    let mut last_was_hyphen = false;
    for ch in title.chars() {
        let lower = ch.to_ascii_lowercase();
        if lower.is_ascii_alphanumeric() {
            slug.push(lower);
            last_was_hyphen = false;
        } else if !last_was_hyphen {
            slug.push('-');
            last_was_hyphen = true;
        }
    }
    let trimmed = slug.trim_matches('-');
    format!("score:{trimmed}")
}

/// The HTML comment that carries the marker in a rendered issue body.
fn marker_comment(marker: &str) -> String {
    format!("<!-- tutti-score-marker: {marker} -->")
}

/// The exact body that will be written for a proposed issue: the author body, then the
/// EARS acceptance criteria, then a dependencies note (if any), then the hidden marker
/// comment. Deterministic (a re-run produces the identical body).
pub fn render_issue_body(issue: &ProposedIssue) -> String {
    let marker = issue_marker(&issue.title);
    let mut out = issue.body.clone();

    if !issue.acceptance.is_empty() {
        out.push_str("\n\n## Acceptance\n");
        for line in &issue.acceptance {
            out.push_str("- ");
            out.push_str(line);
            out.push('\n');
        }
        // Drop the trailing newline the loop just added; the next section (or the
        // marker) adds its own leading blank line.
        out.pop();
    }

    if !issue.deps.is_empty() {
        out.push_str("\n\nDepends on: ");
        out.push_str(&issue.deps.join(", "));
    }

    out.push_str("\n\n");
    out.push_str(&marker_comment(&marker));

    out
}

/// A human-readable review of the whole plan (what will be created), for the
/// propose -> review step. Deterministic.
pub fn render_plan(plan: &BacklogPlan) -> String {
    let mut out = String::new();
    out.push_str("# Backlog plan\n");

    if let Some(m) = &plan.milestone {
        out.push_str("\nMilestone: ");
        out.push_str(&m.title);
        if let Some(due) = &m.due {
            out.push_str(" (due ");
            out.push_str(due);
            out.push(')');
        }
        out.push('\n');
    }

    for epic in &plan.epics {
        out.push_str("\nEpic: ");
        out.push_str(&epic.title);
        out.push('\n');
        for issue in &epic.issues {
            out.push_str("  - ");
            out.push_str(&issue.title);
            out.push_str(" (");
            out.push_str(&issue.acceptance.len().to_string());
            out.push_str(" acceptance criteria)\n");
        }
    }

    if !plan.loose_issues.is_empty() {
        out.push_str("\nLoose issues:\n");
        for issue in &plan.loose_issues {
            out.push_str("  - ");
            out.push_str(&issue.title);
            out.push_str(" (");
            out.push_str(&issue.acceptance.len().to_string());
            out.push_str(" acceptance criteria)\n");
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn issue(title: &str) -> ProposedIssue {
        ProposedIssue {
            title: title.to_string(),
            body: "Do the thing.".to_string(),
            labels: vec![],
            acceptance: vec![],
            deps: vec![],
        }
    }

    #[test]
    fn issue_marker_is_stable_and_slugified() {
        assert_eq!(
            issue_marker("E6: Forge Decomposer!"),
            "score:e6-forge-decomposer"
        );
    }

    #[test]
    fn render_issue_body_includes_acceptance_deps_and_marker() {
        let mut i = issue("Wire the seeder");
        i.acceptance = vec!["WHEN seed runs THEN issues are created".to_string()];
        i.deps = vec!["Model the plan".to_string(), "Render the plan".to_string()];

        let body = render_issue_body(&i);

        assert!(body.contains("## Acceptance"));
        assert!(body.contains("- WHEN seed runs THEN issues are created"));
        assert!(body.contains("Depends on: Model the plan, Render the plan"));
        assert!(body.contains(&marker_comment(&issue_marker(&i.title))));
    }

    #[test]
    fn render_issue_body_is_deterministic() {
        let i = issue("Some issue");
        assert_eq!(render_issue_body(&i), render_issue_body(&i));
    }

    #[test]
    fn render_plan_lists_milestone_epics_and_issues() {
        let plan = BacklogPlan {
            milestone: Some(ProposedMilestone {
                title: "v0.1".to_string(),
                due: Some("2026-09-01".to_string()),
                description: String::new(),
            }),
            epics: vec![ProposedEpic {
                title: "E6".to_string(),
                body: String::new(),
                issues: vec![issue("Issue A"), issue("Issue B")],
            }],
            loose_issues: vec![issue("Loose Issue")],
        };

        let rendered = render_plan(&plan);

        assert!(rendered.contains("v0.1"));
        assert!(rendered.contains("E6"));
        assert!(rendered.contains("Issue A"));
        assert!(rendered.contains("Issue B"));
        assert!(rendered.contains("Loose Issue"));
    }

    #[test]
    fn issue_count_sums_epics_and_loose() {
        let plan = BacklogPlan {
            milestone: None,
            epics: vec![ProposedEpic {
                title: "E6".to_string(),
                body: String::new(),
                issues: vec![issue("Issue A"), issue("Issue B")],
            }],
            loose_issues: vec![issue("Loose Issue")],
        };
        assert_eq!(plan.issue_count(), 3);
    }
}
