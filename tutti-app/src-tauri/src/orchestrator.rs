// SPDX-License-Identifier: AGPL-3.0-or-later
//! The orchestrator chat: a Tauri command that drives one `ClaudeSession` turn in the
//! active project's repo checkout, forwards parsed stream events onto `orchestrator://*`
//! events, and persists the transcript. Codegraph context is wired the same as the engine.

use crate::state::AppState;
use std::path::PathBuf;
use tauri::{Emitter, Manager};
use tokio::sync::mpsc;
use tutti_app_core::{
    gate_is_noop, set_gate_commands, transcript_key, GateStatus, MessageKind,
    OrchestratorTranscript, TranscriptMessage,
};
use tutti_backend_claude::session::ClaudeSession;
use tutti_core::config::Config;
use tutti_core::context::{CodeGraph, ContextProvider};
use tutti_core::message::AgentEvent;

/// Resolve `<app data dir>/orchestrator/<key>.json`, creating the dir if needed.
fn transcript_path(app: &tauri::AppHandle, dir: &str) -> Result<PathBuf, String> {
    let base = app.path().app_data_dir().map_err(|e| e.to_string())?;
    let od = base.join("orchestrator");
    std::fs::create_dir_all(&od).map_err(|e| e.to_string())?;
    Ok(od.join(format!("{}.json", transcript_key(dir))))
}

fn load_transcript(app: &tauri::AppHandle, dir: &str) -> Result<OrchestratorTranscript, String> {
    let path = transcript_path(app, dir)?;
    match std::fs::read_to_string(&path) {
        Ok(s) => Ok(OrchestratorTranscript::from_json(&s)),
        Err(_) => Ok(OrchestratorTranscript::default()),
    }
}

fn save_transcript(
    app: &tauri::AppHandle,
    dir: &str,
    t: &OrchestratorTranscript,
) -> Result<(), String> {
    let path = transcript_path(app, dir)?;
    std::fs::write(&path, t.to_json()).map_err(|e| e.to_string())
}

/// The active project's `dir` (repo root as string), or an error if none is loaded.
async fn active_dir(state: &tauri::State<'_, AppState>) -> Result<String, String> {
    let guard = state.project.lock().await;
    let p = guard.as_ref().ok_or("no project loaded")?;
    Ok(p.repo_root.to_string_lossy().into_owned())
}

/// Return the persisted transcript for the active project (empty if none yet).
#[tauri::command]
pub async fn get_transcript(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<OrchestratorTranscript, String> {
    let dir = active_dir(&state).await?;
    load_transcript(&app, &dir)
}

/// Drive one chat turn: append+persist the user message, run the session with codegraph
/// context, stream deltas/tool notices, then persist the assistant reply and emit `done`.
#[tauri::command]
pub async fn send_orchestrator_message(
    message: String,
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<(), String> {
    use std::sync::atomic::Ordering;
    // Single-flight: refuse a second concurrent turn so two turns cannot interleave the
    // transcript read-modify-write. The guard clears the flag on every exit path (including
    // an early `?`), so a failed turn does not wedge the flag set.
    if state.orchestrator_busy.swap(true, Ordering::SeqCst) {
        return Err("a chat turn is already in progress".into());
    }
    struct BusyGuard<'a>(&'a std::sync::atomic::AtomicBool);
    impl Drop for BusyGuard<'_> {
        fn drop(&mut self) {
            self.0.store(false, Ordering::SeqCst);
        }
    }
    let _busy = BusyGuard(&state.orchestrator_busy);

    // Pull owned data out under the lock; nothing borrowed crosses the await points.
    let (dir, repo_root, model, codegraph_enabled) = {
        let guard = state.project.lock().await;
        let p = guard.as_ref().ok_or("no project loaded")?;
        (
            p.repo_root.to_string_lossy().into_owned(),
            p.repo_root.clone(),
            p.config.model.clone(),
            p.config.codegraph_enabled(),
        )
    };

    // Persist the user message before running, so a mid-turn crash does not lose it.
    let mut transcript = load_transcript(&app, &dir)?;
    transcript.push(TranscriptMessage {
        role: "user".into(),
        text: message.clone(),
        kind: MessageKind::Text,
    });
    save_transcript(&app, &dir, &transcript)?;

    // codegraph context, gated by config + binary presence, exactly like the engine path.
    let codegraph = if codegraph_enabled {
        CodeGraph::detect(repo_root.clone())
    } else {
        None
    };
    let mcp_config_path = match &codegraph {
        Some(cg) => {
            cg.ensure_ready().await;
            write_mcp_config(&cg.mcp_servers())
        }
        None => None,
    };

    // Forward parsed events onto orchestrator://delta and orchestrator://tool.
    let (tx, mut rx) = mpsc::channel::<AgentEvent>(64);
    let app_ev = app.clone();
    let forward = tokio::spawn(async move {
        while let Some(ev) = rx.recv().await {
            match ev {
                AgentEvent::Line(text) => {
                    let _ = app_ev.emit("orchestrator://delta", text);
                }
                AgentEvent::ToolUse(name) => {
                    let _ = app_ev.emit("orchestrator://tool", name);
                }
                AgentEvent::Done => {}
            }
        }
    });

    let gate_path = proposal_path(&app)?;
    let session = ClaudeSession::default();
    let result = session
        .turn(
            &message,
            &model,
            transcript.session_id.as_deref(),
            mcp_config_path.as_deref(),
            &repo_root,
            Some(&gate_path),
            tx,
        )
        .await;
    let _ = forward.await;

    match result {
        Ok(outcome) => {
            transcript.session_id = outcome.session_id.clone().or(transcript.session_id);
            // Skip persisting an empty assistant bubble (e.g. a tool-only turn); still save
            // so the updated session_id lands, and always emit done so the UI clears its
            // thinking state.
            if !outcome.assistant_text.is_empty() {
                transcript.push(TranscriptMessage {
                    role: "assistant".into(),
                    text: outcome.assistant_text.clone(),
                    kind: MessageKind::Text,
                });
            }
            save_transcript(&app, &dir, &transcript)?;
            if let Some(proposal) = &outcome.proposal {
                let _ = app.emit("orchestrator://proposal", proposal);
            }
            let _ = app.emit("orchestrator://done", &outcome.assistant_text);
            Ok(())
        }
        Err(e) => {
            let _ = app.emit("orchestrator://error", e.to_string());
            Err(e.to_string())
        }
    }
}

