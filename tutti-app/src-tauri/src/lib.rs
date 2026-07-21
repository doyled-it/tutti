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
            commands::load_project,
            commands::get_board,
            commands::get_issue,
            commands::start_run,
            commands::pause_run,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
