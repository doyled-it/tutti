// SPDX-License-Identifier: AGPL-3.0-or-later
//! Retrofit an existing repo: detect its language, install the opinionated rails
//! additively, merge our config into existing config files, and report the baseline gap.

use std::path::Path;

/// The stack ids retrofit can detect, in a stable order. Each maps to a `StackProfile`
/// id in `scaffold.rs`.
pub fn detect_languages(dir: &Path) -> Vec<String> {
    let has = |name: &str| dir.join(name).exists();
    let mut out = Vec::new();
    if has("pyproject.toml") || has("setup.py") || has("setup.cfg") || has("requirements.txt") {
        out.push("python".to_string());
    }
    if has("Cargo.toml") {
        out.push("rust".to_string());
    }
    if has("package.json") || has("tsconfig.json") {
        out.push("typescript".to_string());
    }
    if has("go.mod") {
        out.push("go".to_string());
    }
    out
}

/// The ignore lines from `wanted` not already present in `existing` (exact match after
/// trimming each existing line), in `wanted` order. Used to append-merge a `.gitignore`
/// without disturbing what is already there.
pub fn gitignore_missing_lines(existing: &str, wanted: &[&str]) -> Vec<String> {
    let present: std::collections::HashSet<&str> = existing.lines().map(|l| l.trim()).collect();
    wanted
        .iter()
        .filter(|w| !present.contains(w.trim()))
        .map(|w| w.to_string())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir_with(files: &[&str]) -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        for f in files {
            std::fs::write(d.path().join(f), "x").unwrap();
        }
        d
    }

    #[test]
    fn detects_each_language_by_marker() {
        assert_eq!(
            detect_languages(dir_with(&["pyproject.toml"]).path()),
            vec!["python"]
        );
        assert_eq!(
            detect_languages(dir_with(&["Cargo.toml"]).path()),
            vec!["rust"]
        );
        assert_eq!(detect_languages(dir_with(&["go.mod"]).path()), vec!["go"]);
        assert_eq!(
            detect_languages(dir_with(&["tsconfig.json"]).path()),
            vec!["typescript"]
        );
    }

    #[test]
    fn python_is_detected_by_any_of_its_markers_once() {
        let d = dir_with(&["setup.py", "requirements.txt"]);
        assert_eq!(detect_languages(d.path()), vec!["python"]);
    }

    #[test]
    fn no_markers_detects_nothing() {
        assert!(detect_languages(dir_with(&["README.md"]).path()).is_empty());
    }

    #[test]
    fn multiple_languages_are_all_reported() {
        let d = dir_with(&["Cargo.toml", "go.mod"]);
        assert_eq!(detect_languages(d.path()), vec!["rust", "go"]);
    }

    #[test]
    fn gitignore_returns_only_missing_lines_preserving_order() {
        let existing = "target/\n.venv/\n";
        let wanted = ["target/", ".venv/", "__pycache__/", ".ruff_cache/"];
        assert_eq!(
            gitignore_missing_lines(existing, &wanted),
            vec!["__pycache__/".to_string(), ".ruff_cache/".to_string()]
        );
    }

    #[test]
    fn gitignore_ignores_surrounding_whitespace_when_matching() {
        let existing = "  target/  \n";
        assert_eq!(
            gitignore_missing_lines(existing, &["target/"]),
            Vec::<String>::new()
        );
    }

    #[test]
    fn gitignore_wants_all_when_file_is_empty() {
        assert_eq!(
            gitignore_missing_lines("", &["a", "b"]),
            vec!["a".to_string(), "b".to_string()]
        );
    }
}
