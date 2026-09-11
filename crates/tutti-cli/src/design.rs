// SPDX-License-Identifier: AGPL-3.0-or-later
//! `tutti design`: an interactive CLI that drives the design chain (the E2/E5 facilitation
//! loop) to a ratified design, grounds an existing repo first, then turns the ratified design
//! into a seedable backlog via a post-chain decompose pass and hands off (new repo: scaffold +
//! seed; existing repo: seed). Resumable and branchable from `.tutti/design/`.
//!
//! The wiring lives here. The real `Facilitator` is a thin adapter over `ClaudeSession::turn`
//! (`ClaudeFacilitator`); the interactive driver is written over an injected `Prompter` and the
//! `Facilitator` seam so it is hermetically testable with a fake of each. The full chain against
//! a real `claude` is a live `#[ignore]` smoke.

use std::path::PathBuf;
use tutti_backend_claude::session::ClaudeSession;
use tutti_core::message::AgentEvent;
use tutti_design::{DesignError, Facilitator, RawTurn};

/// The real `Facilitator`: one facilitation turn is one `ClaudeSession::turn` in `cwd`,
/// resuming the session id from the prior turn. The streamed agent events are drained to a
/// sink for now (a later surface can show them live); the turn's outcome is its full reply
/// text plus the resumable session id, which is exactly what a `RawTurn` carries.
pub struct ClaudeFacilitator {
    pub session: ClaudeSession,
    pub model: String,
    pub cwd: PathBuf,
}

impl Facilitator for ClaudeFacilitator {
    async fn turn(&self, prompt: &str, resume: Option<&str>) -> Result<RawTurn, DesignError> {
        let (tx, mut rx) = tokio::sync::mpsc::channel::<AgentEvent>(64);
        // Drain the streamed events so the bounded channel never backs up the turn. A live
        // surface can replace this sink with a real renderer later.
        let drain = tokio::spawn(async move { while rx.recv().await.is_some() {} });
        let outcome = self
            .session
            .turn(prompt, &self.model, resume, None, &self.cwd, None, tx)
            .await
            .map_err(|e| DesignError::Facilitation(format!("claude turn: {e}")))?;
        let _ = drain.await;
        Ok(RawTurn {
            session_id: outcome.session_id.unwrap_or_default(),
            output: outcome.assistant_text,
        })
    }
}
