// SPDX-License-Identifier: AGPL-3.0-or-later
//! Greening (retrofit 2b): drive the agent to a green gate per target, in an isolated
//! worktree branch that opens a PR. Orchestration over the AgentBackend, Gate, Forge, and
//! GreenWorkspace seams, so it is testable with no `claude`, git, or network.

use crate::domain::{Issue, IssueId, IssueState, PrRequest};
use crate::gate::Gate;
use crate::message::{AgentTask, Role, RolePlaybook};
use crate::traits::{AgentBackend, Forge, Result};
use std::path::PathBuf;

/// An isolated checkout the greening loop runs in: a git worktree on `branch`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GreenCheckout {
    pub branch: String,
    pub path: PathBuf,
}

/// The change a greening branch introduces vs its base, for the anti-cheat audit.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GreenDiff {
    pub changed_files: Vec<PathBuf>,
    pub patch: String,
}

/// A worktree seam for greening: named branches (not issue-coupled like `Workspace`), plus
/// the diff the audit needs and branch-existence for resume.
#[async_trait::async_trait]
pub trait GreenWorkspace: Send + Sync {
    /// True if `branch` already exists locally (drives resume).
    async fn branch_exists(&self, branch: &str) -> Result<bool>;
    /// Create a worktree on `branch`. When `resume` and the branch exists, add the worktree
    /// onto it without resetting; otherwise create/reset `branch` from `base`.
    async fn checkout(&self, branch: &str, base: &str, resume: bool) -> Result<GreenCheckout>;
    /// Stage and commit everything on the checkout's branch. Ok(true) if a commit was made.
    async fn commit_all(&self, co: &GreenCheckout, message: &str) -> Result<bool>;
    /// True if the checkout's branch carries commits beyond `base`. The greener agent (with
    /// the shipped TDD / subagent defaults) commits its own work, so `commit_all` can find a
    /// clean tree while the branch is genuinely ahead of `base`; this lets the caller ship
    /// that branch instead of mistaking it for "nothing changed" (mirrors the engine's
    /// `Workspace::has_commits`, the #37 fix).
    async fn has_commits(&self, co: &GreenCheckout, base: &str) -> Result<bool>;
    /// The diff (changed files + unified patch) of the checkout's branch vs `base`.
    async fn diff(&self, co: &GreenCheckout, base: &str) -> Result<GreenDiff>;
    /// Remove the worktree checkout (the branch persists). Best-effort.
    async fn remove(&self, co: &GreenCheckout) -> Result<()>;
}

/// The result of auditing a greening diff: a reason to reject (block the PR) plus non-fatal
/// flags to surface in the report.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GreenAudit {
    pub reject: Option<String>,
    pub flags: Vec<String>,
}

/// True if `path` is a gate definition (the gate script or a CI workflow), at the repo root
/// or any subdirectory.
fn is_gate_definition(path: &std::path::Path) -> bool {
    let s = path.to_string_lossy().replace('\\', "/");
    s == "scripts/check.sh"
        || s.ends_with("/scripts/check.sh")
        || s.starts_with(".github/workflows/")
        || s.contains("/.github/workflows/")
}

fn is_test_file(path: &std::path::Path) -> bool {
    let s = path.to_string_lossy().replace('\\', "/");
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    s.contains("/tests/")
        || s.starts_with("tests/")
        || name.ends_with("_test.go")
        || name.ends_with(".test.ts")
        || (name.starts_with("test_") && name.ends_with(".py"))
}

fn is_gate_config(path: &std::path::Path) -> bool {
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    matches!(
        name,
        "pyproject.toml" | "tsconfig.json" | "package.json" | "Cargo.toml" | "go.mod"
    )
}

/// Audit a greening branch's diff (changed files + unified patch). Hard-reject when the diff
/// edits the gate definition itself; otherwise collect non-fatal flags (added suppressions,
/// deleted tests, config edits) for the reviewer.
pub fn audit_green_diff(changed_files: &[std::path::PathBuf], patch: &str) -> GreenAudit {
    let mut audit = GreenAudit::default();
    if let Some(p) = changed_files.iter().find(|p| is_gate_definition(p)) {
        audit.reject = Some(format!(
            "the greening diff edits the gate definition ({}); that is gate-weakening, not a fix",
            p.display()
        ));
        return audit;
    }
    let suppressions = patch
        .lines()
        .filter(|l| l.starts_with('+'))
        .filter(|l| {
            l.contains("# type: ignore")
                || l.contains("# noqa")
                || l.contains("#[allow(")
                || l.contains("eslint-disable")
        })
        .count();
    if suppressions > 0 {
        audit.flags.push(format!(
            "added {suppressions} suppression comment(s) (type: ignore / noqa / allow / eslint-disable)"
        ));
    }
    let deleted_tests = changed_files
        .iter()
        .filter(|p| is_test_file(p))
        .filter(|p| {
            let s = p.to_string_lossy();
            patch.contains(&format!("--- a/{s}")) && patch.contains("+++ /dev/null")
        })
        .count();
    if deleted_tests > 0 {
        audit
            .flags
            .push(format!("deleted {deleted_tests} test file(s)"));
    }
    let configs: Vec<_> = changed_files
        .iter()
        .filter(|p| is_gate_config(p))
        .filter_map(|p| p.file_name().and_then(|n| n.to_str()))
        .collect();
    if !configs.is_empty() {
        audit.flags.push(format!(
            "edited gate/linter config ({}); confirm it was not loosened",
            configs.join(", ")
        ));
    }
    audit
}

