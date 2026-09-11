// SPDX-License-Identifier: AGPL-3.0-or-later
//! The real `RepoGrounder`: read an existing repo into a `RepoGrounding` summary the design
//! chain confirms rather than asks from scratch. This is the IO-bound half of the E7a seam
//! (the hermetic types and prompt injection live in `tutti-design`).
//!
//! Every source degrades gracefully: a missing README, an unreadable file, or an absent or
//! erroring `codegraph` binary yields an empty field, never a failed `ground()`. A wrong
//! guess (shape, entities) costs the human a correction in the movement, not a bad session.

use std::path::Path;
use std::process::Command;

use serde::Deserialize;
use tutti_design::{DomainSignal, RepoGrounder, RepoGrounding};

/// The maximum number of characters of concatenated docs to keep in the digest.
const DOCS_DIGEST_CAP: usize = 1500;
/// The maximum number of distinct candidate entities to propose.
const MAX_ENTITIES: usize = 30;
/// The maximum number of candidate seams (traits/interfaces) to propose.
const MAX_SEAMS: usize = 20;
/// The maximum number of top-level containers to list as structure.
const MAX_STRUCTURE: usize = 24;
/// The type-like codegraph kinds whose names become candidate domain entities.
const TYPE_KINDS: &[&str] = &["struct", "enum", "class", "interface"];

/// The real repo grounder. Stateless: it reads the filesystem and shells to `codegraph`.
pub struct RepoGroundingReader;

impl RepoGrounder for RepoGroundingReader {
    fn ground(&self, repo_root: &Path) -> tutti_design::Result<RepoGrounding> {
        let stack = crate::retrofit::detect_languages(repo_root);
        let docs_digest = read_docs_digest(repo_root);
        let is_mobile = detect_mobile(repo_root);
        let (domain, structure, container_count) = codegraph_signal(repo_root);
        // Coarse by design: the container count is the primary signal, but a codegraph-absent
        // repo (container_count == 0) still leans on the number of detected languages so a
        // polyglot repo is not silently forced to the small-CLI shape. Frame confirms it.
        let inferred_shape = tutti_design::infer_shape(is_mobile, container_count.max(stack.len()));
        let already_decided = derive_decisions(&stack, &structure);
        Ok(RepoGrounding {
            stack,
            inferred_shape,
            docs_digest,
            domain,
            structure,
            already_decided,
        })
    }
}

/// Read `README.md`, `AGENTS.md`, and up to a few `docs/*.md` into a single digest, capped at
/// `DOCS_DIGEST_CAP` characters. Missing files are skipped; an empty string when there are
/// none. The cap is applied on a char boundary so a multibyte character is never split.
fn read_docs_digest(root: &Path) -> String {
    let mut buf = String::new();
    for name in ["README.md", "AGENTS.md"] {
        if buf.len() >= DOCS_DIGEST_CAP {
            break;
        }
        if let Ok(s) = std::fs::read_to_string(root.join(name)) {
            append_section(&mut buf, &s);
        }
    }
    if buf.len() < DOCS_DIGEST_CAP {
        if let Ok(rd) = std::fs::read_dir(root.join("docs")) {
            let mut md: Vec<std::path::PathBuf> = rd
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.extension().and_then(|x| x.to_str()) == Some("md"))
                .collect();
            md.sort();
            for p in md.into_iter().take(3) {
                if buf.len() >= DOCS_DIGEST_CAP {
                    break;
                }
                if let Ok(s) = std::fs::read_to_string(&p) {
                    append_section(&mut buf, &s);
                }
            }
        }
    }
    truncate_chars(&buf, DOCS_DIGEST_CAP)
}

/// Append a doc section to the digest, separating it from any previous one with a blank line.
fn append_section(buf: &mut String, text: &str) {
    if !buf.is_empty() {
        buf.push_str("\n\n");
    }
    buf.push_str(text);
}

/// Truncate `s` to at most `cap` bytes, backing up to the nearest char boundary so a
/// multibyte character is never split (slicing at a raw byte offset can panic).
fn truncate_chars(s: &str, cap: usize) -> String {
    if s.len() <= cap {
        return s.to_string();
    }
    let mut end = cap;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    s[..end].to_string()
}

/// A coarse mobile-toolchain probe: a Gradle build (Android) or a `Package.swift` (iOS/Swift),
/// or an `AndroidManifest.xml` in a common location. Coarse on purpose; the human confirms the
/// shape in Frame.
fn detect_mobile(root: &Path) -> bool {
    let has = |rel: &str| root.join(rel).exists();
    has("build.gradle")
        || has("build.gradle.kts")
        || has("app/build.gradle")
        || has("app/build.gradle.kts")
        || has("Package.swift")
        || has("AndroidManifest.xml")
        || has("app/src/main/AndroidManifest.xml")
}

