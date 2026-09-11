// SPDX-License-Identifier: AGPL-3.0-or-later
//! The rails: the eight design movements as declarative data, and the rule that selects
//! which of them a given `ProjectShape` runs.

use crate::shape::ProjectShape;
use serde::{Deserialize, Serialize};

/// The identity of a design movement, in canonical chain order.
///
/// Adding or removing a variant here must be paired with an update to `RAILS` below;
/// `definition` and `movements_for` both assume every variant has a `RAILS` entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MovementId {
    Constitution,
    Frame,
    Impact,
    Domain,
    Decide,
    Structure,
    Slice,
    Decompose,
}

/// One movement's declarative definition. The prompt library (E5) reads these; E1 only
/// needs identity, ordering, and selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Movement {
    pub id: MovementId,
    pub title: &'static str,
    pub guiding_question: &'static str,
    /// The points a movement must cover before it can be ratified (E2/E5 enforce it).
    pub checklist: &'static [&'static str],
    /// Diagram sub-skills this movement may invoke (E4). Names, not implementations.
    pub diagram_skills: &'static [&'static str],
}

/// The canonical eight movements, in chain order. This is the whole rail set; a
/// `ProjectShape` selects a subset via `movements_for`.
pub const RAILS: &[Movement] = &[
    Movement {
        id: MovementId::Constitution,
        title: "Constitution",
        guiding_question: "What must stay true no matter what?",
        checklist: &["principles named", "non-negotiables named"],
        diagram_skills: &[],
    },
    Movement {
        id: MovementId::Frame,
        title: "Frame",
        guiding_question:
            "Who is this for, why do they care the day it ships, what is the budget, what is out?",
        checklist: &[
            "customer named",
            "problem named",
            "appetite set",
            "non-goals listed",
        ],
        diagram_skills: &[],
    },
    Movement {
        id: MovementId::Impact,
        title: "Impact",
        guiding_question: "What behavior change, in which actor, produces the goal?",
        checklist: &[
            "goal stated",
            "actors listed",
            "impacts mapped to deliverables",
        ],
        diagram_skills: &["flow"],
    },
    Movement {
        id: MovementId::Domain,
        title: "Domain",
        guiding_question: "What is the language and shape of this world, and where are the seams?",
        checklist: &[
            "glossary drafted",
            "entities/events listed",
            "seams identified",
        ],
        diagram_skills: &["data_flow"],
    },
    Movement {
        id: MovementId::Decide,
        title: "Decide",
        guiding_question: "What are we building, and why this shape over the alternatives?",
        checklist: &[
            "alternatives considered",
            "choice made",
            "risks named",
            "open questions listed",
        ],
        diagram_skills: &["sequence"],
    },
    Movement {
        id: MovementId::Structure,
        title: "Structure",
        guiding_question: "How do the pieces fit at each zoom level?",
        checklist: &[
            "context drawn",
            "containers drawn",
            "irreversible choices recorded as ADRs",
        ],
        diagram_skills: &["architecture", "network"],
    },
    Movement {
        id: MovementId::Slice,
        title: "Slice",
        guiding_question: "What is the thinnest end-to-end path, then the ribs?",
        checklist: &[
            "backbone mapped",
            "walking skeleton sliced",
            "ribs prioritized",
        ],
        diagram_skills: &["story_map"],
    },
    Movement {
        id: MovementId::Decompose,
        title: "Decompose",
        guiding_question: "What are the small, testable, dependency-ordered units?",
        checklist: &[
            "milestones/epics/issues drafted",
            "EARS acceptance criteria written",
            "dependencies ordered",
        ],
        diagram_skills: &[],
    },
];

/// Look up a movement's definition by id.
pub fn definition(id: MovementId) -> &'static Movement {
    RAILS
        .iter()
        .find(|m| m.id == id)
        .expect("every MovementId has a RAILS entry")
}

/// The directory name (under `skills/design/`) holding a movement's facilitation skill.
///
/// Each movement authored as a `SKILL.md` skill lives in `skills/design/<name>/`, with a
/// frontmatter `name: design-<name>`. This maps the movement identity onto that directory so
/// the loop can load the right guidance.
pub fn skill_dir_name(id: MovementId) -> &'static str {
    match id {
        MovementId::Constitution => "constitution",
        MovementId::Frame => "frame",
        MovementId::Impact => "impact",
        MovementId::Domain => "domain",
        MovementId::Decide => "decide",
        MovementId::Structure => "structure",
        MovementId::Slice => "slice",
        MovementId::Decompose => "decompose",
    }
}

/// The movements a given project shape runs, in canonical order.
///
/// Branching by shape (spec's branching table): a small CLI/library collapses the middle
/// (skips Domain and Structure); mobile and multi-service run the full chain (they differ
/// in per-movement depth, which lands in E5, not in selection).
pub fn movements_for(shape: ProjectShape) -> Vec<MovementId> {
    RAILS
        .iter()
        .map(|m| m.id)
        .filter(|id| match shape {
            ProjectShape::SmallCli => !matches!(id, MovementId::Domain | MovementId::Structure),
            ProjectShape::Mobile | ProjectShape::MultiService => true,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rails_are_in_canonical_order_and_complete() {
        let ids: Vec<MovementId> = RAILS.iter().map(|m| m.id).collect();
        assert_eq!(
            ids,
            vec![
                MovementId::Constitution,
                MovementId::Frame,
                MovementId::Impact,
                MovementId::Domain,
                MovementId::Decide,
                MovementId::Structure,
                MovementId::Slice,
                MovementId::Decompose,
            ]
        );
    }

    #[test]
    fn small_cli_collapses_the_middle() {
        let ids = movements_for(ProjectShape::SmallCli);
        assert!(!ids.contains(&MovementId::Domain));
        assert!(!ids.contains(&MovementId::Structure));
        assert_eq!(ids.len(), 6);
        // Order is preserved (canonical), just with the two dropped.
        assert_eq!(ids.first(), Some(&MovementId::Constitution));
        assert_eq!(ids.last(), Some(&MovementId::Decompose));
    }

    #[test]
    fn mobile_and_multi_service_run_the_full_chain() {
        assert_eq!(movements_for(ProjectShape::Mobile).len(), 8);
        assert_eq!(movements_for(ProjectShape::MultiService).len(), 8);
    }

    #[test]
    fn definition_resolves_every_id() {
        for m in RAILS {
            assert_eq!(definition(m.id).id, m.id);
        }
    }

    #[test]
    fn every_movement_maps_to_a_lowercase_skill_dir() {
        for m in RAILS {
            let dir = skill_dir_name(m.id);
            assert!(!dir.is_empty());
            assert!(
                dir.chars().all(|c| c.is_ascii_lowercase()),
                "{dir} must be lowercase"
            );
        }
        assert_eq!(skill_dir_name(MovementId::Constitution), "constitution");
        assert_eq!(skill_dir_name(MovementId::Decompose), "decompose");
    }
}
