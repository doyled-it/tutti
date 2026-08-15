// SPDX-License-Identifier: AGPL-3.0-or-later
//! Retrofit an existing repo: detect its language, install the opinionated rails
//! additively, merge our config into existing config files, and report the baseline gap.

use std::path::Path;

use toml_edit::{value, Array, DocumentMut, Item, Table};

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

/// Ensure the table at `path` exists (creating intermediate tables), returning it.
/// `None` if an existing segment is present but is not a table (e.g. an inline table),
/// so the caller can bail out without fabricating or clobbering anything.
fn ensure_table<'a>(doc: &'a mut DocumentMut, path: &[&str]) -> Option<&'a mut Table> {
    let mut tbl = doc.as_table_mut();
    for key in path {
        tbl = tbl
            .entry(key)
            .or_insert_with(|| Item::Table(Table::new()))
            .as_table_mut()?;
    }
    Some(tbl)
}

/// Set `key = val` only if the key is absent (additive: preserves a user's value).
fn set_if_absent(tbl: &mut Table, key: &str, val: toml_edit::Value) {
    if !tbl.contains_key(key) {
        tbl.insert(key, value(val));
    }
}

/// Ensure `key` is a string array containing every element of `wanted` (union: adds the
/// missing ones, drops nothing). Only mutates when something is actually missing, so an
/// already-complete array is left byte-identical (idempotency).
fn ensure_str_array(tbl: &mut Table, key: &str, wanted: &[&str]) {
    let item = tbl.entry(key).or_insert(value(Array::new()));
    if let Some(arr) = item.as_array_mut() {
        for w in wanted {
            if !arr.iter().any(|v| v.as_str() == Some(*w)) {
                arr.push(*w);
            }
        }
    }
}

/// Merge Sotto's opinionated Python config into an existing `pyproject.toml`, preserving
/// the user's content and comments. Additive for absent keys; enforces the opinion-defining
/// strictness keys (`mypy strict`, the ruff lint selection); idempotent.
///
/// Returns `None` if `existing` does not parse as TOML, or if a section we need to write
/// into is present but shaped unexpectedly (e.g. an inline table): the file is never
/// fabricated or clobbered, the caller leaves it untouched.
pub fn merge_pyproject(existing: &str) -> Option<String> {
    let mut doc: DocumentMut = existing.parse().ok()?;

    // [dependency-groups] dev: union our tools in.
    let dg = ensure_table(&mut doc, &["dependency-groups"])?;
    ensure_str_array(dg, "dev", &["ruff", "mypy", "pytest"]);

    // [tool.ruff]: line-length + target-version are additive (cosmetic).
    let ruff = ensure_table(&mut doc, &["tool", "ruff"])?;
    set_if_absent(ruff, "line-length", 88_i64.into());
    set_if_absent(ruff, "target-version", "py313".into());
    // [tool.ruff.lint]: enforce our lint selection (union, so a floor not a cap).
    let lint = ensure_table(&mut doc, &["tool", "ruff", "lint"])?;
    ensure_str_array(lint, "extend-select", &["I", "UP", "B", "SIM", "RUF"]);

    // [tool.mypy]: enforce strict = true (opinion-defining); python_version additive.
    let mypy = ensure_table(&mut doc, &["tool", "mypy"])?;
    if mypy.get("strict").and_then(|i| i.as_bool()) != Some(true) {
        mypy.insert("strict", value(true));
    }
    set_if_absent(mypy, "python_version", "3.13".into());

    // [tool.pytest.ini_options]: additive.
    let pytest = ensure_table(&mut doc, &["tool", "pytest", "ini_options"])?;
    set_if_absent(
        pytest,
        "addopts",
        "-q --strict-markers --strict-config".into(),
    );
    if !pytest.contains_key("pythonpath") {
        pytest.insert("pythonpath", value(Array::from_iter(["src"])));
    }
    if !pytest.contains_key("testpaths") {
        pytest.insert("testpaths", value(Array::from_iter(["tests"])));
    }

    Some(doc.to_string())
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

    #[test]
    fn merge_pyproject_adds_our_sections_to_a_bare_file() {
        let out = merge_pyproject("[project]\nname = \"x\"\n").unwrap();
        assert!(out.contains("[tool.ruff]"));
        assert!(out.contains("strict = true")); // mypy
        assert!(out.contains("[tool.pytest.ini_options]"));
        assert!(out.contains("[dependency-groups]"));
        assert!(out.contains("name = \"x\"")); // user content preserved
    }

    #[test]
    fn merge_pyproject_enforces_mypy_strict_over_a_weaker_value() {
        let out = merge_pyproject("[tool.mypy]\nstrict = false\n").unwrap();
        assert!(out.contains("strict = true"));
        assert!(!out.contains("strict = false"));
    }

    #[test]
    fn merge_pyproject_preserves_a_cosmetic_user_value() {
        let out = merge_pyproject("[tool.ruff]\nline-length = 100\n").unwrap();
        assert!(out.contains("line-length = 100"));
    }

    #[test]
    fn merge_pyproject_unions_dev_deps_without_dropping_the_users() {
        let out = merge_pyproject("[dependency-groups]\ndev = [\"pytest-cov\"]\n").unwrap();
        assert!(out.contains("pytest-cov"));
        assert!(out.contains("ruff"));
        assert!(out.contains("mypy"));
    }

    #[test]
    fn merge_pyproject_is_idempotent() {
        let once = merge_pyproject("[project]\nname = \"x\"\n").unwrap();
        let twice = merge_pyproject(&once).unwrap();
        assert_eq!(once, twice, "a second merge must produce no further change");
    }

    #[test]
    fn merge_pyproject_preserves_comments() {
        let out = merge_pyproject("# keep me\n[project]\nname = \"x\"\n").unwrap();
        assert!(out.contains("# keep me"));
    }

    #[test]
    fn merge_pyproject_returns_none_on_unparseable_input_never_fabricating() {
        // A malformed pyproject must not be silently replaced with a fresh file.
        assert!(merge_pyproject("this is not [[[ valid toml = = \"unterminated").is_none());
    }
}
