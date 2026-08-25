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

/// What a scaffold file is for, so one `StackProfile` serves both create (emits all) and
/// retrofit (emits `Tooling`, merges `Config`, never emits `Sample`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileRole {
    /// Placeholder source/tests. Create-only; retrofit never emits these.
    Sample,
    /// New additive files (the gate, CI, AGENTS.md, interpreter pin, .gitignore).
    Tooling,
    /// A config file retrofit merges into when it already exists (pyproject/tsconfig/
    /// package.json). Config files with no registered merger (Cargo.toml, go.mod) are
    /// left untouched by retrofit.
    Config,
}

/// One file a profile emits, relative to the repo root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScaffoldFile {
    pub path: PathBuf,
    pub contents: String,
    pub executable: bool,
    pub role: FileRole,
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

/// The built-in stacks, each an opinionated, agent-friendly per-language shape.
pub fn available_stacks() -> Vec<StackProfile> {
    vec![
        python_profile(),
        rust_profile(),
        typescript_profile(),
        go_profile(),
    ]
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
    let f = |path: &str, contents: String, executable: bool, role: FileRole| ScaffoldFile {
        path: PathBuf::from(path),
        contents,
        executable,
        role,
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
select = ["E", "F", "I", "UP", "B", "C4", "SIM", "PIE", "PERF", "RUF"]

[tool.ruff.lint.per-file-ignores]
"tests/**" = ["B011"]

[tool.mypy]
strict = true
python_version = "3.13"

[tool.pytest.ini_options]
addopts = "-q --strict-markers --strict-config"
pythonpath = ["src"]
testpaths = ["tests"]
"#), false, FileRole::Config),
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
"#), true, FileRole::Tooling),
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
"#), false, FileRole::Tooling),
        f("AGENTS.md", format!(
r#"# AGENTS.md

Python project. One command gates every change:

```
bash scripts/check.sh
```

Run it before opening a PR; it must exit 0. It runs ruff (format + an opinionated lint set:
bugbear, simplify, comprehensions, perf, and ruff's own rules, on top of the pyflakes/
pycodestyle/isort/pyupgrade baseline), mypy --strict, and pytest. Formatting is enforced,
do not hand-tune style. New functions ship with tests in `tests/`. Work merges into
`staging`, never `main`.

Layout: package under `src/{pkg}/`, tests under `tests/` mirroring it.
"#), false, FileRole::Tooling),
        f(".python-version", String::from("3.13\n"), false, FileRole::Tooling),
        f(".gitignore", String::from(
"__pycache__/\n.venv/\n.pytest_cache/\n.mypy_cache/\n.ruff_cache/\n"), false, FileRole::Tooling),
        f(&format!("src/{pkg}/__init__.py"), format!(
"\"\"\"The {pkg} package.\"\"\"\n\nfrom {pkg}.core import add\n\n__all__ = [\"add\"]\n"), false, FileRole::Sample),
        f(&format!("src/{pkg}/core.py"), String::from(
"\"\"\"Core helpers.\"\"\"\n\n\ndef add(a: int, b: int) -> int:\n    \"\"\"Return the sum of two integers.\"\"\"\n    return a + b\n"), false, FileRole::Sample),
        f("tests/test_core.py", format!(
"from {pkg} import add\n\n\ndef test_add() -> None:\n    assert add(2, 3) == 5\n"), false, FileRole::Sample),
    ]
}

/// The opinionated Rust stack: cargo fmt + clippy(-D warnings) + test, a library crate,
/// one canonical `scripts/check.sh` gate, CI, and an AGENTS.md naming the gate.
pub fn rust_profile() -> StackProfile {
    StackProfile {
        id: "rust",
        display_name: "Rust",
        files: rust_files,
        gate_commands: vec!["bash scripts/check.sh".to_string()],
        post_write: Some(PostWriteStep {
            program: "cargo".to_string(),
            args: vec!["generate-lockfile".to_string()],
            describe: "generate Cargo.lock".to_string(),
        }),
    }
}

fn rust_files(ctx: &ScaffoldContext) -> Vec<ScaffoldFile> {
    let pkg = &ctx.package_name;
    let f = |path: &str, contents: String, executable: bool, role: FileRole| ScaffoldFile {
        path: PathBuf::from(path),
        contents,
        executable,
        role,
    };
    vec![
        f(
            "Cargo.toml",
            format!(
                r#"[package]
name = "{pkg}"
version = "0.1.0"
edition = "2021"

[dependencies]

[lints.clippy]
semicolon_if_nothing_returned = "deny"
manual_let_else               = "deny"
explicit_iter_loop            = "deny"
map_unwrap_or                 = "deny"
needless_pass_by_value        = "deny"
inefficient_to_string         = "deny"
implicit_clone                = "deny"
dbg_macro    = "deny"
unwrap_used  = "deny"
expect_used  = "deny"
"#
            ),
            false,
            FileRole::Config,
        ),
        f(
            "clippy.toml",
            String::from(
                r#"allow-unwrap-in-tests = true
allow-expect-in-tests = true
"#,
            ),
            false,
            FileRole::Config,
        ),
        f(
            "src/lib.rs",
            format!(
                r#"//! The {pkg} library.

/// Return the sum of two integers.
pub fn add(a: i64, b: i64) -> i64 {{
    a + b
}}

#[cfg(test)]
mod tests {{
    use super::*;

    #[test]
    fn adds() {{
        assert_eq!(add(2, 3), 5);
    }}
}}
"#
            ),
            false,
            FileRole::Sample,
        ),
        f(
            "scripts/check.sh",
            String::from(
                r#"#!/usr/bin/env bash
# The one canonical gate. CI and the agent both run exactly this.
set -euo pipefail
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
"#,
            ),
            true,
            FileRole::Tooling,
        ),
        f(
            ".github/workflows/ci.yml",
            String::from(
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
      - uses: dtolnay/rust-toolchain@stable
        with:
          components: rustfmt, clippy
      - uses: Swatinem/rust-cache@v2
      - run: bash scripts/check.sh
"#,
            ),
            false,
            FileRole::Tooling,
        ),
        f(
            "AGENTS.md",
            String::from(
                r#"# AGENTS.md

Rust project. One command gates every change:

```
bash scripts/check.sh
```

Run it before opening a PR; it must exit 0. It runs `cargo fmt --check`, `cargo clippy
--all-targets -- -D warnings` against the crate's opinionated `[lints.clippy]` table
(`unwrap_used` and `expect_used` are denied outside `#[cfg(test)]`, `dbg!` is denied
everywhere), and `cargo test`. Formatting is enforced, do not hand-tune style, and clippy
warnings are hard errors. New functions ship with tests. Work merges into
`staging`, never `main`.

Layout: library crate under `src/`, tests inline as `#[cfg(test)]` modules or under `tests/`.
"#,
            ),
            false,
            FileRole::Tooling,
        ),
        f(
            ".gitignore",
            String::from("/target/\n"),
            false,
            FileRole::Tooling,
        ),
    ]
}

/// The opinionated TypeScript stack: bun + tsc(strict) + bun test, one canonical
/// `scripts/check.sh` gate, CI, and an AGENTS.md naming the gate.
pub fn typescript_profile() -> StackProfile {
    StackProfile {
        id: "typescript",
        display_name: "TypeScript",
        files: typescript_files,
        gate_commands: vec!["bash scripts/check.sh".to_string()],
        post_write: Some(PostWriteStep {
            program: "bun".to_string(),
            args: vec!["install".to_string()],
            describe: "generate bun.lock".to_string(),
        }),
    }
}

fn typescript_files(ctx: &ScaffoldContext) -> Vec<ScaffoldFile> {
    let pkg = &ctx.package_name;
    let f = |path: &str, contents: String, executable: bool, role: FileRole| ScaffoldFile {
        path: PathBuf::from(path),
        contents,
        executable,
        role,
    };
    vec![
        f(
            "package.json",
            format!(
                r#"{{
  "name": "{pkg}",
  "version": "0.1.0",
  "type": "module",
  "private": true,
  "scripts": {{
    "check": "tsc --noEmit && biome check . && bun test"
  }},
  "devDependencies": {{
    "typescript": "^5.7.0",
    "bun-types": "^1.1.0",
    "@biomejs/biome": "^2.0.0"
  }}
}}
"#
            ),
            false,
            FileRole::Config,
        ),
        f(
            "tsconfig.json",
            String::from(
                r#"{
  "compilerOptions": {
    "strict": true,
    "noUncheckedIndexedAccess": true,
    "exactOptionalPropertyTypes": true,
    "noImplicitOverride": true,
    "noFallthroughCasesInSwitch": true,
    "noImplicitReturns": true,
    "noPropertyAccessFromIndexSignature": true,
    "verbatimModuleSyntax": true,
    "forceConsistentCasingInFileNames": true,
    "module": "esnext",
    "moduleResolution": "bundler",
    "target": "es2023",
    "types": ["bun-types"],
    "noEmit": true,
    "skipLibCheck": true
  },
  "include": ["src"]
}
"#,
            ),
            false,
            FileRole::Config,
        ),
        f(
            "biome.json",
            String::from(
                r#"{
  "$schema": "https://biomejs.dev/schemas/2.0.0/schema.json",
  "linter": { "enabled": true, "rules": { "recommended": true } },
  "formatter": { "enabled": true, "indentStyle": "space" }
}
"#,
            ),
            false,
            FileRole::Config,
        ),
        f(
            "src/index.ts",
            String::from(
                r#"/** Return the sum of two integers. */
export function add(a: number, b: number): number {
  return a + b;
}
"#,
            ),
            false,
            FileRole::Sample,
        ),
        f(
            "src/index.test.ts",
            String::from(
                r#"import { expect, test } from "bun:test";
import { add } from "./index";

test("add", () => {
  expect(add(2, 3)).toBe(5);
});
"#,
            ),
            false,
            FileRole::Sample,
        ),
        f(
            "scripts/check.sh",
            String::from(
                r#"#!/usr/bin/env bash
# The one canonical gate. CI and the agent both run exactly this.
set -euo pipefail
# Prefer the locked, reproducible install; fall back if there is no lockfile yet (e.g.
# `bun install` was unavailable at scaffold time).
bun install --frozen-lockfile 2>/dev/null || bun install
bunx tsc --noEmit
bunx @biomejs/biome check .
bun test
"#,
            ),
            true,
            FileRole::Tooling,
        ),
        f(
            ".github/workflows/ci.yml",
            String::from(
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
      - uses: oven-sh/setup-bun@v2
      - run: bash scripts/check.sh
"#,
            ),
            false,
            FileRole::Tooling,
        ),
        f(
            "AGENTS.md",
            String::from(
                r#"# AGENTS.md

TypeScript project (Bun). One command gates every change:

```
bash scripts/check.sh
```

Run it before opening a PR; it must exit 0. It installs dependencies, then runs `tsc
--noEmit` (strict type-check, plus `noUncheckedIndexedAccess` and
`exactOptionalPropertyTypes`), `biome check .` (lint and format), and `bun test`. Types
and formatting are enforced; do not loosen them. New functions ship with tests in `src/`.
Work merges into `staging`, never `main`.

Layout: source and colocated `*.test.ts` under `src/`.
"#,
            ),
            false,
            FileRole::Tooling,
        ),
        f(
            ".gitignore",
            String::from("node_modules/\n"),
            false,
            FileRole::Tooling,
        ),
    ]
}

