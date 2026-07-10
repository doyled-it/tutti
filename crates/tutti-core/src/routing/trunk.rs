// SPDX-License-Identifier: AGPL-3.0-or-later
use crate::domain::{BranchPlan, Issue};
use crate::traits::{Result, RoutingStrategy};

/// Every issue's work merges into one configured integration branch.
pub struct Trunk {
    integration_branch: String,
}

impl Trunk {
    pub fn new(integration_branch: &str) -> Self {
        Self {
            integration_branch: integration_branch.to_string(),
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
            create_from: Some("main".into()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::IssueId;

    #[test]
    fn routes_all_issues_to_integration_branch() {
        let s = Trunk::new("version/v0.1");
        let issue = Issue {
            id: IssueId(1),
            title: "t".into(),
            body: String::new(),
            labels: vec![],
            milestone: None,
        };
        assert_eq!(s.target_branch(&issue).unwrap().target, "version/v0.1");
    }
}