/// One unit of greening: a gate to make pass, in a working directory, with a friendly label.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GateTarget {
    pub label: String,
    pub gate: Gate,
}

/// A friendly label for a directory, from its language marker files (falls back to the dir
/// name, or "repo" at the root). Kept local so `tutti-core` needs no `tutti-app-core` dep.
fn label_for_dir(abs: &std::path::Path, rel: &std::path::Path) -> String {
    let has = |n: &str| abs.join(n).exists();
    if has("pyproject.toml") || has("setup.py") || has("requirements.txt") {
        "python".to_string()
    } else if has("Cargo.toml") {
        "rust".to_string()
    } else if has("package.json") || has("tsconfig.json") {
        "typescript".to_string()
    } else if has("go.mod") {
        "go".to_string()
    } else {
        rel.file_name()
            .and_then(|n| n.to_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| "repo".to_string())
    }
}

/// Discover the gate targets to green in `repo`:
/// 1. `gate_override` -> one root target with that command.
/// 2. else each immediate subdirectory containing `scripts/check.sh` -> one target each.
/// 3. else one root target running `config_gate` at the repo root.
pub fn discover_targets(
    repo: &std::path::Path,
    config_gate: &[String],
    gate_override: Option<&str>,
) -> Vec<GateTarget> {
    if let Some(cmd) = gate_override {
        return vec![GateTarget {
            label: label_for_dir(repo, repo),
            gate: Gate {
                commands: vec![cmd.to_string()],
                working_dir: std::path::PathBuf::new(),
            },
        }];
    }
    let mut subdir_targets = Vec::new();
    if let Ok(entries) = std::fs::read_dir(repo) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() && path.join("scripts/check.sh").is_file() {
                let rel = std::path::PathBuf::from(entry.file_name());
                subdir_targets.push(GateTarget {
                    label: label_for_dir(&path, &rel),
                    gate: Gate {
                        commands: vec!["bash scripts/check.sh".to_string()],
                        working_dir: rel,
                    },
                });
            }
        }
    }
    if !subdir_targets.is_empty() {
        // Disambiguate colliding labels (e.g. two Python subdirs) so each target gets a unique
        // branch/worktree path. Keep the plain language label when it is already unique.
        let mut seen: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
        for t in &subdir_targets {
            *seen.entry(t.label.clone()).or_insert(0) += 1;
        }
        for t in &mut subdir_targets {
            if seen[&t.label] > 1 {
                if let Some(dir) = t.gate.working_dir.file_name().and_then(|n| n.to_str()) {
                    t.label = format!("{}-{}", t.label, dir);
                }
            }
        }
        subdir_targets.sort_by(|a, b| a.gate.working_dir.cmp(&b.gate.working_dir));
        return subdir_targets;
    }
    vec![GateTarget {
        label: label_for_dir(repo, repo),
        gate: Gate {
            commands: config_gate.to_vec(),
            working_dir: std::path::PathBuf::new(),
        },
    }]
}

/// Options for a greening run.
#[derive(Debug, Clone)]
pub struct GreenOptions {
    pub max_iters: u32,
    pub fresh: bool,
    pub base: String,
    pub model: String,
}

/// What became of one target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GreenOutcome {
    AlreadyGreen,
    Greened {
        iters: u32,
        pr: u64,
        flags: Vec<String>,
    },
    Exhausted {
        iters: u32,
        gate_log: String,
    },
    Rejected {
        reason: String,
    },
    Error(String),
}

/// One target's result, for the combined report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GreenTargetResult {
    pub label: String,
    pub branch: String,
    pub outcome: GreenOutcome,
}

