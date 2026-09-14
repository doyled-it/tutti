// SPDX-License-Identifier: AGPL-3.0-or-later
mod commands;
mod design;
mod driver;
mod orchestrator;
mod state;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .manage(state::AppState::default())
        .invoke_handler(tauri::generate_handler![
            commands::list_projects,
            commands::add_project,
            commands::switch_project,
            commands::remove_project,
            commands::get_board,
            commands::get_issue,
            commands::apply_triage,
            commands::preview_triage,
            commands::start_run,
            commands::pause_run,
            commands::probe_project,
            commands::init_project,
            commands::preview_tutti_toml,
            commands::list_namespaces,
            commands::list_repos,
            commands::clone_repo,
            commands::create_repo,
            orchestrator::send_orchestrator_message,
            orchestrator::get_transcript,
            orchestrator::apply_gate,
            orchestrator::get_gate_status,
            design::design_start,
            design::design_session_status,
            design::design_begin_movement,
            design::design_reply,
            design::design_revise,
            design::design_ratify,
            design::design_preview,
            design::design_propose_backlog,
            design::design_scaffold,
            design::design_seed_backlog,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
