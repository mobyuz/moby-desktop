use tauri::Manager;

mod agent;
mod api;
mod commands;
mod grants;
mod paths;
mod registry;
mod robot;
mod scheduler;
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
        // Used from Rust only (scheduler.rs). The capability set stays empty, so the webview cannot call it.
        .plugin(tauri_plugin_notification::init())
        .manage(api::Api::new())
        .manage(agent::Agent::new())
        .setup(|app| {
            // %LOCALAPPDATA%\uz.moby.desktop: history stays on this computer and does not roam.
            // If the file cannot be opened the app still starts, without saving.
            let dir = app.path().app_local_data_dir()?;
            if let Ok(old) = app.path().app_data_dir()
                && let Err(error) = store::move_legacy_file(&old, &dir)
            {
                eprintln!("moby: could not move the history file to the local folder: {error}");
            }
            app.manage(store::Store::open(&dir));
            app.manage(robot::Robot::start(app.handle().clone()));
            scheduler::start(app.handle().clone());
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
            agent::agent_turn,
            agent::answer_approval,
            agent::cancel_turn,
            commands::list_registry,
            commands::add_project,
            commands::add_app,
            commands::remove_project,
            commands::remove_app,
            commands::grant_read,
            commands::revoke_grant,
            commands::revoke_session_grant,
            commands::list_reminders,
            commands::delete_reminder,
            commands::list_activity,
            commands::clear_activity,
            robot::robot_subscribe,
            robot::robot_ports,
            robot::robot_connect,
            robot::robot_disconnect,
            robot::robot_begin_pairing,
            robot::robot_cancel_pairing,
            robot::robot_devices,
            robot::robot_unpair,
            robot::robot_dev,
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
