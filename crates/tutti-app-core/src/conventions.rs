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

/// The Python conventions. Idioms match `skills/conventions/references/python.md` headings
/// verbatim (enforced by the drift-guard test).
pub const PYTHON_CONVENTIONS: &[Convention] = &[
    Convention {
        idiom: "Use `pathlib.Path` over `os.path` string juggling.",
        severity: ConventionSeverity::Advisory,
    },
    Convention {
        idiom: "Use `dataclasses` for structured records rather than ad-hoc dicts or tuples.",
        severity: ConventionSeverity::Advisory,
    },
    Convention {
        idiom: "Use f-strings for formatting.",
        severity: ConventionSeverity::Advisory,
    },
    Convention {
        idiom: "Manage resources (files, locks, sessions) with `with` context managers.",
        severity: ConventionSeverity::Correctness,
    },
    Convention {
        idiom: "Type-hint public function signatures (mypy strict makes these load-bearing).",
        severity: ConventionSeverity::Advisory,
    },
    Convention {
        idiom: "Iterate directly and use comprehensions, `enumerate`, and `zip` rather than C-style index loops.",
        severity: ConventionSeverity::Advisory,
    },
    Convention {
        idiom: "Prefer EAFP (try/except) over precondition-checking; do not use a bare `except`.",
        severity: ConventionSeverity::Correctness,
    },
];

/// The TypeScript conventions. Idioms match `skills/conventions/references/typescript.md`
/// headings verbatim (enforced by the drift-guard test).
pub const TYPESCRIPT_CONVENTIONS: &[Convention] = &[
    Convention {
        idiom: "Model variant data as discriminated unions (a shared literal `kind` field) so the compiler narrows each case.",
        severity: ConventionSeverity::Advisory,
    },
    Convention {
        idiom: "Make exhaustive `switch`es provably complete with a `never`-typed default, so a new variant fails compilation.",
        severity: ConventionSeverity::Correctness,
    },
    Convention {
        idiom: "Use `readonly` and `as const` for data that should not be mutated.",
        severity: ConventionSeverity::Advisory,
    },
    Convention {
        idiom: "Prefer `interface` or `type` aliases for public object shapes over inline anonymous types.",
        severity: ConventionSeverity::Advisory,
    },
    Convention {
        idiom: "Prefer type guards over assertions (`as`); assert only when you genuinely know more than the compiler.",
        severity: ConventionSeverity::Correctness,
    },
    Convention {
        idiom: "Avoid `any`; take `unknown` at untyped boundaries and narrow before use; do not silence errors with `// @ts-ignore` or `!` where narrowing would do.",
        severity: ConventionSeverity::Correctness,
    },
];

/// The Go conventions. Idioms match `skills/conventions/references/go.md` headings verbatim
/// (enforced by the drift-guard test).
pub const GO_CONVENTIONS: &[Convention] = &[
    Convention {
        idiom: "Accept interfaces, return concrete types; let the consumer define the interface it needs.",
        severity: ConventionSeverity::Advisory,
    },
    Convention {
        idiom: "Keep interfaces small and defined at the point of use.",
        severity: ConventionSeverity::Advisory,
    },
    Convention {
        idiom: "Use `defer` for cleanup immediately after acquiring a resource.",
        severity: ConventionSeverity::Correctness,
    },
    Convention {
        idiom: "Avoid naked returns in anything longer than a few lines.",
        severity: ConventionSeverity::Advisory,
    },
    Convention {
        idiom: "Write table-driven tests with subtests (`t.Run`).",
        severity: ConventionSeverity::Advisory,
    },
    Convention {
        idiom: "Pass `context.Context` as the first parameter for cancelable or request-scoped work; do not store it in structs.",
        severity: ConventionSeverity::Correctness,
    },
    Convention {
        idiom: "Handle every error explicitly; wrap with `fmt.Errorf(\"...: %w\", err)` and inspect with `errors.Is`/`errors.As`; never string-match a message; do not discard an error with `_` unless deliberate.",
        severity: ConventionSeverity::Correctness,
    },
];

const SKILL_MD: &str = include_str!("../../../skills/conventions/SKILL.md");
const RUST_REFERENCE: &str = include_str!("../../../skills/conventions/references/rust.md");
const PYTHON_REFERENCE: &str = include_str!("../../../skills/conventions/references/python.md");
const TYPESCRIPT_REFERENCE: &str =
    include_str!("../../../skills/conventions/references/typescript.md");
const GO_REFERENCE: &str = include_str!("../../../skills/conventions/references/go.md");

