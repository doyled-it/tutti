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
///
/// A parent table this call creates (an intermediate, never the leaf we write keys into,
/// and never a pre-existing table that may hold the user's own keys) is marked implicit,
/// so the merged file does not sprout an empty `[tool]` / `[tool.pytest]` header.
fn ensure_table<'a>(doc: &'a mut DocumentMut, path: &[&str]) -> Option<&'a mut Table> {
    if path.is_empty() {
        return Some(doc.as_table_mut());
    }
    let last = path.len() - 1;
    let mut tbl = doc.as_table_mut();
    for (idx, key) in path.iter().enumerate() {
        let existed = tbl.contains_key(key);
        let child = tbl
            .entry(key)
            .or_insert_with(|| Item::Table(Table::new()))
            .as_table_mut()?;
        if !existed && idx != last {
            child.set_implicit(true);
        }
        tbl = child;
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
///
/// Returns `None` if `key` is present but is NOT an array. Our array keys (`extend-select`,
/// the dev group) are opinion-defining, so a wrong-shaped value means we cannot enforce our
/// floor: the caller bails and leaves the whole file untouched rather than silently keeping
/// the user's value while rewriting the rest (which would report a false "merged" state).
fn ensure_str_array(tbl: &mut Table, key: &str, wanted: &[&str]) -> Option<()> {
    if tbl.contains_key(key) && tbl.get(key).and_then(|i| i.as_array()).is_none() {
        return None;
    }
    let item = tbl.entry(key).or_insert(value(Array::new()));
    let arr = item.as_array_mut()?;
    for w in wanted {
        if !arr.iter().any(|v| v.as_str() == Some(*w)) {
            arr.push(*w);
        }
    }
    Some(())
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
    ensure_str_array(dg, "dev", &["ruff", "mypy", "pytest"])?;

    // [tool.ruff]: line-length + target-version are additive (cosmetic).
    let ruff = ensure_table(&mut doc, &["tool", "ruff"])?;
    set_if_absent(ruff, "line-length", 88_i64.into());
    set_if_absent(ruff, "target-version", "py313".into());
    // [tool.ruff.lint]: enforce our lint selection (union, so a floor not a cap).
    let lint = ensure_table(&mut doc, &["tool", "ruff", "lint"])?;
    ensure_str_array(lint, "extend-select", &["I", "UP", "B", "SIM", "RUF"])?;

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

/// Strip JSONC comments (`//` line and `/* */` block) that are OUTSIDE string literals, so a
/// real `tsconfig.json` parses. String contents are preserved, including `//` inside a value
/// such as an `https://` schema URL (a naive scan would corrupt those).
fn strip_jsonc(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    let mut in_string = false;
    let mut escaped = false;
    while let Some(c) = chars.next() {
        if in_string {
            out.push(c);
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_string = false;
            }
            continue;
        }
        match c {
            '"' => {
                in_string = true;
                out.push(c);
            }
            '/' if chars.peek() == Some(&'/') => {
                // Line comment: consume through the newline (keep the newline).
                for n in chars.by_ref() {
                    if n == '\n' {
                        out.push('\n');
                        break;
                    }
                }
            }
            '/' if chars.peek() == Some(&'*') => {
                chars.next(); // consume the '*'
                              // Block comment: consume through the closing '*/'.
                let mut prev = '\0';
                for n in chars.by_ref() {
                    if prev == '*' && n == '/' {
                        break;
                    }
                    prev = n;
                }
            }
            _ => out.push(c),
        }
    }
    out
}

/// Remove trailing commas (a comma whose next non-whitespace character is `}` or `]`) that
/// are OUTSIDE string literals, so a real, comma-trailing `tsconfig.json` parses as JSON.
/// Trailing commas are legal JSONC and idiomatic in editor-generated tsconfig files.
fn strip_trailing_commas(input: &str) -> String {
    let chars: Vec<char> = input.chars().collect();
    let mut out = String::with_capacity(input.len());
    let mut in_string = false;
    let mut escaped = false;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if in_string {
            out.push(c);
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_string = false;
            }
            i += 1;
            continue;
        }
        if c == '"' {
            in_string = true;
            out.push(c);
            i += 1;
            continue;
        }
        if c == ',' {
            let mut j = i + 1;
            while j < chars.len() && chars[j].is_whitespace() {
                j += 1;
            }
            if j < chars.len() && (chars[j] == '}' || chars[j] == ']') {
                i += 1; // drop this trailing comma
                continue;
            }
        }
        out.push(c);
        i += 1;
    }
    out
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

