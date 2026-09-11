// SPDX-License-Identifier: AGPL-3.0-or-later
//! End-to-end: the conventions skill actually reaches an agent's prompt. This is the one
//! test that crosses every crate boundary the feature spans (the provider in
//! `tutti-app-core`, the language detection in its retrofit module, the `ConventionsProvider`
//! seam and `AgentTask` in `tutti-core`, and `build_prompt` in `tutti-backend-claude`). Each
//! crate's own tests see only one side of that chain; this proves the whole path.

use tutti_app_core::ConventionsSkill;
use tutti_backend_claude::prompt::build_prompt;
use tutti_core::conventions::ConventionsProvider;
use tutti_core::domain::{Issue, IssueId, IssueState};
use tutti_core::message::{AgentTask, Role, RolePlaybook};

fn task_with(role: Role, skill_preamble: Option<String>) -> AgentTask {
    AgentTask {
        playbook: RolePlaybook {
            role,
            skills: vec![],
        },
        issue: Issue {
            id: IssueId(1),
            title: "Do the thing".into(),
            body: "details".into(),
            labels: vec![],
            milestone: None,
            state: IssueState::Open,
        },
        worktree_branch: "feat/x".into(),
        model: "m".into(),
        review: None,
        mcp_servers: vec![],
        skill_preamble,
    }
}

#[test]
fn reviewer_prompt_carries_the_conventions_for_a_rust_worktree() {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(d.path().join("Cargo.toml"), "[package]\nname='x'\n").unwrap();

    // The provider resolves the preamble for the detected language.
    let preamble = ConventionsSkill.preamble_for(Role::Reviewer, d.path());
    assert!(preamble.is_some(), "rust worktree yields a preamble");

    // That preamble reaches the built reviewer prompt, and the reviewer's convention clause
    // (which is conditional on an injected reference) is active.
    let task = task_with(Role::Reviewer, preamble);
    let prompt = build_prompt(&task, std::path::Path::new("/wt/.tutti/review.json"));
    assert!(prompt.contains("Rust conventions"), "prompt carries the reference");
    assert!(
        prompt.contains("conventions reference above"),
        "the reviewer is pointed at the injected reference"
    );
    assert!(
        prompt.contains("tagged advisory as Minor"),
        "the reviewer is given the severity-tag contract"
    );
}

#[test]
fn implementer_prompt_carries_both_references_for_a_polyglot_worktree() {
    let d = tempfile::tempdir().unwrap();
    std::fs::write(d.path().join("Cargo.toml"), "[package]\nname='x'\n").unwrap();
    std::fs::write(d.path().join("go.mod"), "module example.com/x\n\ngo 1.23\n").unwrap();

    let preamble = ConventionsSkill.preamble_for(Role::Implementer, d.path());
    let task = task_with(Role::Implementer, preamble);
    let prompt = build_prompt(&task, std::path::Path::new("/wt/.tutti/handoff.json"));
    assert!(prompt.contains("Rust conventions"));
    assert!(prompt.contains("Go conventions"));
}

#[test]
fn a_worktree_with_no_reference_language_leaves_the_reviewer_correctness_only() {
    // A language with no reference file (or none detected) injects nothing, and the reviewer
    // prompt must not dangle a pointer at an absent reference.
    let d = tempfile::tempdir().unwrap();
    // No recognized language marker.
    std::fs::write(d.path().join("README.md"), "hi\n").unwrap();

    let preamble = ConventionsSkill.preamble_for(Role::Reviewer, d.path());
    assert!(preamble.is_none(), "no detected language, no preamble");

    let task = task_with(Role::Reviewer, preamble);
    let prompt = build_prompt(&task, std::path::Path::new("/wt/.tutti/review.json"));
    assert!(!prompt.contains("conventions reference above"));
    assert!(prompt.contains("correctness bugs the gate cannot see"));
}