/// One symbol hit from `codegraph query ... --json`: an array of `{"node": {...}}`.
#[derive(Deserialize)]
struct Hit {
    node: NodeInfo,
}

/// The subset of a codegraph node we read. Extra JSON fields are ignored by serde.
#[derive(Deserialize)]
struct NodeInfo {
    name: String,
}

/// One entry from `codegraph files --json`: an array of `{"path": ..., ...}`.
#[derive(Deserialize)]
struct FileEntry {
    path: String,
}

/// Best-effort domain/structure signal from codegraph: `(entities + seams, structure,
/// container_count)`. Returns empty/zero when the binary is absent or any step errors, so an
/// un-indexable repo still grounds (on stack + docs alone).
fn codegraph_signal(root: &Path) -> (DomainSignal, Vec<String>, usize) {
    if !codegraph_available() {
        return (DomainSignal::default(), Vec::new(), 0);
    }
    ensure_index(root);

    let mut entities: Vec<String> = Vec::new();
    for kind in TYPE_KINDS {
        for name in query_names(root, kind, MAX_ENTITIES) {
            if !entities.contains(&name) {
                entities.push(name);
            }
            if entities.len() >= MAX_ENTITIES {
                break;
            }
        }
        if entities.len() >= MAX_ENTITIES {
            break;
        }
    }
    // Seams are the code's real abstraction boundaries: Rust traits (and other languages'
    // interfaces, which codegraph reports under the `interface` kind, already folded into
    // entities above; traits are the Rust-specific seam signal).
    let seams = query_names(root, "trait", MAX_SEAMS);
    let structure = read_structure(root);
    let container_count = structure.len();
    (DomainSignal { entities, seams }, structure, container_count)
}

/// Whether the `codegraph` binary is runnable (probed via `codegraph --version`). Mirrors
/// `tutti_core::context::CodeGraph::detect`.
fn codegraph_available() -> bool {
    Command::new("codegraph")
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Ensure a codegraph index exists under `root`. If `.codegraph/` is present it is left alone
/// (codegraph's own watcher keeps it fresh). Otherwise `codegraph init -i <root>` builds it,
/// best-effort: any failure is swallowed so a missing index degrades to an empty signal rather
/// than a failed ground.
fn ensure_index(root: &Path) {
    if root.join(".codegraph").exists() {
        return;
    }
    let _ = Command::new("codegraph")
        .arg("init")
        .arg("-i")
        .arg(root)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
}

/// Run `codegraph query "" --kind <kind> --limit <limit> -p <root> --json` and return the
/// matched symbol names. Empty on any error (binary missing, non-zero exit, unparseable JSON).
/// The empty search term lists every symbol of the kind.
fn query_names(root: &Path, kind: &str, limit: usize) -> Vec<String> {
    let output = Command::new("codegraph")
        .arg("query")
        .arg("")
        .arg("--kind")
        .arg(kind)
        .arg("--limit")
        .arg(limit.to_string())
        .arg("-p")
        .arg(root)
        .arg("--json")
        .output();
    let Ok(output) = output else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }
    match serde_json::from_slice::<Vec<Hit>>(&output.stdout) {
        Ok(hits) => hits.into_iter().map(|h| h.node.name).collect(),
        Err(_) => Vec::new(),
    }
}

/// Derive the top-level containers from `codegraph files --json`: workspace members
/// (`crates/<name>`, `packages/<name>`, ...) and other top-level source directories, distinct
/// and sorted, capped at `MAX_STRUCTURE`. Empty on any codegraph error.
fn read_structure(root: &Path) -> Vec<String> {
    let output = Command::new("codegraph")
        .arg("files")
        .arg("-p")
        .arg(root)
        .arg("--json")
        .output();
    let Ok(output) = output else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }
    let files: Vec<FileEntry> = match serde_json::from_slice(&output.stdout) {
        Ok(f) => f,
        Err(_) => return Vec::new(),
    };
    let mut containers: Vec<String> = Vec::new();
    for f in &files {
        if let Some(c) = top_container(&f.path) {
            if !containers.contains(&c) {
                containers.push(c);
            }
        }
    }
    containers.sort();
    containers.truncate(MAX_STRUCTURE);
    containers
}