/// Build the greener stage's task: fix the code so `target.gate` passes.
fn greener_task(target: &GateTarget, gate_log: &str, model: &str) -> AgentTask {
    AgentTask {
        playbook: RolePlaybook {
            role: Role::Greener,
            skills: Vec::new(),
        },
        issue: Issue {
            id: IssueId(0),
            title: format!("Green up the {} gate", target.label),
            body: format!(
                "Make the gate pass by fixing the code (working dir: {}). The current failure:\n\n{}",
                target.gate.working_dir.display(),
                gate_log
            ),
            labels: Vec::new(),
            milestone: None,
            state: IssueState::Open,
        },
        worktree_branch: format!("green/{}", target.label),
        model: model.to_string(),
        review: None,
        mcp_servers: Vec::new(),
    }
}

/// Green one target: worktree (resume or fresh) -> loop gate/agent -> anti-cheat -> commit -> PR.
/// Every terminal path removes the worktree checkout (the branch persists).
pub async fn green_target(
    target: &GateTarget,
    opts: &GreenOptions,
    backend: &dyn AgentBackend,
    workspace: &dyn GreenWorkspace,
    forge: &dyn Forge,
) -> GreenTargetResult {
    let branch = format!("green/{}", target.label);
    let outcome = match green_target_inner(target, opts, backend, workspace, forge, &branch).await {
        Ok(outcome) => outcome,
        Err(e) => GreenOutcome::Error(e),
    };
    GreenTargetResult {
        label: target.label.clone(),
        branch,
        outcome,
    }
}

async fn green_target_inner(
    target: &GateTarget,
    opts: &GreenOptions,
    backend: &dyn AgentBackend,
    workspace: &dyn GreenWorkspace,
    forge: &dyn Forge,
    branch: &str,
) -> std::result::Result<GreenOutcome, String> {
    let resume = !opts.fresh
        && workspace
            .branch_exists(branch)
            .await
            .map_err(|e| e.to_string())?;
    let co = workspace
        .checkout(branch, &opts.base, resume)
        .await
        .map_err(|e| e.to_string())?;

    // Run the whole body, then ALWAYS remove the checkout (Ok or Err). The branch persists,
    // so a later resume can add a fresh worktree onto it; leaving this one behind would wedge
    // that (`git worktree add` refuses a path that already exists).
    let result = green_body(target, opts, backend, workspace, forge, branch, &co).await;
    let _ = workspace.remove(&co).await;
    result
}

/// The post-checkout body: baseline gate check, the fix loop, the anti-cheat audit, commit,
/// and PR. Never touches the checkout's lifecycle; the caller removes it unconditionally.
async fn green_body(
    target: &GateTarget,
    opts: &GreenOptions,
    backend: &dyn AgentBackend,
    workspace: &dyn GreenWorkspace,
    forge: &dyn Forge,
    branch: &str,
    co: &GreenCheckout,
) -> std::result::Result<GreenOutcome, String> {
    let mut last = target.gate.run(&co.path).await.map_err(|e| e.to_string())?;
    if last.passed {
        return Ok(GreenOutcome::AlreadyGreen);
    }

    let mut iters = 0;
    for i in 1..=opts.max_iters {
        iters = i;
        let (tx, mut rx) = tokio::sync::mpsc::channel(64);
        let drain = tokio::spawn(async move { while rx.recv().await.is_some() {} });
        let task = greener_task(target, &last.log, &opts.model);
        let run = backend.run(task, &co.path, tx).await;
        let _ = drain.await;
        if let Err(e) = run {
            return Err(format!("agent run failed: {e}"));
        }
        last = target.gate.run(&co.path).await.map_err(|e| e.to_string())?;
        if last.passed {
            break;
        }
    }

    if !last.passed {
        return Ok(GreenOutcome::Exhausted {
            iters,
            gate_log: last.log,
        });
    }

    let diff = workspace
        .diff(co, &opts.base)
        .await
        .map_err(|e| e.to_string())?;
    let audit = audit_green_diff(&diff.changed_files, &diff.patch);
    if let Some(reason) = audit.reject {
        return Ok(GreenOutcome::Rejected { reason });
    }
    let committed = workspace
        .commit_all(
            co,
            &format!("fix({}): green up the gate\n\nPart of #40", target.label),
        )
        .await
        .map_err(|e| e.to_string())?;
    // Ship the branch if EITHER commit_all just committed the agent's uncommitted edits, OR
    // the branch already carries commits beyond base. The greener agent (shipped TDD /
    // subagent defaults) commits its own work, so `commit_all` routinely finds a clean tree
    // even though the fix is real and the branch is ahead of base. Only a branch with no new
    // commits at all is a true no-op worth skipping (a `gh pr create` on an empty branch
    // errors). This mirrors the engine's #37 fix (`Workspace::has_commits`).
    let has_work = committed
        || workspace
            .has_commits(co, &opts.base)
            .await
            .map_err(|e| e.to_string())?;
    if !has_work {
        return Ok(GreenOutcome::AlreadyGreen);
    }
    forge.push_branch(branch).await.map_err(|e| e.to_string())?;
    let pr = forge
        .open_pr(PrRequest {
            base: opts.base.clone(),
            head: branch.to_string(),
            title: format!("Green up the {} gate", target.label),
            body: format!(
                "Automated greening of the `{}` gate in {} iteration(s).{}",
                target.label,
                iters,
                if audit.flags.is_empty() {
                    String::new()
                } else {
                    format!("\n\nReview flags:\n- {}", audit.flags.join("\n- "))
                }
            ),
            labels: Vec::new(),
        })
        .await
        .map_err(|e| e.to_string())?;
    Ok(GreenOutcome::Greened {
        iters,
        pr: pr.number,
        flags: audit.flags,
    })
}

