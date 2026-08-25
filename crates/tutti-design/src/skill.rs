// SPDX-License-Identifier: AGPL-3.0-or-later
//! Load and represent an Anthropic-style `SKILL.md` skill directory: the YAML
//! frontmatter (`name`, `description`), the markdown body, and the sibling reference
//! and script files a skill carries alongside `SKILL.md`.

use crate::error::DesignError;
use std::path::{Path, PathBuf};

/// The two required SKILL.md frontmatter fields (Anthropic standard). Other fields
/// (allowed-tools, metadata, etc.) are tool-specific and not needed here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frontmatter {
    pub name: String,
    pub description: String,
}

/// A loaded SKILL.md skill directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skill {
    pub name: String,
    pub description: String,
    /// The markdown body after the frontmatter block.
    pub body: String,
    /// The skill's directory.
    pub dir: PathBuf,
    /// Sibling reference files next to SKILL.md (any other `*.md`), sorted.
    pub references: Vec<PathBuf>,
    /// Files under a `scripts/` subdirectory, if present, sorted.
    pub scripts: Vec<PathBuf>,
}

/// Split a SKILL.md file's content into its YAML frontmatter fields and its body.
///
/// The frontmatter is the block between the first two lines that are exactly `---`.
/// Only the two required scalar keys are parsed (`name`, `description`); values may be
/// bare or wrapped in single or double quotes. A file with no frontmatter, a frontmatter
/// missing either key, or a duplicate key is an error.
pub fn parse_frontmatter(content: &str) -> Result<(Frontmatter, String), DesignError> {
    let mut lines = content.lines();

    let first = lines
        .next()
        .ok_or_else(|| DesignError::Skill("empty SKILL.md content".to_string()))?;
    if first != "---" {
        return Err(DesignError::Skill(
            "SKILL.md must open with a `---` frontmatter fence on the first line".to_string(),
        ));
    }

    let mut name: Option<String> = None;
    let mut description: Option<String> = None;
    let mut closed = false;
    let mut consumed_bytes = first.len() + 1; // "---\n"

    for line in lines {
        consumed_bytes += line.len() + 1;
        if line == "---" {
            closed = true;
            break;
        }
        let Some((key, raw_value)) = line.split_once(':') else {
            continue;
        };
        let key = key.trim();
        let value = strip_matching_quotes(raw_value.trim());

        match key {
            "name" => {
                if name.is_some() {
                    return Err(DesignError::Skill(
                        "duplicate `name` key in SKILL.md frontmatter".to_string(),
                    ));
                }
                name = Some(value.to_string());
            }
            "description" => {
                if description.is_some() {
                    return Err(DesignError::Skill(
                        "duplicate `description` key in SKILL.md frontmatter".to_string(),
                    ));
                }
                description = Some(value.to_string());
            }
            _ => {}
        }
    }

    if !closed {
        return Err(DesignError::Skill(
            "SKILL.md frontmatter is missing its closing `---` fence".to_string(),
        ));
    }

    let name = name.ok_or_else(|| {
        DesignError::Skill("SKILL.md frontmatter is missing required key `name`".to_string())
    })?;
    let description = description.ok_or_else(|| {
        DesignError::Skill("SKILL.md frontmatter is missing required key `description`".to_string())
    })?;

    let body = content
        .get(consumed_bytes.min(content.len())..)
        .unwrap_or("")
        .strip_prefix('\n')
        .unwrap_or_else(|| {
            content
                .get(consumed_bytes.min(content.len())..)
                .unwrap_or("")
        })
        .to_string();

    Ok((Frontmatter { name, description }, body))
}

/// Strip a single matching pair of surrounding `'` or `"` quotes from a value, if present.
fn strip_matching_quotes(value: &str) -> &str {
    let bytes = value.as_bytes();
    if bytes.len() >= 2 {
        let first = bytes[0];
        let last = bytes[bytes.len() - 1];
        if (first == b'\'' || first == b'"') && first == last {
            return &value[1..value.len() - 1];
        }
    }
    value
}

