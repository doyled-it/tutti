// SPDX-License-Identifier: AGPL-3.0-or-later
//! Managed application state: the loaded project (config + forge) and the run status.

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use tokio::sync::Mutex;
use tutti_core::config::Config;
use tutti_core::traits::Forge;

/// A loaded project: the parsed config, the concrete forge adapter, and the repo
/// location. Held across commands so `get_board`/`get_issue`/`start_run` can reuse it.
pub struct Project {
    pub config: Config,
    pub forge: Box<dyn Forge>,
    pub repo: String,
    pub repo_root: PathBuf,
    // Not read back from state yet: `load_project` returns it directly in the
    // `ProjectSummary` response. Kept on `Project` for a later command (e.g. a
    // "current project" query) that reads it out of managed state instead.
    #[allow(dead_code)]
    pub name: String,
}

/// The state of the continuous run driver.
#[derive(Default, Clone, serde::Serialize)]
#[serde(rename_all = "snake_case", tag = "state")]
pub enum RunState {
    #[default]
    Idle,
    Running,
    Pausing,
}

/// The run's current state and its cancel flag (set once a run starts).
#[derive(Default)]
pub struct RunInfo {
    pub state: RunState,
    pub cancel: Option<Arc<AtomicBool>>,
}

/// Top-level managed state for the Tauri app.
#[derive(Default)]
pub struct AppState {
    pub project: Mutex<Option<Project>>,
    pub run: Mutex<RunInfo>,
}
