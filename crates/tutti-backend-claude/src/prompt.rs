// SPDX-License-Identifier: AGPL-3.0-or-later
//! Build the `claude -p` prompt for a role: skill activation, issue context, and the
//! fixed handoff-protocol postamble that tells the agent where to write its result.

use std::path::Path;
use tutti_core::message::{AgentTask, Role};

/// Convert a skill ref (`superpowers:x` or `local-skill`) to its slash-command form.
fn skill_command(skill: &str) -> String {
    format!("/{skill}")
}

/// The absolute path (as a string) the agent must write its result JSON to.
pub fn output_path(worktree: &Path, role: Role) -> std::path::PathBuf {
    let name = match role {
        Role::Reviewer => "review.json",
        Role::Planner => "plan.json",
        _ => "handoff.json",
    };
    worktree.join(".tutti").join(name)
}

/// Build the full prompt string for `task`, telling the agent to write its result to
/// `out_path`. Pure: no IO.
pub fn build_prompt(task: &AgentTask, out_path: &Path) -> String {
    let skills = task
        .playbook
        .skills
        .iter()
        .map(|s| skill_command(s))
        .collect::<Vec<_>>()
        .join(" ");

    let role_line = match task.playbook.role {
        Role::Implementer => "Implement the issue below, test-first.",
        Role::Reviewer => {
            "Adversarially review the current work for correctness. Assume the gate (format, \
             lint, types) and CI are already green: your job is to find the real bugs they \
             cannot see. Hunt for wrong conditions, off-by-one and boundary errors, \
             unhandled inputs, panics and overflow, broken invariants, incorrect error \
             handling, and missing test coverage of real behavior. Do NOT raise formatting, \
             style, naming, or structural findings: those are settled by the opinionated \
             gate and the project's conventions. Report only substantive findings, each with \
             an honest severity (blocking or major means it must be fixed before shipping; \
             minor is a small correctness or coverage note)."
        }
        Role::FixApplier => "Apply the review findings below to the current work.",
        Role::Planner => "Decide the next action for this project.",
        Role::Greener => {
            "Make the repository's gate pass by fixing the underlying code. Do NOT suppress \
             errors (`# type: ignore`, `# noqa`, `#[allow(...)]`, `eslint-disable`), weaken \
             the gate configuration, or delete tests to make it pass. Fix the root cause. The \
             current gate failure is in the issue body."
        }
    };

    let schema = match task.playbook.role {
        Role::Reviewer => {
            "{\"findings\":[{\"severity\":\"blocking|major|minor\",\"file\":\"...\",\"line\":<int|null>,\"claim\":\"...\"}],\"verdict\":\"Approve|RequestChanges\"}"
        }
        Role::Planner => {
            // A PlanDecision. `action` is one of: \"NextIssue\", \"Stop\", or a tagged object
            // {\"CreateIssues\":[{\"title\":\"...\",\"body\":\"...\",\"labels\":[\"...\"]}]} or
            // {\"CloseMilestone\":\"<title>\"}. Only NextIssue and CreateIssues are auto-executed;
            // CloseMilestone and any needs_human decision are surfaced to a human.
            //
            // A proposed issue may carry `milestone` and `epic` placement hints, named by the
            // titles shown in the tracking snapshot. Both are optional and default to null,
            // which files the issue at the top level. A title that matches nothing is ignored
            // rather than fatal, so a guess costs placement, never the issue.
            "{\"action\":\"NextIssue\"|\"Stop\"|{\"CreateIssues\":[{\"title\":\"...\",\"body\":\"...\",\"labels\":[\"...\"],\"milestone\":\"<snapshot milestone title>|null\",\"epic\":\"<epic title>|null\"}]}|{\"CloseMilestone\":\"...\"},\"rationale\":\"...\",\"needs_human\":<bool>}"
        }
        _ => {
            "{\"issue\":<int>,\"branch\":\"...\",\"target\":{\"target\":\"...\",\"create_from\":\"...|null\"},\"pr_title\":\"...\",\"pr_body\":\"...\",\"labels\":[\"...\"],\"decision_note\":\"...|null\"}"
        }
    };

    let review_ctx = task
        .review
        .as_ref()
        .map(|r| {
            format!(
                "\n\nReview findings to address:\n{}",
                serde_json::to_string(r).unwrap_or_default()
            )
        })
        .unwrap_or_default();

    let codegraph_hint = if task.mcp_servers.iter().any(|s| s.name == "codegraph") {
        "\n\nA `codegraph_explore` MCP tool is available. Prefer it over reading files to \
         map structure, callers, and change impact; it answers structural questions from a \
         pre-built index."
    } else {
        ""
    };

    format!(
        "{skills}\n\n{role_line}\n\nIssue #{num}: {title}\n\n{body}{review_ctx}{codegraph_hint}\n\n\
         When you are done, write your result as JSON matching this schema to the file \
         `{out}` (create the `.tutti` directory if needed). Write ONLY that file for the \
         result; do not print the JSON.\nSchema: {schema}",
        num = task.issue.id.0,
        title = task.issue.title,
        body = task.issue.body,
        out = out_path.display(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use tutti_core::domain::{Issue, IssueId, IssueState};
    use tutti_core::message::RolePlaybook;

    fn task(role: Role, skills: Vec<String>) -> AgentTask {
        AgentTask {
            playbook: RolePlaybook { role, skills },
            issue: Issue {
                id: IssueId(42),
                title: "Do X".into(),
                body: "details".into(),
                labels: vec![],
                milestone: None,
                state: IssueState::Open,
            },
            worktree_branch: "feat/issue-42".into(),
            model: "m".into(),
            review: None,
            mcp_servers: vec![],
        }
    }

    #[test]
    fn prompt_activates_skills_as_slash_commands() {
        let p = build_prompt(
            &task(
                Role::Implementer,
                vec!["superpowers:test-driven-development".into()],
            ),
            Path::new("/wt/.tutti/handoff.json"),
        );
        assert!(p.contains("/superpowers:test-driven-development"));
        assert!(p.contains("Issue #42: Do X"));
        assert!(p.contains("/wt/.tutti/handoff.json"));
    }

    #[test]
    fn reviewer_prompt_is_adversarial_and_correctness_scoped() {
        let p = build_prompt(
            &task(Role::Reviewer, vec![]),
            Path::new("/wt/.tutti/review.json"),
        );
        assert!(p.contains("Adversarially review"));
        assert!(p.contains("Do NOT raise formatting"));

        let implementer_p = build_prompt(
            &task(Role::Implementer, vec![]),
            Path::new("/wt/.tutti/handoff.json"),
        );
        assert!(!implementer_p.contains("Adversarially review"));
        assert!(!implementer_p.contains("Do NOT raise formatting"));
    }

    #[test]
    fn reviewer_output_path_is_review_json() {
        assert!(output_path(Path::new("/wt"), Role::Reviewer).ends_with(".tutti/review.json"));
        assert!(output_path(Path::new("/wt"), Role::Implementer).ends_with(".tutti/handoff.json"));
    }

    #[test]
    fn planner_output_path_is_plan_json() {
        assert!(output_path(Path::new("/wt"), Role::Planner).ends_with(".tutti/plan.json"));
    }

    #[test]
    fn greener_prompt_states_the_no_cheat_constraint() {
        let mut t = task(Role::Implementer, vec![]);
        t.playbook.role = Role::Greener;
        t.issue.title = "Green up the python gate".into();
        t.issue.body = "$ bash scripts/check.sh\nmypy: 3 errors".into();
        let p = build_prompt(&t, std::path::Path::new("/wt"));
        assert!(p.contains("gate pass"), "states the goal");
        assert!(
            p.contains("type: ignore") || p.contains("suppress"),
            "forbids suppression"
        );
        assert!(
            p.contains("mypy: 3 errors"),
            "includes the current gate log (issue body)"
        );
    }

    #[test]
    fn nudge_present_only_when_mcp_servers_are_wired() {
        use tutti_core::mcp::McpServer;
        let mut t = task(Role::Implementer, vec![]);
        let out = std::path::PathBuf::from("/tmp/.tutti/handoff.json");
        assert!(!build_prompt(&t, &out).contains("codegraph_explore"));

        t.mcp_servers = vec![McpServer {
            name: "codegraph".into(),
            command: "codegraph".into(),
            args: vec!["serve".into(), "--mcp".into()],
        }];
        assert!(build_prompt(&t, &out).contains("codegraph_explore"));
    }
}
