// SPDX-License-Identifier: AGPL-3.0-or-later
use crate::domain::{BranchPlan, Issue};
use crate::traits::{Result, RoutingStrategy};

/// Every issue's work merges into one configured integration branch.
pub struct Trunk {
    integration_branch: String,
    trunk: String,
}

impl Trunk {
    pub fn new(integration_branch: &str, trunk: &str) -> Self {
        Self {
            integration_branch: integration_branch.to_string(),
            trunk: trunk.to_string(),
        }
    }
}

impl RoutingStrategy for Trunk {
    fn name(&self) -> &'static str {
        "trunk"
    }
    fn target_branch(&self, _issue: &Issue) -> Result<BranchPlan> {
        Ok(BranchPlan {
            target: self.integration_branch.clone(),
            create_from: Some(self.trunk.clone()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::IssueId;

    #[test]
    fn routes_all_issues_to_integration_branch() {
        let s = Trunk::new("version/v0.1", "main");
        let issue = Issue {
            id: IssueId(1),
            title: "t".into(),
            body: String::new(),
            labels: vec![],
            milestone: None,
        };
        let plan = s.target_branch(&issue).unwrap();
        assert_eq!(plan.target, "version/v0.1");
        assert_eq!(plan.create_from.as_deref(), Some("main"));
    }
}