/// Ensure `root[key]` is a JSON object, mutating IN PLACE so an existing key keeps its
/// position (serde_json's `preserve_order` feature holds insertion order). Returns `None`
/// if the key is present as a non-object, non-null value, so the caller bails and leaves
/// the whole file untouched rather than discarding the user's value.
fn ensure_object<'a>(
    root: &'a mut Map<String, Value>,
    key: &str,
) -> Option<&'a mut Map<String, Value>> {
    match root.get(key) {
        Some(Value::Object(_)) | Some(Value::Null) | None => {}
        Some(_) => return None,
    }
    let slot = root
        .entry(key.to_string())
        .or_insert_with(|| Value::Object(Map::new()));
    if slot.is_null() {
        *slot = Value::Object(Map::new());
    }
    slot.as_object_mut()
}

/// Parse a `tsconfig.json` as JSONC (comments and trailing commas tolerated). `None` if it
/// still does not parse as a JSON object after that.
fn parse_tsconfig(existing: &str) -> Option<Map<String, Value>> {
    let cleaned = strip_trailing_commas(&strip_jsonc(existing));
    json_root_object(serde_json::from_str(&cleaned))
}

/// Merge Sotto's strict TypeScript compiler flags into an existing `tsconfig.json`.
/// Enforces the strictness flags; preserves every other option AND the user's key order;
/// idempotent. Returns `None` (leave the file untouched) if the existing content is not a
/// JSON object, or if `compilerOptions` is present as a non-object.
///
/// Note: JSON has no comments, so JSONC comments in the source are not carried into the
/// merged output. `plan_retrofit` surfaces that as a note on the merge so it is not a
/// silent change; the preview diff shows the removed lines explicitly.
pub fn merge_tsconfig(existing: &str) -> Option<String> {
    let mut root = parse_tsconfig(existing)?;
    {
        let co = ensure_object(&mut root, "compilerOptions")?;
        // Opinion-defining strictness flags: enforced (set even over a weaker value).
        co.insert("strict".into(), Value::Bool(true));
        co.insert("noUncheckedIndexedAccess".into(), Value::Bool(true));
        // Additive defaults: only when the user has not set them.
        for (k, v) in [
            ("module", Value::String("esnext".into())),
            ("moduleResolution", Value::String("bundler".into())),
            ("target", Value::String("es2023".into())),
            ("skipLibCheck", Value::Bool(true)),
        ] {
            co.entry(k.to_string()).or_insert(v);
        }
    }
    Some(serde_json::to_string_pretty(&Value::Object(root)).unwrap() + "\n")
}

/// Merge the `typescript` + `bun-types` dev dependencies and the `check` script into an
/// existing `package.json`, preserving every other field AND the user's key order.
/// Idempotent. Returns `None` (leave the file untouched) if the existing content is not a
/// JSON object, or if `scripts`/`devDependencies` is present as a non-object.
pub fn merge_package_json(existing: &str) -> Option<String> {
    let mut root = json_root_object(serde_json::from_str(existing))?;
    {
        let scripts = ensure_object(&mut root, "scripts")?;
        scripts
            .entry("check".to_string())
            .or_insert(Value::String("tsc --noEmit && bun test".into()));
    }
    {
        let dev = ensure_object(&mut root, "devDependencies")?;
        dev.entry("typescript".to_string())
            .or_insert(Value::String("^5.7.0".into()));
        dev.entry("bun-types".to_string())
            .or_insert(Value::String("^1.1.0".into()));
    }
    Some(serde_json::to_string_pretty(&Value::Object(root)).unwrap() + "\n")
}

/// A config file that exists and will be merged, with a preview diff.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigMerge {
    pub path: PathBuf,
    pub new_contents: String,
    /// A line-level diff old -> new for the operator to confirm.
    pub diff: String,
    /// An optional heads-up about the merge (e.g. JSONC comments not being preserved),
    /// surfaced in the preview so a reformat is never a silent surprise.
    pub note: Option<String>,
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

/// The outcome of attempting to merge a `Config` file.
enum MergeAttempt {
    /// No registered merger for this file (Cargo.toml, go.mod): leave it untouched.
    NoMerger,
    /// The merger ran and produced this content.
    Merged(String),
    /// The existing file could not be parsed: leave it untouched, never fabricate over it.
    Unparseable,
}

