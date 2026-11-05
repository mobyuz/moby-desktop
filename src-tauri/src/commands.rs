//! Commands for the "Tools and projects" and "Activity" screens. Every one is scoped to the
//! signed-in account taken from the API client. The webview never sends a path: folders and
//! programs are picked in a native dialog that Rust opens, and the result is checked before it is
//! stored.

use std::path::PathBuf;

use serde::Serialize;
use tauri::{AppHandle, Manager, State};

use crate::agent::Agent;
use crate::api::{Api, Failure};
use crate::confirm::{Confirmer, Prompt};
use crate::grants::SessionGrant;
use crate::paths;
use crate::registry::{self, App, AuditEntry, Grant, Project, Reminder};
use crate::store::{Store, now_ms};
use crate::text;

/// A project as the page sees it. The real location never leaves Rust: only the last two parts.
#[derive(Serialize, Debug, PartialEq)]
pub struct ProjectView {
    id: String,
    name: String,
    short_path: String,
    created_at: i64,
}

#[derive(Serialize, Debug, PartialEq)]
pub struct AppView {
    id: String,
    name: String,
    short_path: String,
    created_at: i64,
}

/// `C:\Users\me\work\moby-api` becomes `work\moby-api`.
pub fn short_path(path: &str) -> String {
    let parts: Vec<&str> = path.split('\\').filter(|p| !p.is_empty()).collect();
    let tail = &parts[parts.len().saturating_sub(2)..];
    text::short_line(&tail.join("\\"), 80)
}

impl From<&Project> for ProjectView {
    fn from(p: &Project) -> Self {
        Self {
            id: p.id.clone(),
            name: p.name.clone(),
            short_path: short_path(&p.path),
            created_at: p.created_at,
        }
    }
}

impl From<&App> for AppView {
    fn from(a: &App) -> Self {
        Self {
            id: a.id.clone(),
            name: a.name.clone(),
            short_path: short_path(&a.path),
            created_at: a.created_at,
        }
    }
}

#[derive(Serialize)]
pub struct Overview {
    projects: Vec<ProjectView>,
    apps: Vec<AppView>,
    grants: Vec<Grant>,
    session_grants: Vec<SessionGrant>,
}

fn path_failure(error: paths::PathError) -> Failure {
    Failure::new(
        "invalid_path",
        &format!("That can't be added: {}.", paths::explain(error)),
    )
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
            dialog
                .set_title("Choose a project folder. Moby will be able to read its file names and Git status (not what is inside files) until you turn that off.")
                .pick_folder()
        } else {
            dialog
                .set_title("Choose a program")
                .add_filter("Programs", &["exe"])
                .pick_file()
        }
    })
    .await
    .map_err(|_| Failure::new("unexpected", "Something went wrong. Try again."))
}

#[tauri::command]
pub async fn list_registry(
    api: State<'_, Api>,
    store: State<'_, Store>,
    agent: State<'_, Agent>,
) -> Result<Overview, Failure> {
    let user = api.account_id()?;
    let now = now_ms();
    let (projects, apps, grants) =
        store.with(|db| Ok((db.projects(&user)?, db.apps(&user)?, db.grants(&user, now)?)))?;
    Ok(Overview {
        projects: projects.iter().map(ProjectView::from).collect(),
        apps: apps.iter().map(AppView::from).collect(),
        grants,
        session_grants: agent.sessions.list(&user, api.epoch()),
    })
}

#[tauri::command]
pub async fn add_project(
    app: AppHandle,
    api: State<'_, Api>,
    store: State<'_, Store>,
) -> Result<Option<ProjectView>, Failure> {
    let user = api.account_id()?;
    let Some(picked) = pick(&app, true).await? else {
        return Ok(None);
    };
    let dir = paths::register_dir(picked.to_str().unwrap_or("")).map_err(path_failure)?;
    let name = dir.path.rsplit('\\').next().unwrap_or("");
    let project = store.with(|db| {
        db.add_project(
            &user,
            &text::display_name(name, registry::NAME_CHARS),
            &dir.path,
            now_ms(),
        )
    })?;
    Ok(Some(ProjectView::from(&project)))
}

#[tauri::command]
pub async fn add_app(
    app: AppHandle,
    api: State<'_, Api>,
    store: State<'_, Store>,
) -> Result<Option<AppView>, Failure> {
    let user = api.account_id()?;
    let Some(picked) = pick(&app, false).await? else {
        return Ok(None);
    };
    let program = paths::register_program(picked.to_str().unwrap_or("")).map_err(path_failure)?;
    let file = program.path.rsplit('\\').next().unwrap_or("");
    let name = file.get(..file.len().saturating_sub(4)).unwrap_or(file);
    let added = store.with(|db| {
        db.add_app(
            &user,
            &text::display_name(name, registry::NAME_CHARS),
            &program.path,
            now_ms(),
        )
    })?;
    Ok(Some(AppView::from(&added)))
}

#[tauri::command]
pub async fn remove_project(
    api: State<'_, Api>,
    store: State<'_, Store>,
    agent: State<'_, Agent>,
    id: String,
) -> Result<(), Failure> {
    let user = api.account_id()?;
    store.with(|db| db.remove_project(&user, &id))?;
    agent.sessions.forget_resource(&user, &id);
    if let Some(root) = registry::root_of(&id) {
        agent.sessions.forget_resource(&user, &root);
    }
    Ok(())
}

