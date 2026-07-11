// SPDX-License-Identifier: AGPL-3.0-or-later
//! Pure parsers for `gh --json` output. Kept separate from the shelling so they are
//! unit-testable against captured fixtures.

use serde::Deserialize;
use tutti_core::domain::{CiState, Issue, IssueId, SelectFilter};

#[derive(Deserialize)]
struct GhLabel {
    name: String,
}
#[derive(Deserialize)]
struct GhIssue {
    number: u64,
    title: String,
    #[serde(default)]
    body: String,
    #[serde(default)]
    labels: Vec<GhLabel>,
    #[serde(default, rename = "milestone")]
    milestone: Option<GhMilestone>,
}
#[derive(Deserialize)]
struct GhMilestone {
    title: String,
}

/// Parse `gh issue list --json number,title,body,labels,milestone` output and return
/// the first issue that has `require_label` and none of `skip_labels`.
pub fn first_ready_issue(json: &str, filter: &SelectFilter) -> Option<Issue> {
    let issues: Vec<GhIssue> = serde_json::from_str(json).ok()?;
    issues.into_iter().map(to_issue).find(|i| {
        i.has_label(&filter.require_label) && !filter.skip_labels.iter().any(|s| i.has_label(s))
    })
}

fn to_issue(g: GhIssue) -> Issue {
    Issue {
        id: IssueId(g.number),
        title: g.title,
        body: g.body,
        labels: g.labels.into_iter().map(|l| l.name).collect(),
        milestone: g.milestone.map(|m| m.title),
    }
}

/// Parse the PR number from `gh pr create` output. gh prints the PR URL, sometimes
/// followed by an informational line, so take the last non-empty line and then its last
/// `/`-segment. Returns None if no line yields a numeric trailing segment.
pub fn parse_pr_number(out: &str) -> Option<u64> {
    let line = out.lines().map(str::trim).rfind(|l| !l.is_empty())?;
    line.rsplit('/').next()?.trim().parse::<u64>().ok()
}

/// Map `gh pr checks --json state` output to a single `CiState`: Fail if any failed,
/// Pending if any pending/queued, else Pass. Unknown states are treated as Pending.
pub fn overall_ci_state(json: &str) -> CiState {
    #[derive(Deserialize)]
    struct Check {
        #[serde(default)]
        state: String,
    }
    let checks: Vec<Check> = match serde_json::from_str(json) {
        Ok(c) => c,
        Err(_) => return CiState::Pending,
    };
    if checks.is_empty() {
        return CiState::Pending;
    }
    let mut any_pending = false;
    for c in &checks {
        match c.state.to_uppercase().as_str() {
            "FAILURE" | "ERROR" | "CANCELLED" | "TIMED_OUT" | "ACTION_REQUIRED" => {
                return CiState::Fail
            }
            "SUCCESS" | "NEUTRAL" | "SKIPPED" => {}
            _ => any_pending = true, // PENDING, QUEUED, IN_PROGRESS, unknown
        }
    }
    if any_pending {
        CiState::Pending
    } else {
        CiState::Pass
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn filter() -> SelectFilter {
        SelectFilter {
            require_label: "status:ready".into(),
            skip_labels: vec!["status:needs-human".into()],
        }
    }

    #[test]
    fn picks_first_ready_skipping_needs_human() {
        let json = r#"[
          {"number":1,"title":"a","body":"","labels":[{"name":"status:ready"},{"name":"status:needs-human"}],"milestone":null},
          {"number":2,"title":"b","body":"x","labels":[{"name":"status:ready"}],"milestone":{"title":"Phase 1"}}
        ]"#;
        let issue = first_ready_issue(json, &filter()).unwrap();
        assert_eq!(issue.id.0, 2);
        assert_eq!(issue.milestone.as_deref(), Some("Phase 1"));
    }

    #[test]
    fn ci_fail_wins() {
        assert_eq!(
            overall_ci_state(r#"[{"state":"SUCCESS"},{"state":"FAILURE"}]"#),
            CiState::Fail
        );
    }
    #[test]
    fn ci_pending_when_any_pending() {
        assert_eq!(
            overall_ci_state(r#"[{"state":"SUCCESS"},{"state":"PENDING"}]"#),
            CiState::Pending
        );
    }
    #[test]
    fn ci_pass_when_all_success() {
        assert_eq!(
            overall_ci_state(r#"[{"state":"SUCCESS"},{"state":"SKIPPED"}]"#),
            CiState::Pass
        );
    }
    #[test]
    fn ci_empty_is_pending() {
        assert_eq!(overall_ci_state("[]"), CiState::Pending);
    }

    #[test]
    fn pr_number_simple_url() {
        assert_eq!(
            parse_pr_number("https://github.com/o/r/pull/123"),
            Some(123)
        );
    }

    #[test]
    fn pr_number_trailing_newline() {
        assert_eq!(
            parse_pr_number("https://github.com/o/r/pull/123\n"),
            Some(123)
        );
    }

    #[test]
    fn pr_number_multiline_with_info_line() {
        // gh sometimes prints an informational line after the URL; the URL is not last.
        let out = "https://github.com/o/r/pull/456\nWarning: some notice\n";
        // The last non-empty line is not a URL, so this must fail to parse rather than
        // silently returning a wrong number.
        assert_eq!(parse_pr_number(out), None);
    }

    #[test]
    fn pr_number_url_is_last_nonempty_line() {
        let out = "Creating pull request for feat/x into main\nhttps://github.com/o/r/pull/789\n\n";
        assert_eq!(parse_pr_number(out), Some(789));
    }

    #[test]
    fn pr_number_garbage_is_none() {
        assert_eq!(parse_pr_number("not a url"), None);
    }
}
