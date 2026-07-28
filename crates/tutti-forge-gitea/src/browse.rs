// SPDX-License-Identifier: AGPL-3.0-or-later
//! The Gitea `ForgeBrowser`: drives `tea api`. Two Gitea-specific traps are handled
//! here: the tea login name is a local alias that can differ from the username, and
//! error responses are a JSON object (not an array) with a zero exit status.

use async_trait::async_trait;
use serde::Deserialize;
use tutti_core::browse::{ForgeBrowser, Namespace, NamespaceKind, NewRepo, RemoteRepo};
use tutti_core::traits::{EngineError, Result};

/// Browses Gitea/Forgejo/Codeberg via `tea api`, using a named login.
pub struct GiteaBrowser {
    /// The `tea` login name (a local alias). May differ from the username it maps to.
    pub login: String,
}

#[derive(Deserialize)]
struct GtUser {
    login: String,
}
#[derive(Deserialize)]
struct GtOrg {
    username: String,
}
#[derive(Deserialize)]
struct GtRepo {
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
#[derive(Deserialize)]
struct GtError {
    message: String,
}

/// If `json` is a Gitea error object (`{"message": ...}`), return its message as an
/// error; otherwise `None`. Gitea returns these with a zero exit status, so the caller
/// cannot rely on the process status to detect failure.
fn as_error(json: &str) -> Option<EngineError> {
    let trimmed = json.trim_start();
    if trimmed.starts_with('{') {
        if let Ok(e) = serde_json::from_str::<GtError>(json) {
            return Some(EngineError::Forge(format!("gitea: {}", e.message)));
        }
    }
    None
}

/// Parse `tea api user` into the user's own namespace. The `login` here is the real
/// username, which is what the repo call must use, not the tea login alias.
pub fn parse_user_namespace(json: &str) -> Result<Namespace> {
    if let Some(e) = as_error(json) {
        return Err(e);
    }
    let u: GtUser = serde_json::from_str(json)
        .map_err(|e| EngineError::Forge(format!("parse tea user: {e}")))?;
    Ok(Namespace {
        path: u.login.clone(),
        name: u.login,
        kind: NamespaceKind::User,
    })
}

/// Parse `tea api user/orgs` into org namespaces.
pub fn parse_org_namespaces(json: &str) -> Result<Vec<Namespace>> {
    if let Some(e) = as_error(json) {
        return Err(e);
    }
    let orgs: Vec<GtOrg> = serde_json::from_str(json)
        .map_err(|e| EngineError::Forge(format!("parse tea orgs: {e}")))?;
    Ok(orgs
        .into_iter()
        .map(|o| Namespace {
            path: o.username.clone(),
            name: o.username,
            kind: NamespaceKind::Org,
        })
        .collect())
}

/// Parse a `tea api .../repos` list.
pub fn parse_repos(json: &str) -> Result<Vec<RemoteRepo>> {
    if let Some(e) = as_error(json) {
        return Err(e);
    }
    let repos: Vec<GtRepo> = serde_json::from_str(json)
        .map_err(|e| EngineError::Forge(format!("parse tea repos: {e}")))?;
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

/// Parse a single-repo create response. Gitea returns errors as a JSON object with a zero
/// exit status, so check for that first (same as the list parsers).
pub fn parse_created_repo(json: &str) -> Result<RemoteRepo> {
    if let Some(e) = as_error(json) {
        return Err(e);
    }
    let r: GtRepo = serde_json::from_str(json)
        .map_err(|e| EngineError::Forge(format!("parse tea created repo: {e}")))?;
    Ok(RemoteRepo {
        full_path: r.full_name,
        name: r.name,
        description: r.description.filter(|d| !d.is_empty()),
        clone_url: r.clone_url,
        private: r.private,
        archived: r.archived,
    })
}

impl GiteaBrowser {
    /// `tea api --login <login>` with the endpoint LAST (urfave-cli v1 stops parsing
    /// flags after the first positional).
    async fn api(&self, endpoint: &str) -> Result<String> {
        crate::run("tea", &["api", "--login", &self.login, endpoint], None).await
    }

    /// `tea api --login <l> -X POST -d <body> <endpoint>`. Flags and body MUST precede the
    /// endpoint positional (urfave-cli v1 stops parsing flags after it).
    async fn api_post(&self, endpoint: &str, body: &str) -> Result<String> {
        crate::run(
            "tea",
            &[
                "api",
                "--login",
                &self.login,
                "-X",
                "POST",
                "-d",
                body,
                endpoint,
            ],
            None,
        )
        .await
    }
}

#[async_trait]
impl ForgeBrowser for GiteaBrowser {
    async fn list_namespaces(&self) -> Result<Vec<Namespace>> {
        // The user namespace carries the real username, which the repo call keys on.
        let user = parse_user_namespace(&self.api("user").await?)?;
        let mut out = vec![user];
        out.extend(parse_org_namespaces(&self.api("user/orgs").await?)?);
        Ok(out)
    }
    async fn list_repos(&self, ns: &Namespace) -> Result<Vec<RemoteRepo>> {
        let endpoint = match ns.kind {
            NamespaceKind::Org => format!("orgs/{}/repos", ns.path),
            _ => format!("users/{}/repos", ns.path),
        };
        parse_repos(&self.api(&endpoint).await?)
    }
    async fn create_repo(&self, ns: &Namespace, spec: &NewRepo) -> Result<RemoteRepo> {
        // Different endpoints by kind: `/user/repos` vs `/orgs/{org}/repos`.
        let endpoint = match ns.kind {
            NamespaceKind::User => "user/repos".to_string(),
            _ => format!("orgs/{}/repos", ns.path),
        };
        let mut body = serde_json::json!({
            "name": spec.name,
            "private": spec.private,
            "auto_init": true,
        });
        if let Some(d) = spec.description.as_deref().filter(|d| !d.is_empty()) {
            body["description"] = serde_json::json!(d);
        }
        parse_created_repo(&self.api_post(&endpoint, &body.to_string()).await?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_user_orgs_repos() {
        let ns = parse_user_namespace(include_str!("../tests/fixtures/browse_user.json")).unwrap();
        assert_eq!(ns.path, "doyled-it");
        let orgs =
            parse_org_namespaces(include_str!("../tests/fixtures/browse_orgs.json")).unwrap();
        assert_eq!(orgs[0].path, "some-org");
        let repos = parse_repos(include_str!("../tests/fixtures/browse_repos.json")).unwrap();
        assert_eq!(repos.len(), 2);
        assert!(repos[1].private);
    }

    #[test]
    fn error_object_becomes_an_error_not_a_parse_failure() {
        let err = parse_repos(include_str!("../tests/fixtures/browse_error.json")).unwrap_err();
        assert!(format!("{err}").contains("user redirect does not exist"));
    }

    #[test]
    fn parses_a_created_repo() {
        let r = parse_created_repo(include_str!("../tests/fixtures/create_repo.json")).unwrap();
        assert_eq!(r.full_path, "doyled-it/widget");
        assert!(r.private);
        assert_eq!(r.clone_url, "https://codeberg.org/doyled-it/widget.git");
    }

    #[test]
    fn created_repo_error_object_is_an_error() {
        let err =
            parse_created_repo(include_str!("../tests/fixtures/browse_error.json")).unwrap_err();
        assert!(format!("{err}").contains("gitea"));
    }
}
