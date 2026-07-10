// SPDX-License-Identifier: AGPL-3.0-or-later
use crate::domain::{BranchPlan, Issue};
use crate::traits::{EngineError, Result, RoutingStrategy};

/// Ports SOTTO's model: an issue's milestone (e.g. "Phase 2") maps to
/// `milestone/phase-2`, created from the highest lower-numbered phase (or main).
pub struct PhaseStacking;

impl PhaseStacking {
    /// Extract the phase number from a milestone title like "Phase 2" or "phase-2".
    fn phase_num(milestone: &str) -> Option<u32> {
        let lower = milestone.to_lowercase();
        let after = lower.split("phase").nth(1)?;
        let digits: String = after
            .chars()
            .skip_while(|c| !c.is_ascii_digit())
            .take_while(|c| c.is_ascii_digit())
            .collect();
        digits.parse().ok()
    }
}

impl RoutingStrategy for PhaseStacking {
    fn name(&self) -> &'static str {
        "phase_stacking"
    }
    fn target_branch(&self, issue: &Issue) -> Result<BranchPlan> {
        let milestone = issue.milestone.as_deref().ok_or_else(|| {
            EngineError::Routing(format!("issue {} has no milestone", issue.id.0))
        })?;
        let n = Self::phase_num(milestone).ok_or_else(|| {
            EngineError::Routing(format!("milestone '{milestone}' has no phase number"))
        })?;
        let from = if n == 0 {
            "main".to_string()
        } else {
            format!("milestone/phase-{}", n - 1)
        };
        Ok(BranchPlan {
            target: format!("milestone/phase-{n}"),
            create_from: Some(from),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::IssueId;

    fn issue_in(milestone: Option<&str>) -> Issue {
        Issue {
            id: IssueId(1),
            title: "t".into(),
            body: String::new(),
            labels: vec![],
            milestone: milestone.map(str::to_string),
        }
    }

    #[test]
    fn phase_two_stacks_off_phase_one() {
        let plan = PhaseStacking
            .target_branch(&issue_in(Some("Phase 2")))
            .unwrap();
        assert_eq!(plan.target, "milestone/phase-2");
        assert_eq!(plan.create_from.as_deref(), Some("milestone/phase-1"));
    }

    #[test]
    fn phase_zero_branches_from_main() {
        let plan = PhaseStacking
            .target_branch(&issue_in(Some("phase-0")))
            .unwrap();
        assert_eq!(plan.create_from.as_deref(), Some("main"));
    }

    #[test]
    fn missing_milestone_errors() {
        assert!(PhaseStacking.target_branch(&issue_in(None)).is_err());
    }
}
