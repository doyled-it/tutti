// SPDX-License-Identifier: AGPL-3.0-or-later
//! The GitLab `ForgeBrowser`: drives `glab api`.

use async_trait::async_trait;
use serde::Deserialize;
use tutti_core::browse::{ForgeBrowser, Namespace, NamespaceKind, NewRepo, RemoteRepo};
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

/// Parse a single-project create response (`POST /projects`). `private` is
/// `visibility != "public"`, the same rule the list parser uses.
pub fn parse_created_project(json: &str) -> Result<RemoteRepo> {
    let p: GlProject = serde_json::from_str(json)
        .map_err(|e| EngineError::Forge(format!("parse glab created project: {e}")))?;
    Ok(RemoteRepo {
        full_path: p.path_with_namespace,
        name: p.name,
        description: p.description.filter(|d| !d.is_empty()),
        clone_url: p.http_url_to_repo,
        private: p.visibility != "public",
        archived: false,
    })
}

/// Page size for the paginated list calls. GitLab caps `per_page` at 100.
const PER_PAGE: usize = 100;
/// Runaway guard on the page loop. 100 pages is 10,000 items, well past any real
/// namespace; hitting it means something is wrong, not that a user has that many repos.
const MAX_PAGES: usize = 100;

#[async_trait]
impl ForgeBrowser for GitLabBrowser {
    async fn list_namespaces(&self) -> Result<Vec<Namespace>> {
        let mut out = vec![parse_user_namespace(&glab(&["api", "user"]).await?)?];
        // `glab api --paginate` concatenates the per-page arrays (`][` between them),
        // which is not valid JSON, so page explicitly and parse each page on its own.
        out.extend(
            paginate(
                |page| format!("groups?min_access_level=30&per_page={PER_PAGE}&page={page}"),
                fetch_page,
                parse_group_namespaces,
            )
            .await?,
        );
        Ok(out)
    }
    async fn list_repos(&self, ns: &Namespace) -> Result<Vec<RemoteRepo>> {
        let base = match ns.kind {
            NamespaceKind::Group => format!("groups/{}/projects", ns.path),
            _ => format!("users/{}/projects", ns.path),
        };
        paginate(
            |page| format!("{base}?per_page={PER_PAGE}&page={page}"),
            fetch_page,
            parse_projects,
        )
        .await
    }
    async fn create_repo(&self, ns: &Namespace, spec: &NewRepo) -> Result<RemoteRepo> {
        let visibility = if spec.private { "private" } else { "public" };
        // On GitLab `Namespace.path` is the numeric id; a group needs `namespace_id`, the
        // authenticated user's own namespace is the default when it is omitted.
        let mut fields: Vec<(&str, &str)> = vec![
            ("name", spec.name.as_str()),
            ("visibility", visibility),
            ("initialize_with_readme", "true"),
        ];
        if let NamespaceKind::Group = ns.kind {
            fields.push(("namespace_id", ns.path.as_str()));
        }
        if let Some(d) = spec.description.as_deref().filter(|d| !d.is_empty()) {
            fields.push(("description", d));
        }
        parse_created_project(&glab_post("projects", &fields).await?)
    }
}

/// Fetch one page's raw JSON from `glab api`. Separated from `paginate` so the loop can
/// be tested without shelling out.
async fn fetch_page(endpoint: String) -> Result<String> {
    glab(&["api", &endpoint]).await
}

/// Walk pages 1.. building each page's endpoint with `endpoint`, fetching with `fetch`
/// and parsing with `parse`, until a page comes back shorter than a full page (the last
/// one) or the runaway guard trips. Each page is a self-contained JSON array, so parsing
/// is per page rather than over the concatenated `--paginate` output.
async fn paginate<T, F, Fut>(
    endpoint: impl Fn(usize) -> String,
    fetch: F,
    parse: impl Fn(&str) -> Result<Vec<T>>,
) -> Result<Vec<T>>
where
    F: Fn(String) -> Fut,
    Fut: std::future::Future<Output = Result<String>>,
{
    let mut out = Vec::new();
    for page in 1..=MAX_PAGES {
        let items = parse(&fetch(endpoint(page)).await?)?;
        let full = items.len() == PER_PAGE;
        out.extend(items);
        if !full {
            break;
        }
    }
    Ok(out)
}

async fn glab(args: &[&str]) -> Result<String> {
    crate::run("glab", args, None).await
}

/// `glab api -X POST -f k=v ... <endpoint>`. Fields become `-f key=value` form args.
async fn glab_post(endpoint: &str, fields: &[(&str, &str)]) -> Result<String> {
    let mut args: Vec<String> = vec!["api".into(), "-X".into(), "POST".into()];
    for (k, v) in fields {
        args.push("-f".into());
        args.push(format!("{k}={v}"));
    }
    args.push(endpoint.into());
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    glab(&refs).await
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

    #[tokio::test]
    async fn paginate_stops_on_a_short_page_and_accumulates() {
        // Two full pages then a short one: three fetches, all items accumulated. The
        // fetch returns the page number as its "json"; the parse turns it into that
        // many items (PER_PAGE for pages 1-2, one for page 3).
        let out = paginate(
            |page| format!("p{page}"),
            |endpoint| async move { Ok(endpoint) },
            |page: &str| {
                let n = if page == "p3" { 1 } else { PER_PAGE };
                Ok((0..n).map(|i| format!("{page}-{i}")).collect())
            },
        )
        .await
        .unwrap();
        assert_eq!(out.len(), PER_PAGE * 2 + 1);
        assert_eq!(out[0], "p1-0");
        assert_eq!(out.last().unwrap(), "p3-0");
    }

    #[tokio::test]
    async fn paginate_stops_immediately_on_a_short_first_page() {
        let fetches = std::sync::atomic::AtomicUsize::new(0);
        let out: Vec<String> = paginate(
            |_page| "p".into(),
            |endpoint| {
                fetches.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                async move { Ok(endpoint) }
            },
            |_page| Ok(vec!["only".into()]),
        )
        .await
        .unwrap();
        assert_eq!(fetches.load(std::sync::atomic::Ordering::Relaxed), 1);
        assert_eq!(out, vec!["only".to_string()]);
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

    #[test]
    fn parses_a_created_project() {
        let r =
            parse_created_project(include_str!("../tests/fixtures/create_project.json")).unwrap();
        assert_eq!(r.full_path, "doyled-it/widget");
        assert!(r.private);
        assert_eq!(r.clone_url, "https://gitlab.com/doyled-it/widget.git");
        assert_eq!(r.description.as_deref(), Some("a thing"));
    }
}
