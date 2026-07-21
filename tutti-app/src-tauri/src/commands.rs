// SPDX-License-Identifier: AGPL-3.0-or-later
//! Tauri commands: manage the multi-project store (list, add, switch, remove), read
//! the board/issue detail, and drive a run.

use crate::driver;
use crate::state::{AppState, Project, RunState};
use std::path::PathBuf;
use tauri::Manager;
use tutti_app_core::{
    assemble_board, issue_detail, Board, IssueDetail, ProjectEntry, ProjectStore,
};
use tutti_core::config::{Config, ForgeKind};
use tutti_core::traits::{EngineError, Forge, Result as EngineResult};
use tutti_core::tracking::MilestoneId;
use tutti_forge_gitea::GiteaForge;
use tutti_forge_github::GitHubForge;
use tutti_forge_gitlab::GitLabForge;

/// Resolve `<app data dir>/projects.json`, creating the app data dir first if it does
/// not exist yet.
fn store_path(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir.join("projects.json"))
}

/// Load the project store from disk. A missing or unreadable file yields an empty
/// store (first run); `ProjectStore::from_json` already tolerates garbage content.
fn load_store(app: &tauri::AppHandle) -> Result<ProjectStore, String> {
    let path = store_path(app)?;
    match std::fs::read_to_string(&path) {
        Ok(s) => Ok(ProjectStore::from_json(&s)),
        Err(_) => Ok(ProjectStore::default()),
    }
}

/// Persist the project store to disk as pretty-printed JSON.
fn save_store(app: &tauri::AppHandle, store: &ProjectStore) -> Result<(), String> {
    std::fs::write(store_path(app)?, store.to_json()).map_err(|e| e.to_string())
}

/// Shell out to `git remote get-url origin` and parse the owner/repo path from it. Returns
/// None if there is no `origin` remote or the URL cannot be parsed.
fn detect_repo(root: &std::path::Path) -> Option<String> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["remote", "get-url", "origin"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let url = String::from_utf8_lossy(&out.stdout);
    tutti_app_core::repo_from_remote(&url)
}

/// Resolve a project's repo, config, and forge, and set it as the active project in
/// managed state. Shared by `add_project` and `switch_project`; this is the same work
/// the old single-project `load_project` command did.
async fn activate(
    dir: &str,
    repo: Option<String>,
    state: &tauri::State<'_, AppState>,
) -> Result<ProjectEntry, String> {
    let root = PathBuf::from(dir);
    let cfg = Config::load(&root.join("tutti.toml")).map_err(|e| e.to_string())?;
    let repo = repo
        .filter(|r| !r.trim().is_empty())
        .or_else(|| detect_repo(&root))
        .ok_or("could not determine the repo from the folder's git remote; enter owner/repo manually")?;
    let forge = build_forge(&cfg, &repo, root.clone()).map_err(|e| e.to_string())?;
    let name = root
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| repo.clone());
    let forge_kind = match cfg.forge.kind {
        ForgeKind::GitHub => "github",
        ForgeKind::Gitea => "gitea",
        ForgeKind::GitLab => "gitlab",
    }
    .to_string();
    *state.project.lock().await = Some(Project {
        config: cfg,
        forge,
        repo: repo.clone(),
        repo_root: root,
        name: name.clone(),
    });
    Ok(ProjectEntry {
        dir: dir.to_string(),
        repo,
        name,
        forge: forge_kind,
    })
}

/// The full project list plus which one is active, as returned to the frontend.
#[derive(serde::Serialize)]
pub struct ProjectList {
    pub projects: Vec<ProjectEntry>,
    pub active: Option<String>,
}

#[tauri::command]
pub async fn list_projects(app: tauri::AppHandle) -> Result<ProjectList, String> {
    let s = load_store(&app)?;
    Ok(ProjectList {
        projects: s.projects,
        active: s.active,
    })
}

