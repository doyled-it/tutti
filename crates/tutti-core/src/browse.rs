// SPDX-License-Identifier: AGPL-3.0-or-later
//! Browsing a forge before any repo is chosen. This is deliberately not part of the
//! `Forge` trait: every `Forge` is constructed with a repo, and browsing happens to find
//! one. It depends only on the CLI and, for Gitea, the login.

use crate::traits::Result;
use async_trait::async_trait;

/// What kind of thing owns repos on a forge.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum NamespaceKind {
    /// The authenticated user's own account.
    User,
    /// A GitHub or Gitea organization.
    Org,
    /// A GitLab group.
    Group,
}

/// One place repos can live: an account, an org, or a group.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Namespace {
    /// The key the repo listing needs. A login or org name on GitHub/Gitea; a numeric
    /// id (as a string) on GitLab, where the projects call is keyed by id, not path.
    pub path: String,
    /// Human-readable label for the picker. Falls back to `path` when there is nothing
    /// better (on GitLab this holds the `full_path`).
    pub name: String,
    pub kind: NamespaceKind,
}

/// A repo the user could clone and adopt.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RemoteRepo {
    /// `owner/repo`, or `group/subgroup/project` on GitLab. This is exactly the string
    /// the resulting `tutti.toml` records, so it must match what the `Forge` adapter for
    /// this kind expects as its repo.
    pub full_path: String,
    pub name: String,
    pub description: Option<String>,
    pub clone_url: String,
    /// Not public. On GitHub/Gitea this is the `private` bool; on GitLab it is
    /// `visibility != "public"`, so an `internal` project is correctly not-public.
    pub private: bool,
    /// Archived on GitHub. Never true where the forge does not report it. Surfaced as a
    /// muted marker, not filtered out.
    pub archived: bool,
}

/// A repo to create. Always auto-initialized with a README (unconditional, not a field):
/// an empty repo clones to an unborn branch with no forge-set default, which the wizard's
/// first commit and label seeding assume against.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct NewRepo {
    pub name: String,
    pub description: Option<String>,
    pub private: bool,
}

/// Browse a forge's namespaces and their repos. Repo-independent by design.
#[async_trait]
pub trait ForgeBrowser: Send + Sync {
    /// The namespaces the authenticated user can see: their own account first, then
    /// their orgs or groups.
    async fn list_namespaces(&self) -> Result<Vec<Namespace>>;
    /// The repos in one namespace.
    async fn list_repos(&self, ns: &Namespace) -> Result<Vec<RemoteRepo>>;
    /// Create `spec` under `ns`, returning it in the same shape `list_repos` yields so its
    /// `clone_url` feeds the existing clone path unchanged.
    async fn create_repo(&self, ns: &Namespace, spec: &NewRepo) -> Result<RemoteRepo>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_repo_round_trips_through_clone() {
        let r = RemoteRepo {
            full_path: "o/r".into(),
            name: "r".into(),
            description: None,
            clone_url: "https://x/o/r.git".into(),
            private: true,
            archived: false,
        };
        assert_eq!(r.clone().full_path, "o/r");
    }
}