#[tauri::command]
pub async fn remove_app(
    api: State<'_, Api>,
    store: State<'_, Store>,
    agent: State<'_, Agent>,
    id: String,
) -> Result<(), Failure> {
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
        "search_files" => registry::root_of(&project_id)
            .ok_or_else(|| Failure::new("invalid_input", "That isn't a project."))?,
        _ => {
            return Err(Failure::new(
                "invalid_input",
                "That tool can't be allowed ahead of time.",
            ));
        }
    };
    if store.with(|db| db.project(&user, &project_id))?.is_none() {
        return Err(Failure::new("not_found", "That item is no longer here."));
    }
    if session_only {
        agent.sessions.add(&user, api.epoch(), &tool, &resource);
        Ok(())
    } else {
        store.with(|db| {
            db.grant_read(&user, &tool, &resource, None, now_ms())
                .map(drop)
        })
    }
}

#[tauri::command]
pub async fn revoke_grant(
    api: State<'_, Api>,
    store: State<'_, Store>,
    grant_id: String,
) -> Result<(), Failure> {
    let user = api.account_id()?;
    store.with(|db| db.revoke_grant(&user, &grant_id))
}

#[tauri::command]
pub async fn revoke_session_grant(
    api: State<'_, Api>,
    agent: State<'_, Agent>,
    tool: String,
    key: String,
) -> Result<(), Failure> {
    let user = api.account_id()?;
    if agent.sessions.revoke(&user, api.epoch(), &tool, &key) {
        Ok(())
    } else {
        Err(Failure::new("not_found", "That item is no longer here."))
    }
}

#[tauri::command]
pub async fn list_reminders(
    api: State<'_, Api>,
    store: State<'_, Store>,
) -> Result<Vec<Reminder>, Failure> {
    let user = api.account_id()?;
    store.with(|db| db.reminders(&user))
}

#[tauri::command]
pub async fn delete_reminder(
    api: State<'_, Api>,
    store: State<'_, Store>,
    id: String,
) -> Result<(), Failure> {
    let user = api.account_id()?;
    store.with(|db| db.delete_reminder(&user, &id))
}

#[tauri::command]
pub async fn list_activity(
    api: State<'_, Api>,
    store: State<'_, Store>,
) -> Result<Vec<AuditEntry>, Failure> {
    let user = api.account_id()?;
    store.with(|db| db.audit_list(&user))
}

/// Deletes this account's activity log. Returns how many entries went.
#[tauri::command]
pub async fn clear_activity(
    api: State<'_, Api>,
    store: State<'_, Store>,
    agent: State<'_, Agent>,
) -> Result<usize, Failure> {
    let user = api.account_id()?;
    clear_activity_with(&store, &user, agent.confirmer().as_ref()).await
}

/// Clearing the log is confirmed in a native window, so a compromised page cannot wipe the
/// evidence of what it did. Nothing is deleted unless the person chose OK.
pub(crate) async fn clear_activity_with(
    store: &Store,
    user: &str,
    confirmer: &dyn Confirmer,
) -> Result<usize, Failure> {
    let prompt = Prompt::new(
        "Clear the activity log?",
        &[
            "This deletes every entry of the activity log for your account on this computer."
                .to_owned(),
        ],
        "delete them",
    );
    if !confirmer.confirm(prompt).await {
        return Err(Failure::new(
            "canceled",
            "The activity log was not cleared.",
        ));
    }
    store.with(|db| db.audit_clear(user))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::confirm::testing::Scripted;
    use crate::paths::testing::Scratch;
    use crate::registry::AuditRecord;

    fn store_with_entries() -> (Scratch, Store) {
        let scratch = Scratch::new();
        let store = Store::open(&scratch.path("data"));
        let record = AuditRecord {
            tool: "git_status",
            resource_id: None,
            decision: "auto",
            duration_ms: None,
            result_code: Some("ok"),
        };
        store
            .with(|db| db.audit_add("alice", &record, now_ms()))
            .unwrap();
        (scratch, store)
    }

    #[tokio::test]
    async fn the_activity_log_is_cleared_only_after_a_native_ok() {
        let (_dir, store) = store_with_entries();
        let refuse = Scripted::new(&[false]);
        let error = clear_activity_with(&store, "alice", &refuse)
            .await
            .unwrap_err();
        assert_eq!(error.code(), "canceled");
        assert_eq!(
            store.with(|db| db.audit_list("alice")).unwrap().len(),
            1,
            "nothing deleted without OK"
        );
        assert_eq!(refuse.shown().len(), 1);

        let allow = Scripted::new(&[true]);
        assert_eq!(
            clear_activity_with(&store, "alice", &allow).await.unwrap(),
            1
        );
        assert!(store.with(|db| db.audit_list("alice")).unwrap().is_empty());
    }

    #[test]
    fn the_page_gets_two_path_parts_and_never_the_real_path() {
        assert_eq!(
            short_path("C:\\Users\\me\\work\\moby-api"),
            "work\\moby-api"
        );
        assert_eq!(short_path("C:\\tools\\ed.exe"), "tools\\ed.exe");
        assert_eq!(short_path("C:\\x"), "C:\\x");
        let project = Project {
            id: "proj_1".into(),
            name: "n".into(),
            path: "C:\\secret\\deep\\place".into(),
            created_at: 1,
        };
        let json = serde_json::to_string(&ProjectView::from(&project)).unwrap();
        assert!(
            !json.contains("secret") && !json.contains("C:") && json.contains("deep"),
            "{json}"
        );
        let app = App {
            id: "app_1".into(),
            name: "n".into(),
            path: "C:\\secret\\bin\\a.exe".into(),
            created_at: 1,
        };
        let json = serde_json::to_string(&AppView::from(&app)).unwrap();
        assert!(!json.contains("secret") && !json.contains("C:"), "{json}");
    }
}
