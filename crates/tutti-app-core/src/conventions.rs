// SPDX-License-Identifier: AGPL-3.0-or-later
//! The single source of truth for the per-language convention list. Consumed by `scaffold`
//! (which emits the AGENTS.md idiom bullets from it) and by the conventions skill provider
//! (which injects it), so the skill, the constitution, and this spine cannot drift.

use tutti_design::skill::parse_frontmatter;

/// Whether violating a convention is a real defect the mechanical gate cannot catch
/// (`Correctness`, which the reviewer raises as a gating Major finding) or a subjective
/// preference (`Advisory`, a Minor note that never gates and is never auto-fixed).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConventionSeverity {
    Correctness,
    Advisory,
}

/// One convention: a one-line idiom (verbatim the heading in the skill reference file) and
/// its severity tag.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Convention {
    pub idiom: &'static str,
    pub severity: ConventionSeverity,
}

/// The Rust conventions. Idioms match `crates/../skills/conventions/references/rust.md`
/// headings verbatim (enforced by the drift-guard test in Task 4).
pub const RUST_CONVENTIONS: &[Convention] = &[
    Convention {
        idiom: "Make illegal states unrepresentable: model variants as enums and match them exhaustively.",
        severity: ConventionSeverity::Correctness,
    },
    Convention {
        idiom: "Prefer `?` for error propagation; reserve `unwrap`/`expect` for tests and provable invariants, and give `expect` a message.",
        severity: ConventionSeverity::Correctness,
    },
    Convention {
        idiom: "Do not panic across a public API; return a `Result` instead.",
        severity: ConventionSeverity::Correctness,
    },
    Convention {
        idiom: "Use newtypes over stringly-typed or primitive-obsessed APIs, so the representation can change without breaking callers.",
        severity: ConventionSeverity::Advisory,
    },
    Convention {
        idiom: "Accept borrowed or generic arguments (`&str`, `&[T]`, `impl AsRef<_>`) and return owned values (`String`, `Vec<T>`).",
        severity: ConventionSeverity::Advisory,
    },
    Convention {
        idiom: "Derive standard traits (`Debug`, `Clone`, `PartialEq`) rather than hand-implementing; derive `Debug` on all public types.",
        severity: ConventionSeverity::Advisory,
    },
];

const SKILL_MD: &str = include_str!("../../../skills/conventions/SKILL.md");
const RUST_REFERENCE: &str = include_str!("../../../skills/conventions/references/rust.md");

/// The reference markdown for a language id (`retrofit::detect_languages` ids), if we have one.
fn reference_for(language: &str) -> Option<&'static str> {
    match language {
        "rust" => Some(RUST_REFERENCE),
        _ => None,
    }
}

/// Build the conventions preamble for the given detected languages: the SKILL.md body (the
/// shared implementer/reviewer contract) followed by each matching language reference.
/// `None` when no detected language has a reference (nothing to inject).
pub fn conventions_preamble(languages: &[String]) -> Option<String> {
    let refs: Vec<&str> = languages.iter().filter_map(|l| reference_for(l)).collect();
    if refs.is_empty() {
        return None;
    }
    // parse_frontmatter returns (Frontmatter, body); fall back to the whole file if it ever
    // fails to parse, so a malformed edit degrades to injecting the raw text rather than
    // silently injecting nothing.
    let body = parse_frontmatter(SKILL_MD)
        .map(|(_, body)| body)
        .unwrap_or_else(|_| SKILL_MD.to_string());
    let mut out = body.trim_end().to_string();
    for r in refs {
        out.push_str("\n\n");
        out.push_str(r.trim_end());
    }
    Some(out)
}

/// The conventions skill wired as a `ConventionsProvider`: it detects the worktree's
/// language(s) and injects the matching reference(s) for the coding roles.
pub struct ConventionsSkill;

impl tutti_core::conventions::ConventionsProvider for ConventionsSkill {
    fn preamble_for(
        &self,
        role: tutti_core::message::Role,
        worktree: &std::path::Path,
    ) -> Option<String> {
        use tutti_core::message::Role;
        // Only the roles that write or review code get conventions.
        if !matches!(role, Role::Implementer | Role::FixApplier | Role::Reviewer) {
            return None;
        }
        let languages = crate::retrofit::detect_languages(worktree);
        conventions_preamble(&languages)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn skill_dir() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../skills/conventions")
    }

