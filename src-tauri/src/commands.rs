//! Commands for the "Tools and projects" and "Activity" screens. Every one is scoped to the
//! signed-in account taken from the API client. The webview never sends a path: folders and
//! programs are picked in a native dialog that Rust opens, and the result is checked before it is
//! stored.

use std::path::PathBuf;

use serde::Serialize;
use tauri::{AppHandle, Manager, State};

use crate::agent::Agent;
use crate::api::{Api, Failure};
use crate::grants::SessionGrant;
use crate::paths;
use crate::registry::{self, App, AuditEntry, Grant, Project, Reminder};
use crate::store::{Store, now_ms};
use crate::text;

#[derive(Serialize)]
pub struct Overview {
    projects: Vec<Project>,
    apps: Vec<App>,
    grants: Vec<Grant>,
    session_grants: Vec<SessionGrant>,
}

fn path_failure(error: paths::PathError) -> Failure {
    Failure::new("invalid_path", &format!("That can't be added: {}.", paths::explain(error)))
}

/// Opens a native dialog and waits for the person. Returns nothing if they cancel.
async fn pick(app: &AppHandle, folder: bool) -> Result<Option<PathBuf>, Failure> {
    let window = app.get_webview_window("main");
    tauri::async_runtime::spawn_blocking(move || {
        let mut dialog = rfd::FileDialog::new();
        if let Some(window) = &window {
            dialog = dialog.set_parent(window);
        }
        if folder {
            dialog.set_title("Choose a project folder").pick_folder()
        } else {
            dialog.set_title("Choose a program").add_filter("Programs", &["exe"]).pick_file()
        }
    })
    .await
    .map_err(|_| Failure::new("unexpected", "Something went wrong. Try again."))
}

#[tauri::command]
pub async fn list_registry(api: State<'_, Api>, store: State<'_, Store>, agent: State<'_, Agent>) -> Result<Overview, Failure> {
    let user = api.account_id()?;
    let now = now_ms();
    let (projects, apps, grants) = store.with(|db| Ok((db.projects(&user)?, db.apps(&user)?, db.grants(&user, now)?)))?;
    Ok(Overview { projects, apps, grants, session_grants: agent.sessions.list(&user, api.epoch()) })
}

#[tauri::command]
pub async fn add_project(app: AppHandle, api: State<'_, Api>, store: State<'_, Store>) -> Result<Option<Project>, Failure> {
    let user = api.account_id()?;
    let Some(picked) = pick(&app, true).await? else { return Ok(None) };
    let dir = paths::register_dir(picked.to_str().unwrap_or("")).map_err(path_failure)?;
    let name = dir.path.rsplit('\\').next().unwrap_or("");
    let project = store.with(|db| db.add_project(&user, &text::display_name(name, registry::NAME_CHARS), &dir.path, now_ms()))?;
    Ok(Some(project))
}

#[tauri::command]
pub async fn add_app(app: AppHandle, api: State<'_, Api>, store: State<'_, Store>) -> Result<Option<App>, Failure> {
    let user = api.account_id()?;
    let Some(picked) = pick(&app, false).await? else { return Ok(None) };
    let program = paths::register_program(picked.to_str().unwrap_or("")).map_err(path_failure)?;
    let file = program.path.rsplit('\\').next().unwrap_or("");
    let name = file.get(..file.len().saturating_sub(4)).unwrap_or(file);
    let added = store.with(|db| db.add_app(&user, &text::display_name(name, registry::NAME_CHARS), &program.path, now_ms()))?;
    Ok(Some(added))
}

#[tauri::command]
pub async fn remove_project(api: State<'_, Api>, store: State<'_, Store>, agent: State<'_, Agent>, id: String) -> Result<(), Failure> {
    let user = api.account_id()?;
    store.with(|db| db.remove_project(&user, &id))?;
    agent.sessions.forget_resource(&user, &id);
    if let Some(root) = registry::root_of(&id) {
        agent.sessions.forget_resource(&user, &root);
    }
    Ok(())
}

#[tauri::command]
pub async fn remove_app(api: State<'_, Api>, store: State<'_, Store>, agent: State<'_, Agent>, id: String) -> Result<(), Failure> {
    let user = api.account_id()?;
    store.with(|db| db.remove_app(&user, &id))?;
    agent.sessions.forget_resource(&user, &id);
    Ok(())
}

/// Gives a read tool access to a registered project, always (until revoked) or for this session.
#[tauri::command]
pub async fn grant_read(
    api: State<'_, Api>,
    store: State<'_, Store>,
    agent: State<'_, Agent>,
    project_id: String,
    tool: String,
    session_only: bool,
) -> Result<(), Failure> {
    let user = api.account_id()?;
    let resource = match tool.as_str() {
        "git_status" => project_id.clone(),
        "search_files" => registry::root_of(&project_id).ok_or_else(|| Failure::new("invalid_input", "That isn't a project."))?,
        _ => return Err(Failure::new("invalid_input", "That tool can't be allowed ahead of time.")),
    };
    if store.with(|db| db.project(&user, &project_id))?.is_none() {
        return Err(Failure::new("not_found", "That item is no longer here."));
    }
    if session_only {
        agent.sessions.add(&user, api.epoch(), &tool, &resource);
        Ok(())
    } else {
        store.with(|db| db.grant_read(&user, &tool, &resource, None, now_ms()).map(drop))
    }
}

#[tauri::command]
pub async fn revoke_grant(api: State<'_, Api>, store: State<'_, Store>, grant_id: String) -> Result<(), Failure> {
    let user = api.account_id()?;
    store.with(|db| db.revoke_grant(&user, &grant_id))
}

#[tauri::command]
pub async fn revoke_session_grant(api: State<'_, Api>, agent: State<'_, Agent>, tool: String, key: String) -> Result<(), Failure> {
    let user = api.account_id()?;
    if agent.sessions.revoke(&user, api.epoch(), &tool, &key) { Ok(()) } else { Err(Failure::new("not_found", "That item is no longer here.")) }
}

#[tauri::command]
pub async fn list_reminders(api: State<'_, Api>, store: State<'_, Store>) -> Result<Vec<Reminder>, Failure> {
    let user = api.account_id()?;
    store.with(|db| db.reminders(&user))
}

#[tauri::command]
pub async fn delete_reminder(api: State<'_, Api>, store: State<'_, Store>, id: String) -> Result<(), Failure> {
    let user = api.account_id()?;
    store.with(|db| db.delete_reminder(&user, &id))
}

#[tauri::command]
pub async fn list_activity(api: State<'_, Api>, store: State<'_, Store>) -> Result<Vec<AuditEntry>, Failure> {
    let user = api.account_id()?;
    store.with(|db| db.audit_list(&user))
}

/// Deletes this account's activity log. Returns how many entries went.
#[tauri::command]
pub async fn clear_activity(api: State<'_, Api>, store: State<'_, Store>) -> Result<usize, Failure> {
    let user = api.account_id()?;
    store.with(|db| db.audit_clear(&user))
}
