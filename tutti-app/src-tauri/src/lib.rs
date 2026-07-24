// SPDX-License-Identifier: AGPL-3.0-or-later
mod commands;
mod driver;
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
            commands::start_run,
            commands::pause_run,
            commands::probe_project,
            commands::init_project,
            commands::preview_tutti_toml,
            commands::list_namespaces,
            commands::list_repos,
            commands::clone_repo,
            commands::create_repo,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
