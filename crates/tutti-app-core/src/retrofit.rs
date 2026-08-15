// SPDX-License-Identifier: AGPL-3.0-or-later
//! Retrofit an existing repo: detect its language, install the opinionated rails
//! additively, merge our config into existing config files, and report the baseline gap.

use std::path::{Path, PathBuf};

use toml_edit::{value, Array, DocumentMut, Item, Table};

use crate::scaffold::{FileRole, ScaffoldContext, ScaffoldFile, StackProfile};

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

use serde_json::{Map, Value};

/// Strip `//` line comments so a JSONC `tsconfig.json` parses. Block comments are rare in
/// tsconfig; a merged output is re-serialized without comments (a reformat the plan surfaces).
fn strip_jsonc(input: &str) -> String {
    input
        .lines()
        .map(|l| match l.find("//") {
            Some(i) => &l[..i],
            None => l,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// A nested value coerced to an object (an absent or non-object nested field becomes an
/// empty object we then fill). The ROOT is guarded separately (see `json_root_object`).
fn object_or_new(v: Value) -> Map<String, Value> {
    match v {
        Value::Object(m) => m,
        _ => Map::new(),
    }
}

/// The root object of a JSON document, or `None` if it does not parse as a JSON object.
/// Returning `None` preserves the no-clobber guarantee: a malformed or non-object config is
/// left untouched by the caller rather than replaced with a fresh file.
fn json_root_object(parsed: Result<Value, serde_json::Error>) -> Option<Map<String, Value>> {
    match parsed.ok()? {
        Value::Object(m) => Some(m),
        _ => None,
    }
}

/// Merge Sotto's strict TypeScript compiler flags into an existing `tsconfig.json`.
/// Enforces the strictness flags; preserves other options; idempotent. Returns `None`
/// (leave the file untouched) if the existing content is not a JSON object.
pub fn merge_tsconfig(existing: &str) -> Option<String> {
    let mut root = json_root_object(serde_json::from_str(&strip_jsonc(existing)))?;
    let mut co = object_or_new(root.remove("compilerOptions").unwrap_or(Value::Null));
    for (k, v) in [
        ("strict", Value::Bool(true)),
        ("noUncheckedIndexedAccess", Value::Bool(true)),
    ] {
        co.insert(k.to_string(), v);
    }
    for (k, v) in [
        ("module", Value::String("esnext".into())),
        ("moduleResolution", Value::String("bundler".into())),
        ("target", Value::String("es2023".into())),
        ("skipLibCheck", Value::Bool(true)),
    ] {
        co.entry(k.to_string()).or_insert(v);
    }
    root.insert("compilerOptions".into(), Value::Object(co));
    Some(serde_json::to_string_pretty(&Value::Object(root)).unwrap() + "\n")
}

/// Merge the `typescript` + `bun-types` dev dependencies and the `check` script into an
/// existing `package.json`, preserving every other field. Idempotent. Returns `None`
/// (leave the file untouched) if the existing content is not a JSON object.
pub fn merge_package_json(existing: &str) -> Option<String> {
    let mut root = json_root_object(serde_json::from_str(existing))?;

    let mut scripts = object_or_new(root.remove("scripts").unwrap_or(Value::Null));
    scripts
        .entry("check".to_string())
        .or_insert(Value::String("tsc --noEmit && bun test".into()));
    root.insert("scripts".into(), Value::Object(scripts));

    let mut dev = object_or_new(root.remove("devDependencies").unwrap_or(Value::Null));
    dev.entry("typescript".to_string())
        .or_insert(Value::String("^5.7.0".into()));
    dev.entry("bun-types".to_string())
        .or_insert(Value::String("^1.1.0".into()));
    root.insert("devDependencies".into(), Value::Object(dev));

    Some(serde_json::to_string_pretty(&Value::Object(root)).unwrap() + "\n")
}

/// A config file that exists and will be merged, with a preview diff.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigMerge {
    pub path: PathBuf,
    pub new_contents: String,
    /// A unified diff old -> new for the operator to confirm.
    pub diff: String,
}

/// A tooling file skipped because it already exists (reported, never overwritten).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkipReason {
    pub path: PathBuf,
    pub why: String,
}

/// The computed, not-yet-applied retrofit.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RetrofitPlan {
    pub adds: Vec<ScaffoldFile>,
    pub merges: Vec<ConfigMerge>,
    pub gitignore_append: Vec<String>,
    pub skipped: Vec<SkipReason>,
    pub already: Vec<PathBuf>,
}

/// Whether retrofit has a registered merger for this `Config` file. Cargo.toml and go.mod
/// have none (their gates need no config), so retrofit leaves them untouched.
fn has_merger(stack_id: &str, path: &Path) -> bool {
    matches!(
        (stack_id, path.file_name().and_then(|s| s.to_str())),
        ("python", Some("pyproject.toml"))
            | ("typescript", Some("tsconfig.json"))
            | ("typescript", Some("package.json"))
    )
}