/// The top-level container a repo-relative path belongs to: a workspace member directory
/// (`crates/<name>`) for the common monorepo roots, otherwise the path's first directory
/// segment. `None` for a top-level file (no directory), which is not a container.
fn top_container(path: &str) -> Option<String> {
    let segs: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    match segs.as_slice() {
        [root_dir, member, ..]
            if matches!(
                *root_dir,
                "crates" | "packages" | "services" | "apps" | "libs"
            ) =>
        {
            Some(format!("{root_dir}/{member}"))
        }
        [dir, rest @ ..] if !rest.is_empty() => Some((*dir).to_string()),
        _ => None,
    }
}

/// A modest list of already-settled decisions read from the code: the language(s) in use and
/// the existing module structure. Coarse on purpose; these are things a grounded Decide /
/// Structure movement should confirm rather than relitigate.
fn derive_decisions(stack: &[String], structure: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    if !stack.is_empty() {
        out.push(format!("language is {}", stack.join(", ")));
    }
    if !structure.is_empty() {
        out.push(format!("existing modules: {}", structure.join(", ")));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ground_reads_stack_and_docs_without_a_codegraph_dependency() {
        // The hermetic path: a repo with a Cargo.toml and a README grounds to the right stack
        // and a non-empty docs digest whether or not codegraph is installed (the codegraph
        // step degrades to empty when the binary is absent), and never errors.
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("Cargo.toml"), "[package]\nname = \"x\"\n").unwrap();
        std::fs::write(
            d.path().join("README.md"),
            "A private, self-hosted voice assistant.",
        )
        .unwrap();
        let g = RepoGroundingReader.ground(d.path()).unwrap();
        assert_eq!(g.stack, vec!["rust".to_string()]);
        assert!(
            g.docs_digest
                .contains("private, self-hosted voice assistant"),
            "docs digest: {:?}",
            g.docs_digest
        );
    }

    #[test]
    fn docs_digest_truncation_is_char_boundary_safe() {
        // A cap landing in the middle of a multibyte char must back up, never split or panic.
        let s = "é".repeat(1000); // each 'é' is two bytes
        let t = truncate_chars(&s, 1501); // odd cap lands mid-character
        assert!(t.len() <= 1501);
        assert!(
            std::str::from_utf8(t.as_bytes()).is_ok(),
            "truncation kept valid UTF-8"
        );
    }

    #[test]
    fn top_container_groups_workspace_members_and_top_dirs() {
        assert_eq!(
            top_container("crates/tutti-core/src/lib.rs"),
            Some("crates/tutti-core".to_string())
        );
        assert_eq!(
            top_container("src/main.rs"),
            Some("src".to_string()),
            "a plain top-level dir is its own container"
        );
        assert_eq!(
            top_container("Cargo.toml"),
            None,
            "a top-level file is not a container"
        );
    }

    #[test]
    fn derive_decisions_names_language_and_modules() {
        let d = derive_decisions(
            &["rust".to_string()],
            &[
                "crates/tutti-core".to_string(),
                "crates/tutti-cli".to_string(),
            ],
        );
        assert!(d.iter().any(|s| s.contains("language is rust")));
        assert!(d.iter().any(|s| s.contains("crates/tutti-core")));
    }

    #[test]
    fn derive_decisions_is_empty_when_nothing_is_known() {
        assert!(derive_decisions(&[], &[]).is_empty());
    }

    /// Live: needs the `codegraph` binary on PATH (CI runners lack it, hence ignored). Builds a
    /// tiny real fixture, indexes it, and asserts the type-like symbols are read as entities.
    /// Run with `env -C <repo> cargo test -p tutti-app-core -- --ignored`.
    #[test]
    #[ignore = "live: needs codegraph on PATH"]
    fn codegraph_signal_reads_entities_from_a_real_repo() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(
            d.path().join("Cargo.toml"),
            "[package]\nname = \"fx\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        )
        .unwrap();
        std::fs::create_dir_all(d.path().join("src")).unwrap();
        std::fs::write(
            d.path().join("src/lib.rs"),
            "pub struct Session {\n    pub id: u32,\n}\n\npub enum Movement {\n    Frame,\n    Decide,\n}\n",
        )
        .unwrap();
        let (domain, _structure, _count) = codegraph_signal(d.path());
        assert!(
            domain.entities.iter().any(|e| e == "Session"),
            "entities: {:?}",
            domain.entities
        );
        assert!(
            domain.entities.iter().any(|e| e == "Movement"),
            "entities: {:?}",
            domain.entities
        );
    }
}
