// SPDX-License-Identifier: AGPL-3.0-or-later
//! The single source of truth for the per-language convention list. Consumed by `scaffold`
//! (which emits the AGENTS.md idiom bullets from it) and by the conventions skill provider
//! (which injects it), so the skill, the constitution, and this spine cannot drift.

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
            // The tag line follows the heading; assert the correct tag is present near it.
            let after = rust_md.split(&heading).nth(1).expect("heading present");
            let tag = match c.severity {
                ConventionSeverity::Correctness => "Tag: correctness",
                ConventionSeverity::Advisory => "Tag: advisory",
            };
            let window = &after[..after.len().min(200)];
            assert!(
                window.contains(tag),
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
            assert!(!c.idiom.contains('\n'), "idiom must be one line: {}", c.idiom);
        }
    }
}
