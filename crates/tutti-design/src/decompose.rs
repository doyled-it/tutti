// SPDX-License-Identifier: AGPL-3.0-or-later
//! The forge decomposer: a `BacklogPlan` data model (the milestone/epic/issue tree a Score
//! design proposes), a deterministic human-readable renderer for the propose -> review step,
//! and an idempotent seeder that creates the tree through `tutti-core`'s `Forge` seam.
//!
//! Generating a `BacklogPlan` from design artifacts (the story map, the Decompose movement)
//! is out of scope here; this module starts from an already-built plan.

use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use tutti_core::message::NewIssue;
use tutti_core::traits::{EngineError, Forge};

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

/// What a seed run did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeedReport {
    /// Titles of issues newly created this run.
    pub created: Vec<String>,
    /// Titles of issues skipped because their marker already existed on the forge.
    pub skipped: Vec<String>,
}

/// Wrap an `EngineError` from a `Forge` call into a `DesignError::Forge`.
fn wrap(e: EngineError) -> crate::error::DesignError {
    crate::error::DesignError::Forge(e.to_string())
}

/// The marker prefix, as it appears embedded in a rendered body.
const MARKER_PREFIX: &str = "<!-- tutti-score-marker: ";
const MARKER_SUFFIX: &str = " -->";

/// Pull the `score:...` marker out of an issue body, if it carries one.
fn extract_marker(body: &str) -> Option<String> {
    let start = body.find(MARKER_PREFIX)? + MARKER_PREFIX.len();
    let rest = &body[start..];
    let end = rest.find(MARKER_SUFFIX)?;
    Some(rest[..end].to_string())
}

/// Build the `NewIssue` a proposed issue seeds as, with `ready_label` folded into its
/// labels (deduplicated against any author-supplied copy).
fn new_issue_for(issue: &ProposedIssue, ready_label: &str) -> NewIssue {
    let mut labels = issue.labels.clone();
    if !labels.iter().any(|l| l == ready_label) {
        labels.push(ready_label.to_string());
    }
    NewIssue {
        title: issue.title.clone(),
        body: render_issue_body(issue),
        labels,
        milestone: None,
        epic: None,
    }
}

