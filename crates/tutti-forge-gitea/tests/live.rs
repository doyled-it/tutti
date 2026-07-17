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
            &format!(
                "repos/workslocally/tutti-tea-sandbox/issues/{}",
                issue.id.0
            ),
            "-d",
            "{\"state\":\"closed\"}",
        ])
        .output();
}