/// Green every target concurrently and collect the results. A per-target failure is captured
/// in its `GreenTargetResult` (never aborts the batch). Concurrency is bounded so a
/// pathological target count cannot spawn unboundedly.
pub async fn green_all(
    targets: &[GateTarget],
    opts: &GreenOptions,
    backend: &dyn AgentBackend,
    workspace: &dyn GreenWorkspace,
    forge: &dyn Forge,
) -> Vec<GreenTargetResult> {
    use futures::stream::{self, StreamExt};
    const MAX_CONCURRENT: usize = 4;
    stream::iter(targets.iter())
        .map(|t| green_target(t, opts, backend, workspace, forge))
        .buffer_unordered(MAX_CONCURRENT)
        .collect()
        .await
}

#[cfg(test)]
pub(crate) mod fakes {
    use super::*;
    use std::sync::Mutex;

    /// A GreenWorkspace over a real tempdir so the real `Gate` can run in it, but with git
    /// operations faked. `existing_branches` seeds resume; `committed` records commits.
    pub struct FakeGreenWorkspace {
        pub dir: std::path::PathBuf,
        pub existing_branches: Vec<String>,
        pub committed: Mutex<Vec<String>>,
        pub diff: Mutex<GreenDiff>,
        pub resumed: Mutex<Vec<String>>,
        pub removed: Mutex<Vec<String>>,
        /// What `commit_all` returns; false simulates nothing-to-commit (empty net diff).
        pub commit_result: bool,
        /// What `has_commits` returns; true simulates the agent having committed its own work
        /// (branch ahead of base while `commit_all` finds a clean tree).
        pub branch_ahead: bool,
    }

    impl FakeGreenWorkspace {
        pub fn new(dir: std::path::PathBuf) -> Self {
            Self {
                dir,
                existing_branches: Vec::new(),
                committed: Mutex::new(Vec::new()),
                diff: Mutex::new(GreenDiff::default()),
                resumed: Mutex::new(Vec::new()),
                removed: Mutex::new(Vec::new()),
                commit_result: true,
                branch_ahead: false,
            }
        }
    }

    #[async_trait::async_trait]
    impl GreenWorkspace for FakeGreenWorkspace {
        async fn branch_exists(&self, branch: &str) -> Result<bool> {
            Ok(self.existing_branches.iter().any(|b| b == branch))
        }
        async fn checkout(&self, branch: &str, _base: &str, resume: bool) -> Result<GreenCheckout> {
            if resume && self.existing_branches.iter().any(|b| b == branch) {
                self.resumed.lock().unwrap().push(branch.to_string());
            }
            Ok(GreenCheckout {
                branch: branch.to_string(),
                path: self.dir.clone(),
            })
        }
        async fn commit_all(&self, _co: &GreenCheckout, message: &str) -> Result<bool> {
            self.committed.lock().unwrap().push(message.to_string());
            Ok(self.commit_result)
        }
        async fn has_commits(&self, _co: &GreenCheckout, _base: &str) -> Result<bool> {
            Ok(self.branch_ahead)
        }
        async fn diff(&self, _co: &GreenCheckout, _base: &str) -> Result<GreenDiff> {
            Ok(self.diff.lock().unwrap().clone())
        }
        async fn remove(&self, co: &GreenCheckout) -> Result<()> {
            self.removed.lock().unwrap().push(co.branch.clone());
            Ok(())
        }
    }