/// Create the plan on the forge, idempotently. `ready_label` is applied to every created
/// leaf issue (e.g. "status:ready") so the drain engine can select it; the label is created
/// first if absent (diffed against list_labels). A milestone and epics are created (or
/// reused by title if one with the same title already exists). Each issue's marker is
/// checked against existing issue bodies (`Forge::list_issues`); an issue whose marker is
/// already present is skipped, not recreated.
///
/// Returns a `SeedReport`. Any `Forge` error is wrapped in `DesignError::Forge`.
pub async fn seed(
    plan: &BacklogPlan,
    forge: &dyn Forge,
    ready_label: &str,
) -> Result<SeedReport, crate::error::DesignError> {
    let mut report = SeedReport {
        created: Vec::new(),
        skipped: Vec::new(),
    };

    // Ensure the ready label exists before anything is created.
    let labels = forge.list_labels().await.map_err(wrap)?;
    if !labels.iter().any(|(name, _)| name == ready_label) {
        forge
            .create_label(ready_label, "ededed")
            .await
            .map_err(wrap)?;
    }

    // Reuse a milestone with the same title, or create one.
    let milestone_id = match &plan.milestone {
        Some(m) => {
            let existing = forge.list_milestones().await.map_err(wrap)?;
            let id = match existing.into_iter().find(|x| x.title == m.title) {
                Some(found) => found.id,
                None => {
                    forge
                        .create_milestone(&m.title, m.due.as_deref(), &m.description)
                        .await
                        .map_err(wrap)?
                        .id
                }
            };
            Some(id)
        }
        None => None,
    };

    // Markers already present on the forge, so a re-run can recognize its own prior work.
    let existing_markers: HashSet<String> = forge
        .list_issues()
        .await
        .map_err(wrap)?
        .iter()
        .filter_map(|i| extract_marker(&i.body))
        .collect();

    let existing_epics = forge.list_epics().await.map_err(wrap)?;

    for epic in &plan.epics {
        let epic_id = match existing_epics.iter().find(|e| e.title == epic.title) {
            Some(found) => found.id,
            None => {
                forge
                    .create_epic(&epic.title, &epic.body)
                    .await
                    .map_err(wrap)?
                    .id
            }
        };

        for issue in &epic.issues {
            if existing_markers.contains(&issue_marker(&issue.title)) {
                report.skipped.push(issue.title.clone());
                continue;
            }
            let new = new_issue_for(issue, ready_label);
            forge
                .create_issue(&new, milestone_id, Some(epic_id))
                .await
                .map_err(wrap)?;
            report.created.push(issue.title.clone());
        }
    }

    for issue in &plan.loose_issues {
        if existing_markers.contains(&issue_marker(&issue.title)) {
            report.skipped.push(issue.title.clone());
            continue;
        }
        let new = new_issue_for(issue, ready_label);
        forge
            .create_issue(&new, milestone_id, None)
            .await
            .map_err(wrap)?;
        report.created.push(issue.title.clone());
    }

    Ok(report)
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

    fn sample_plan() -> BacklogPlan {
        BacklogPlan {
            milestone: None,
            epics: vec![ProposedEpic {
                title: "E6".to_string(),
                body: "The forge decomposer.".to_string(),
                issues: vec![issue("Issue A"), issue("Issue B")],
            }],
            loose_issues: vec![issue("Loose Issue")],
        }
    }

    mod seeding {
        use super::*;
        use tutti_core::domain::CiState;
        use tutti_core::testing::fake_forge::FakeForge;

        #[tokio::test]
        async fn seed_creates_the_whole_tree() {
            let forge = FakeForge::new(vec![], CiState::Pass);
            let plan = sample_plan();

            let report = seed(&plan, &forge, "status:ready").await.unwrap();

            assert_eq!(report.created.len(), 3);
            assert!(report.created.contains(&"Issue A".to_string()));
            assert!(report.created.contains(&"Issue B".to_string()));
            assert!(report.created.contains(&"Loose Issue".to_string()));
            assert!(report.skipped.is_empty());

            let issues = forge.list_issues().await.unwrap();
            assert_eq!(issues.len(), 3);
            for created in &issues {
                assert!(created.labels.contains(&"status:ready".to_string()));
            }
        }

        #[tokio::test]
        async fn seed_is_idempotent() {
            let forge = FakeForge::new(vec![], CiState::Pass);
            let plan = sample_plan();

            seed(&plan, &forge, "status:ready").await.unwrap();
            let before = forge.list_issues().await.unwrap().len();

            let second = seed(&plan, &forge, "status:ready").await.unwrap();

            assert!(second.created.is_empty());
            assert_eq!(second.skipped.len(), plan.issue_count());
            let after = forge.list_issues().await.unwrap().len();
            assert_eq!(before, after);
        }

        #[tokio::test]
        async fn seed_creates_the_ready_label_when_absent() {
            let forge = FakeForge::new(vec![], CiState::Pass);
            let plan = BacklogPlan {
                milestone: None,
                epics: vec![],
                loose_issues: vec![issue("Solo issue")],
            };

            let report = seed(&plan, &forge, "status:ready").await.unwrap();

            assert_eq!(report.created, vec!["Solo issue".to_string()]);
            // FakeForge's list_labels/create_label are inert stubs (they never persist
            // state; a real adapter does), so the only observable proof that the
            // create-if-absent path ran without error is that the created issue itself
            // carries the label the seeder just ensured exists.
            let issues = forge.list_issues().await.unwrap();
            assert!(issues[0].labels.contains(&"status:ready".to_string()));
        }

        #[tokio::test]
        async fn seed_applies_author_labels_plus_ready_label() {
            let forge = FakeForge::new(vec![], CiState::Pass);
            let mut tagged = issue("Tagged issue");
            tagged.labels = vec!["enhancement".to_string()];
            let plan = BacklogPlan {
                milestone: None,
                epics: vec![],
                loose_issues: vec![tagged],
            };

            seed(&plan, &forge, "status:ready").await.unwrap();

            let issues = forge.list_issues().await.unwrap();
            let created = &issues[0];
            assert!(created.labels.contains(&"enhancement".to_string()));
            assert!(created.labels.contains(&"status:ready".to_string()));
        }

        #[tokio::test]
        async fn seed_reuses_an_existing_milestone_by_title() {
            let forge = FakeForge::new(vec![], CiState::Pass);
            forge.create_milestone("v0.1", None, "").await.unwrap();

            let plan = BacklogPlan {
                milestone: Some(ProposedMilestone {
                    title: "v0.1".to_string(),
                    due: None,
                    description: String::new(),
                }),
                epics: vec![],
                loose_issues: vec![issue("Some issue")],
            };

            seed(&plan, &forge, "status:ready").await.unwrap();

            let milestones = forge.list_milestones().await.unwrap();
            assert_eq!(milestones.len(), 1);
        }
    }
}
