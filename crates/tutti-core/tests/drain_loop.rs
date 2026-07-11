// SPDX-License-Identifier: AGPL-3.0-or-later
//! Black-box: the public API drains multiple issues and stops cleanly.

use tutti_core::config::{default_roles, Config};
use tutti_core::domain::{BranchPlan, CiState, Issue, IssueId, SelectFilter};
use tutti_core::engine::{Engine, IterOutcome};
use tutti_core::gate::Gate;
use tutti_core::message::*;
use tutti_core::testing::{FakeBackend, FakeForge};

fn cfg() -> Config {
    Config {
        trunk: "main".into(),
        routing: "trunk".into(),
        integration_branch: "version/v0.1".into(),
        model: "fake".into(),
        max_issues_per_run: 5,
        ci_max_polls: 40,
        poll_delay_secs: 0,
        select: SelectFilter {
            require_label: "status:ready".into(),
            skip_labels: vec!["status:needs-human".into()],
        },
        gate: Gate {
            commands: vec!["true".into()],
            working_dir: Default::default(),
        },
        roles: default_roles(),
        merge_mode: tutti_core::domain::MergeMode::Merge,
    }
}

fn ready(id: u64) -> Issue {
    Issue {
        id: IssueId(id),
        title: format!("i{id}"),
        body: String::new(),
        labels: vec!["status:ready".into()],
        milestone: None,
    }
}

fn ship(id: u64) -> AgentOutcome {
    AgentOutcome {
        status: AgentStatus::ReadyToShip,
        handoff: Some(Handoff {
            issue: IssueId(id),
            branch: format!("feat/issue-{id}"),
            target: BranchPlan {
                target: "IGNORED".into(),
                create_from: None,
            },
            pr_title: "t".into(),
            pr_body: "b".into(),
            labels: vec![],
            decision_note: None,
        }),
        review: None,
        summary: "ok".into(),
        usage: Usage::default(),
        blocked_reason: None,
    }
}

fn approve() -> AgentOutcome {
    AgentOutcome {
        status: AgentStatus::ReadyToShip,
        handoff: None,
        review: Some(ReviewReport {
            findings: vec![],
            verdict: Verdict::Approve,
        }),
        summary: "lgtm".into(),
        usage: Usage::default(),
        blocked_reason: None,
    }
}

#[tokio::test]
async fn drains_two_ready_issues_then_stops() {
    let cfg = cfg();
    // The fake selector returns the FIRST matching issue; once #1 is done (label
    // flips to status:done) it stops matching, so #2 is next, then none.
    let forge = FakeForge::new(vec![ready(1), ready(2)], CiState::Pass);
    let backend = FakeBackend::new()
        .script(Role::Implementer, ship(1))
        .script(Role::Reviewer, approve())
        .script(Role::Implementer, ship(2))
        .script(Role::Reviewer, approve());
    let engine = Engine::new(
        &cfg,
        &forge,
        &backend,
        Box::new(tutti_core::testing::NoopWorkspace::default()),
    )
    .unwrap();

    let (shipped, plan) = engine.drain().await.unwrap();
    assert_eq!(shipped, 2);
    assert!(plan.is_some());
    assert!(forge.is_done(IssueId(1)) && forge.is_done(IssueId(2)));
    assert_eq!(engine.run_one().await.unwrap(), IterOutcome::NoReadyWork);
}