/// Run the registered merger for a `Config` file. Returns `None` when the existing file
/// cannot be parsed (retrofit then leaves it untouched, never fabricating over it).
fn run_merger(stack_id: &str, path: &Path, existing: &str) -> Option<String> {
    match (stack_id, path.file_name().and_then(|s| s.to_str())) {
        ("python", Some("pyproject.toml")) => merge_pyproject(existing),
        ("typescript", Some("tsconfig.json")) => merge_tsconfig(existing),
        ("typescript", Some("package.json")) => merge_package_json(existing),
        _ => None,
    }
}

/// A minimal line-level +/- diff for the preview. Deliberately simple: it exists to let the
/// operator see what changes, not to be a patch tool.
fn simple_diff(old: &str, new: &str) -> String {
    if old == new {
        return String::new();
    }
    let mut out = String::new();
    for line in old.lines() {
        out.push_str(&format!("- {line}\n"));
    }
    for line in new.lines() {
        out.push_str(&format!("+ {line}\n"));
    }
    out
}

/// The ignore lines each stack wants in `.gitignore`.
fn gitignore_wants(stack_id: &str) -> &'static [&'static str] {
    match stack_id {
        "python" => &[
            "__pycache__/",
            ".venv/",
            ".pytest_cache/",
            ".mypy_cache/",
            ".ruff_cache/",
        ],
        "rust" => &["/target/"],
        "typescript" => &["node_modules/"],
        "go" => &["*.exe", "*.test", "*.out"],
        _ => &[],
    }
}

/// Compute the retrofit for `profile` against `dir` without writing anything.
pub fn plan_retrofit(dir: &Path, profile: &StackProfile, ctx: &ScaffoldContext) -> RetrofitPlan {
    let mut plan = RetrofitPlan::default();
    for file in (profile.files)(ctx) {
        let abs = dir.join(&file.path);
        let exists = abs.exists();
        match file.role {
            FileRole::Sample => {}
            FileRole::Tooling => {
                if file.path == Path::new(".gitignore") {
                    continue;
                }
                if exists {
                    plan.skipped.push(SkipReason {
                        path: file.path.clone(),
                        why: "already present; left untouched".into(),
                    });
                } else {
                    plan.adds.push(file);
                }
            }
            FileRole::Config => {
                if !exists {
                    plan.adds.push(file);
                    continue;
                }
                if !has_merger(profile.id, &file.path) {
                    plan.already.push(file.path.clone());
                    continue;
                }
                let existing = std::fs::read_to_string(&abs).unwrap_or_default();
                match run_merger(profile.id, &file.path, &existing) {
                    Some(new_contents) if new_contents != existing => {
                        let diff = simple_diff(&existing, &new_contents);
                        plan.merges.push(ConfigMerge {
                            path: file.path.clone(),
                            new_contents,
                            diff,
                        });
                    }
                    Some(_) => plan.already.push(file.path.clone()),
                    None => plan.skipped.push(SkipReason {
                        path: file.path.clone(),
                        why: "could not parse; left untouched".into(),
                    }),
                }
            }
        }
    }
    let gi_path = dir.join(".gitignore");
    let existing = std::fs::read_to_string(&gi_path).unwrap_or_default();
    plan.gitignore_append = gitignore_missing_lines(&existing, gitignore_wants(profile.id));
    plan
}

use std::io::Write as _;

/// What `apply_retrofit` wrote.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RetrofitReport {
    pub written: Vec<PathBuf>,
    pub merged: Vec<PathBuf>,
    pub gitignore_appended: usize,
}

/// Apply a confirmed plan to disk. Creates parent dirs, sets the executable bit on the
/// gate, writes merged config files in place, and appends the missing .gitignore lines.
/// Call only after the operator has confirmed the plan.
pub fn apply_retrofit(dir: &Path, plan: &RetrofitPlan) -> std::io::Result<RetrofitReport> {
    let mut report = RetrofitReport::default();
    for file in &plan.adds {
        let abs = dir.join(&file.path);
        if let Some(parent) = abs.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&abs, &file.contents)?;
        #[cfg(unix)]
        if file.executable {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&abs, std::fs::Permissions::from_mode(0o755))?;
        }
        report.written.push(file.path.clone());
    }
    for merge in &plan.merges {
        std::fs::write(dir.join(&merge.path), &merge.new_contents)?;
        report.merged.push(merge.path.clone());
    }
    if !plan.gitignore_append.is_empty() {
        let gi = dir.join(".gitignore");
        // If the file exists without a trailing newline, add one first so the first
        // appended line does not join the last existing line.
        let needs_leading_newline = match std::fs::read_to_string(&gi) {
            Ok(s) => !s.is_empty() && !s.ends_with('\n'),
            Err(_) => false,
        };
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&gi)?;
        if needs_leading_newline {
            writeln!(f)?;
        }
        for line in &plan.gitignore_append {
            writeln!(f, "{line}")?;
        }
        report.gitignore_appended = plan.gitignore_append.len();
    }
    Ok(report)
}

