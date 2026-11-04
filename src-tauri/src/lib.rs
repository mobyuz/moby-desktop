use tauri::Manager;

/// Starts the app. Only one instance runs; a second launch focuses the first window.
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.unminimize();
                let _ = window.set_focus();
            }
        }))
        .run(tauri::generate_context!())
        .expect("failed to run Moby Desktop");
}