/// The opinionated Go stack: gofmt + go vet + go test, one canonical `scripts/check.sh`
/// gate, CI, and an AGENTS.md naming the gate.
pub fn go_profile() -> StackProfile {
    StackProfile {
        id: "go",
        display_name: "Go",
        files: go_files,
        gate_commands: vec!["bash scripts/check.sh".to_string()],
        post_write: None,
    }
}

fn go_files(ctx: &ScaffoldContext) -> Vec<ScaffoldFile> {
    let pkg = &ctx.package_name;
    let f = |path: &str, contents: String, executable: bool, role: FileRole| ScaffoldFile {
        path: PathBuf::from(path),
        contents,
        executable,
        role,
    };
    // Go source is tab-indented (gofmt enforces it), so these use explicit `\t`.
    vec![
        f(
            "go.mod",
            format!("module example.com/{pkg}\n\ngo 1.23\n"),
            false,
            FileRole::Config,
        ),
        f(
            &format!("{pkg}.go"),
            format!(
                "// Package {pkg} is a small library.\npackage {pkg}\n\n\
             // Add returns the sum of two integers.\nfunc Add(a, b int) int {{\n\
             \treturn a + b\n}}\n"
            ),
            false,
            FileRole::Sample,
        ),
        f(
            &format!("{pkg}_test.go"),
            format!(
                "package {pkg}\n\nimport \"testing\"\n\n\
             func TestAdd(t *testing.T) {{\n\
             \tif got := Add(2, 3); got != 5 {{\n\
             \t\tt.Errorf(\"Add(2, 3) = %d, want 5\", got)\n\
             \t}}\n}}\n"
            ),
            false,
            FileRole::Sample,
        ),
        f(
            ".golangci.yml",
            String::from(
                r#"version: "2"
linters:
  default: none
  enable:
    - errcheck
    - govet
    - ineffassign
    - staticcheck
    - unused
"#,
            ),
            false,
            FileRole::Config,
        ),
        f(
            "scripts/check.sh",
            String::from(
                r#"#!/usr/bin/env bash
# The one canonical gate. CI and the agent both run exactly this.
set -euo pipefail
test -z "$(gofmt -l .)" && go vet ./... && golangci-lint run ./... && go test ./...
"#,
            ),
            true,
            FileRole::Tooling,
        ),
        f(
            ".github/workflows/ci.yml",
            String::from(
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
      - uses: actions/setup-go@v5
        with:
          go-version: "1.23"
      - uses: golangci/golangci-lint-action@v6
        with:
          version: v2.0.0
          install-mode: binary
      - run: bash scripts/check.sh
"#,
            ),
            false,
            FileRole::Tooling,
        ),
        f(
            "AGENTS.md",
            String::from(
                r#"# AGENTS.md

Go project. One command gates every change:

```
bash scripts/check.sh
```

Run it before opening a PR; it must exit 0. It checks `gofmt` (no unformatted files), then
runs `go vet ./...`, `golangci-lint run ./...` (staticcheck, errcheck, ineffassign, unused),
and `go test ./...`. `golangci-lint` is a prerequisite, not part of the Go toolchain.
Formatting is enforced by gofmt, do not hand-tune style. New functions ship with
`*_test.go` tests. Work merges into `staging`, never `main`.

Layout: package sources at the module root, tests as `*_test.go` beside them.
"#,
            ),
            false,
            FileRole::Tooling,
        ),
        f(
            ".gitignore",
            String::from("*.exe\n*.test\n*.out\n"),
            false,
            FileRole::Tooling,
        ),
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
        for needle in [
            "[tool.ruff]",
            "strict = true",
            "pytest",
            "requires-python",
            "\"B\"",
            "\"SIM\"",
            "\"RUF\"",
        ] {
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
    fn python_profile_tags_file_roles() {
        let files = (python_profile().files)(&ctx());
        let role_of = |rel: &str| {
            files
                .iter()
                .find(|f| f.path == std::path::Path::new(rel))
                .unwrap_or_else(|| panic!("missing {rel}"))
                .role
        };
        assert_eq!(role_of("src/my_repo/core.py"), FileRole::Sample);
        assert_eq!(role_of("tests/test_core.py"), FileRole::Sample);
        assert_eq!(role_of("scripts/check.sh"), FileRole::Tooling);
        assert_eq!(role_of(".github/workflows/ci.yml"), FileRole::Tooling);
        assert_eq!(role_of("pyproject.toml"), FileRole::Config);
    }

    #[test]
    fn available_stacks_lists_every_language_profile() {
        let ids: Vec<&str> = available_stacks().iter().map(|s| s.id).collect();
        for id in ["python", "rust", "typescript", "go"] {
            assert!(ids.contains(&id), "available_stacks missing {id}");
        }
    }

    /// Every profile shares the uniform canonical gate and emits the four agent-facing
    /// contract files (the gate, CI, AGENTS.md, .gitignore), each executable-correct.
    #[test]
    fn every_profile_emits_the_uniform_contract() {
        for profile in available_stacks() {
            let id = profile.id;
            assert_eq!(
                profile.gate_commands,
                vec!["bash scripts/check.sh".to_string()],
                "{id} gate is not the canonical scripts/check.sh"
            );
            let files = (profile.files)(&ctx());
            let by_path = |rel: &str| {
                files
                    .iter()
                    .find(|f| f.path == std::path::Path::new(rel))
                    .unwrap_or_else(|| panic!("{id} missing {rel}"))
            };
            let check = by_path("scripts/check.sh");
            assert!(check.executable, "{id} scripts/check.sh must be executable");
            assert!(
                check.contents.starts_with("#!/usr/bin/env bash"),
                "{id} scripts/check.sh must be a bash script"
            );
            let ci = &by_path(".github/workflows/ci.yml").contents;
            assert!(
                ci.contains("bash scripts/check.sh"),
                "{id} CI must run the canonical gate"
            );
            let agents = &by_path("AGENTS.md").contents;
            assert!(
                agents.contains("bash scripts/check.sh"),
                "{id} AGENTS.md must name the canonical gate"
            );
            assert!(
                agents.contains("`staging`"),
                "{id} AGENTS.md must state the staging convention"
            );
            by_path(".gitignore");
        }
    }

    #[test]
    fn rust_profile_emits_the_opinionated_stack() {
        let files = (rust_profile().files)(&ctx());
        let by_path = |rel: &str| {
            files
                .iter()
                .find(|f| f.path == std::path::Path::new(rel))
                .unwrap_or_else(|| panic!("missing {rel}"))
        };
        let cargo_toml = &by_path("Cargo.toml").contents;
        assert!(cargo_toml.contains("name = \"my_repo\""));
        assert!(cargo_toml.contains("[lints.clippy]"));
        assert!(cargo_toml.contains("unwrap_used  = \"deny\""));
        assert!(by_path("clippy.toml")
            .contents
            .contains("allow-unwrap-in-tests"));
        by_path("src/lib.rs");
        let check = &by_path("scripts/check.sh").contents;
        for needle in [
            "cargo fmt --check",
            "cargo clippy --all-targets -- -D warnings",
            "cargo test",
        ] {
            assert!(check.contains(needle), "rust check.sh missing {needle}");
        }
        assert!(by_path(".github/workflows/ci.yml")
            .contents
            .contains("dtolnay/rust-toolchain"));
    }

    #[test]
    fn typescript_profile_emits_the_opinionated_stack() {
        let files = (typescript_profile().files)(&ctx());
        let by_path = |rel: &str| {
            files
                .iter()
                .find(|f| f.path == std::path::Path::new(rel))
                .unwrap_or_else(|| panic!("missing {rel}"))
        };
        assert!(by_path("package.json")
            .contents
            .contains("\"name\": \"my_repo\""));
        let tsconfig = &by_path("tsconfig.json").contents;
        for needle in [
            "\"strict\": true",
            "\"noUncheckedIndexedAccess\": true",
            "\"exactOptionalPropertyTypes\": true",
        ] {
            assert!(tsconfig.contains(needle), "tsconfig missing {needle}");
        }
        assert!(by_path("biome.json").contents.contains("\"linter\""));
        by_path("src/index.ts");
        by_path("src/index.test.ts");
        let check = &by_path("scripts/check.sh").contents;
        for needle in [
            "bun install",
            "bunx tsc --noEmit",
            "biome check",
            "bun test",
        ] {
            assert!(check.contains(needle), "ts check.sh missing {needle}");
        }
        assert!(by_path(".github/workflows/ci.yml")
            .contents
            .contains("oven-sh/setup-bun"));
    }

    #[test]
    fn go_profile_emits_the_opinionated_stack() {
        let files = (go_profile().files)(&ctx());
        let by_path = |rel: &str| {
            files
                .iter()
                .find(|f| f.path == std::path::Path::new(rel))
                .unwrap_or_else(|| panic!("missing {rel}"))
        };
        assert!(by_path("go.mod")
            .contents
            .contains("module example.com/my_repo"));
        // Go sources are tab-indented (gofmt), never spaces.
        let src = &by_path("my_repo.go").contents;
        assert!(
            src.contains("\treturn a + b"),
            "go source must be tab-indented"
        );
        assert!(src.contains("package my_repo"));
        assert!(
            src.contains("// Package my_repo"),
            "go source must carry a package doc comment (staticcheck ST1000)"
        );
        by_path("my_repo_test.go");
        let golangci = &by_path(".golangci.yml").contents;
        for needle in ["staticcheck", "errcheck"] {
            assert!(golangci.contains(needle), "golangci.yml missing {needle}");
        }
        let check = &by_path("scripts/check.sh").contents;
        for needle in [
            "gofmt -l .",
            "go vet ./...",
            "golangci-lint run",
            "go test ./...",
        ] {
            assert!(check.contains(needle), "go check.sh missing {needle}");
        }
        assert!(by_path(".github/workflows/ci.yml")
            .contents
            .contains("actions/setup-go"));
        assert!(by_path(".github/workflows/ci.yml")
            .contents
            .contains("golangci-lint"));
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
        assert_scaffold_passes_its_own_gate(&python_profile());
    }

    /// Emit `profile` into a temp dir (running its real post-write) and run its
    /// `scripts/check.sh`, asserting a green first run. This is the only check that proves
    /// a generated repo is green on its first CI, which content-only assertions cannot.
    fn assert_scaffold_passes_its_own_gate(profile: &StackProfile) {
        let dir = tempfile::tempdir().unwrap();
        let c = ctx();
        scaffold(dir.path(), profile, &c, &|s| run_post_write(dir.path(), s)).unwrap();
        let out = std::process::Command::new("bash")
            .arg("scripts/check.sh")
            .current_dir(dir.path())
            .output()
            .expect("run scripts/check.sh");
        assert!(
            out.status.success(),
            "the emitted {} scaffold failed its own gate\n--- stdout ---\n{}\n--- stderr ---\n{}",
            profile.display_name,
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
    }

    #[test]
    #[ignore = "live: needs a cargo toolchain with rustfmt + clippy on PATH"]
    fn rust_scaffold_passes_its_own_gate() {
        assert_scaffold_passes_its_own_gate(&rust_profile());
    }

    #[test]
    #[ignore = "live: needs bun (and network for `bun install`) on PATH"]
    fn typescript_scaffold_passes_its_own_gate() {
        assert_scaffold_passes_its_own_gate(&typescript_profile());
    }

    #[test]
    #[ignore = "live: needs a go toolchain (gofmt, go) on PATH"]
    fn go_scaffold_passes_its_own_gate() {
        assert_scaffold_passes_its_own_gate(&go_profile());
    }
}
