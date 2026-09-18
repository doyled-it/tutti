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
use tutti_core::context::CodeGraph;
use tutti_core::engine::Engine;
use tutti_core::events::{EngineEvent, EngineHooks, SubsessionEvent};
use tutti_git::GitWorkspace;

/// Start a continuous run. Fails if a run is already active.
///
/// `tauri::State` borrows the `AppHandle` it came from, and that borrow cannot cross a
/// `tokio::spawn` boundary. So this pulls only OWNED data out of the shared `Project`
/// (the config, repo, and repo root) while briefly holding the lock, then rebuilds the
/// forge adapter inside the spawned task via `build_forge`. The task looks up
/// `AppState` again through the cloned, `'static` `AppHandle`; nothing borrowed from
/// this call crosses into the task.
pub async fn start(
    app: tauri::AppHandle,
    state: &tauri::State<'_, AppState>,
) -> Result<(), String> {
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

    // Subsession channel: engine per-role streams -> forwarder task -> webview.
    let (sub_tx, mut sub_rx) = tokio::sync::mpsc::unbounded_channel::<SubsessionEvent>();
    let app_sub = app.clone();
    tokio::spawn(async move {
        while let Some(ev) = sub_rx.recv().await {
            let _ = app_sub.emit("subsession://event", &ev);
        }
    });

    let run_cancel = cancel.clone();
    let app_run = app.clone();
    tokio::spawn(async move {
        let outcome = run_loop(config, repo, repo_root, run_cancel, tx, sub_tx).await;

        let st = app_run.state::<AppState>();
        {
            let mut run = st.run.lock().await;
            run.state = RunState::Idle;
            run.cancel = None;
        }
        // Surface a run that ended on an error rather than swallowing it into a silent idle.
        // The reason goes to the backend log (visible in the dev console / terminal) and to a
        // dedicated UI event, so the user sees WHY a run stopped instead of "idle, 0 shipped".
        let error = match outcome {
            Ok(()) => None,
            Err(reason) => {
                eprintln!("[tutti] run ended on error: {reason}");
                let _ = app_run.emit("engine://run-error", reason.clone());
                Some(reason)
            }
        };
        // A guaranteed terminal signal for the UI, emitted on every exit path (including
        // an engine error, where `drain_with` returns before its own DrainComplete). The
        // frontend drives run-state off this, not off the per-pass DrainComplete, so a
        // failed run never leaves the UI stuck in "running". The optional reason lets a
        // listener that missed `run-error` still show why it stopped.
        let _ = app_run.emit("engine://run-ended", error);
    });

    Ok(())
}

/// Build the adapters and drain repeatedly until no work is ready or `cancel` fires.
/// `drain_with` emits `DrainStarted`/`DrainComplete` per pass (used by the UI to reconcile
/// the board); the run's own start/end is signalled separately by the caller.
/// Run the drain loop, returning the error that ended it (if any) rather than swallowing it, so
/// the caller can surface why a run stopped. `Ok(())` means a clean end (no ready work or a
/// cancel); `Err` carries a human-readable reason.
async fn run_loop(
    config: Config,
    repo: String,
    repo_root: std::path::PathBuf,
    cancel: Arc<AtomicBool>,
    tx: tokio::sync::mpsc::UnboundedSender<EngineEvent>,
    sub_tx: tokio::sync::mpsc::UnboundedSender<SubsessionEvent>,
) -> Result<(), String> {
    let forge = build_forge(&config, &repo, repo_root.clone())
        .map_err(|e| format!("could not build the forge adapter: {e}"))?;
    let backend = ClaudeBackend::default();
    let workspace = GitWorkspace::new(repo_root.clone());
    let engine = Engine::new(&config, forge.as_ref(), &backend, Box::new(workspace))
        .map_err(|e| format!("could not build the engine: {e}"))?;
    // codegraph context, gated by config and binary presence. `detect` returns None when
    // the binary is absent, so this is a full no-op on machines without codegraph.
    let codegraph = if config.codegraph_enabled() {
        CodeGraph::detect(repo_root.clone())
    } else {
        None
    };
    let engine = match &codegraph {
        Some(cg) => engine.with_context(cg),
        None => engine,
    };

    let hooks = EngineHooks {
        sink: Some(tx),
        cancel: Some(cancel.clone()),
        subsession: Some(sub_tx),
    };

    // Reclaim any issue left `status:in-progress` by a prior run that was killed or crashed mid
    // issue (the selector only sees `status:ready`, so an orphaned in-progress issue would never
    // be picked up again). Best effort: a failure here must not block the run.
    let _ = engine.reclaim_orphaned_in_progress().await;

    loop {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        match engine.drain_with(&hooks).await {
            Ok((shipped, _)) if shipped > 0 => continue, // more may be ready
            Ok(_) => break,                              // 0 shipped: no ready work or a clean stop
            // A real engine error (a git worktree collision, a forge/API failure, ...) previously
            // vanished into a silent idle. Return it so the caller logs it and tells the user.
            Err(e) => return Err(format!("the engine stopped on an error: {e}")),
        }
    }
    Ok(())
}
