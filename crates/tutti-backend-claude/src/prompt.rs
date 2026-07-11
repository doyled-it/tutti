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
        Role::Reviewer => "Review the current work for this issue and report findings.",
        Role::FixApplier => "Apply the review findings below to the current work.",
        Role::Planner => "Decide the next action for this project.",
    };

    let schema = match task.playbook.role {
        Role::Reviewer => {
            "{\"findings\":[{\"severity\":\"blocking|major|minor\",\"file\":\"...\",\"line\":<int|null>,\"claim\":\"...\"}],\"verdict\":\"Approve|RequestChanges\"}"
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

    format!(
        "{skills}\n\n{role_line}\n\nIssue #{num}: {title}\n\n{body}{review_ctx}\n\n\
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
    use tutti_core::domain::{Issue, IssueId};
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
            },
            worktree_branch: "feat/issue-42".into(),
            model: "m".into(),
            review: None,
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
    fn reviewer_output_path_is_review_json() {
        assert!(output_path(Path::new("/wt"), Role::Reviewer).ends_with(".tutti/review.json"));
        assert!(output_path(Path::new("/wt"), Role::Implementer).ends_with(".tutti/handoff.json"));
    }
}
