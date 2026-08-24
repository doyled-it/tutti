// SPDX-License-Identifier: AGPL-3.0-or-later
//! Greening (retrofit 2b): drive the agent to a green gate per target, in an isolated
//! worktree branch that opens a PR. Orchestration over the AgentBackend, Gate, Forge, and
//! GreenWorkspace seams, so it is testable with no `claude`, git, or network.

use crate::traits::Result;
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
    }

    impl FakeGreenWorkspace {
        pub fn new(dir: std::path::PathBuf) -> Self {
            Self {
                dir,
                existing_branches: Vec::new(),
                committed: Mutex::new(Vec::new()),
                diff: Mutex::new(GreenDiff::default()),
                resumed: Mutex::new(Vec::new()),
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
            Ok(true)
        }
        async fn diff(&self, _co: &GreenCheckout, _base: &str) -> Result<GreenDiff> {
            Ok(self.diff.lock().unwrap().clone())
        }
        async fn remove(&self, _co: &GreenCheckout) -> Result<()> {
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

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
}
