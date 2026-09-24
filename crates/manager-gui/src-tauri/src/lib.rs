mod commands;
mod dto;
mod jobs;

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
            commands::start_delete,
            commands::job_action,
            commands::answer_error,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
