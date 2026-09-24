mod commands;
mod compare;
mod dto;
mod duplicates;
mod jobs;
mod search;

use commands::AppState;
use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            app.manage(AppState::new(app.handle().clone()));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::list_dir,
            commands::home_dir,
            commands::parent_of,
            commands::start_delete,
            commands::start_transfer,
            commands::job_action,
            commands::answer_error,
            commands::answer_conflict,
            commands::start_search,
            commands::cancel_search,
            commands::start_compare,
            commands::cancel_compare,
            commands::sync_compare,
            commands::start_duplicates,
            commands::cancel_duplicates,
            commands::connect,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
