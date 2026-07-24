// SPDX-License-Identifier: AGPL-3.0-or-later
//! Opt-in live tier for the GitLab adapter. Requires an authenticated `glab` and the
//! sandbox project. Never part of the required gate.
//!
//! Run with:
//!   cargo test -p tutti-forge-gitlab --features live -- --ignored --nocapture
#![cfg(feature = "live")]

use tutti_core::domain::{PrHandle, ShipRecord};
use tutti_core::message::NewIssue;
use tutti_core::status::StatusLabels;
use tutti_core::traits::Forge;
use tutti_forge_gitlab::GitLabForge;

fn sandbox() -> GitLabForge {
    GitLabForge {
        project: "84564301".into(),
        status_labels: StatusLabels {
            ready: "status::ready".into(),
            in_progress: "status::in-progress".into(),
            done: "status::done".into(),
        },
        repo_root: std::path::PathBuf::from("."),
    }
}

#[tokio::test]
#[ignore = "requires live gitlab.com + authenticated glab"]
async fn tracking_and_status_round_trip() {
    let f = sandbox();

    // 1. Create a milestone.
    let ms = f
        .create_milestone("live-3bglab", None, "temporary live-tier milestone")
        .await
        .expect("create milestone");

    // 2. Create an issue under it (carries status::ready).
    let issue = f
        .create_issue(
            &NewIssue {
                title: "live-3bglab issue".into(),
                body: "temporary".into(),
                labels: vec!["status::ready".into()],
            },
            Some(ms.id),
            None,
        )
        .await
        .expect("create issue");

    // 3. Claim it: status flips to in-progress via scoped-label add+remove.
    let _guard = f.claim(issue.id).await.expect("claim");

    // 4. The milestone now has this child.
    let children = f.milestone_children(ms.id).await.expect("children");
    assert!(children.iter().any(|c| c.id == issue.id));

    // 5. Record it done. `ShipRecord` has no `Default`, so build it directly; the PR
    // fields are unused by the GitLab adapter's `record` (it only flips the status
    // label), but a real-looking handle keeps the test honest about the shape.
    let outcome = ShipRecord {
        pr: PrHandle {
            number: 0,
            branch: "live-3bglab".into(),
        },
        decision_note: Some("live tier round-trip".into()),
    };
    f.record(issue.id, &outcome).await.expect("record");

    // 6. Close the milestone.
    f.close_milestone(ms.id).await.expect("close milestone");

    // Epic degradation: the sandbox project is in a USER namespace (no group), so
    // create_epic must return Unsupported rather than erroring some other way. This is
    // the live-validatable half of the epic path; the create/link/list success path
    // runs against a real Premium group.
    let epic_err = f.create_epic("live-epic", "x").await;
    assert!(
        matches!(
            epic_err,
            Err(tutti_core::traits::EngineError::Unsupported(_))
        ),
        "expected Unsupported for epics without a group, got {epic_err:?}"
    );
    // list_epics degrades to an empty list, never an error.
    assert!(f.list_epics().await.expect("list_epics ok").is_empty());

    // Cleanup: close the throwaway issue so the sandbox stays tidy.
    // (Milestone is already closed; leaving it closed is fine.)
    let _ = std::process::Command::new("glab")
        .args([
            "api",
            "-X",
            "PUT",
            &format!("projects/84564301/issues/{}", issue.id.0),
            "-f",
            "state_event=close",
        ])
        .output();
}

#[tokio::test]
#[ignore = "hits the real glab CLI"]
async fn browse_lists_own_namespace_and_sandbox() {
    use tutti_core::browse::ForgeBrowser;
    use tutti_forge_gitlab::GitLabBrowser;
    let b = GitLabBrowser;
    let ns = b.list_namespaces().await.unwrap();
    let own = ns.first().expect("user namespace");
    let repos = b.list_repos(own).await.unwrap();
    assert!(repos
        .iter()
        .any(|r| r.full_path == "doyled-it/tutti-glab-sandbox"));
}

#[tokio::test]
#[ignore = "creates a real project on gitlab.com under doyled-it"]
async fn create_repo_makes_a_cloneable_project() {
    use tutti_core::browse::{ForgeBrowser, NamespaceKind, NewRepo};
    use tutti_forge_gitlab::{parse_user_namespace, GitLabBrowser};

    let name = format!("tutti-create-test-{}", std::process::id());

    // The user's own namespace: path is the numeric id (create under the user needs no
    // namespace_id, but we resolve the namespace the same way the app would).
    let user_json = std::process::Command::new("glab")
        .args(["api", "user"])
        .output()
        .expect("glab api user");
    let ns = parse_user_namespace(&String::from_utf8_lossy(&user_json.stdout)).expect("parse user");
    assert_eq!(ns.kind, NamespaceKind::User);

    let b = GitLabBrowser;
    let spec = NewRepo {
        name: name.clone(),
        description: Some("tutti live create test".into()),
        private: true,
    };
    let repo = b.create_repo(&ns, &spec).await.expect("create_repo");
    let full = repo.full_path.clone();
    assert!(repo.private);
    assert!(full.ends_with(&name));

    struct RepoCleanup(String);
    impl Drop for RepoCleanup {
        fn drop(&mut self) {
            let encoded = self.0.replace('/', "%2F");
            let endpoint = format!("projects/{encoded}");
            let _ = std::process::Command::new("glab")
                .args(["api", "-X", "DELETE", &endpoint])
                .output();
        }
    }
    let _cleanup = RepoCleanup(full.clone());

    let dir = std::env::temp_dir().join(&name);
    let _ = std::fs::remove_dir_all(&dir);
    let out = std::process::Command::new("git")
        .args(["clone", &repo.clone_url, dir.to_str().unwrap()])
        .output()
        .expect("git clone");
    assert!(
        out.status.success(),
        "clone failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        dir.join("README.md").exists(),
        "initialize_with_readme should create a README"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
