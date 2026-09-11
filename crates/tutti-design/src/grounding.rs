// SPDX-License-Identifier: AGPL-3.0-or-later
//! Read an existing repo into a grounding summary the design chain confirms rather than asks
//! from scratch. Consumer-owned seam; the real grounder lives in tutti-app-core.

use crate::error::Result;
use crate::shape::ProjectShape;
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Domain signal read from the code: candidate entities (types/nouns) and module seams.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct DomainSignal {
    pub entities: Vec<String>,
    pub seams: Vec<String>,
}

/// A read-only summary of what an existing repo already is, injected into grounded movements.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoGrounding {
    pub stack: Vec<String>,
    pub inferred_shape: ProjectShape,
    pub docs_digest: String,
    pub domain: DomainSignal,
    pub structure: Vec<String>,
    pub already_decided: Vec<String>,
}

/// The read-a-repo seam. Consumer-owned so tutti-design stays hermetic; the real impl
/// (detect_languages + a docs reader + codegraph) lives in tutti-app-core and is wired at E7.
pub trait RepoGrounder {
    fn ground(&self, repo_root: &Path) -> Result<RepoGrounding>;
}

/// Infer the project shape from a mobile-toolchain marker and the number of top-level
/// containers (crates/packages/services). Coarse on purpose: Frame presents it for
/// confirmation, so a wrong guess costs a correction, not a bad session.
pub fn infer_shape(is_mobile: bool, container_count: usize) -> ProjectShape {
    if is_mobile {
        ProjectShape::Mobile
    } else if container_count > 1 {
        ProjectShape::MultiService
    } else {
        ProjectShape::SmallCli
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shape::ProjectShape;

    #[test]
    fn repo_grounding_roundtrips_through_serde() {
        let g = RepoGrounding {
            stack: vec!["rust".into()],
            inferred_shape: ProjectShape::SmallCli,
            docs_digest: "a private voice assistant".into(),
            domain: DomainSignal {
                entities: vec!["Session".into()],
                seams: vec!["transport".into()],
            },
            structure: vec!["crates/tutti-core".into()],
            already_decided: vec!["transport is iroh".into()],
        };
        let j = serde_json::to_string(&g).unwrap();
        assert_eq!(g, serde_json::from_str::<RepoGrounding>(&j).unwrap());
    }

    #[test]
    fn infer_shape_golden_cases() {
        // A mobile marker wins.
        assert_eq!(infer_shape(true, 1), ProjectShape::Mobile);
        assert_eq!(infer_shape(true, 5), ProjectShape::Mobile);
        // No mobile marker: multiple containers -> multi-service, else small.
        assert_eq!(infer_shape(false, 1), ProjectShape::SmallCli);
        assert_eq!(infer_shape(false, 0), ProjectShape::SmallCli);
        assert_eq!(infer_shape(false, 3), ProjectShape::MultiService);
    }
}
