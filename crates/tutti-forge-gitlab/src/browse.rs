// SPDX-License-Identifier: AGPL-3.0-or-later
//! The GitLab `ForgeBrowser`: drives `glab api`.

use async_trait::async_trait;
use serde::Deserialize;
use tutti_core::browse::{ForgeBrowser, Namespace, NamespaceKind, RemoteRepo};
use tutti_core::traits::{EngineError, Result};

/// Browses GitLab via `glab api`. GitLab keys the projects call by numeric id, so the
/// user's own id is fetched once and stored.
pub struct GitLabBrowser;

#[derive(Deserialize)]
struct GlUser {
    id: u64,
    username: String,
}
#[derive(Deserialize)]
struct GlGroup {
    id: u64,
    full_path: String,
    name: String,
}
#[derive(Deserialize)]
struct GlProject {
    name: String,
    path_with_namespace: String,
    #[serde(default)]
    description: Option<String>,
    http_url_to_repo: String,
    visibility: String,
}

/// Parse `glab api user` into the user's own namespace. The namespace `path` is the
/// numeric id (the projects call needs the id, not the username), and `name` is the
/// readable username.
pub fn parse_user_namespace(json: &str) -> Result<Namespace> {
    let u: GlUser = serde_json::from_str(json)
        .map_err(|e| EngineError::Forge(format!("parse glab user: {e}")))?;
    Ok(Namespace {
        path: u.id.to_string(),
        name: u.username,
        kind: NamespaceKind::User,
    })
}

/// Parse `glab api groups` into group namespaces (id in `path`, full_path in `name`).
pub fn parse_group_namespaces(json: &str) -> Result<Vec<Namespace>> {
    let groups: Vec<GlGroup> = serde_json::from_str(json)
        .map_err(|e| EngineError::Forge(format!("parse glab groups: {e}")))?;
    Ok(groups
        .into_iter()
        .map(|g| Namespace {
            path: g.id.to_string(),
            name: if g.full_path.is_empty() {
                g.name
            } else {
                g.full_path
            },
            kind: NamespaceKind::Group,
        })
        .collect())
}

/// Parse a `glab api .../projects` list. `private` is `visibility != "public"`.
pub fn parse_projects(json: &str) -> Result<Vec<RemoteRepo>> {
    let projects: Vec<GlProject> = serde_json::from_str(json)
        .map_err(|e| EngineError::Forge(format!("parse glab projects: {e}")))?;
    Ok(projects
        .into_iter()
        .map(|p| RemoteRepo {
            full_path: p.path_with_namespace,
            name: p.name,
            description: p.description.filter(|d| !d.is_empty()),
            clone_url: p.http_url_to_repo,
            private: p.visibility != "public",
            archived: false,
        })
        .collect())
}

#[async_trait]
impl ForgeBrowser for GitLabBrowser {
    async fn list_namespaces(&self) -> Result<Vec<Namespace>> {
        let mut out = vec![parse_user_namespace(&glab(&["api", "user"]).await?)?];
        out.extend(parse_group_namespaces(
            &glab(&["api", "groups?min_access_level=30&per_page=100"]).await?,
        )?);
        Ok(out)
    }
    async fn list_repos(&self, ns: &Namespace) -> Result<Vec<RemoteRepo>> {
        let endpoint = match ns.kind {
            NamespaceKind::Group => format!("groups/{}/projects?per_page=100", ns.path),
            _ => format!("users/{}/projects?per_page=100", ns.path),
        };
        parse_projects(&glab(&["api", &endpoint]).await?)
    }
}

async fn glab(args: &[&str]) -> Result<String> {
    crate::run("glab", args, None).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_user_and_groups_by_id() {
        let ns = parse_user_namespace(include_str!("../tests/fixtures/browse_user.json")).unwrap();
        assert_eq!(ns.path, "3080110");
        assert_eq!(ns.name, "doyled-it");
        let groups =
            parse_group_namespaces(include_str!("../tests/fixtures/browse_groups.json")).unwrap();
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[1].name, "container-manager/backend");
    }

    #[test]
    fn internal_visibility_is_not_public() {
        let projects =
            parse_projects(include_str!("../tests/fixtures/browse_projects.json")).unwrap();
        assert_eq!(projects.len(), 3);
        assert!(projects[0].private); // private
        assert!(!projects[1].private); // public
        assert!(projects[2].private); // internal is not public
        assert_eq!(projects[0].full_path, "doyled-it/tutti-glab-sandbox");
    }
}
