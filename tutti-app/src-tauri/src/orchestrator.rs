// SPDX-License-Identifier: AGPL-3.0-or-later
//! The orchestrator chat: a Tauri command that drives one `ClaudeSession` turn in the
//! active project's repo checkout, forwards parsed stream events onto `orchestrator://*`
//! events, and persists the transcript. Codegraph context is wired the same as the engine.

use crate::state::AppState;
use std::path::PathBuf;
use tauri::{Emitter, Manager};
use tokio::sync::mpsc;
use tutti_app_core::{transcript_key, MessageKind, OrchestratorTranscript, TranscriptMessage};
use tutti_backend_claude::session::ClaudeSession;
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

    let session = ClaudeSession::default();
    let result = session
        .turn(
            &message,
            &model,
            transcript.session_id.as_deref(),
            mcp_config_path.as_deref(),
            &repo_root,
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