    #[test]
    fn rust_reference_headings_match_the_spine_verbatim() {
        let rust_md =
            std::fs::read_to_string(skill_dir().join("references/rust.md")).expect("rust.md");
        for c in RUST_CONVENTIONS {
            let heading = format!("## {}", c.idiom);
            assert!(
                rust_md.contains(&heading),
                "rust.md is missing the spine idiom as a heading:\n{heading}"
            );
            // The tag line lives in this convention's own section: the text after its
            // heading, up to the next `## ` heading. Bounding to the section (rather than a
            // fixed byte window) means a wrong tag cannot be satisfied by a sibling section's
            // tag, and there is no risk of slicing a multibyte char mid-boundary.
            let after = rust_md.split(&heading).nth(1).expect("heading present");
            let section = after.split("\n## ").next().unwrap_or(after);
            let tag = match c.severity {
                ConventionSeverity::Correctness => "Tag: correctness",
                ConventionSeverity::Advisory => "Tag: advisory",
            };
            assert!(
                section.contains(tag),
                "wrong or missing tag for idiom:\n{}\nexpected {tag}",
                c.idiom
            );
        }
    }

    #[test]
    fn conventions_skill_passes_the_e15_harness() {
        let skill = tutti_design::skill::load(&skill_dir()).expect("skill loads");
        assert!(
            tutti_design::lint::lint(&skill).is_empty(),
            "skill lint violations: {:?}",
            tutti_design::lint::lint(&skill)
        );
        let evals = tutti_design::eval::load_evals(&skill_dir()).expect("evals load");
        assert!(
            tutti_design::eval::has_minimum_evals(&evals),
            "need at least the minimum eval records"
        );
    }

    #[test]
    fn embedded_skill_matches_disk() {
        let disk_skill = std::fs::read_to_string(skill_dir().join("SKILL.md")).unwrap();
        let disk_rust = std::fs::read_to_string(skill_dir().join("references/rust.md")).unwrap();
        assert_eq!(SKILL_MD, disk_skill, "embedded SKILL.md drifted from disk");
        assert_eq!(
            RUST_REFERENCE, disk_rust,
            "embedded rust.md drifted from disk"
        );
    }

    #[test]
    fn preamble_for_rust_includes_the_contract_and_a_rust_idiom() {
        let p = conventions_preamble(&["rust".to_string()]).expect("rust preamble");
        assert!(
            p.contains("If you are reviewing"),
            "carries the SKILL.md contract"
        );
        assert!(p.contains("Rust conventions"), "carries the rust reference");
        assert!(
            p.contains(RUST_CONVENTIONS[0].idiom),
            "carries a spine idiom"
        );
    }

    #[test]
    fn preamble_is_none_for_an_unknown_language() {
        assert!(conventions_preamble(&["cobol".to_string()]).is_none());
    }

    #[test]
    fn provider_injects_for_coding_roles_in_a_rust_worktree() {
        use tutti_core::conventions::ConventionsProvider;
        use tutti_core::message::Role;
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("Cargo.toml"), "[package]\nname='x'\n").unwrap();
        let p = ConventionsSkill;
        let out = p.preamble_for(Role::Implementer, d.path());
        assert!(out.is_some(), "rust worktree, implementer gets a preamble");
        assert!(out.unwrap().contains("Rust conventions"));
        assert!(
            p.preamble_for(Role::Planner, d.path()).is_none(),
            "planner gets nothing"
        );
        // A directory with no known language markers gets nothing.
        let empty = tempfile::tempdir().unwrap();
        assert!(p.preamble_for(Role::Reviewer, empty.path()).is_none());
    }

    #[test]
    fn rust_spine_is_nonempty_and_carries_both_severities() {
        assert!(!RUST_CONVENTIONS.is_empty());
        assert!(RUST_CONVENTIONS
            .iter()
            .any(|c| c.severity == ConventionSeverity::Correctness));
        assert!(RUST_CONVENTIONS
            .iter()
            .any(|c| c.severity == ConventionSeverity::Advisory));
        // Idioms are non-empty one-liners (no newlines): they double as reference headings.
        for c in RUST_CONVENTIONS {
            assert!(!c.idiom.trim().is_empty());
            assert!(
                !c.idiom.contains('\n'),
                "idiom must be one line: {}",
                c.idiom
            );
        }
    }
}
