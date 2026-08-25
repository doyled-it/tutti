// SPDX-License-Identifier: AGPL-3.0-or-later
//! Structural lint over a loaded `Skill`: the deterministic authoring rules Anthropic's
//! SKILL.md guidance calls for (name charset and length, description length, no XML-ish
//! tags, a bounded body, and forward-slash paths). This is a purely structural check;
//! it says nothing about whether the skill's content is actually good.

use crate::skill::Skill;

/// A single structural-lint violation of the SKILL.md authoring rules.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Violation {
    /// A stable machine rule id, e.g. "name-charset", "description-length".
    pub rule: &'static str,
    /// A human-readable description of what is wrong.
    pub message: String,
}

const MAX_NAME_LEN: usize = 64;
const MAX_DESCRIPTION_LEN: usize = 1024;
const MAX_BODY_LINES: usize = 500;
const RESERVED_WORDS: [&str; 2] = ["anthropic", "claude"];

/// Check a skill against the deterministic SKILL.md authoring rules (Anthropic guidance).
/// Returns every violation found; an empty vec means the skill is structurally clean.
pub fn lint(skill: &Skill) -> Vec<Violation> {
    let mut violations = Vec::new();

    if skill.name.is_empty() {
        violations.push(Violation {
            rule: "name-present",
            message: "skill name is empty".to_string(),
        });
    }

    if skill.name.len() > MAX_NAME_LEN {
        violations.push(Violation {
            rule: "name-length",
            message: format!(
                "skill name is {} characters, max is {MAX_NAME_LEN}",
                skill.name.len()
            ),
        });
    }

    if !skill.name.is_empty() && !is_valid_name_charset(&skill.name) {
        violations.push(Violation {
            rule: "name-charset",
            message: format!(
                "skill name \"{}\" must match ^[a-z0-9-]+$ (lowercase letters, digits, hyphens only)",
                skill.name
            ),
        });
    }

    let lower_name = skill.name.to_lowercase();
    if let Some(word) = RESERVED_WORDS
        .iter()
        .find(|w| lower_name.split('-').any(|segment| segment == **w))
    {
        violations.push(Violation {
            rule: "name-reserved",
            message: format!(
                "skill name \"{}\" contains the reserved word \"{word}\"",
                skill.name
            ),
        });
    }

    if skill.description.is_empty() {
        violations.push(Violation {
            rule: "description-present",
            message: "skill description is empty".to_string(),
        });
    }

    if skill.description.len() > MAX_DESCRIPTION_LEN {
        violations.push(Violation {
            rule: "description-length",
            message: format!(
                "skill description is {} characters, max is {MAX_DESCRIPTION_LEN}",
                skill.description.len()
            ),
        });
    }

    if contains_xml_tag(&skill.name) {
        violations.push(Violation {
            rule: "no-xml-tags",
            message: "skill name contains an XML-ish tag".to_string(),
        });
    }
    if contains_xml_tag(&skill.description) {
        violations.push(Violation {
            rule: "no-xml-tags",
            message: "skill description contains an XML-ish tag".to_string(),
        });
    }

    // `str::lines()` does not count a trailing terminator as an extra line, unlike
    // `split('\n')`, which over-counts a canonical newline-terminated body by one.
    let body_lines = skill.body.lines().count();
    if body_lines > MAX_BODY_LINES {
        violations.push(Violation {
            rule: "body-length",
            message: format!("skill body is {body_lines} lines, max is {MAX_BODY_LINES}"),
        });
    }

    // `references`/`scripts` are `PathBuf`s this crate itself built with `Path::join`, so
    // they carry the host's native separator and say nothing about what the author wrote.
    // Scan the author's own content instead for a backslash used as a path separator.
    if contains_backslash_path(&skill.body) || contains_backslash_path(&skill.description) {
        violations.push(Violation {
            rule: "forward-slash-paths",
            message: "skill content uses a backslash as a path separator; use forward slashes"
                .to_string(),
        });
    }

    violations
}

/// True when `text` contains a `\` flanked on both sides by a non-whitespace,
/// non-backslash character, the shape of a Windows-style path separator (`scripts\check.sh`)
/// rather than ordinary prose punctuation.
fn contains_backslash_path(text: &str) -> bool {
    let chars: Vec<char> = text.chars().collect();
    chars.iter().enumerate().any(|(i, &c)| {
        if c != '\\' {
            return false;
        }
        let before_ok = i > 0 && {
            let b = chars[i - 1];
            !b.is_whitespace() && b != '\\'
        };
        let after_ok = i + 1 < chars.len() && {
            let a = chars[i + 1];
            !a.is_whitespace() && a != '\\'
        };
        before_ok && after_ok
    })
}

