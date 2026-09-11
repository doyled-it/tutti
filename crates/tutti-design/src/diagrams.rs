// SPDX-License-Identifier: AGPL-3.0-or-later
//! Tests that every diagram sub-skill loads, lints clean, ships evals, and carries a
//! well-formed example in the house SVG vocabulary, and that every diagram id a movement
//! references has a skill.

#[cfg(test)]
mod tests {
    use crate::movement::RAILS;
    use std::collections::BTreeSet;
    use std::path::PathBuf;

    const DIAGRAM_IDS: &[&str] = &[
        "flow",
        "data_flow",
        "sequence",
        "architecture",
        "network",
        "story_map",
    ];

    fn diagram_dir(id: &str) -> PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../skills/design/diagrams")
            .join(id)
    }

    #[test]
    fn every_diagram_skill_loads_lints_and_has_evals() {
        for id in DIAGRAM_IDS {
            let dir = diagram_dir(id);
            let skill = crate::skill::load(&dir).unwrap_or_else(|e| panic!("{id} load: {e:?}"));
            assert!(
                crate::lint::lint(&skill).is_empty(),
                "{id} lint: {:?}",
                crate::lint::lint(&skill)
            );
            let evals =
                crate::eval::load_evals(&dir).unwrap_or_else(|e| panic!("{id} evals: {e:?}"));
            assert!(
                crate::eval::has_minimum_evals(&evals),
                "{id} needs >=3 evals"
            );
        }
    }

    #[test]
    fn every_diagram_example_is_well_formed_and_uses_the_vocabulary() {
        for id in DIAGRAM_IDS {
            let svg = std::fs::read_to_string(diagram_dir(id).join("example.svg"))
                .unwrap_or_else(|e| panic!("{id} example.svg: {e}"));
            crate::svg::is_well_formed_svg(&svg)
                .unwrap_or_else(|e| panic!("{id} example.svg not well formed: {e}"));
            assert!(svg.contains("<svg"), "{id} example must be an svg");
            // Uses at least one house-style class (svg-node / svg-zone), so it matches the page.
            assert!(
                svg.contains("svg-node") || svg.contains("svg-zone"),
                "{id} example must use the house vocabulary"
            );
        }
    }

    #[test]
    fn every_movement_referenced_diagram_id_has_a_skill() {
        let authored: BTreeSet<&str> = DIAGRAM_IDS.iter().copied().collect();
        let referenced: BTreeSet<&str> = RAILS
            .iter()
            .flat_map(|m| m.diagram_skills.iter().copied())
            .collect();
        for id in &referenced {
            assert!(
                authored.contains(id),
                "movement references diagram `{id}` but no skill is authored for it"
            );
        }
    }
}
