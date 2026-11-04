//! The local tools and the checks that stand in front of them.
//!
//! A model request goes through the same chain every time (see `docs/tool-threat-notes.md`):
//!
//! 1. schema: the request is already a strict `ToolRequest` when it gets here
//! 2. resolve: ids become registry rows of the signed-in account, or the call is refused
//! 3. grant: read tools need a valid read grant; nothing else runs on a grant alone
//! 4. confirmation: side effects wait for the person (this file only says *whether*; `agent.rs` asks)
//! 5. run: fixed arguments, no shell, a time limit, an output limit, paths checked again
//! 6. sanitise: control and bidi characters out, truncated, fenced as untrusted data
//! 7. audit: tool, resource id, decision, time, duration, result code; never content
//!
//! [`plan`] covers steps 2 to 4 and [`execute_blocking`] covers 5 and 6. Neither takes anything
//! from the webview: the account comes from the API client and the request from a validated stream.

pub mod git;
pub mod launch;
pub mod search;

use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

use moby_protocol::{ToolFailureCode, ToolName, ToolRequest};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

use crate::grants::SessionGrants;
use crate::paths;
use crate::registry::{self, App, Project};
use crate::store::{Store, now_ms};
use crate::text;

pub const TOOL_TIMEOUT: Duration = Duration::from_secs(10);
pub const MAX_OUTPUT_BYTES: usize = 8 * 1024;
const MAX_REMINDER_AHEAD_MS: i64 = 366 * 86_400_000;

pub fn tool_name(request: &ToolRequest) -> &'static str {
    match request {
        ToolRequest::GitStatus { .. } => "git_status",
        ToolRequest::OpenApp { .. } => "open_app",
        ToolRequest::OpenProject { .. } => "open_project",
        ToolRequest::SearchFiles { .. } => "search_files",
        ToolRequest::CreateReminder { .. } => "create_reminder",
    }
}

pub fn name_of(tool: ToolName) -> &'static str {
    match tool {
        ToolName::GitStatus => "git_status",
        ToolName::OpenApp => "open_app",
        ToolName::OpenProject => "open_project",
        ToolName::SearchFiles => "search_files",
        ToolName::CreateReminder => "create_reminder",
    }
}

/// What a request resolved to. Holds registry rows, never model text that is used as a path.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    Git { project: Project },
    Search { project: Project, query: String, max: usize },
    OpenApp { app: App },
    OpenProject { project: Project, app: Option<App> },
    Reminder { text: String, due_ms: i64, due_label: String },
}

impl Action {
    pub fn tool(&self) -> &'static str {
        match self {
            Self::Git { .. } => "git_status",
            Self::Search { .. } => "search_files",
            Self::OpenApp { .. } => "open_app",
            Self::OpenProject { .. } => "open_project",
            Self::Reminder { .. } => "create_reminder",
        }
    }

    /// The id recorded in the activity log.
    pub fn resource_id(&self) -> Option<String> {
        match self {
            Self::Git { project } | Self::OpenProject { project, .. } => Some(project.id.clone()),
            Self::Search { project, .. } => registry::root_of(&project.id),
            Self::OpenApp { app } => Some(app.id.clone()),
            Self::Reminder { .. } => None,
        }
    }

    /// The key a session grant is filed under: the whole choice the person approved.
    pub fn grant_key(&self) -> Option<String> {
        match self {
            Self::Git { project } => Some(project.id.clone()),
            Self::Search { project, .. } => registry::root_of(&project.id),
            Self::OpenApp { app } => Some(app.id.clone()),
            Self::OpenProject { project, app } => Some(format!("{}+{}", project.id, app.as_ref().map_or("explorer", |a| a.id.as_str()))),
            Self::Reminder { .. } => None,
        }
    }

    /// The sentence the app itself writes. The model never contributes to it, except through
    /// registry names and the reminder text, which are cleaned and shown as quoted data.
    pub fn describe(&self) -> String {
        match self {
            Self::Git { project } => format!("Run git status in project {}", project.name),
            Self::Search { project, .. } => format!("Search file names in project {}", project.name),
            Self::OpenApp { app } => format!("Open {}", app.name),
            Self::OpenProject { project, app } => format!("Open project {} in {}", project.name, app.as_ref().map_or("Windows Explorer", |a| a.name.as_str())),
            Self::Reminder { due_label, .. } => format!("Create a reminder for {due_label}"),
        }
    }

    /// Extra lines for the approval dialog. Local paths are shown to the person only.
    pub fn details(&self) -> Vec<String> {
        match self {
            Self::OpenApp { app } => vec![format!("Program: {}", app.path)],
            Self::OpenProject { project, app } => {
                let mut lines = vec![format!("Folder: {}", project.path)];
                if let Some(app) = app {
                    lines.push(format!("Program: {}", app.path));
                }
                lines
            }
            Self::Reminder { text, .. } => vec![format!("Reminder text: \u{201C}{text}\u{201D}")],
            Self::Git { .. } | Self::Search { .. } => Vec::new(),
        }
    }
}

