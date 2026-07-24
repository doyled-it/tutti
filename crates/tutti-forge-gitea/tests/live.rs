// SPDX-License-Identifier: AGPL-3.0-or-later
//! Opt-in live tier for the Gitea adapter. Requires an authenticated `tea` login and a
//! real Codeberg sandbox. Never part of the required gate.
//!
//! Run with:
//!   cargo test -p tutti-forge-gitea --features live -- --ignored --nocapture
#![cfg(feature = "live")]

use tutti_core::domain::{PrHandle, ShipRecord};
use tutti_core::message::NewIssue;
use tutti_core::status::StatusLabels;
use tutti_core::traits::Forge;
use tutti_forge_gitea::GiteaForge;

fn sandbox() -> GiteaForge {
    GiteaForge {
        repo: "workslocally/tutti-tea-sandbox".into(),
        login: "icesight-engine".into(),
        status_labels: StatusLabels::default(),
        repo_root: std::path::PathBuf::from("."),
    }
}

#[tokio::test]
#[ignore = "requires live Codeberg + authenticated tea"]
async fn tracking_and_status_round_trip() {
    let f = sandbox();

    // 1. Create a milestone.
    let ms = f
        .create_milestone("live-3btea", None, "temporary live-tier milestone")
        .await
        .expect("create milestone");

    // 2. Create an issue under it (carries status:ready).
    let issue = f
        .create_issue(
            &NewIssue {
                title: "live-3btea issue".into(),
                body: "temporary".into(),
                labels: vec!["status:ready".into()],
            },
            Some(ms.id),
            None,
        )
        .await
        .expect("create issue");

    // 3. Claim it: status flips to in-progress.
    let _guard = f.claim(issue.id).await.expect("claim");

    // 4. The milestone now has this child.
    let children = f.milestone_children(ms.id).await.expect("children");
    assert!(children.iter().any(|c| c.id == issue.id));

    // 5. Record it done. `ShipRecord` has no `Default`, so build it directly; the PR
    // fields are unused by the Gitea adapter's `record` (it only flips the status
    // label), but a real-looking handle keeps the test honest about the shape.
    let outcome = ShipRecord {
        pr: PrHandle {
            number: 0,
            branch: "live-3btea".into(),
        },
        decision_note: Some("live tier round-trip".into()),
    };
    f.record(issue.id, &outcome).await.expect("record");

    // 6. Close the milestone.
    f.close_milestone(ms.id).await.expect("close milestone");

    // Cleanup: close the issue so the sandbox stays tidy.
    // (Milestone is already closed; leaving it closed is fine.)
    let _ = std::process::Command::new("tea")
        .args([
            "api",
            "--login",
            "icesight-engine",
            "-X",
            "PATCH",
            &format!("repos/workslocally/tutti-tea-sandbox/issues/{}", issue.id.0),
            "-d",
            "{\"state\":\"closed\"}",
        ])
        .output();
}

#[tokio::test]
#[ignore = "hits the real tea CLI, login doyled-it"]
async fn browse_lists_own_namespace() {
    use tutti_core::browse::ForgeBrowser;
    use tutti_forge_gitea::GiteaBrowser;
    let b = GiteaBrowser {
        login: "doyled-it".into(),
    };
    let ns = b.list_namespaces().await.unwrap();
    assert!(ns.iter().any(|n| n.path == "doyled-it"));
}

#[tokio::test]
#[ignore = "creates a real repo on Codeberg under doyled-it"]
async fn create_repo_makes_a_cloneable_repo() {
    use tutti_core::browse::{ForgeBrowser, Namespace, NamespaceKind, NewRepo};
    use tutti_forge_gitea::GiteaBrowser;

    // The tea login alias (from `tea login list`). Defaults to "codeberg"; override with
    // TUTTI_TEA_LOGIN if yours differs. The namespace is doyled-it regardless.
    let login = std::env::var("TUTTI_TEA_LOGIN").unwrap_or_else(|_| "codeberg".into());
    let name = format!("tutti-create-test-{}", std::process::id());
    let full = format!("doyled-it/{name}");

    struct RepoCleanup {
        login: String,
        full: String,
    }
    impl Drop for RepoCleanup {
        fn drop(&mut self) {
            let endpoint = format!("repos/{}", self.full);
            let _ = std::process::Command::new("tea")
                .args(["api", "--login", &self.login, "-X", "DELETE", &endpoint])
                .output();
        }
    }
    let _cleanup = RepoCleanup {
        login: login.clone(),
        full: full.clone(),
    };

    let b = GiteaBrowser {
        login: login.clone(),
    };
    let ns = Namespace {
        path: "doyled-it".into(),
        name: "doyled-it".into(),
        kind: NamespaceKind::User,
    };
    let spec = NewRepo {
        name: name.clone(),
        description: Some("tutti live create test".into()),
        private: true,
    };
    let repo = b.create_repo(&ns, &spec).await.expect("create_repo");
    assert_eq!(repo.full_path, full);
    assert!(repo.private);

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
        "auto-init should create a README"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
