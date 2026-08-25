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
    if let Some(word) = RESERVED_WORDS.iter().find(|w| lower_name.contains(*w)) {
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

    let body_lines = skill.body.split('\n').count();
    if body_lines > MAX_BODY_LINES {
        violations.push(Violation {
            rule: "body-length",
            message: format!("skill body is {body_lines} lines, max is {MAX_BODY_LINES}"),
        });
    }

    let has_backslash_path = skill
        .references
        .iter()
        .chain(skill.scripts.iter())
        .any(|p| p.to_string_lossy().contains('\\'));
    if has_backslash_path {
        violations.push(Violation {
            rule: "forward-slash-paths",
            message: "a reference or script path contains a backslash; use forward slashes"
                .to_string(),
        });
    }

    violations
}

fn is_valid_name_charset(name: &str) -> bool {
    name.chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// True when `text` contains a `<...>` XML-ish tag: a `<` followed later by a `>`.
fn contains_xml_tag(text: &str) -> bool {
    if let Some(open) = text.find('<') {
        text[open + 1..].contains('>')
    } else {
        false
    }
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
}