#[derive(Debug, PartialEq)]
pub enum Plan {
    /// Allowed without asking. `decision` goes to the activity log.
    Run { action: Action, decision: &'static str },
    /// Wait for the person. `allow_session` says whether "for this session" may be offered.
    Confirm { action: Action, allow_session: bool },
    /// Refused before anything happened.
    Refuse { code: ToolFailureCode, decision: &'static str, resource_id: Option<String> },
}

fn refuse(code: ToolFailureCode, decision: &'static str, resource_id: Option<&str>) -> Plan {
    Plan::Refuse { code, decision, resource_id: resource_id.map(str::to_owned) }
}

/// Steps 2 to 4. `epoch` ties session grants to the current sign-in.
pub fn plan(store: &Store, sessions: &SessionGrants, user: &str, epoch: u64, request: &ToolRequest, now: i64) -> Plan {
    let lookup = store.with(|db| {
        Ok(match request {
            ToolRequest::GitStatus { project_id } => (db.project(user, project_id.as_str())?, None, db.has_grant(user, "git_status", project_id.as_str(), now)?),
            ToolRequest::SearchFiles { root_id, .. } => {
                let project = registry::project_of_root(root_id.as_str()).map_or(Ok(None), |id| db.project(user, &id))?;
                (project, None, db.has_grant(user, "search_files", root_id.as_str(), now)?)
            }
            ToolRequest::OpenApp { app_id } => (None, db.app(user, app_id.as_str())?, false),
            ToolRequest::OpenProject { project_id, app_id } => {
                let app = match app_id {
                    Some(id) => db.app(user, id.as_str())?,
                    None => None,
                };
                (db.project(user, project_id.as_str())?, app, false)
            }
            ToolRequest::CreateReminder { .. } => (None, None, false),
        })
    });
    let Ok((project, app, granted)) = lookup else { return refuse(ToolFailureCode::Failed, "error", None) };

    match request {
        ToolRequest::GitStatus { project_id } => {
            let Some(project) = project else { return refuse(ToolFailureCode::Unavailable, "unknown_resource", None) };
            let action = Action::Git { project };
            read_plan(action, granted, sessions, user, epoch, project_id.as_str())
        }
        ToolRequest::SearchFiles { root_id, query, max_results } => {
            let Some(project) = project else { return refuse(ToolFailureCode::Unavailable, "unknown_resource", None) };
            let action = Action::Search { project, query: query.as_str().to_owned(), max: usize::from(max_results.get()) };
            read_plan(action, granted, sessions, user, epoch, root_id.as_str())
        }
        ToolRequest::OpenApp { .. } => {
            let Some(app) = app else { return refuse(ToolFailureCode::Unavailable, "unknown_resource", None) };
            side_effect_plan(Action::OpenApp { app }, sessions, user, epoch)
        }
        ToolRequest::OpenProject { app_id, .. } => {
            let Some(project) = project else { return refuse(ToolFailureCode::Unavailable, "unknown_resource", None) };
            if app_id.is_some() && app.is_none() {
                return refuse(ToolFailureCode::Unavailable, "unknown_resource", Some(&project.id));
            }
            side_effect_plan(Action::OpenProject { project, app }, sessions, user, epoch)
        }
        ToolRequest::CreateReminder { text: reminder, due_at } => {
            let cleaned = text::one_line(reminder.as_str());
            if cleaned.is_empty() {
                return refuse(ToolFailureCode::InvalidRequest, "rejected", None);
            }
            let Ok(due) = OffsetDateTime::parse(due_at.as_str(), &Rfc3339) else { return refuse(ToolFailureCode::InvalidRequest, "rejected", None) };
            let due_ms = i64::try_from(due.unix_timestamp_nanos() / 1_000_000).unwrap_or(i64::MAX);
            if due_ms <= now || due_ms > now.saturating_add(MAX_REMINDER_AHEAD_MS) {
                return refuse(ToolFailureCode::InvalidRequest, "rejected", None);
            }
            let offset = due.offset();
            let due_label = format!(
                "{:04}-{:02}-{:02} {:02}:{:02} (UTC{}{:02}:{:02})",
                due.year(),
                u8::from(due.month()),
                due.day(),
                due.hour(),
                due.minute(),
                if offset.is_negative() { '-' } else { '+' },
                offset.whole_hours().abs(),
                offset.minutes_past_hour().abs()
            );
            Plan::Confirm { action: Action::Reminder { text: cleaned, due_ms, due_label }, allow_session: false }
        }
    }
}

fn read_plan(action: Action, granted: bool, sessions: &SessionGrants, user: &str, epoch: u64, resource: &str) -> Plan {
    if granted {
        return Plan::Run { action, decision: "auto" };
    }
    if sessions.has(user, epoch, action.tool(), resource) {
        return Plan::Run { action, decision: "session_grant" };
    }
    refuse(ToolFailureCode::Denied, "no_grant", Some(resource))
}

fn side_effect_plan(action: Action, sessions: &SessionGrants, user: &str, epoch: u64) -> Plan {
    let key = action.grant_key().unwrap_or_default();
    if sessions.has(user, epoch, action.tool(), &key) {
        return Plan::Run { action, decision: "session_grant" };
    }
    Plan::Confirm { action, allow_session: true }
}

fn path_code(error: paths::PathError) -> ToolFailureCode {
    match error {
        paths::PathError::Missing | paths::PathError::Changed | paths::PathError::Reparse | paths::PathError::WrongKind => ToolFailureCode::Unavailable,
        _ => ToolFailureCode::InvalidRequest,
    }
}

/// Steps 5 and 6 for the blocking tools. Runs on a worker thread. Paths are verified here, after
/// any confirmation, so a folder swapped for a junction while the dialog was open is caught.
pub fn execute_blocking(action: &Action, cancel: &Arc<AtomicBool>, started: Instant, limit: Duration) -> Result<String, ToolFailureCode> {
    match action {
        Action::Git { project } => {
            let dir = paths::verify_dir(&project.path).map_err(path_code)?;
            let summary = git::status(&dir, cancel)?;
            Ok(text::untrusted_output("git_status", &summary.render(), MAX_OUTPUT_BYTES))
        }
        Action::Search { project, query, max } => {
            let dir = paths::verify_dir(&project.path).map_err(path_code)?;
            let found = search::search(&dir, query, *max, cancel, started + limit)?;
            let body = if found.is_empty() { "no matching file names".to_owned() } else { format!("{} matching file names:\n{}", found.len(), found.join("\n")) };
            Ok(text::untrusted_output("search_files", &body, MAX_OUTPUT_BYTES))
        }
        Action::OpenApp { app } => {
            launch::open_app(&app.path)?;
            Ok("ok".to_owned())
        }
        Action::OpenProject { project, app } => {
            let dir = paths::verify_dir(&project.path).map_err(path_code)?;
            launch::open_project(app.as_ref().map(|a| a.path.as_str()), &dir)?;
            Ok("ok".to_owned())
        }
        Action::Reminder { .. } => Err(ToolFailureCode::Failed),
    }
}

/// Saves a confirmed reminder and returns its id.
pub fn save_reminder(store: &Store, user: &str, action: &Action) -> Result<String, ToolFailureCode> {
    let Action::Reminder { text, due_ms, .. } = action else { return Err(ToolFailureCode::Failed) };
    store.with(|db| db.add_reminder(user, text, *due_ms, now_ms())).map_err(|_| ToolFailureCode::Failed)
}

pub fn failure_text(code: ToolFailureCode) -> &'static str {
    match code {
        ToolFailureCode::Denied => "Not allowed",
        ToolFailureCode::InvalidRequest => "The request was not valid",
        ToolFailureCode::Unavailable => "Not available",
        ToolFailureCode::Canceled => "Canceled",
        ToolFailureCode::Timeout => "Took too long",
        ToolFailureCode::Failed => "Failed",
        ToolFailureCode::PartialResult => "Only a partial result was possible",
    }
}

pub fn code_str(code: ToolFailureCode) -> &'static str {
    match code {
        ToolFailureCode::Denied => "denied",
        ToolFailureCode::InvalidRequest => "invalid_request",
        ToolFailureCode::Unavailable => "unavailable",
        ToolFailureCode::Canceled => "canceled",
        ToolFailureCode::Timeout => "timeout",
        ToolFailureCode::Failed => "failed",
        ToolFailureCode::PartialResult => "partial_result",
    }
}
