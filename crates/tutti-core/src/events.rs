// SPDX-License-Identifier: AGPL-3.0-or-later
//! Live progress events emitted by the drain loop, and the hooks that carry them.

use serde::{Deserialize, Serialize};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use crate::message::Role;

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

/// A per-role turn stream from the drain loop, keyed by issue id + role. Serializable so
/// the desktop app can forward it straight to the webview. Live-only (never persisted).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SubsessionEvent {
    /// A role turn began. `title` is the issue title (carried so the pane needs no lookup;
    /// the Planner uses the synthetic planner issue's title).
    Started { issue: u64, role: Role, title: String },
    /// A chunk of assistant text.
    Delta { issue: u64, role: Role, text: String },
    /// A tool-use aside (the tool name).
    Tool { issue: u64, role: Role, name: String },
    /// The turn finished. `summary` is a role-aware one-liner; `ok` drives the status dot.
    Completed {
        issue: u64,
        role: Role,
        summary: String,
        ok: bool,
    },
}

/// Optional collaborators the drain loop consults. Both default to inert, so existing
/// callers (CLI, tests) are unchanged.
#[derive(Clone, Default)]
pub struct EngineHooks {
    /// Where lifecycle events go. `None` = no emission.
    pub sink: Option<tokio::sync::mpsc::UnboundedSender<EngineEvent>>,
    /// When set true, the drain stops after the issue in flight finishes.
    pub cancel: Option<Arc<AtomicBool>>,
    /// Where per-role turn streams go. `None` = no emission (CLI, tests): unchanged behavior.
    pub subsession: Option<tokio::sync::mpsc::UnboundedSender<SubsessionEvent>>,
}

impl EngineHooks {
    pub(crate) fn emit(&self, ev: EngineEvent) {
        if let Some(s) = &self.sink {
            // A closed receiver just means the UI went away; ignore.
            let _ = s.send(ev);
        }
    }

    pub(crate) fn emit_subsession(&self, ev: SubsessionEvent) {
        if let Some(s) = &self.subsession {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::Role;

    #[test]
    fn subsession_event_serializes_with_tagged_snake_case() {
        let ev = SubsessionEvent::Completed {
            issue: 42,
            role: Role::FixApplier,
            summary: "ready to ship".into(),
            ok: true,
        };
        let json = serde_json::to_string(&ev).unwrap();
        assert!(json.contains(r#""kind":"completed""#), "got {json}");
        assert!(json.contains(r#""role":"fix_applier""#), "got {json}");
        assert!(json.contains(r#""issue":42"#), "got {json}");
        assert!(json.contains(r#""ok":true"#), "got {json}");
    }

    #[test]
    fn default_hooks_have_no_subsession_sink() {
        let hooks = EngineHooks::default();
        assert!(hooks.subsession.is_none());
    }
}