/// The reference markdown for a language id (`retrofit::detect_languages` ids), if we have one.
fn reference_for(language: &str) -> Option<&'static str> {
    match language {
        "rust" => Some(RUST_REFERENCE),
        "python" => Some(PYTHON_REFERENCE),
        "typescript" => Some(TYPESCRIPT_REFERENCE),
        "go" => Some(GO_REFERENCE),
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

    /// Section-bounded drift guard shared by the per-language reference tests: every spine
    /// idiom must appear as a `## ` heading in the reference file, and its tag line must live
    /// in that heading's own section (up to the next `## `), so a wrong tag cannot be
    /// satisfied by a sibling section's tag.
    fn assert_reference_headings_match_the_spine(file: &str, conventions: &[Convention]) {
        let md = std::fs::read_to_string(skill_dir().join("references").join(file))
            .unwrap_or_else(|_| panic!("read {file}"));
        for c in conventions {
            let heading = format!("## {}", c.idiom);
            assert!(
                md.contains(&heading),
                "{file} is missing the spine idiom as a heading:\n{heading}"
            );
            let after = md.split(&heading).nth(1).expect("heading present");
            let section = after.split("\n## ").next().unwrap_or(after);
            let tag = match c.severity {
                ConventionSeverity::Correctness => "Tag: correctness",
                ConventionSeverity::Advisory => "Tag: advisory",
            };
            assert!(
                section.contains(tag),
                "wrong or missing tag for idiom in {file}:\n{}\nexpected {tag}",
                c.idiom
            );
        }
    }

    /// A spine is a usable convention list: non-empty, carries both severities, and every
    /// idiom is a non-empty one-liner (they double as reference headings).
    fn assert_spine_is_wellformed(conventions: &[Convention]) {
        assert!(!conventions.is_empty());
        assert!(conventions
            .iter()
            .any(|c| c.severity == ConventionSeverity::Correctness));
        assert!(conventions
            .iter()
            .any(|c| c.severity == ConventionSeverity::Advisory));
        for c in conventions {
            assert!(!c.idiom.trim().is_empty());
            assert!(
                !c.idiom.contains('\n'),
                "idiom must be one line: {}",
                c.idiom
            );
        }
    }

    #[test]
    fn python_reference_headings_match_the_spine_verbatim() {
        assert_reference_headings_match_the_spine("python.md", PYTHON_CONVENTIONS);
    }

    #[test]
    fn typescript_reference_headings_match_the_spine_verbatim() {
        assert_reference_headings_match_the_spine("typescript.md", TYPESCRIPT_CONVENTIONS);
    }

    #[test]
    fn go_reference_headings_match_the_spine_verbatim() {
        assert_reference_headings_match_the_spine("go.md", GO_CONVENTIONS);
    }

    #[test]
    fn python_spine_is_nonempty_and_carries_both_severities() {
        assert_spine_is_wellformed(PYTHON_CONVENTIONS);
    }

    #[test]
    fn typescript_spine_is_nonempty_and_carries_both_severities() {
        assert_spine_is_wellformed(TYPESCRIPT_CONVENTIONS);
    }

    #[test]
    fn go_spine_is_nonempty_and_carries_both_severities() {
        assert_spine_is_wellformed(GO_CONVENTIONS);
    }

    #[test]
    fn preamble_for_each_language_includes_the_contract_and_an_idiom() {
        for (lang, needle, first) in [
            ("python", "Python conventions", PYTHON_CONVENTIONS[0].idiom),
            (
                "typescript",
                "TypeScript conventions",
                TYPESCRIPT_CONVENTIONS[0].idiom,
            ),
            ("go", "Go conventions", GO_CONVENTIONS[0].idiom),
        ] {
            let p = conventions_preamble(&[lang.to_string()])
                .unwrap_or_else(|| panic!("{lang} preamble"));
            assert!(
                p.contains("If you are reviewing"),
                "{lang} carries the SKILL.md contract"
            );
            assert!(p.contains(needle), "{lang} carries its reference");
            assert!(p.contains(first), "{lang} carries a spine idiom");
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
        let disk_python =
            std::fs::read_to_string(skill_dir().join("references/python.md")).unwrap();
        let disk_typescript =
            std::fs::read_to_string(skill_dir().join("references/typescript.md")).unwrap();
        let disk_go = std::fs::read_to_string(skill_dir().join("references/go.md")).unwrap();
        assert_eq!(SKILL_MD, disk_skill, "embedded SKILL.md drifted from disk");
        assert_eq!(
            RUST_REFERENCE, disk_rust,
            "embedded rust.md drifted from disk"
        );
        assert_eq!(
            PYTHON_REFERENCE, disk_python,
            "embedded python.md drifted from disk"
        );
        assert_eq!(
            TYPESCRIPT_REFERENCE, disk_typescript,
            "embedded typescript.md drifted from disk"
        );
        assert_eq!(GO_REFERENCE, disk_go, "embedded go.md drifted from disk");
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
    fn provider_injects_every_detected_language_in_a_polyglot_worktree() {
        use tutti_core::conventions::ConventionsProvider;
        use tutti_core::message::Role;
        // A worktree that trips two language markers gets both references, once each.
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("Cargo.toml"), "[package]\nname='x'\n").unwrap();
        std::fs::write(d.path().join("go.mod"), "module example.com/x\n\ngo 1.23\n").unwrap();
        let out = ConventionsSkill
            .preamble_for(Role::Reviewer, d.path())
            .expect("a polyglot worktree gets a preamble");
        assert!(
            out.contains("Rust conventions"),
            "carries the rust reference"
        );
        assert!(out.contains("Go conventions"), "carries the go reference");
        // The shared contract body appears exactly once (not once per language).
        assert_eq!(
            out.matches("If you are reviewing").count(),
            1,
            "the SKILL.md contract is injected once, not per language"
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
            assert!(
                !c.idiom.contains('\n'),
                "idiom must be one line: {}",
                c.idiom
            );
        }
    }
}