/// Attempt to merge a `Config` file: dispatch to the registered merger by (stack id, file
/// name). One match arm is the single source of truth for what has a merger.
fn merge_config(stack_id: &str, path: &Path, existing: &str) -> MergeAttempt {
    let merged = match (stack_id, path.file_name().and_then(|s| s.to_str())) {
        ("python", Some("pyproject.toml")) => merge_pyproject(existing),
        ("typescript", Some("tsconfig.json")) => merge_tsconfig(existing),
        ("typescript", Some("package.json")) => merge_package_json(existing),
        _ => return MergeAttempt::NoMerger,
    };
    match merged {
        Some(s) => MergeAttempt::Merged(s),
        None => MergeAttempt::Unparseable,
    }
}

/// A line-level diff (old -> new) for the operator to confirm: changed lines marked `-`/`+`
/// with a few lines of surrounding context, and long unchanged runs elided as `  ...`. Empty
/// when the two are identical. Deliberately a preview aid, not a patch tool.
fn line_diff(old: &str, new: &str) -> String {
    if old == new {
        return String::new();
    }
    let a: Vec<&str> = old.lines().collect();
    let b: Vec<&str> = new.lines().collect();
    let (n, m) = (a.len(), b.len());
    // LCS length table (row n+1 by m+1), filled from the bottom-right.
    let mut lcs = vec![vec![0usize; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            lcs[i][j] = if a[i] == b[j] {
                lcs[i + 1][j + 1] + 1
            } else {
                lcs[i + 1][j].max(lcs[i][j + 1])
            };
        }
    }
    #[derive(PartialEq)]
    enum Op {
        Ctx,
        Del,
        Add,
    }
    let mut ops: Vec<(Op, &str)> = Vec::new();
    let (mut i, mut j) = (0usize, 0usize);
    while i < n && j < m {
        if a[i] == b[j] {
            ops.push((Op::Ctx, a[i]));
            i += 1;
            j += 1;
        } else if lcs[i + 1][j] >= lcs[i][j + 1] {
            ops.push((Op::Del, a[i]));
            i += 1;
        } else {
            ops.push((Op::Add, b[j]));
            j += 1;
        }
    }
    while i < n {
        ops.push((Op::Del, a[i]));
        i += 1;
    }
    while j < m {
        ops.push((Op::Add, b[j]));
        j += 1;
    }

    const CONTEXT: usize = 3;
    let mut keep = vec![false; ops.len()];
    for (idx, (op, _)) in ops.iter().enumerate() {
        if *op != Op::Ctx {
            let lo = idx.saturating_sub(CONTEXT);
            let hi = (idx + CONTEXT + 1).min(ops.len());
            for k in keep.iter_mut().take(hi).skip(lo) {
                *k = true;
            }
        }
    }
    let mut out = String::new();
    let mut elided = false;
    for (idx, (op, line)) in ops.iter().enumerate() {
        if !keep[idx] {
            if !elided {
                out.push_str("  ...\n");
                elided = true;
            }
            continue;
        }
        elided = false;
        out.push_str(match op {
            Op::Ctx => "  ",
            Op::Del => "- ",
            Op::Add => "+ ",
        });
        out.push_str(line);
        out.push('\n');
    }
    out
}

/// A heads-up to attach to a merge when the output cannot round-trip everything in the
/// source. Today: a JSONC `tsconfig.json` whose comments will not survive the JSON merge.
fn merge_note(path: &Path, existing: &str) -> Option<String> {
    let name = path.file_name().and_then(|s| s.to_str())?;
    if name == "tsconfig.json" && strip_jsonc(existing) != existing {
        return Some("JSON has no comments; the comments in this file are not preserved.".into());
    }
    None
}

