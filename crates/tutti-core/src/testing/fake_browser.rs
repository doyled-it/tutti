// SPDX-License-Identifier: AGPL-3.0-or-later
//! In-memory `ForgeBrowser` for hermetic app-core tests.

use crate::browse::{ForgeBrowser, Namespace, RemoteRepo};
use crate::traits::Result;
use async_trait::async_trait;
use std::collections::HashMap;

/// Returns canned namespaces, and repos keyed by `Namespace::path`.
pub struct FakeBrowser {
    pub namespaces: Vec<Namespace>,
    pub repos: HashMap<String, Vec<RemoteRepo>>,
}

#[async_trait]
impl ForgeBrowser for FakeBrowser {
    async fn list_namespaces(&self) -> Result<Vec<Namespace>> {
        Ok(self.namespaces.clone())
    }
    async fn list_repos(&self, ns: &Namespace) -> Result<Vec<RemoteRepo>> {
        Ok(self.repos.get(&ns.path).cloned().unwrap_or_default())
    }
}