/// Write the MCP servers to a per-process temp file and return its path, or None. Mirrors
/// the backend's own `--mcp-config` handling (never in the worktree). Best-effort.
fn write_mcp_config(servers: &[tutti_core::mcp::McpServer]) -> Option<String> {
    if servers.is_empty() {
        return None;
    }
    let dir = std::env::temp_dir().join(format!("tutti-mcp-{}", std::process::id()));
    std::fs::create_dir_all(&dir).ok()?;
    tutti_core::mcp::write_mcp_config(servers, &dir)
        .ok()
        .map(|p| p.to_string_lossy().into_owned())
}

/// The absolute path (outside any repo) the agent is told to write a proposal to.
///
/// Deliberately NOT `std::env::temp_dir()`. On Linux that is `/tmp`: world-writable, and a
/// process id is readable from `/proc`, so any local user could pre-create the file we are
/// about to read. The sticky bit would then stop us unlinking it, and an attacker-authored
/// gate proposal ends up one click from `[gate].commands`, which the engine runs through
/// `sh -c`. The app data dir is per-user and not world-writable, which removes the race
/// entirely; on unix we also clamp the directory to 0700 in case the parent is permissive.
///
/// Per-process filename; turns are single-flight so reuse across turns is safe (each turn
/// clears it).
fn proposal_path(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| e.to_string())?
        .join("proposals");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // Best-effort: a failure here is not fatal, since the app data dir is already
        // per-user; this only narrows an unusually permissive parent.
        let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
    }
    Ok(dir.join(format!("proposal-{}.json", std::process::id())))
}

/// Read the active project's current gate status.
#[tauri::command]
pub async fn get_gate_status(state: tauri::State<'_, AppState>) -> Result<GateStatus, String> {
    let guard = state.project.lock().await;
    let p = guard.as_ref().ok_or("no project loaded")?;
    let commands = p.config.gate.commands.clone();
    Ok(GateStatus {
        is_noop: gate_is_noop(&commands),
        commands,
    })
}

/// Write `commands` into the active project's `tutti.toml` (preserving every other key),
/// reload `Config` into managed state, and return the new gate status. Run-guarded: it
/// changes what a drain verifies, so it is refused while an engine run is active.
#[tauri::command]
pub async fn apply_gate(
    commands: Vec<String>,
    state: tauri::State<'_, AppState>,
) -> Result<GateStatus, String> {
    if !matches!(state.run.lock().await.state, crate::state::RunState::Idle) {
        return Err("finish the current run before changing the gate".into());
    }
    // Drop blank/whitespace-only entries, then reject an empty gate: an empty command list
    // passes vacuously (Gate::run verifies nothing) and, unlike the explicit `["true"]`
    // no-op, would slip past the intent of this command. A caller wanting no verification
    // sets the explicit `["true"]` no-op. Filtering also keeps a stray blank the agent
    // proposed alongside real commands from becoming a no-op `sh -c ""` line in the gate.
    let commands: Vec<String> = commands
        .into_iter()
        .filter(|c| !c.trim().is_empty())
        .collect();
    if commands.is_empty() {
        return Err(
            "a gate needs at least one command (use \"true\" for an explicit no-op)".into(),
        );
    }
    let mut guard = state.project.lock().await;
    let p = guard.as_mut().ok_or("no project loaded")?;
    let toml_path = p.repo_root.join("tutti.toml");
    let existing = std::fs::read_to_string(&toml_path).map_err(|e| e.to_string())?;
    let updated = set_gate_commands(&existing, &commands).map_err(|e| e.to_string())?;
    std::fs::write(&toml_path, &updated).map_err(|e| e.to_string())?;
    // Reload so the in-memory config (and everything reading it) reflects the new gate.
    let cfg = Config::load(&toml_path).map_err(|e| e.to_string())?;
    let commands = cfg.gate.commands.clone();
    p.config = cfg;
    Ok(GateStatus {
        is_noop: gate_is_noop(&commands),
        commands,
    })
}