/// Compute the retrofit for `profile` against `dir` without writing anything.
pub fn plan_retrofit(dir: &Path, profile: &StackProfile, ctx: &ScaffoldContext) -> RetrofitPlan {
    let mut plan = RetrofitPlan::default();
    // The profile's own `.gitignore` contents are the single source of truth for the ignore
    // lines to append (stashed here rather than duplicated in a second table).
    let mut gitignore_wanted: Vec<String> = Vec::new();
    for file in (profile.files)(ctx) {
        let abs = dir.join(&file.path);
        let exists = abs.exists();
        match file.role {
            FileRole::Sample => {}
            FileRole::Tooling => {
                if file.path == Path::new(".gitignore") {
                    gitignore_wanted = file
                        .contents
                        .lines()
                        .filter(|l| !l.trim().is_empty())
                        .map(|l| l.to_string())
                        .collect();
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
                // A file that exists but cannot be read (a directory, a permission-denied
                // file, an I/O error) must be left untouched, not treated as empty: an empty
                // string parses as valid empty TOML, which would fabricate a full config over
                // the user's real file.
                let existing = match std::fs::read_to_string(&abs) {
                    Ok(s) => s,
                    Err(_) => {
                        plan.skipped.push(SkipReason {
                            path: file.path.clone(),
                            why: "could not read; left untouched".into(),
                        });
                        continue;
                    }
                };
                match merge_config(profile.id, &file.path, &existing) {
                    // Config with no merger (Cargo.toml/go.mod): intentionally untouched.
                    MergeAttempt::NoMerger => plan.already.push(file.path.clone()),
                    MergeAttempt::Merged(new_contents) if new_contents != existing => {
                        let diff = line_diff(&existing, &new_contents);
                        let note = merge_note(&file.path, &existing);
                        plan.merges.push(ConfigMerge {
                            path: file.path.clone(),
                            new_contents,
                            diff,
                            note,
                        });
                    }
                    // Already satisfies our config: nothing to do.
                    MergeAttempt::Merged(_) => plan.already.push(file.path.clone()),
                    // Could not parse (or present as a wrong shape we cannot merge into):
                    // leave untouched and report, never fabricate or partial-write over it.
                    MergeAttempt::Unparseable => plan.skipped.push(SkipReason {
                        path: file.path.clone(),
                        why: "could not parse; left untouched".into(),
                    }),
                }
            }
        }
    }
    let gi_path = dir.join(".gitignore");
    let existing = std::fs::read_to_string(&gi_path).unwrap_or_default();
    let wanted: Vec<&str> = gitignore_wanted.iter().map(|s| s.as_str()).collect();
    plan.gitignore_append = gitignore_missing_lines(&existing, &wanted);
    plan
}

/// What `apply_retrofit` wrote.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RetrofitReport {
    pub written: Vec<PathBuf>,
    pub merged: Vec<PathBuf>,
    pub gitignore_appended: usize,
}

/// Fail if any component of `rel`'s parent path already exists as a non-directory, so a
/// blocked path (e.g. `.github` is a file) is caught BEFORE any write, never mid-apply.
fn preflight_parents(dir: &Path, rel: &Path) -> std::io::Result<()> {
    let abs = dir.join(rel);
    let mut cur = abs.parent();
    while let Some(p) = cur {
        if p == dir {
            break;
        }
        if p.exists() && !p.is_dir() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                format!(
                    "cannot create {}: {} exists and is not a directory",
                    rel.display(),
                    p.display()
                ),
            ));
        }
        cur = p.parent();
    }
    Ok(())
}

/// Write `contents` to `abs` atomically: write a sibling temp file, set the exec bit if
/// asked, then rename over the target (an atomic replace on POSIX). A failure mid-write
/// leaves the original file intact rather than a half-written one.
fn write_atomic(abs: &Path, contents: &str, executable: bool) -> std::io::Result<()> {
    let parent = abs.parent().unwrap_or_else(|| Path::new("."));
    let fname = abs
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "tutti".into());
    let tmp = parent.join(format!(".{fname}.tutti-tmp"));
    std::fs::write(&tmp, contents)?;
    #[cfg(unix)]
    if executable {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755))?;
    }
    std::fs::rename(&tmp, abs)?;
    Ok(())
}

