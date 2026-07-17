// SPDX-License-Identifier: AGPL-3.0-or-later
//! Opt-in live test: requires an authenticated `gh` on PATH and write access to the
//! throwaway sandbox repo `doyled-it/tutti-live-sandbox`. Run with:
//!   cargo test -p tutti-forge-github --features live -- --ignored
#![cfg(feature = "live")]

use tutti_core::message::NewIssue;
use tutti_core::tracking::TrackState;
use tutti_core::traits::Forge;
use tutti_forge_github::GitHubForge;

const SANDBOX_REPO: &str = "doyled-it/tutti-live-sandbox";

/// Deletes the throwaway issue and milestone this test created, even if an assertion
/// panics first. `gh` has no async client here, so cleanup runs synchronously in
/// `Drop`; the default (non-`abort`) panic strategy still unwinds through it.
struct Cleanup {
    repo: String,
    issue_number: Option<u64>,
    milestone_number: Option<u64>,
}

impl Drop for Cleanup {
    fn drop(&mut self) {
        if let Some(n) = self.issue_number {
            // `deleteIssue` needs the issue's GraphQL node id, not its number.
            let node_id = std::process::Command::new("gh")
                .args([
                    "issue",
                    "view",
                    &n.to_string(),
                    "--repo",
                    &self.repo,
                    "--json",
                    "id",
                    "--jq",
                    ".id",
                ])
                .output();
            if let Ok(out) = node_id {
                let id = String::from_utf8_lossy(&out.stdout).trim().to_string();
                if !id.is_empty() {
                    let _ = std::process::Command::new("gh")
                        .args([
                            "api",
                            "graphql",
                            "-f",
                            "query=mutation($id:ID!){deleteIssue(input:{issueId:$id}){clientMutationId}}",
                            "-f",
                            &format!("id={id}"),
                        ])
                        .output();
                }
            }
        }
        if let Some(m) = self.milestone_number {
            let endpoint = format!("repos/{}/milestones/{}", self.repo, m);
            let _ = std::process::Command::new("gh")
                .args(["api", "--method", "DELETE", &endpoint])
                .output();
        }
    }
}

#[tokio::test]
#[ignore = "hits the real GitHub API against a throwaway sandbox repo"]
async fn milestone_and_issue_tracking_round_trip() {
    let forge = GitHubForge {
        repo: SANDBOX_REPO.to_string(),
        status_labels: tutti_core::status::StatusLabels {
            ready: "status:ready".into(),
            in_progress: "status:in-progress".into(),
            done: "status:done".into(),
        },
        repo_root: std::path::PathBuf::from("."),
    };

    let mut cleanup = Cleanup {
        repo: SANDBOX_REPO.to_string(),
        issue_number: None,
        milestone_number: None,
    };

    let milestone = forge
        .create_milestone("live-tier-test", None, "temp")
        .await
        .expect("create_milestone");
    assert_ne!(milestone.id.0, 0, "created milestone should have a real id");
    cleanup.milestone_number = Some(milestone.id.0);

    let new_issue = NewIssue {
        title: "live tracking child".to_string(),
        body: "temp".to_string(),
        labels: vec!["status:ready".to_string()],
    };
    let issue = forge
        .create_issue(&new_issue, Some(milestone.id), None)
        .await
        .expect("create_issue");
    cleanup.issue_number = Some(issue.id.0);
    assert_eq!(
        issue.milestone.as_deref(),
        Some("live-tier-test"),
        "created issue should carry the milestone it was filed under"
    );

    // GitHub's issues-by-milestone list index lags a second or two behind issue
    // creation: the issue object carries the milestone instantly, but the list query
    // does not reflect it immediately. Poll briefly so the test matches real API
    // behavior. Production auto-close reads children after SHIPPING an already-indexed
    // issue, so it never hits this lag.
    let mut children = Vec::new();
    for _ in 0..8 {
        children = forge
            .milestone_children(milestone.id)
            .await
            .expect("milestone_children");
        if children.iter().any(|i| i.id == issue.id) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(1500));
    }
    assert!(
        children.iter().any(|i| i.id == issue.id),
        "milestone_children should include the issue created under it (after index settle)"
    );

    forge
        .close_milestone(milestone.id)
        .await
        .expect("close_milestone");

    let milestones = forge.list_milestones().await.expect("list_milestones");
    let closed = milestones
        .iter()
        .find(|m| m.id == milestone.id)
        .expect("closed milestone should still be listed with state=all");
    assert_eq!(closed.state, TrackState::Closed);

    // `cleanup` drops here, deleting the throwaway issue and milestone.
}