    /// A GreenWorkspace that routes each branch to its own dir, so green_all can drive
    /// multiple targets that each run the real Gate in a distinct tempdir.
    pub struct RoutingGreenWorkspace {
        pub dirs: std::collections::HashMap<String, std::path::PathBuf>,
        pub removed: Mutex<Vec<String>>,
    }
    impl RoutingGreenWorkspace {
        pub fn new(map: Vec<(String, std::path::PathBuf)>) -> Self {
            Self {
                dirs: map.into_iter().collect(),
                removed: Mutex::new(Vec::new()),
            }
        }
    }
    #[async_trait::async_trait]
    impl GreenWorkspace for RoutingGreenWorkspace {
        async fn branch_exists(&self, _branch: &str) -> Result<bool> {
            Ok(false)
        }
        async fn checkout(
            &self,
            branch: &str,
            _base: &str,
            _resume: bool,
        ) -> Result<GreenCheckout> {
            let path = self
                .dirs
                .get(branch)
                .cloned()
                .unwrap_or_else(|| std::path::PathBuf::from("/nonexistent"));
            Ok(GreenCheckout {
                branch: branch.to_string(),
                path,
            })
        }
        async fn commit_all(&self, _co: &GreenCheckout, _message: &str) -> Result<bool> {
            Ok(true)
        }
        async fn has_commits(&self, _co: &GreenCheckout, _base: &str) -> Result<bool> {
            Ok(false)
        }
        async fn diff(&self, _co: &GreenCheckout, _base: &str) -> Result<GreenDiff> {
            Ok(GreenDiff::default())
        }
        async fn remove(&self, co: &GreenCheckout) -> Result<()> {
            self.removed.lock().unwrap().push(co.branch.clone());
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn touch(dir: &std::path::Path, rel: &str) {
        let p = dir.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, "x").unwrap();
    }