#[tauri::command]
pub async fn add_project(
    app: tauri::AppHandle,
    dir: String,
    repo: Option<String>,
    state: tauri::State<'_, AppState>,
) -> Result<ProjectEntry, String> {
    let entry = activate(&dir, repo, &state).await?;
    let mut store = load_store(&app)?;
    store.upsert(entry.clone());
    save_store(&app, &store)?;
    Ok(entry)
}

#[tauri::command]
pub async fn switch_project(
    app: tauri::AppHandle,
    dir: String,
    state: tauri::State<'_, AppState>,
) -> Result<(), String> {
    if !matches!(state.run.lock().await.state, RunState::Idle) {
        return Err("pause the run before switching projects".into());
    }
    activate(&dir, None, &state).await?;
    let mut store = load_store(&app)?;
    store.set_active(&dir);
    save_store(&app, &store)?;
    Ok(())
}

#[tauri::command]
pub async fn remove_project(
    app: tauri::AppHandle,
    dir: String,
    state: tauri::State<'_, AppState>,
) -> Result<(), String> {
    let mut store = load_store(&app)?;
    let was_active = store.active.as_deref() == Some(dir.as_str());
    store.remove(&dir);
    save_store(&app, &store)?;
    if was_active {
        *state.project.lock().await = None;
    }
    Ok(())
}

#[tauri::command]
pub async fn get_board(
    milestone: Option<u64>,
    state: tauri::State<'_, AppState>,
) -> Result<Board, String> {
    let guard = state.project.lock().await;
    let p = guard.as_ref().ok_or("no project loaded")?;
    assemble_board(p.forge.as_ref(), &p.config, milestone.map(MilestoneId))
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn get_issue(
    id: u64,
    state: tauri::State<'_, AppState>,
) -> Result<IssueDetail, String> {
    // Increment 1: assemble detail from the current board read. A dedicated single-issue
    // Forge read is a later refinement; for now, find the issue across all milestones.
    let guard = state.project.lock().await;
    let p = guard.as_ref().ok_or("no project loaded")?;
    issue_detail(p.forge.as_ref(), &p.config, id)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn start_run(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<(), String> {
    driver::start(app, &state).await
}

#[tauri::command]
pub async fn pause_run(state: tauri::State<'_, AppState>) -> Result<(), String> {
    let mut run = state.run.lock().await;
    if let Some(c) = &run.cancel {
        c.store(true, std::sync::atomic::Ordering::Relaxed);
        run.state = RunState::Pausing;
    }
    Ok(())
}

/// Build the concrete forge adapter for `cfg.forge.kind`. A faithful copy of
/// `crates/tutti-cli/src/wire.rs`'s `build()` match, adapted to return just the forge
/// (the CLI's `LiveAdapters` also bundles a backend and workspace the app builds
/// separately inside the run driver) and to resolve the login from `cfg.forge.login`.
pub(crate) fn build_forge(
    cfg: &Config,
    repo: &str,
    repo_root: PathBuf,
) -> EngineResult<Box<dyn Forge>> {
    let status_labels = cfg.status_labels();
    let forge: Box<dyn Forge> = match cfg.forge.kind {
        ForgeKind::GitHub => Box::new(GitHubForge {
            repo: repo.to_string(),
            status_labels,
            repo_root: repo_root.clone(),
        }),
        ForgeKind::Gitea => {
            let login = cfg.forge.login.as_deref().ok_or_else(|| {
                EngineError::Forge(
                    "forge kind 'gitea' requires a login (set [forge].login)".into(),
                )
            })?;
            Box::new(GiteaForge {
                repo: repo.to_string(),
                login: login.to_string(),
                status_labels,
                repo_root: repo_root.clone(),
            })
        }
        ForgeKind::GitLab => Box::new(GitLabForge {
            project: repo.to_string(),
            status_labels,
            repo_root: repo_root.clone(),
        }),
    };
    Ok(forge)
}
