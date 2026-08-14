// SPDX-License-Identifier: AGPL-3.0-or-later
//! Opinionated per-language scaffolds. A `StackProfile` is declarative data: the files
//! to emit, the canonical gate command, and an optional post-write step. The `scaffold`
//! emitter writes a profile into a repo directory, never clobbering an existing file.

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
}
