// SPDX-License-Identifier: AGPL-3.0-or-later
//! Opinionated per-language scaffolds. A `StackProfile` is declarative data: the files
//! to emit, the canonical gate command, and an optional post-write step. The `scaffold`
//! emitter writes a profile into a repo directory, never clobbering an existing file.

use std::io::Write;
use std::path::{Path, PathBuf};

/// Context threaded into a profile's file set so names flow into the emitted content.
#[derive(Debug, Clone)]
pub struct ScaffoldContext {
    /// The repo name as the user typed it (e.g. "My-Repo").
    pub repo_name: String,
    /// A valid Python package identifier derived from `repo_name` (e.g. "my_repo").
    pub package_name: String,
}

/// One file a profile emits, relative to the repo root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScaffoldFile {
    pub path: PathBuf,
    pub contents: String,
    pub executable: bool,
}

/// A best-effort command run after the files are written (e.g. `uv lock`).
#[derive(Debug, Clone)]
pub struct PostWriteStep {
    pub program: String,
    pub args: Vec<String>,
    /// Shown in a warning if the step is skipped or fails.
    pub describe: String,
}

/// An opinionated language scaffold.
pub struct StackProfile {
    pub id: &'static str,
    pub display_name: &'static str,
    /// The files to write, computed from the context.
    pub files: fn(&ScaffoldContext) -> Vec<ScaffoldFile>,
    /// The canonical gate written into tutti.toml's [gate].
    pub gate_commands: Vec<String>,
    /// A best-effort step run after the files land.
    pub post_write: Option<PostWriteStep>,
}

/// What `scaffold` did, for surfacing to the operator.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct ScaffoldReport {
    pub written: Vec<PathBuf>,
    pub skipped: Vec<PathBuf>,
    pub warnings: Vec<String>,
}

/// Normalize a repo name into a valid Python package identifier: lowercase, every run of
/// non-alphanumeric characters collapsed to a single `_`, a leading digit prefixed with
/// `_`, and an empty result falling back to `app`.
pub fn package_name(repo: &str) -> String {
    let mut out = String::new();
    let mut last_underscore = false;
    for c in repo.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
            last_underscore = false;
        } else if !last_underscore && !out.is_empty() {
            out.push('_');
            last_underscore = true;
        }
    }
    let out = out.trim_matches('_').to_string();
    if out.is_empty() {
        return "app".to_string();
    }
    if out.chars().next().is_some_and(|c| c.is_ascii_digit()) {
        return format!("_{out}");
    }
    out
}

/// The built-in stacks. This slice ships exactly one.
pub fn available_stacks() -> Vec<StackProfile> {
    vec![python_profile()]
}

/// Look up a stack profile by id.
pub fn stack_profile(id: &str) -> Option<StackProfile> {
    available_stacks().into_iter().find(|s| s.id == id)
}

/// The opinionated Python stack: uv + ruff + mypy(strict) + pytest, src/ layout, one
/// canonical `scripts/check.sh` gate, CI, and an AGENTS.md naming the gate.
pub fn python_profile() -> StackProfile {
    StackProfile {
        id: "python",
        display_name: "Python",
        files: python_files,
        gate_commands: vec!["bash scripts/check.sh".to_string()],
        post_write: Some(PostWriteStep {
            program: "uv".to_string(),
            args: vec!["lock".to_string()],
            describe: "generate uv.lock".to_string(),
        }),
    }
}

