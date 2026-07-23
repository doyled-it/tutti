// SPDX-License-Identifier: AGPL-3.0-or-later
//! The GitHub `ForgeBrowser`: drives `gh api`.

use async_trait::async_trait;
use serde::Deserialize;
use tutti_core::browse::{ForgeBrowser, Namespace, NamespaceKind, RemoteRepo};
use tutti_core::traits::Result;

/// Browses GitHub via `gh api`. Needs no repo, unlike `GitHubForge`.
pub struct GitHubBrowser;

#[derive(Deserialize)]
struct GhUser {
    login: String,
}
#[derive(Deserialize)]
struct GhOrg {
    login: String,
}
#[derive(Deserialize)]
struct GhRepo {
    full_name: String,
    name: String,
    #[serde(default)]
    description: Option<String>,
    clone_url: String,
    #[serde(default)]
    private: bool,
    #[serde(default)]
    archived: bool,
}

/// Parse `gh api user` into the user's own namespace.
pub fn parse_user_namespace(json: &str) -> Result<Namespace> {
    let u: GhUser = serde_json::from_str(json)
        .map_err(|e| tutti_core::traits::EngineError::Forge(format!("parse gh user: {e}")))?;
    Ok(Namespace {
        path: u.login.clone(),
        name: u.login,
        kind: NamespaceKind::User,
    })
}

/// Parse `gh api user/orgs` into org namespaces.
pub fn parse_org_namespaces(json: &str) -> Result<Vec<Namespace>> {
    let orgs: Vec<GhOrg> = serde_json::from_str(json)
        .map_err(|e| tutti_core::traits::EngineError::Forge(format!("parse gh orgs: {e}")))?;
    Ok(orgs
        .into_iter()
        .map(|o| Namespace {
            path: o.login.clone(),
            name: o.login,
            kind: NamespaceKind::Org,
        })
        .collect())
}

/// Parse a `gh api .../repos` list.
pub fn parse_repos(json: &str) -> Result<Vec<RemoteRepo>> {
    let repos: Vec<GhRepo> = serde_json::from_str(json)
        .map_err(|e| tutti_core::traits::EngineError::Forge(format!("parse gh repos: {e}")))?;
    Ok(repos
        .into_iter()
        .map(|r| RemoteRepo {
            full_path: r.full_name,
            name: r.name,
            description: r.description.filter(|d| !d.is_empty()),
            clone_url: r.clone_url,
            private: r.private,
            archived: r.archived,
        })
        .collect())
}

#[async_trait]
impl ForgeBrowser for GitHubBrowser {
    async fn list_namespaces(&self) -> Result<Vec<Namespace>> {
        let mut out = vec![parse_user_namespace(&gh(&["api", "user"]).await?)?];
        out.extend(parse_org_namespaces(
            &gh(&["api", "user/orgs", "--paginate"]).await?,
        )?);
        Ok(out)
    }
    async fn list_repos(&self, ns: &Namespace) -> Result<Vec<RemoteRepo>> {
        let endpoint = match ns.kind {
            NamespaceKind::Org => format!("orgs/{}/repos", ns.path),
            _ => format!("users/{}/repos", ns.path),
        };
        parse_repos(&gh(&["api", &endpoint, "--paginate"]).await?)
    }
}

async fn gh(args: &[&str]) -> Result<String> {
    crate::run("gh", args, None).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_user_and_orgs() {
        let ns = parse_user_namespace(include_str!("../tests/fixtures/browse_user.json")).unwrap();
        assert_eq!(ns.path, "doyled-it");
        assert_eq!(ns.kind, NamespaceKind::User);
        let orgs =
            parse_org_namespaces(include_str!("../tests/fixtures/browse_orgs.json")).unwrap();
        assert_eq!(orgs.len(), 2);
        assert_eq!(orgs[0].path, "EpicGames");
        assert_eq!(orgs[0].kind, NamespaceKind::Org);
    }

    #[test]
    fn parses_repos_with_private_and_archived() {
        let repos = parse_repos(include_str!("../tests/fixtures/browse_repos.json")).unwrap();
        assert_eq!(repos.len(), 3);
        assert_eq!(repos[0].full_path, "doyled-it/agent-view");
        assert!(!repos[0].private);
        assert!(repos[1].private);
        assert_eq!(repos[1].description, None);
        assert!(repos[2].archived);
    }
}