fn is_valid_name_charset(name: &str) -> bool {
    name.chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// True when `text` contains a `<...>` XML-ish tag: a `<` immediately followed by an ASCII
/// letter, `/`, or `!` (a plausible tag opener, e.g. `<b>`, `</b>`, `<!--`), with a later
/// `>`. A bare comparison like `x < 5 and result > 10` does not qualify, since `<` there is
/// followed by a digit and a space, not a tag-like character.
fn contains_xml_tag(text: &str) -> bool {
    let bytes = text.as_bytes();
    for (i, &b) in bytes.iter().enumerate() {
        if b != b'<' {
            continue;
        }
        let Some(&next) = bytes.get(i + 1) else {
            continue;
        };
        let looks_like_tag_open = next.is_ascii_alphabetic() || next == b'/' || next == b'!';
        if looks_like_tag_open && text[i + 1..].contains('>') {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn well_formed() -> Skill {
        Skill {
            name: "pdf-extractor".to_string(),
            description: "Extract text from PDF files.".to_string(),
            body: "# PDF Extractor\nUse this skill to pull text out of PDFs.\n".to_string(),
            dir: PathBuf::from("/tmp/pdf-extractor"),
            references: vec![PathBuf::from("/tmp/pdf-extractor/reference.md")],
            scripts: vec![PathBuf::from("/tmp/pdf-extractor/scripts/extract.py")],
        }
    }

    #[test]
    fn lint_passes_a_well_formed_skill() {
        assert_eq!(lint(&well_formed()), vec![]);
    }

    #[test]
    fn lint_flags_uppercase_or_bad_charset_name() {
        let mut skill = well_formed();
        skill.name = "Bad_Name".to_string();
        let violations = lint(&skill);
        assert!(violations.iter().any(|v| v.rule == "name-charset"));
    }

    #[test]
    fn lint_flags_a_too_long_name() {
        let mut skill = well_formed();
        skill.name = "a".repeat(65);
        let violations = lint(&skill);
        assert!(violations.iter().any(|v| v.rule == "name-length"));
    }

    #[test]
    fn lint_flags_reserved_words() {
        let mut skill = well_formed();
        skill.name = "claude-helper".to_string();
        let violations = lint(&skill);
        assert!(violations.iter().any(|v| v.rule == "name-reserved"));
    }

    #[test]
    fn lint_flags_empty_description() {
        let mut skill = well_formed();
        skill.description = String::new();
        let violations = lint(&skill);
        assert!(violations.iter().any(|v| v.rule == "description-present"));
    }

    #[test]
    fn lint_flags_a_too_long_description() {
        let mut skill = well_formed();
        skill.description = "a".repeat(1025);
        let violations = lint(&skill);
        assert!(violations.iter().any(|v| v.rule == "description-length"));
    }

    #[test]
    fn lint_flags_xml_tags_in_description() {
        let mut skill = well_formed();
        skill.description = "Use this <b>skill</b> for PDFs.".to_string();
        let violations = lint(&skill);
        assert!(violations.iter().any(|v| v.rule == "no-xml-tags"));
    }

    #[test]
    fn lint_flags_an_over_long_body() {
        let mut skill = well_formed();
        skill.body = "line\n".repeat(501);
        let violations = lint(&skill);
        assert!(violations.iter().any(|v| v.rule == "body-length"));
    }

    #[test]
    fn lint_can_report_multiple_violations_at_once() {
        let mut skill = well_formed();
        skill.name = "Bad_Name".to_string();
        skill.description = String::new();
        let violations = lint(&skill);
        assert!(violations.iter().any(|v| v.rule == "name-charset"));
        assert!(violations.iter().any(|v| v.rule == "description-present"));
    }

    #[test]
    fn lint_accepts_exactly_500_lines_and_flags_501() {
        let mut skill = well_formed();
        skill.body = "line\n".repeat(500);
        assert!(!lint(&skill).iter().any(|v| v.rule == "body-length"));

        skill.body = "line\n".repeat(501);
        assert!(lint(&skill).iter().any(|v| v.rule == "body-length"));
    }

    #[test]
    fn lint_does_not_flag_a_bare_comparison_as_an_xml_tag() {
        let mut skill = well_formed();
        skill.description = "use when x < 5 and result > 10".to_string();
        assert!(!lint(&skill).iter().any(|v| v.rule == "no-xml-tags"));
    }

    #[test]
    fn lint_still_flags_a_real_xml_tag() {
        let mut skill = well_formed();
        skill.description = "Use this <b>skill</b> for PDFs.".to_string();
        assert!(lint(&skill).iter().any(|v| v.rule == "no-xml-tags"));
    }

    #[test]
    fn lint_flags_a_backslash_path_in_the_body() {
        let mut skill = well_formed();
        skill.body = "See scripts\\check.sh for details.".to_string();
        assert!(lint(&skill).iter().any(|v| v.rule == "forward-slash-paths"));
    }

    #[test]
    fn lint_does_not_flag_native_forward_slash_script_paths() {
        // The enumerated paths on disk use forward slashes; the rule reads author content,
        // not these PathBufs, so a normal skill is never flagged for its own file listing.
        let skill = well_formed();
        assert!(!lint(&skill).iter().any(|v| v.rule == "forward-slash-paths"));
    }

    #[test]
    fn lint_does_not_flag_a_body_with_no_backslash() {
        let mut skill = well_formed();
        skill.body = "Nothing but plain prose here.".to_string();
        assert!(!lint(&skill).iter().any(|v| v.rule == "forward-slash-paths"));
    }

    #[test]
    fn lint_does_not_flag_a_substring_reserved_word() {
        let mut skill = well_formed();
        skill.name = "misanthropic-helper".to_string();
        assert!(!lint(&skill).iter().any(|v| v.rule == "name-reserved"));
    }

    #[test]
    fn lint_flags_a_whole_segment_reserved_word() {
        let mut skill = well_formed();
        skill.name = "claude-helper".to_string();
        assert!(lint(&skill).iter().any(|v| v.rule == "name-reserved"));
    }
}