/// Build the profile's gate and run it once under `dir`, returning the pass flag and the
/// combined log for the baseline report. Async because the engine's Gate runs processes.
pub async fn run_baseline_gate(
    dir: &Path,
    profile: &StackProfile,
) -> tutti_core::traits::Result<tutti_core::gate::GateOutcome> {
    let gate = tutti_core::gate::Gate {
        commands: profile.gate_commands.clone(),
        working_dir: PathBuf::new(),
    };
    gate.run(dir).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scaffold::{package_name, stack_profile, ScaffoldContext};

    fn ctx() -> ScaffoldContext {
        ScaffoldContext {
            repo_name: "legacy".into(),
            package_name: package_name("legacy"),
        }
    }

    #[test]
    fn plan_excludes_sample_files_and_adds_only_missing_tooling() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("pyproject.toml"), "[project]\nname='x'\n").unwrap();
        let profile = stack_profile("python").unwrap();
        let plan = plan_retrofit(d.path(), &profile, &ctx());
        assert!(plan
            .adds
            .iter()
            .all(|f| f.path != std::path::Path::new("src/legacy/core.py")));
        assert!(plan
            .adds
            .iter()
            .any(|f| f.path == std::path::Path::new("scripts/check.sh")));
        assert!(plan
            .merges
            .iter()
            .any(|m| m.path == std::path::Path::new("pyproject.toml")));
        assert!(plan
            .adds
            .iter()
            .all(|f| f.path != std::path::Path::new("pyproject.toml")));
    }

    #[test]
    fn plan_skips_a_tooling_file_that_already_exists() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("Cargo.toml"), "[package]\nname='x'\n").unwrap();
        std::fs::create_dir_all(d.path().join("scripts")).unwrap();
        std::fs::write(d.path().join("scripts/check.sh"), "own\n").unwrap();
        let profile = stack_profile("rust").unwrap();
        let plan = plan_retrofit(d.path(), &profile, &ctx());
        assert!(plan
            .skipped
            .iter()
            .any(|s| s.path == std::path::Path::new("scripts/check.sh")));
        assert!(plan
            .adds
            .iter()
            .all(|f| f.path != std::path::Path::new("scripts/check.sh")));
    }

    #[test]
    fn plan_skips_an_unparseable_config_instead_of_clobbering_it() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(
            d.path().join("pyproject.toml"),
            "this is [[[ not valid = =\n",
        )
        .unwrap();
        let profile = stack_profile("python").unwrap();
        let plan = plan_retrofit(d.path(), &profile, &ctx());
        assert!(plan
            .skipped
            .iter()
            .any(|s| s.path == std::path::Path::new("pyproject.toml")));
        assert!(plan
            .merges
            .iter()
            .all(|m| m.path != std::path::Path::new("pyproject.toml")));
    }

    #[test]
    fn plan_leaves_a_config_with_no_merger_in_already() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("Cargo.toml"), "[package]\nname='x'\n").unwrap();
        let profile = stack_profile("rust").unwrap();
        let plan = plan_retrofit(d.path(), &profile, &ctx());
        assert!(plan
            .already
            .contains(&std::path::PathBuf::from("Cargo.toml")));
        assert!(plan
            .merges
            .iter()
            .all(|m| m.path != Path::new("Cargo.toml")));
    }

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

    #[test]
    fn merge_tsconfig_enforces_strict_flags() {
        let out = merge_tsconfig("{ \"compilerOptions\": { \"strict\": false } }").unwrap();
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["compilerOptions"]["strict"], serde_json::json!(true));
        assert_eq!(
            v["compilerOptions"]["noUncheckedIndexedAccess"],
            serde_json::json!(true)
        );
    }

    #[test]
    fn merge_tsconfig_is_idempotent() {
        let once = merge_tsconfig("{}").unwrap();
        let twice = merge_tsconfig(&once).unwrap();
        assert_eq!(once, twice);
    }

    #[test]
    fn merge_tsconfig_returns_none_on_unparseable_input() {
        assert!(merge_tsconfig("{ not valid").is_none());
    }

    #[test]
    fn merge_package_json_adds_dev_deps_and_check_script_keeping_user_fields() {
        let out = merge_package_json("{ \"name\": \"keep\", \"scripts\": { \"build\": \"x\" } }")
            .unwrap();
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["name"], serde_json::json!("keep"));
        assert_eq!(v["scripts"]["build"], serde_json::json!("x")); // preserved
        assert!(v["scripts"]["check"].is_string()); // added
        assert!(v["devDependencies"]["typescript"].is_string()); // added
    }

    #[test]
    fn merge_package_json_returns_none_on_unparseable_input() {
        assert!(merge_package_json("{ not valid").is_none());
    }

    #[test]
    fn apply_writes_adds_merges_and_appends_gitignore() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("pyproject.toml"), "[project]\nname='x'\n").unwrap();
        std::fs::write(d.path().join(".gitignore"), "custom/\n").unwrap();
        let profile = stack_profile("python").unwrap();
        let plan = plan_retrofit(d.path(), &profile, &ctx());
        let report = apply_retrofit(d.path(), &plan).unwrap();

        assert!(d.path().join("scripts/check.sh").exists());
        assert!(d.path().join("AGENTS.md").exists());
        let pp = std::fs::read_to_string(d.path().join("pyproject.toml")).unwrap();
        assert!(pp.contains("[tool.ruff]"));
        assert!(pp.contains("name='x'") || pp.contains("name = 'x'"));
        let gi = std::fs::read_to_string(d.path().join(".gitignore")).unwrap();
        assert!(gi.contains("custom/"));
        assert!(gi.contains(".ruff_cache/"));
        assert!(!report.written.is_empty());
    }

    #[test]
    fn apply_is_idempotent_a_second_plan_is_empty() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("Cargo.toml"), "[package]\nname='x'\n").unwrap();
        let profile = stack_profile("rust").unwrap();
        apply_retrofit(d.path(), &plan_retrofit(d.path(), &profile, &ctx())).unwrap();
        let second = plan_retrofit(d.path(), &profile, &ctx());
        assert!(second.adds.is_empty(), "no new adds on a retrofitted repo");
        assert!(second.merges.is_empty(), "no new merges");
        assert!(second.gitignore_append.is_empty(), "no new ignore lines");
    }

    /// Live: retrofit a clean Go fixture (valid, gofmt-clean, test-passing code, no tooling)
    /// and run the emitted gate. It should be green because there is nothing to fix. Ignored
    /// by default; run with `env -C <repo> cargo test -p tutti-app-core -- --ignored`.
    #[tokio::test]
    #[ignore = "live: needs a go toolchain on PATH"]
    async fn retrofit_clean_go_repo_is_green() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(
            d.path().join("go.mod"),
            "module example.com/clean\n\ngo 1.23\n",
        )
        .unwrap();
        std::fs::write(
            d.path().join("clean.go"),
            "package clean\n\n// Add returns the sum.\nfunc Add(a, b int) int {\n\treturn a + b\n}\n",
        )
        .unwrap();
        std::fs::write(
            d.path().join("clean_test.go"),
            "package clean\n\nimport \"testing\"\n\nfunc TestAdd(t *testing.T) {\n\tif Add(1, 2) != 3 {\n\t\tt.Fail()\n\t}\n}\n",
        )
        .unwrap();
        let profile = stack_profile("go").unwrap();
        let plan = plan_retrofit(d.path(), &profile, &ctx());
        apply_retrofit(d.path(), &plan).unwrap();
        let outcome = run_baseline_gate(d.path(), &profile).await.unwrap();
        assert!(
            outcome.passed,
            "clean repo gate should be green:\n{}",
            outcome.log
        );
    }

    /// Live: retrofit a dirty Go fixture (a real build/vet error) and confirm the baseline
    /// gate is RED and the log names the problem, which is the honest-gap-report contract.
    #[tokio::test]
    #[ignore = "live: needs a go toolchain on PATH"]
    async fn retrofit_dirty_go_repo_reports_the_gap() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(
            d.path().join("go.mod"),
            "module example.com/dirty\n\ngo 1.23\n",
        )
        .unwrap();
        std::fs::write(
            d.path().join("dirty.go"),
            "package dirty\n\nfunc Add(a, b int) int {\n\treturn a + b\n}\n",
        )
        .unwrap();
        std::fs::write(
            d.path().join("dirty_test.go"),
            "package dirty\n\nimport \"testing\"\n\nfunc TestBad(t *testing.T) {\n\t_ = Missing()\n}\n",
        )
        .unwrap();
        let profile = stack_profile("go").unwrap();
        let plan = plan_retrofit(d.path(), &profile, &ctx());
        apply_retrofit(d.path(), &plan).unwrap();
        let outcome = run_baseline_gate(d.path(), &profile).await.unwrap();
        assert!(!outcome.passed, "dirty repo gate should be red");
        assert!(
            outcome.log.contains("Missing") || outcome.log.to_lowercase().contains("undefined"),
            "the baseline log should name the failure:\n{}",
            outcome.log
        );
    }
}
