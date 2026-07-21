// SPDX-License-Identifier: AGPL-3.0-or-later
//! Live progress events emitted by the drain loop, and the hooks that carry them.

use serde::{Deserialize, Serialize};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

/// A lifecycle event emitted between issues during a drain. Serializable so the desktop
/// app can forward it straight to the webview.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EngineEvent {
    DrainStarted,
    IssueClaimed { id: u64, title: String },
    IssueShipped { id: u64 },
    IssueReleased { id: u64 },
    DrainComplete { shipped: u32 },
}

/// Optional collaborators the drain loop consults. Both default to inert, so existing
/// callers (CLI, tests) are unchanged.
#[derive(Clone, Default)]
pub struct EngineHooks {
    /// Where lifecycle events go. `None` = no emission.
    pub sink: Option<tokio::sync::mpsc::UnboundedSender<EngineEvent>>,
    /// When set true, the drain stops after the issue in flight finishes.
    pub cancel: Option<Arc<AtomicBool>>,
}

impl EngineHooks {
    pub(crate) fn emit(&self, ev: EngineEvent) {
        if let Some(s) = &self.sink {
            // A closed receiver just means the UI went away; ignore.
            let _ = s.send(ev);
        }
    }

    pub(crate) fn cancelled(&self) -> bool {
        self.cancel
            .as_ref()
            .map(|c| c.load(std::sync::atomic::Ordering::Relaxed))
            .unwrap_or(false)
    }
}