/// Load a skill from its directory (expects `<dir>/SKILL.md`).
pub fn load(dir: &Path) -> Result<Skill, DesignError> {
    let skill_md = dir.join("SKILL.md");
    let content = std::fs::read_to_string(&skill_md)
        .map_err(|e| DesignError::Skill(format!("reading {}: {e}", skill_md.display())))?;
    let (frontmatter, body) = parse_frontmatter(&content)?;

    let mut references = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file()
                && path.extension().and_then(|e| e.to_str()) == Some("md")
                && path.file_name().and_then(|n| n.to_str()) != Some("SKILL.md")
            {
                references.push(path);
            }
        }
    }
    references.sort();

    let mut scripts = Vec::new();
    let scripts_dir = dir.join("scripts");
    if let Ok(entries) = std::fs::read_dir(&scripts_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() {
                scripts.push(path);
            }
        }
    }
    scripts.sort();

    Ok(Skill {
        name: frontmatter.name,
        description: frontmatter.description,
        body,
        dir: dir.to_path_buf(),
        references,
        scripts,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn parse_frontmatter_extracts_name_and_description() {
        let content = "---\nname: pdf-extractor\ndescription: Extract text from PDFs.\n---\n# Body\nSome content.\n";
        let (fm, body) = parse_frontmatter(content).unwrap();
        assert_eq!(fm.name, "pdf-extractor");
        assert_eq!(fm.description, "Extract text from PDFs.");
        assert_eq!(body, "# Body\nSome content.\n");
    }

    #[test]
    fn parse_frontmatter_strips_matching_quotes() {
        let content = "---\nname: x\ndescription: \"Does X. Use when Y.\"\n---\nbody\n";
        let (fm, _) = parse_frontmatter(content).unwrap();
        assert_eq!(fm.description, "Does X. Use when Y.");
    }

    #[test]
    fn parse_frontmatter_errors_without_fence() {
        let content = "name: x\ndescription: y\n";
        assert!(matches!(
            parse_frontmatter(content),
            Err(DesignError::Skill(_))
        ));
    }

    #[test]
    fn parse_frontmatter_errors_when_a_required_key_is_missing() {
        let content = "---\nname: x\n---\nbody\n";
        assert!(matches!(
            parse_frontmatter(content),
            Err(DesignError::Skill(_))
        ));
    }

    #[test]
    fn parse_frontmatter_ignores_unknown_keys() {
        let content = "---\nname: x\ndescription: y\nlicense: MIT\n---\nbody\n";
        let (fm, _) = parse_frontmatter(content).unwrap();
        assert_eq!(fm.name, "x");
        assert_eq!(fm.description, "y");
    }

    #[test]
    fn load_reads_a_skill_dir_with_references_and_scripts() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("SKILL.md"),
            "---\nname: demo\ndescription: A demo skill.\n---\n# Demo\nUse it well.\n",
        )
        .unwrap();
        fs::write(dir.path().join("reference.md"), "# Reference\n").unwrap();
        fs::create_dir(dir.path().join("scripts")).unwrap();
        fs::write(dir.path().join("scripts").join("check.sh"), "#!/bin/sh\n").unwrap();

        let skill = load(dir.path()).unwrap();
        assert_eq!(skill.name, "demo");
        assert_eq!(skill.description, "A demo skill.");
        assert_eq!(skill.body, "# Demo\nUse it well.\n");
        assert_eq!(skill.references, vec![dir.path().join("reference.md")]);
        assert_eq!(
            skill.scripts,
            vec![dir.path().join("scripts").join("check.sh")]
        );
    }

    #[test]
    fn load_errors_when_skill_md_absent() {
        let dir = tempfile::tempdir().unwrap();
        assert!(matches!(load(dir.path()), Err(DesignError::Skill(_))));
    }
}