/// Apply a confirmed plan to disk. Creates parent dirs, sets the executable bit on the
/// gate, writes merged config files in place, and appends the missing .gitignore lines.
/// Each file is written atomically (temp + rename), and a pre-flight rejects a plan whose
/// parent path is blocked by a file before anything is written. Call only after the operator
/// has confirmed the plan.
pub fn apply_retrofit(dir: &Path, plan: &RetrofitPlan) -> std::io::Result<RetrofitReport> {
    // Pre-flight the whole plan first, so a blocked path fails before any partial write.
    for file in &plan.adds {
        preflight_parents(dir, &file.path)?;
    }
    let mut report = RetrofitReport::default();
    for file in &plan.adds {
        let abs = dir.join(&file.path);
        if let Some(parent) = abs.parent() {
            std::fs::create_dir_all(parent)?;
        }
        write_atomic(&abs, &file.contents, file.executable)?;
        report.written.push(file.path.clone());
    }
    for merge in &plan.merges {
        write_atomic(&dir.join(&merge.path), &merge.new_contents, false)?;
        report.merged.push(merge.path.clone());
    }
    if !plan.gitignore_append.is_empty() {
        let gi = dir.join(".gitignore");
        // Build the full new content and write it atomically, so an append never leaves the
        // file partially extended. A trailing newline is ensured before appending so the
        // first new line does not join the last existing line.
        let mut contents = std::fs::read_to_string(&gi).unwrap_or_default();
        if !contents.is_empty() && !contents.ends_with('\n') {
            contents.push('\n');
        }
        for line in &plan.gitignore_append {
            contents.push_str(line);
            contents.push('\n');
        }
        write_atomic(&gi, &contents, false)?;
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
    fn merge_tsconfig_preserves_a_url_string_containing_double_slash() {
        let out = merge_tsconfig(
            "{\n  \"compilerOptions\": {},\n  \"$schema\": \"https://json.schemastore.org/tsconfig\"\n}",
        )
        .unwrap();
        assert!(out.contains("https://json.schemastore.org/tsconfig"));
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["compilerOptions"]["strict"], serde_json::json!(true));
    }

    #[test]
    fn merge_tsconfig_ignores_a_real_line_comment() {
        let out = merge_tsconfig("{\n  // editor comment\n  \"compilerOptions\": {}\n}").unwrap();
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["compilerOptions"]["strict"], serde_json::json!(true));
    }

    #[test]
    fn merge_tsconfig_returns_none_when_compiler_options_is_not_an_object() {
        // A present-but-non-object field must NOT be silently discarded.
        assert!(merge_tsconfig("{\"compilerOptions\": \"weird\"}").is_none());
    }

    #[test]
    fn merge_package_json_returns_none_when_scripts_is_not_an_object() {
        assert!(merge_package_json("{\"scripts\": \"weird\"}").is_none());
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

    #[test]
    fn merge_package_json_preserves_user_key_order() {
        // With preserve_order, the user's top-level order is kept and our keys append.
        let out = merge_package_json("{\"name\":\"z\",\"version\":\"1.0.0\",\"type\":\"module\"}")
            .unwrap();
        let name = out.find("\"name\"").unwrap();
        let version = out.find("\"version\"").unwrap();
        let typ = out.find("\"type\"").unwrap();
        let dev = out.find("\"devDependencies\"").unwrap();
        assert!(
            name < version && version < typ,
            "user order preserved: {out}"
        );
        assert!(typ < dev, "our added keys append after the user's: {out}");
    }

    #[test]
    fn merge_tsconfig_is_idempotent_on_an_already_configured_file() {
        // A tsconfig that already has our flags (in any order) must not churn on re-merge.
        let once = merge_tsconfig("{\"compilerOptions\":{\"strict\":true}}").unwrap();
        let twice = merge_tsconfig(&once).unwrap();
        assert_eq!(once, twice);
    }

    #[test]
    fn merge_tsconfig_tolerates_trailing_commas() {
        // Trailing commas are legal JSONC and common in real tsconfig files.
        let out = merge_tsconfig("{\n  \"compilerOptions\": {\n    \"strict\": true,\n  },\n}")
            .expect("a trailing-comma tsconfig should parse and merge");
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(
            v["compilerOptions"]["noUncheckedIndexedAccess"],
            serde_json::json!(true)
        );
    }

    #[test]
    fn merge_pyproject_bails_when_extend_select_is_not_an_array() {
        // A present-but-wrong-shaped opinion-defining value must NOT be silently ignored
        // while the rest of the file is rewritten: the whole merge bails (reported skipped).
        assert!(merge_pyproject("[tool.ruff.lint]\nextend-select = \"I\"\n").is_none());
    }

    #[test]
    fn merge_pyproject_bails_when_dev_group_is_not_an_array() {
        assert!(merge_pyproject("[dependency-groups]\ndev = \"ruff\"\n").is_none());
    }

    #[test]
    fn merge_pyproject_emits_no_empty_parent_headers() {
        let out = merge_pyproject("[project]\nname = \"x\"\n").unwrap();
        assert!(
            !out.lines().any(|l| l.trim() == "[tool]"),
            "no bare [tool] header:\n{out}"
        );
        assert!(
            !out.lines().any(|l| l.trim() == "[tool.pytest]"),
            "no bare [tool.pytest] header:\n{out}"
        );
        // The real leaf tables are still present.
        assert!(out.contains("[tool.ruff]"));
        assert!(out.contains("[tool.pytest.ini_options]"));
    }

    #[test]
    fn plan_skips_an_unreadable_config_instead_of_fabricating_over_it() {
        // A path that exists but cannot be read as a file (here: a directory named
        // pyproject.toml) must be left untouched, never treated as empty-and-merged.
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir(d.path().join("pyproject.toml")).unwrap();
        let profile = stack_profile("python").unwrap();
        let plan = plan_retrofit(d.path(), &profile, &ctx());
        assert!(plan
            .skipped
            .iter()
            .any(|s| s.path == Path::new("pyproject.toml")));
        assert!(plan
            .merges
            .iter()
            .all(|m| m.path != Path::new("pyproject.toml")));
    }

    #[test]
    fn merge_tsconfig_notes_comment_loss() {
        // A tsconfig WITH comments merges, and plan_retrofit attaches a note about it.
        let d = tempfile::tempdir().unwrap();
        std::fs::write(
            d.path().join("tsconfig.json"),
            "{\n  // my comment\n  \"compilerOptions\": {}\n}\n",
        )
        .unwrap();
        // typescript profile detects on tsconfig.json; give it one to detect.
        let profile = stack_profile("typescript").unwrap();
        let plan = plan_retrofit(d.path(), &profile, &ctx());
        let m = plan
            .merges
            .iter()
            .find(|m| m.path == Path::new("tsconfig.json"))
            .expect("tsconfig is merged");
        assert!(
            m.note.as_deref().is_some_and(|n| n.contains("comments")),
            "a comment-bearing tsconfig merge is noted: {:?}",
            m.note
        );
    }

    #[test]
    fn line_diff_shows_only_changed_lines_with_context_not_a_full_dump() {
        let old = (1..=20)
            .map(|n| format!("line {n}"))
            .collect::<Vec<_>>()
            .join("\n");
        let mut new_lines: Vec<String> = old.lines().map(|s| s.to_string()).collect();
        new_lines[9] = "line 10 CHANGED".to_string();
        let new = new_lines.join("\n");
        let diff = line_diff(&old, &new);
        assert!(
            diff.contains("- line 10\n"),
            "shows the removed line:\n{diff}"
        );
        assert!(
            diff.contains("+ line 10 CHANGED\n"),
            "shows the added line:\n{diff}"
        );
        assert!(
            diff.contains("  ..."),
            "elides distant unchanged runs:\n{diff}"
        );
        assert!(
            !diff.contains("line 1\n") || !diff.contains("line 20"),
            "not a full dump"
        );
        assert_eq!(
            line_diff("same\n", "same\n"),
            "",
            "identical inputs diff to empty"
        );
    }

    #[test]
    fn apply_preflight_rejects_a_blocked_parent_without_partial_writes() {
        // `.github` present as a FILE blocks creating `.github/workflows/ci.yml`. Apply must
        // fail before writing anything (no half-retrofitted repo).
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("Cargo.toml"), "[package]\nname='x'\n").unwrap();
        std::fs::write(d.path().join(".github"), "i am a file\n").unwrap();
        let profile = stack_profile("rust").unwrap();
        let plan = plan_retrofit(d.path(), &profile, &ctx());
        assert!(
            apply_retrofit(d.path(), &plan).is_err(),
            "blocked parent should error"
        );
        assert!(
            !d.path().join("scripts/check.sh").exists(),
            "nothing should be written when the plan is rejected"
        );
        assert!(!d.path().join("AGENTS.md").exists());
    }

    #[test]
    fn apply_leaves_no_temp_files_behind() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("Cargo.toml"), "[package]\nname='x'\n").unwrap();
        let profile = stack_profile("rust").unwrap();
        apply_retrofit(d.path(), &plan_retrofit(d.path(), &profile, &ctx())).unwrap();
        // No `.tutti-tmp` sidecar left in the repo root or scripts/.
        for dir in [d.path().to_path_buf(), d.path().join("scripts")] {
            if let Ok(rd) = std::fs::read_dir(&dir) {
                for e in rd.flatten() {
                    let name = e.file_name();
                    assert!(
                        !name.to_string_lossy().contains("tutti-tmp"),
                        "leftover temp file: {:?}",
                        e.path()
                    );
                }
            }
        }
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
