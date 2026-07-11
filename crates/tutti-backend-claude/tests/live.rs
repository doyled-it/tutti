// SPDX-License-Identifier: AGPL-3.0-or-later
//! Opt-in live test: requires an authenticated `claude` on PATH. Run with:
//!   cargo test -p tutti-backend-claude --features live -- --ignored
#![cfg(feature = "live")]

use std::path::Path;
use tutti_backend_claude::ClaudeBackend;
use tutti_core::domain::{Issue, IssueId};
use tutti_core::message::{AgentTask, Role, RolePlaybook};
use tutti_core::traits::AgentBackend;

#[tokio::test]
#[ignore = "spawns a real claude process"]
async fn claude_writes_a_handoff() {
    let wt = tempfile::tempdir().unwrap();
    let backend = ClaudeBackend::default();
    let task = AgentTask {
        playbook: RolePlaybook {
            role: Role::Implementer,
            skills: vec![],
        },
        issue: Issue {
            id: IssueId(1),
            title: "write a handoff".into(),
            body: "Do nothing except emit the handoff JSON artifact as instructed.".into(),
            labels: vec![],
            milestone: None,
        },
        worktree_branch: "feat/issue-1".into(),
        model: "claude-opus-4-8".into(),
        review: None,
    };
    let (tx, mut rx) = tokio::sync::mpsc::channel(64);
    tokio::spawn(async move { while rx.recv().await.is_some() {} });
    let outcome = backend.run(task, wt.path(), tx).await.unwrap();
    // Either it produced a handoff (ReadyToShip) or cleanly reported Blocked; never panics.
    assert!(matches!(
        outcome.status,
        tutti_core::message::AgentStatus::ReadyToShip | tutti_core::message::AgentStatus::Blocked
    ));
}
