// SPDX-License-Identifier: AGPL-3.0-or-later
//! The continuous run driver: spawns a task that builds the engine adapters and drains
//! the project repeatedly until no work is ready or the run is cancelled, forwarding
//! `EngineEvent`s to the webview as it goes.

use crate::commands::build_forge;
use crate::state::{AppState, RunState};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tauri::{Emitter, Manager};
use tutti_backend_claude::ClaudeBackend;
use tutti_core::config::Config;
use tutti_core::engine::Engine;
use tutti_core::events::{EngineEvent, EngineHooks};
use tutti_git::GitWorkspace;

/// Start a continuous run. Fails if a run is already active.
///
/// `tauri::State` borrows the `AppHandle` it came from, and that borrow cannot cross a
/// `tokio::spawn` boundary. So this pulls only OWNED data out of the shared `Project`
/// (the config, repo, and repo root) while briefly holding the lock, then rebuilds the
/// forge adapter inside the spawned task via `build_forge`. The task looks up
/// `AppState` again through the cloned, `'static` `AppHandle`; nothing borrowed from
/// this call crosses into the task.
pub async fn start(app: tauri::AppHandle, state: &tauri::State<'_, AppState>) -> Result<(), String> {
    let (config, repo, repo_root): (Config, String, std::path::PathBuf) = {
        let guard = state.project.lock().await;
        let p = guard.as_ref().ok_or("no project loaded")?;
        (p.config.clone(), p.repo.clone(), p.repo_root.clone())
    };

    let cancel = Arc::new(AtomicBool::new(false));
    {
        let mut run = state.run.lock().await;
        if matches!(run.state, RunState::Running | RunState::Pausing) {
            return Err("a run is already active".into());
        }
        run.state = RunState::Running;
        run.cancel = Some(cancel.clone());
    }

    // Event channel: engine -> forwarder task -> webview.
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<EngineEvent>();
    let app_ev = app.clone();
    tokio::spawn(async move {
        while let Some(ev) = rx.recv().await {
            let _ = app_ev.emit("engine://progress", &ev);
        }
    });

    let run_cancel = cancel.clone();
    let app_run = app.clone();
    tokio::spawn(async move {
        run_loop(config, repo, repo_root, run_cancel, tx).await;

        let st = app_run.state::<AppState>();
        {
            let mut run = st.run.lock().await;
            run.state = RunState::Idle;
            run.cancel = None;
        }
        // A guaranteed terminal signal for the UI, emitted on every exit path (including
        // an engine error, where `drain_with` returns before its own DrainComplete). The
        // frontend drives run-state off this, not off the per-pass DrainComplete, so a
        // failed run never leaves the UI stuck in "running".
        let _ = app_run.emit("engine://run-ended", ());
    });

    Ok(())
}

/// Build the adapters and drain repeatedly until no work is ready or `cancel` fires.
/// `drain_with` emits `DrainStarted`/`DrainComplete` per pass (used by the UI to reconcile
/// the board); the run's own start/end is signalled separately by the caller.
async fn run_loop(
    config: Config,
    repo: String,
    repo_root: std::path::PathBuf,
    cancel: Arc<AtomicBool>,
    tx: tokio::sync::mpsc::UnboundedSender<EngineEvent>,
) {
    let forge = match build_forge(&config, &repo, repo_root.clone()) {
        Ok(f) => f,
        Err(_) => return,
    };
    let backend = ClaudeBackend::default();
    let workspace = GitWorkspace::new(repo_root);
    let engine = match Engine::new(&config, forge.as_ref(), &backend, Box::new(workspace)) {
        Ok(e) => e,
        Err(_) => return,
    };

    let hooks = EngineHooks {
        sink: Some(tx),
        cancel: Some(cancel.clone()),
    };

    loop {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        match engine.drain_with(&hooks).await {
            Ok((shipped, _)) if shipped > 0 => continue, // more may be ready
            _ => break,                                   // 0 shipped or error
        }
    }
}