    #[test]
    fn discover_single_root_target_uses_the_config_gate() {
        let d = tempfile::tempdir().unwrap();
        touch(d.path(), "scripts/check.sh");
        touch(d.path(), "pyproject.toml");
        let targets = discover_targets(d.path(), &["bash scripts/check.sh".to_string()], None);
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].gate.working_dir, std::path::PathBuf::new());
        assert_eq!(
            targets[0].gate.commands,
            vec!["bash scripts/check.sh".to_string()]
        );
        assert_eq!(targets[0].label, "python");
    }

    #[test]
    fn discover_finds_one_target_per_subdir_gate() {
        let d = tempfile::tempdir().unwrap();
        touch(d.path(), "py/scripts/check.sh");
        touch(d.path(), "py/pyproject.toml");
        touch(d.path(), "rs/scripts/check.sh");
        touch(d.path(), "rs/Cargo.toml");
        let mut targets = discover_targets(d.path(), &["bash scripts/check.sh".to_string()], None);
        targets.sort_by(|a, b| a.label.cmp(&b.label));
        assert_eq!(targets.len(), 2);
        assert_eq!(targets[0].label, "python");
        assert_eq!(targets[0].gate.working_dir, std::path::PathBuf::from("py"));
        assert_eq!(targets[1].label, "rust");
        assert_eq!(targets[1].gate.working_dir, std::path::PathBuf::from("rs"));
    }

    #[test]
    fn discover_disambiguates_colliding_labels_across_same_language_subdirs() {
        let d = tempfile::tempdir().unwrap();
        touch(d.path(), "api/scripts/check.sh");
        touch(d.path(), "api/pyproject.toml");
        touch(d.path(), "worker/scripts/check.sh");
        touch(d.path(), "worker/pyproject.toml");
        let mut targets = discover_targets(d.path(), &["bash scripts/check.sh".to_string()], None);
        targets.sort_by(|a, b| a.label.cmp(&b.label));
        assert_eq!(targets.len(), 2);
        assert_ne!(targets[0].label, targets[1].label, "labels must be unique");
        assert!(targets[0].label.contains("python"));
        assert!(targets[1].label.contains("python"));
        assert_eq!(targets[0].label, "python-api");
        assert_eq!(targets[1].label, "python-worker");
    }

    #[test]
    fn discover_gate_override_is_a_single_root_target() {
        let d = tempfile::tempdir().unwrap();
        touch(d.path(), "py/scripts/check.sh"); // ignored when overridden
        let targets = discover_targets(d.path(), &["true".to_string()], Some("make ci"));
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].gate.commands, vec!["make ci".to_string()]);
        assert_eq!(targets[0].gate.working_dir, std::path::PathBuf::new());
    }

    #[test]
    fn audit_rejects_editing_the_gate_script() {
        let a = audit_green_diff(&[PathBuf::from("scripts/check.sh")], "- old\n+ new\n");
        assert!(a.reject.is_some(), "editing the gate is a reject");
    }

    #[test]
    fn audit_rejects_editing_a_ci_workflow() {
        let a = audit_green_diff(&[PathBuf::from(".github/workflows/ci.yml")], "");
        assert!(a.reject.is_some());
    }

    #[test]
    fn audit_rejects_a_subdir_gate_script() {
        let a = audit_green_diff(&[PathBuf::from("py/scripts/check.sh")], "");
        assert!(a.reject.is_some(), "a subdir gate script is still the gate");
    }

    #[test]
    fn audit_flags_but_allows_suppressions() {
        let patch = "+ x = 1  # type: ignore\n+ y = 2  # noqa\n";
        let a = audit_green_diff(&[PathBuf::from("src/x.py")], patch);
        assert!(a.reject.is_none());
        assert!(
            a.flags.iter().any(|f| f.contains("suppress")),
            "flags: {:?}",
            a.flags
        );
    }

    #[test]
    fn audit_flags_deleted_tests_and_config_edits() {
        let a = audit_green_diff(
            &[
                PathBuf::from("tests/test_core.py"),
                PathBuf::from("pyproject.toml"),
            ],
            "--- a/tests/test_core.py\n+++ /dev/null\n",
        );
        assert!(a.reject.is_none());
        assert!(a.flags.iter().any(|f| f.contains("test")));
        assert!(a
            .flags
            .iter()
            .any(|f| f.contains("pyproject.toml") || f.contains("config")));
    }

    #[test]
    fn audit_clean_diff_has_no_reject_no_flags() {
        let a = audit_green_diff(&[PathBuf::from("src/x.py")], "+ def f(): pass\n");
        assert!(a.reject.is_none());
        assert!(a.flags.is_empty());
    }

    use crate::greening::fakes::FakeGreenWorkspace;
    use crate::message::{AgentEvent, AgentOutcome, AgentStatus, AgentTask, Usage};
    use crate::testing::FakeForge;
    use crate::traits::AgentBackend;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Arc;
    use tokio::sync::mpsc::Sender;

    struct MarkerBackend {
        green_on: u32,
        calls: Arc<AtomicU32>,
    }
    #[async_trait::async_trait]
    impl AgentBackend for MarkerBackend {
        async fn run(
            &self,
            _task: AgentTask,
            worktree: &std::path::Path,
            _tx: Sender<AgentEvent>,
        ) -> Result<AgentOutcome> {
            let n = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
            if n >= self.green_on {
                std::fs::write(worktree.join(".green"), "ok").unwrap();
            }
            Ok(AgentOutcome {
                status: AgentStatus::ReadyToShip,
                handoff: None,
                review: None,
                plan: None,
                summary: String::new(),
                usage: Usage::default(),
                blocked_reason: None,
            })
        }
    }

    fn marker_gate() -> Gate {
        Gate {
            commands: vec!["test -f .green".to_string()],
            working_dir: std::path::PathBuf::new(),
        }
    }
    fn gopts() -> GreenOptions {
        GreenOptions {
            max_iters: 5,
            fresh: false,
            base: "staging".into(),
            model: "m".into(),
        }
    }

    #[tokio::test]
    async fn green_target_greens_and_opens_a_pr() {
        let d = tempfile::tempdir().unwrap();
        let ws = FakeGreenWorkspace::new(d.path().to_path_buf());
        let backend = MarkerBackend {
            green_on: 2,
            calls: Arc::new(AtomicU32::new(0)),
        };
        let forge = FakeForge::new(vec![], crate::domain::CiState::Pass);
        let target = GateTarget {
            label: "python".into(),
            gate: marker_gate(),
        };
        let r = green_target(&target, &gopts(), &backend, &ws, &forge).await;
        assert!(
            matches!(r.outcome, GreenOutcome::Greened { iters: 2, .. }),
            "{:?}",
            r.outcome
        );
        assert_eq!(ws.committed.lock().unwrap().len(), 1, "committed once");
        assert_eq!(forge.pr_count(), 1, "exactly one PR opened");
        assert_eq!(
            ws.removed.lock().unwrap().len(),
            1,
            "checkout removed on the happy path"
        );
    }

    #[tokio::test]
    async fn green_target_already_green_does_nothing() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join(".green"), "ok").unwrap();
        let ws = FakeGreenWorkspace::new(d.path().to_path_buf());
        let calls = Arc::new(AtomicU32::new(0));
        let backend = MarkerBackend {
            green_on: 99,
            calls: calls.clone(),
        };
        let forge = FakeForge::new(vec![], crate::domain::CiState::Pass);
        let target = GateTarget {
            label: "python".into(),
            gate: marker_gate(),
        };
        let r = green_target(&target, &gopts(), &backend, &ws, &forge).await;
        assert!(matches!(r.outcome, GreenOutcome::AlreadyGreen));
        assert_eq!(calls.load(Ordering::SeqCst), 0, "no agent call");
        assert_eq!(forge.pr_count(), 0, "no PR opened");
    }

    #[tokio::test]
    async fn green_target_exhausts_without_a_pr() {
        let d = tempfile::tempdir().unwrap();
        let ws = FakeGreenWorkspace::new(d.path().to_path_buf());
        let backend = MarkerBackend {
            green_on: 99,
            calls: Arc::new(AtomicU32::new(0)),
        };
        let forge = FakeForge::new(vec![], crate::domain::CiState::Pass);
        let target = GateTarget {
            label: "python".into(),
            gate: marker_gate(),
        };
        let r = green_target(&target, &gopts(), &backend, &ws, &forge).await;
        assert!(
            matches!(r.outcome, GreenOutcome::Exhausted { iters: 5, .. }),
            "{:?}",
            r.outcome
        );
        assert_eq!(forge.pr_count(), 0, "no PR opened");
    }

    #[tokio::test]
    async fn green_target_rejects_a_gate_edit_and_opens_no_pr() {
        let d = tempfile::tempdir().unwrap();
        let ws = FakeGreenWorkspace::new(d.path().to_path_buf());
        *ws.diff.lock().unwrap() = GreenDiff {
            changed_files: vec![std::path::PathBuf::from("scripts/check.sh")],
            patch: "- strict\n+ lenient\n".into(),
        };
        let backend = MarkerBackend {
            green_on: 1,
            calls: Arc::new(AtomicU32::new(0)),
        };
        let forge = FakeForge::new(vec![], crate::domain::CiState::Pass);
        let target = GateTarget {
            label: "python".into(),
            gate: marker_gate(),
        };
        let r = green_target(&target, &gopts(), &backend, &ws, &forge).await;
        assert!(
            matches!(r.outcome, GreenOutcome::Rejected { .. }),
            "{:?}",
            r.outcome
        );
        assert_eq!(forge.pr_count(), 0, "no PR opened");
        assert!(
            ws.committed.lock().unwrap().is_empty(),
            "a rejected diff must not be committed"
        );
        assert_eq!(
            ws.removed.lock().unwrap().len(),
            1,
            "checkout removed even when rejected"
        );
    }

    #[tokio::test]
    async fn green_target_skips_push_and_pr_when_nothing_was_committed() {
        // A TRUE no-op: commit_all staged nothing AND the branch is not ahead of base.
        let d = tempfile::tempdir().unwrap();
        let ws = FakeGreenWorkspace {
            commit_result: false,
            branch_ahead: false,
            ..FakeGreenWorkspace::new(d.path().to_path_buf())
        };
        let backend = MarkerBackend {
            green_on: 1,
            calls: Arc::new(AtomicU32::new(0)),
        };
        let forge = FakeForge::new(vec![], crate::domain::CiState::Pass);
        let target = GateTarget {
            label: "python".into(),
            gate: marker_gate(),
        };
        let r = green_target(&target, &gopts(), &backend, &ws, &forge).await;
        assert!(
            matches!(r.outcome, GreenOutcome::AlreadyGreen),
            "{:?}",
            r.outcome
        );
        assert_eq!(forge.pr_count(), 0, "an empty net diff must not open a PR");
    }

    #[tokio::test]
    async fn green_target_ships_when_the_agent_committed_its_own_fix() {
        // The greener agent (TDD / subagent defaults) commits its own work, so `commit_all`
        // finds a clean tree (commit_result=false) even though the branch is genuinely ahead
        // of base (branch_ahead=true). That branch must still be pushed and open a PR, not be
        // mistaken for a no-op. Regression for the #37-class bug the live shakeout caught.
        let d = tempfile::tempdir().unwrap();
        let ws = FakeGreenWorkspace {
            commit_result: false,
            branch_ahead: true,
            ..FakeGreenWorkspace::new(d.path().to_path_buf())
        };
        let backend = MarkerBackend {
            green_on: 1,
            calls: Arc::new(AtomicU32::new(0)),
        };
        let forge = FakeForge::new(vec![], crate::domain::CiState::Pass);
        let target = GateTarget {
            label: "python".into(),
            gate: marker_gate(),
        };
        let r = green_target(&target, &gopts(), &backend, &ws, &forge).await;
        assert!(
            matches!(r.outcome, GreenOutcome::Greened { .. }),
            "{:?}",
            r.outcome
        );
        assert_eq!(
            forge.pr_count(),
            1,
            "the agent's committed fix must open a PR"
        );
    }

    #[tokio::test]
    async fn green_target_resumes_an_existing_branch() {
        let d = tempfile::tempdir().unwrap();
        let ws = FakeGreenWorkspace {
            existing_branches: vec!["green/python".to_string()],
            ..FakeGreenWorkspace::new(d.path().to_path_buf())
        };
        let backend = MarkerBackend {
            green_on: 1,
            calls: Arc::new(AtomicU32::new(0)),
        };
        let forge = FakeForge::new(vec![], crate::domain::CiState::Pass);
        let target = GateTarget {
            label: "python".into(),
            gate: marker_gate(),
        };
        let opts = GreenOptions {
            fresh: false,
            ..gopts()
        };
        let _ = green_target(&target, &opts, &backend, &ws, &forge).await;
        assert_eq!(
            ws.resumed.lock().unwrap().as_slice(),
            ["green/python".to_string()]
        );
    }

    #[tokio::test]
    async fn green_target_fresh_suppresses_resume() {
        let d = tempfile::tempdir().unwrap();
        let ws = FakeGreenWorkspace {
            existing_branches: vec!["green/python".to_string()],
            ..FakeGreenWorkspace::new(d.path().to_path_buf())
        };
        let backend = MarkerBackend {
            green_on: 1,
            calls: Arc::new(AtomicU32::new(0)),
        };
        let forge = FakeForge::new(vec![], crate::domain::CiState::Pass);
        let target = GateTarget {
            label: "python".into(),
            gate: marker_gate(),
        };
        let opts = GreenOptions {
            fresh: true,
            ..gopts()
        };
        let _ = green_target(&target, &opts, &backend, &ws, &forge).await;
        assert!(
            ws.resumed.lock().unwrap().is_empty(),
            "fresh must suppress resume"
        );
    }

    #[tokio::test]
    async fn green_target_flags_surface_but_still_open_a_pr() {
        let d = tempfile::tempdir().unwrap();
        let ws = FakeGreenWorkspace::new(d.path().to_path_buf());
        *ws.diff.lock().unwrap() = GreenDiff {
            changed_files: vec![std::path::PathBuf::from("src/x.py")],
            patch: "+ x = 1  # type: ignore\n".into(),
        };
        let backend = MarkerBackend {
            green_on: 1,
            calls: Arc::new(AtomicU32::new(0)),
        };
        let forge = FakeForge::new(vec![], crate::domain::CiState::Pass);
        let target = GateTarget {
            label: "python".into(),
            gate: marker_gate(),
        };
        let r = green_target(&target, &gopts(), &backend, &ws, &forge).await;
        match r.outcome {
            GreenOutcome::Greened { flags, .. } => {
                assert!(!flags.is_empty(), "the suppression comment must flag");
            }
            other => panic!("expected Greened, got {other:?}"),
        }
        assert_eq!(
            forge.pr_count(),
            1,
            "a flagged-but-not-rejected diff still opens a PR"
        );
    }

    struct ErrBackend;
    #[async_trait::async_trait]
    impl AgentBackend for ErrBackend {
        async fn run(
            &self,
            _task: AgentTask,
            _worktree: &std::path::Path,
            _tx: Sender<AgentEvent>,
        ) -> Result<AgentOutcome> {
            Err(crate::traits::EngineError::Backend("boom".to_string()))
        }
    }

    #[tokio::test]
    async fn green_target_cleans_up_the_checkout_on_an_agent_error() {
        let d = tempfile::tempdir().unwrap();
        let ws = FakeGreenWorkspace::new(d.path().to_path_buf());
        let backend = ErrBackend;
        let forge = FakeForge::new(vec![], crate::domain::CiState::Pass);
        let target = GateTarget {
            label: "python".into(),
            gate: marker_gate(),
        };
        let r = green_target(&target, &gopts(), &backend, &ws, &forge).await;
        assert!(
            matches!(r.outcome, GreenOutcome::Error(_)),
            "{:?}",
            r.outcome
        );
        assert_eq!(
            ws.removed.lock().unwrap().len(),
            1,
            "checkout removed even when the agent run errors"
        );
    }

    #[tokio::test]
    async fn green_all_runs_every_target_and_collects_mixed_results() {
        let d1 = tempfile::tempdir().unwrap();
        std::fs::write(d1.path().join(".green"), "ok").unwrap(); // target "a" already green
        let d2 = tempfile::tempdir().unwrap(); // target "b" never greens
        let ws = fakes::RoutingGreenWorkspace::new(vec![
            ("green/a".into(), d1.path().to_path_buf()),
            ("green/b".into(), d2.path().to_path_buf()),
        ]);
        let backend = MarkerBackend {
            green_on: 99,
            calls: Arc::new(AtomicU32::new(0)),
        };
        let forge = FakeForge::new(vec![], crate::domain::CiState::Pass);
        let targets = vec![
            GateTarget {
                label: "a".into(),
                gate: marker_gate(),
            },
            GateTarget {
                label: "b".into(),
                gate: marker_gate(),
            },
        ];
        let results = green_all(&targets, &gopts(), &backend, &ws, &forge).await;
        assert_eq!(results.len(), 2);
        assert!(results
            .iter()
            .any(|r| matches!(r.outcome, GreenOutcome::AlreadyGreen)));
        assert!(results
            .iter()
            .any(|r| matches!(r.outcome, GreenOutcome::Exhausted { .. })));
    }
}
