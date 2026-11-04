use tauri::Manager;

mod api;
mod paths;
mod registry;
mod store;
mod text;
mod tools;

/// Starts the app. Only one instance runs; a second launch focuses the first window.
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.unminimize();
                let _ = window.set_focus();
            }
        }))
        .manage(api::Api::new())
        .setup(|app| {
            // %APPDATA%\uz.moby.desktop. If the file cannot be opened the app still starts, without saving.
            let dir = app.path().app_data_dir()?;
            app.manage(store::Store::open(&dir));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            api::restore_session,
            api::sign_in,
            api::register,
            api::forgot_password,
            api::sign_out,
            api::get_usage,
            api::list_installations,
            api::revoke_installation,
            api::update_profile,
            api::delete_account,
            api::export_account,
            api::agent_turn,
            store::get_settings,
            store::set_language,
            store::set_retention,
            store::list_conversations,
            store::create_conversation,
            store::load_messages,
            store::append_message,
            store::delete_conversation,
            store::delete_all_history,
        ])
        .run(tauri::generate_context!())
        .expect("failed to run Moby Desktop");
}