fn python_files(ctx: &ScaffoldContext) -> Vec<ScaffoldFile> {
    let pkg = &ctx.package_name;
    let f = |path: &str, contents: String, executable: bool| ScaffoldFile {
        path: PathBuf::from(path),
        contents,
        executable,
    };
    vec![
        f("pyproject.toml", format!(
r#"[project]
name = "{pkg}"
version = "0.1.0"
requires-python = ">=3.13"
dependencies = []

[tool.uv]
package = false

[dependency-groups]
dev = ["ruff", "mypy", "pytest"]

[tool.ruff]
line-length = 88
target-version = "py313"

[tool.ruff.lint]
extend-select = ["I", "UP", "B", "SIM", "RUF"]

[tool.mypy]
strict = true
python_version = "3.13"

[tool.pytest.ini_options]
addopts = "-q --strict-markers --strict-config"
pythonpath = ["src"]
testpaths = ["tests"]
"#), false),
        f("scripts/check.sh", String::from(
r#"#!/usr/bin/env bash
# The one canonical gate. CI and the agent both run exactly this.
set -euo pipefail
# The dev tools live in the `dev` dependency-group (not optional-dependencies), so it is
# `--group dev`, not `--extra dev`. Prefer the locked resolve, fall back if there is no
# lock yet (e.g. `uv lock` was unavailable at scaffold time).
uv sync --frozen --group dev 2>/dev/null || uv sync --group dev
uv run ruff format --check .
uv run ruff check .
uv run mypy --strict src
uv run pytest
"#), true),
        f(".github/workflows/ci.yml", String::from(
r#"name: CI
on:
  pull_request:
  push:
    branches: [main]
jobs:
  check:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: astral-sh/setup-uv@v5
        with:
          enable-cache: true
      - run: bash scripts/check.sh
"#), false),
        f("AGENTS.md", format!(
r#"# AGENTS.md

Python project. One command gates every change:

```
bash scripts/check.sh
```

Run it before opening a PR; it must exit 0. It runs ruff (format + lint), mypy --strict,
and pytest. Formatting is enforced, do not hand-tune style. New functions ship with tests
in `tests/`. Work merges into `staging`, never `main`.

Layout: package under `src/{pkg}/`, tests under `tests/` mirroring it.
"#), false),
        f(".python-version", String::from("3.13\n"), false),
        f(".gitignore", String::from(
"__pycache__/\n.venv/\n.pytest_cache/\n.mypy_cache/\n.ruff_cache/\n"), false),
        f(&format!("src/{pkg}/__init__.py"), format!(
"\"\"\"The {pkg} package.\"\"\"\n\nfrom {pkg}.core import add\n\n__all__ = [\"add\"]\n"), false),
        f(&format!("src/{pkg}/core.py"), String::from(
"\"\"\"Core helpers.\"\"\"\n\n\ndef add(a: int, b: int) -> int:\n    \"\"\"Return the sum of two integers.\"\"\"\n    return a + b\n"), false),
        f("tests/test_core.py", format!(
"from {pkg} import add\n\n\ndef test_add() -> None:\n    assert add(2, 3) == 5\n"), false),
    ]
}

/// A seam over the profile's post-write shell-out, so tests can inject success/failure
/// without a real binary. The default runner is `run_post_write`.
pub type PostWriteRunner<'a> = dyn Fn(&PostWriteStep) -> std::result::Result<(), String> + 'a;

/// Write `profile` into `dir`. Never overwrites an existing file (records it in
/// `skipped`). Runs the profile's post-write step via `run_post`, recording a warning on
/// failure rather than aborting.
pub fn scaffold(
    dir: &Path,
    profile: &StackProfile,
    ctx: &ScaffoldContext,
    run_post: &PostWriteRunner<'_>,
) -> std::io::Result<ScaffoldReport> {
    let mut report = ScaffoldReport::default();
    for file in (profile.files)(ctx) {
        let abs = dir.join(&file.path);
        if abs.exists() {
            report.skipped.push(file.path.clone());
            continue;
        }
        if let Some(parent) = abs.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut f = std::fs::File::create(&abs)?;
        f.write_all(file.contents.as_bytes())?;
        #[cfg(unix)]
        if file.executable {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = f.metadata()?.permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&abs, perms)?;
        }
        report.written.push(file.path.clone());
    }
    // A fully-skipped rerun (everything already scaffolded) has nothing new for the
    // post-write step to act on, so it deliberately does not shell out again (e.g. no
    // redundant `uv lock`).
    if !report.written.is_empty() {
        if let Some(step) = &profile.post_write {
            if let Err(e) = run_post(step) {
                report
                    .warnings
                    .push(format!("could not {} ({e})", step.describe));
            }
        }
    }
    Ok(report)
}

/// The default post-write runner: run the program in `dir`, mapping a non-zero exit or a
/// missing binary to an `Err`.
pub fn run_post_write(dir: &Path, step: &PostWriteStep) -> std::result::Result<(), String> {
    let out = std::process::Command::new(&step.program)
        .args(&step.args)
        .current_dir(dir)
        .output()
        .map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).into_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn package_name_normalizes_to_a_valid_identifier() {
        assert_eq!(package_name("My-Repo"), "my_repo");
        assert_eq!(package_name("golf stats"), "golf_stats");
        assert_eq!(package_name("2cool"), "_2cool");
        assert_eq!(package_name("a.b.c"), "a_b_c");
        assert_eq!(package_name(""), "app");
    }

    fn ctx() -> ScaffoldContext {
        ScaffoldContext {
            repo_name: "My-Repo".into(),
            package_name: package_name("My-Repo"),
        }
    }

    #[test]
    fn python_profile_emits_the_opinionated_stack() {
        let p = stack_profile("python").expect("python profile exists");
        assert_eq!(p.gate_commands, vec!["bash scripts/check.sh".to_string()]);
        let files = (p.files)(&ctx());
        let by_path = |rel: &str| {
            files
                .iter()
                .find(|f| f.path == std::path::Path::new(rel))
                .unwrap_or_else(|| panic!("missing {rel}"))
        };
        // The pinned, agent-friendly tooling is present in pyproject.
        let pyproject = &by_path("pyproject.toml").contents;
        for needle in ["[tool.ruff]", "strict = true", "pytest", "requires-python"] {
            assert!(pyproject.contains(needle), "pyproject missing {needle}");
        }
        // The canonical gate runs every check and is executable.
        let check = by_path("scripts/check.sh");
        assert!(check.executable);
        for needle in [
            "ruff format --check",
            "ruff check",
            "mypy --strict",
            "pytest",
        ] {
            assert!(check.contents.contains(needle), "check.sh missing {needle}");
        }
        // The package + a passing placeholder test exist under the src layout.
        by_path("src/my_repo/__init__.py");
        by_path("src/my_repo/core.py");
        by_path("tests/test_core.py");
        // AGENTS.md names the one gate command.
        assert!(by_path("AGENTS.md")
            .contents
            .contains("bash scripts/check.sh"));
        // CI and interpreter pin.
        by_path(".github/workflows/ci.yml");
        by_path(".python-version");
        by_path(".gitignore");
    }

    #[test]
    fn available_stacks_lists_python() {
        assert!(available_stacks().iter().any(|s| s.id == "python"));
    }

    #[test]
    fn scaffold_writes_the_profile_and_sets_exec_bit() {
        let dir = tempfile::tempdir().unwrap();
        let report = scaffold(dir.path(), &python_profile(), &ctx(), &|_| Ok(())).unwrap();
        assert!(dir.path().join("pyproject.toml").exists());
        assert!(dir.path().join("src/my_repo/core.py").exists());
        assert!(report.skipped.is_empty());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(dir.path().join("scripts/check.sh"))
                .unwrap()
                .permissions()
                .mode();
            assert!(mode & 0o111 != 0, "check.sh should be executable");
        }
    }

    #[test]
    fn scaffold_never_clobbers_an_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(".gitignore"), "KEEP\n").unwrap();
        let report = scaffold(dir.path(), &python_profile(), &ctx(), &|_| Ok(())).unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.path().join(".gitignore")).unwrap(),
            "KEEP\n",
            "existing file must be untouched"
        );
        assert!(report
            .skipped
            .contains(&std::path::PathBuf::from(".gitignore")));
    }

    #[test]
    fn scaffold_records_a_warning_when_post_write_fails() {
        let dir = tempfile::tempdir().unwrap();
        let report = scaffold(dir.path(), &python_profile(), &ctx(), &|_| {
            Err("uv not found".to_string())
        })
        .unwrap();
        assert!(report.warnings.iter().any(|w| w.contains("uv.lock")));
    }

    #[test]
    fn scaffold_never_touches_permissions_of_a_skipped_executable_file() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("scripts")).unwrap();
        std::fs::write(dir.path().join("scripts/check.sh"), "ORIGINAL\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(
                dir.path().join("scripts/check.sh"),
                std::fs::Permissions::from_mode(0o644),
            )
            .unwrap();
        }
        let report = scaffold(dir.path(), &python_profile(), &ctx(), &|_| Ok(())).unwrap();
        assert!(report
            .skipped
            .contains(&std::path::PathBuf::from("scripts/check.sh")));
        assert_eq!(
            std::fs::read_to_string(dir.path().join("scripts/check.sh")).unwrap(),
            "ORIGINAL\n",
            "existing content must be untouched"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(dir.path().join("scripts/check.sh"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(
                mode & 0o777,
                0o644,
                "an existing file's permissions must not be changed to 0755"
            );
        }
    }

    #[test]
    fn scaffold_skips_post_write_when_a_rerun_writes_nothing_new() {
        let dir = tempfile::tempdir().unwrap();
        scaffold(dir.path(), &python_profile(), &ctx(), &|_| Ok(())).unwrap();
        let called = std::cell::Cell::new(false);
        let report = scaffold(dir.path(), &python_profile(), &ctx(), &|_| {
            called.set(true);
            Ok(())
        })
        .unwrap();
        assert!(report.written.is_empty(), "the rerun writes nothing new");
        assert!(
            !called.get(),
            "post_write must not run when nothing was written"
        );
    }

    #[test]
    fn run_post_write_succeeds_for_a_zero_exit_program() {
        let dir = tempfile::tempdir().unwrap();
        let step = PostWriteStep {
            program: "true".to_string(),
            args: vec![],
            describe: "run true".to_string(),
        };
        assert!(run_post_write(dir.path(), &step).is_ok());
    }

    #[test]
    fn run_post_write_errs_for_a_nonzero_exit_program() {
        let dir = tempfile::tempdir().unwrap();
        let step = PostWriteStep {
            program: "false".to_string(),
            args: vec![],
            describe: "run false".to_string(),
        };
        assert!(run_post_write(dir.path(), &step).is_err());
    }

    #[test]
    fn run_post_write_errs_for_a_missing_binary() {
        let dir = tempfile::tempdir().unwrap();
        let step = PostWriteStep {
            program: "definitely-not-a-real-binary-xyzzy".to_string(),
            args: vec![],
            describe: "run a missing binary".to_string(),
        };
        assert!(run_post_write(dir.path(), &step).is_err());
    }

    /// Live check: emit the Python profile and run its real gate (uv sync, ruff, mypy,
    /// pytest). Ignored by default so the hermetic gate needs no toolchain; run on demand
    /// with `env -C <repo> cargo test -p tutti-app-core -- --ignored`. This is the one
    /// check that proves a generated repo is green on its first run, which the
    /// content-only assertions above cannot.
    #[test]
    #[ignore = "live: needs uv, ruff, mypy, pytest reachable via uv on PATH"]
    fn python_scaffold_passes_its_own_gate() {
        let dir = tempfile::tempdir().unwrap();
        let c = ctx();
        scaffold(dir.path(), &python_profile(), &c, &|s| {
            run_post_write(dir.path(), s)
        })
        .unwrap();
        let out = std::process::Command::new("bash")
            .arg("scripts/check.sh")
            .current_dir(dir.path())
            .output()
            .expect("run scripts/check.sh");
        assert!(
            out.status.success(),
            "the emitted Python scaffold failed its own gate\n--- stdout ---\n{}\n--- stderr ---\n{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
    }
}
