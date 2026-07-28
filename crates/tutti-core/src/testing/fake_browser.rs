// SPDX-License-Identifier: AGPL-3.0-or-later
//! In-memory `ForgeBrowser` for hermetic app-core tests.

use crate::browse::{ForgeBrowser, Namespace, NewRepo, RemoteRepo};
use crate::traits::Result;
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Mutex;

/// Returns canned namespaces, and repos keyed by `Namespace::path`. Records `create_repo`
/// calls in `created` and returns a synthesized `RemoteRepo` for the created name.
pub struct FakeBrowser {
    pub namespaces: Vec<Namespace>,
    pub repos: HashMap<String, Vec<RemoteRepo>>,
    pub created: Mutex<Vec<(Namespace, NewRepo)>>,
}

#[async_trait]
impl ForgeBrowser for FakeBrowser {
    async fn list_namespaces(&self) -> Result<Vec<Namespace>> {
        Ok(self.namespaces.clone())
    }
    async fn list_repos(&self, ns: &Namespace) -> Result<Vec<RemoteRepo>> {
        Ok(self.repos.get(&ns.path).cloned().unwrap_or_default())
    }
    async fn create_repo(&self, ns: &Namespace, spec: &NewRepo) -> Result<RemoteRepo> {
        self.created
            .lock()
            .unwrap()
            .push((ns.clone(), spec.clone()));
        Ok(RemoteRepo {
            full_path: format!("{}/{}", ns.path, spec.name),
            name: spec.name.clone(),
            description: spec.description.clone(),
            clone_url: format!("https://example.test/{}/{}.git", ns.path, spec.name),
            private: spec.private,
            archived: false,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::browse::NamespaceKind;

    #[tokio::test]
    async fn create_repo_records_and_synthesizes() {
        let b = FakeBrowser {
            namespaces: vec![],
            repos: HashMap::new(),
            created: Mutex::new(vec![]),
        };
        let ns = Namespace {
            path: "doyled-it".into(),
            name: "doyled-it".into(),
            kind: NamespaceKind::User,
        };
        let spec = NewRepo {
            name: "widget".into(),
            description: Some("a thing".into()),
            private: true,
        };
        let repo = b.create_repo(&ns, &spec).await.unwrap();
        assert_eq!(repo.full_path, "doyled-it/widget");
        assert!(repo.private);
        assert!(repo.clone_url.ends_with("doyled-it/widget.git"));
        assert_eq!(b.created.lock().unwrap().len(), 1);
    }
}
