//! The agent turn, in Rust. The webview hands over the conversation text and the language; this
//! module builds the request (including which tools and resources exist, from the registry),
//! streams the answer, and runs the loop of "model asks, app checks, person confirms, tool runs,
//! result goes back". The webview cannot name a path or start a tool: all it can do is answer an
//! approval that this module issued from an API stream, with `answer_approval`.
//!
//! Limits per turn: 4 tool rounds, 10 s per tool, 8 KiB of output, 2 minutes to answer a dialog.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use moby_protocol::{
    AgentMessage, AgentTurnRequest, AgentTurnResponse, Expression, Language, LocalTime,
    ResourceKind, Script, StableId, ToolCall, ToolFailureCode, ToolName, ToolRequest, ToolResource,
    ToolResult,
};
use serde::{Deserialize, Serialize};
use tauri::State;
use tauri::ipc::Channel;
use tokio::sync::{Notify, oneshot};
use uuid::Uuid;

use crate::api::{Api, Failure, Frames, open_agent_stream};
use crate::confirm::{Confirmer, Prompt};
use crate::grants::SessionGrants;
use crate::registry::{self, AuditRecord};
use crate::store::{Store, now_ms};
use crate::text;
use crate::tools::{self, Action, Plan};

pub const MAX_ROUNDS: usize = 4;
pub const APPROVAL_TIMEOUT: Duration = Duration::from_secs(120);
const MAX_MESSAGES: usize = 64;
const MAX_TEXT_BYTES: usize = 8 * 1024;

/// The user's current time for the model, or nothing when Windows cannot give the offset.
fn local_time() -> Option<LocalTime> {
    format_local_time(time::OffsetDateTime::now_local().ok()?)
}

fn format_local_time(at: time::OffsetDateTime) -> Option<LocalTime> {
    let offset = at.offset();
    LocalTime::try_from(format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}{}{:02}:{:02}",
        at.year(),
        u8::from(at.month()),
        at.day(),
        at.hour(),
        at.minute(),
        at.second(),
        if offset.is_negative() { '-' } else { '+' },
        offset.whole_hours().unsigned_abs(),
        offset.minutes_past_hour().unsigned_abs(),
    ))
    .ok()
}

/// What the person chose in the in-page dialog, which now only serves reminders. Launches are
/// confirmed in a native window and have no webview answer at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Decision {
    Deny,
    Once,
}

/// Events for the webview. The tool request itself is never among them.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum UiEvent {
    SpeechDelta {
        text: String,
    },
    /// Provisional speech from a request that ended in a tool call is dropped.
    ResetSpeech,
    ApprovalRequired {
        approval_id: String,
        tool: &'static str,
        /// Written by this app.
        title: String,
        /// Written by this app from registry rows. Local paths, shown to the person only.
        details: Vec<String>,
        /// What the model said alongside the request. Untrusted; shown apart from the title.
        moby_says: Option<String>,
        timeout_ms: u64,
    },
    /// A launch is being confirmed in a native window that Rust owns. Nothing to answer here.
    NativePrompt {
        title: String,
    },
    NativePromptClosed,
    ApprovalClosed {
        approval_id: String,
        outcome: &'static str,
    },
    ToolResult {
        tool: &'static str,
        description: String,
        outcome: &'static str,
        note: String,
    },
    Completed {
        speech_text: String,
        display_text: Option<String>,
        expression_hint: Expression,
    },
    Error {
        error: UiError,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct UiError {
    pub code: String,
    pub message: String,
}

pub trait Sink: Send + Sync {
    fn emit(&self, event: UiEvent);
}

impl Sink for Channel<UiEvent> {
    fn emit(&self, event: UiEvent) {
        let _ = self.send(event);
    }
}

#[derive(Clone)]
pub(crate) struct Cancel {
    flag: Arc<AtomicBool>,
    notify: Arc<Notify>,
}

impl Cancel {
    pub(crate) fn new() -> Self {
        Self {
            flag: Arc::new(AtomicBool::new(false)),
            notify: Arc::new(Notify::new()),
        }
    }

    pub(crate) fn cancel(&self) {
        self.flag.store(true, Ordering::SeqCst);
        self.notify.notify_waiters();
    }

    pub(crate) fn is_canceled(&self) -> bool {
        self.flag.load(Ordering::SeqCst)
    }

    pub(crate) async fn cancelled(&self) {
        loop {
            let notified = self.notify.notified();
            if self.is_canceled() {
                return;
            }
            notified.await;
        }
    }
}

/// The conversation text from the webview. Nothing else about the turn comes from there.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TurnInput {
    messages: Vec<InMessage>,
    language: Language,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct InMessage {
    kind: InKind,
    text: String,
}

#[derive(Debug, Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum InKind {
    User,
    Assistant,
}

impl TurnInput {
    #[cfg(test)]
    pub(crate) fn for_test(text: &str) -> Self {
        Self {
            messages: vec![InMessage {
                kind: InKind::User,
                text: text.to_owned(),
            }],
            language: Language::En,
        }
    }

    /// The text of the newest user message: for a voice turn, it must be the transcript Rust holds.
    pub(crate) fn last_user_text(&self) -> Option<&str> {
        self.messages
            .last()
            .filter(|message| message.kind == InKind::User)
            .map(|message| message.text.as_str())
    }

    /// The language the speaker used, as the speech service heard it.
    pub(crate) fn with_language(mut self, language: Language) -> Self {
        self.language = language;
        self
    }

    fn into_messages(self) -> Result<(Vec<AgentMessage>, Language), Failure> {
        let bad = || Failure::new("invalid_input", "That conversation can't be sent.");
        if self.messages.is_empty()
            || self.messages.len() > MAX_MESSAGES
            || self.messages.last().map(|m| m.kind) != Some(InKind::User)
        {
            return Err(bad());
        }
        let mut out = Vec::with_capacity(self.messages.len());
        for message in self.messages {
            if message.text.trim().is_empty() || message.text.len() > MAX_TEXT_BYTES {
                return Err(bad());
            }
            out.push(match message.kind {
                InKind::User => AgentMessage::User { text: message.text },
                InKind::Assistant => AgentMessage::Assistant { text: message.text },
            });
        }
        Ok((out, self.language))
    }
}

/// What this turn tells the API exists. Fixed for the whole turn, because the API checks the
/// tool calls in the history against it. Whether a call may still run is decided when it runs.
struct Offer {
    tools: Vec<ToolName>,
    resources: Vec<ToolResource>,
}

impl Offer {
    fn allows(&self, request: &ToolRequest) -> bool {
        let has = |kind: ResourceKind, id: &StableId| {
            self.resources.iter().any(|r| r.kind == kind && &r.id == id)
        };
        let (name, ok) = match request {
            ToolRequest::GitStatus { project_id } => {
                (ToolName::GitStatus, has(ResourceKind::Project, project_id))
            }
            ToolRequest::OpenApp { app_id } => (ToolName::OpenApp, has(ResourceKind::App, app_id)),
            ToolRequest::OpenProject { project_id, app_id } => (
                ToolName::OpenProject,
                has(ResourceKind::Project, project_id)
                    && app_id.as_ref().is_none_or(|id| has(ResourceKind::App, id)),
            ),
            ToolRequest::SearchFiles { root_id, .. } => (
                ToolName::SearchFiles,
                has(ResourceKind::SearchRoot, root_id),
            ),
            ToolRequest::CreateReminder { .. } => (ToolName::CreateReminder, true),
        };
        ok && self.tools.contains(&name)
    }
}

fn build_offer(
    store: &Store,
    sessions: &SessionGrants,
    user: &str,
    epoch: u64,
    now: i64,
) -> Result<Offer, Failure> {
    let (projects, apps, grants) =
        store.with(|db| Ok((db.projects(user)?, db.apps(user)?, db.grants(user, now)?)))?;
    let id = |value: &str| StableId::new(value).ok();
    let resource = |kind, value: &str, name: &str| {
        id(value).map(|id| ToolResource {
            kind,
            id,
            display_name: text::display_name(name, 100),
        })
    };
    let mut resources = Vec::new();
    let (mut git, mut search) = (false, false);
    for project in &projects {
        let root = registry::root_of(&project.id).unwrap_or_default();
        let git_ok = grants
            .iter()
            .any(|g| g.tool == "git_status" && g.resource_id == project.id)
            || sessions.has(user, epoch, "git_status", &project.id);
        let search_ok = grants
            .iter()
            .any(|g| g.tool == "search_files" && g.resource_id == root)
            || sessions.has(user, epoch, "search_files", &root);
        git |= git_ok;
        search |= search_ok;
        resources.extend(resource(ResourceKind::Project, &project.id, &project.name));
        if search_ok {
            resources.extend(resource(ResourceKind::SearchRoot, &root, &project.name));
        }
    }
    for app in &apps {
        resources.extend(resource(ResourceKind::App, &app.id, &app.name));
    }
    let mut tool_list = Vec::new();
    if git {
        tool_list.push(ToolName::GitStatus);
    }
    if search {
        tool_list.push(ToolName::SearchFiles);
    }
    if !apps.is_empty() {
        tool_list.push(ToolName::OpenApp);
    }
    if !projects.is_empty() {
        tool_list.push(ToolName::OpenProject);
    }
    tool_list.push(ToolName::CreateReminder);
    Ok(Offer {
        tools: tool_list,
        resources,
    })
}

struct Pending {
    user: String,
    epoch: u64,
    tx: oneshot::Sender<Decision>,
}

pub struct Agent {
    pub sessions: SessionGrants,
    pending: Mutex<HashMap<String, Pending>>,
    active: Mutex<Option<Cancel>>,
    approval_timeout: Duration,
    tool_timeout: Duration,
    confirmer: Arc<dyn Confirmer>,
    runner: Runner,
}

/// Runs the blocking part of a tool. A field so a test can slow it down.
pub(crate) type Runner = Arc<
    dyn Fn(
            &Action,
            &Arc<AtomicBool>,
            Instant,
            Duration,
            &AtomicBool,
        ) -> Result<String, ToolFailureCode>
        + Send
        + Sync,
>;

/// Clears the running-turn slot, whatever way the turn ends.
struct TurnGuard<'a>(&'a Agent);

impl Drop for TurnGuard<'_> {
    fn drop(&mut self) {
        *self.0.active.lock().unwrap_or_else(PoisonError::into_inner) = None;
    }
}

struct Env<'a> {
    api: &'a Api,
    store: &'a Store,
    sink: &'a dyn Sink,
    user: String,
    epoch: u64,
    cancel: Cancel,
}

impl Agent {
    pub fn new(confirmer: Arc<dyn Confirmer>) -> Self {
        Self::with_timeouts(APPROVAL_TIMEOUT, tools::TOOL_TIMEOUT, confirmer)
    }

    pub(crate) fn with_timeouts(
        approval_timeout: Duration,
        tool_timeout: Duration,
        confirmer: Arc<dyn Confirmer>,
    ) -> Self {
        Self {
            confirmer,
            runner: Arc::new(tools::execute_blocking),
            sessions: SessionGrants::default(),
            pending: Mutex::new(HashMap::new()),
            active: Mutex::new(None),
            approval_timeout,
            tool_timeout,
        }
    }

    pub fn confirmer(&self) -> Arc<dyn Confirmer> {
        Arc::clone(&self.confirmer)
    }

    fn pending(&self) -> std::sync::MutexGuard<'_, HashMap<String, Pending>> {
        self.pending.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Ends whatever is running and forgets every approval and session grant. Called on sign-out.
    pub fn end_session(&self) {
        self.cancel_turn();
        self.pending().clear();
        self.sessions.clear();
    }

    pub fn cancel_turn(&self) {
        if let Some(cancel) = self
            .active
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
        {
            cancel.cancel();
        }
    }

    /// The only way an approval is answered. It must name an id this module issued, for this
    /// account and this sign-in, that has not been answered, expired or canceled. The first answer wins.
    pub fn answer(
        &self,
        user: &str,
        epoch: u64,
        approval_id: &str,
        decision: Decision,
    ) -> Result<(), Failure> {
        let gone = || Failure::new("not_found", "That request is no longer waiting.");
        let mut pending = self.pending();
        pending
            .get(approval_id)
            .filter(|p| p.user == user && p.epoch == epoch)
            .ok_or_else(gone)?;
        let entry = pending.remove(approval_id).ok_or_else(gone)?;
        entry.tx.send(decision).map_err(|_| gone())
    }

    fn begin(&self) -> Result<(Cancel, TurnGuard<'_>), Failure> {
        let mut active = self.active.lock().unwrap_or_else(PoisonError::into_inner);
        if active.is_some() {
            return Err(Failure::new("busy", "Moby is already answering."));
        }
        let cancel = Cancel::new();
        *active = Some(cancel.clone());
        Ok((cancel, TurnGuard(self)))
    }

    fn audit(
        &self,
        env: &Env<'_>,
        tool: &str,
        resource: Option<&str>,
        decision: &str,
        started: Option<Instant>,
        code: &str,
    ) {
        let duration_ms =
            started.map(|at| i64::try_from(at.elapsed().as_millis()).unwrap_or(i64::MAX));
        let record = AuditRecord {
            tool,
            resource_id: resource,
            decision,
            duration_ms,
            result_code: Some(code),
        };
        if env
            .store
            .with(|db| db.audit_add(&env.user, &record, now_ms()))
            .is_err()
        {
            eprintln!("moby: could not write the activity log");
        }
    }

    /// One whole conversation turn, with as many tool rounds as the limit allows.
    pub async fn run_turn(
        &self,
        api: &Api,
        store: &Store,
        sink: &dyn Sink,
        input: TurnInput,
    ) -> Result<(), Failure> {
        let user = api.account_id()?;
        let epoch = api.epoch();
        let (cancel, _guard) = self.begin()?;
        let (mut messages, language) = input.into_messages()?;
        let offer = build_offer(store, &self.sessions, &user, epoch, now_ms())?;
        let env = Env {
            api,
            store,
            sink,
            user,
            epoch,
            cancel,
        };
        let script = (language == Language::Uz).then_some(Script::Latn);
        let mut rounds = 0;

        loop {
            let request = AgentTurnRequest {
                messages: messages.clone(),
                available_tools: offer.tools.clone(),
                resources: offer.resources.clone(),
                language: Some(language),
                script,
                local_time: local_time(),
            };
            let body = serde_json::to_value(&request)
                .map_err(|_| Failure::new("unexpected", "Something went wrong. Try again."))?;
            let response = tokio::select! {
                response = open_agent_stream(api, &body) => response?,
                () = env.cancel.cancelled() => return self.stopped(&env),
            };
            let mut frames = Frames::new(response);
            let mut provisional = String::new();
            let outcome = loop {
                let frame = tokio::select! {
                    frame = frames.next() => frame?,
                    () = env.cancel.cancelled() => return self.stopped(&env),
                };
                let Some(frame) = frame else {
                    return Err(Failure::new(
                        "offline",
                        "Can't connect to Moby. Check your connection and try again.",
                    ));
                };
                match serde_json::from_str::<AgentTurnResponse>(&frame) {
                    Ok(AgentTurnResponse::SpeechDelta { text }) => {
                        if provisional.len() < MAX_TEXT_BYTES {
                            provisional.push_str(&text);
                        }
                        sink.emit(UiEvent::SpeechDelta { text });
                    }
                    Ok(other) => break Ok(other),
                    Err(_) => break Err(frame),
                }
            };

            match outcome {
                Err(frame) => {
                    // Step 1 failed: an unknown tool, an extra field, an oversized or malformed value.
                    sink.emit(UiEvent::ResetSpeech);
                    self.audit(
                        &env,
                        rejected_tool(&frame),
                        None,
                        "rejected",
                        None,
                        "invalid_request",
                    );
                    sink.emit(UiEvent::Error {
                        error: UiError {
                            code: "invalid_tool_request".into(),
                            message:
                                "Moby asked for something this app doesn't allow. Nothing was run."
                                    .into(),
                        },
                    });
                    return Ok(());
                }
                Ok(AgentTurnResponse::Completed {
                    speech_text,
                    display_text,
                    expression_hint,
                }) => {
                    sink.emit(UiEvent::Completed {
                        speech_text,
                        display_text,
                        expression_hint,
                    });
                    return Ok(());
                }
                Ok(AgentTurnResponse::Error { error }) => {
                    sink.emit(UiEvent::Error {
                        error: UiError {
                            code: error.code,
                            message: error.message,
                        },
                    });
                    return Ok(());
                }
                Ok(AgentTurnResponse::SpeechDelta { .. }) => {
                    unreachable!("speech deltas are handled above")
                }
                Ok(AgentTurnResponse::ToolCall { call }) => {
                    sink.emit(UiEvent::ResetSpeech);
                    let tool = tools::tool_name(&call.request);
                    let repeated = messages.iter().any(|m| matches!(m, AgentMessage::ToolCall { call: earlier } if earlier.call_id == call.call_id));
                    if repeated || !offer.allows(&call.request) {
                        self.audit(&env, tool, None, "rejected", None, "invalid_request");
                        sink.emit(UiEvent::Error {
                            error: UiError { code: "invalid_tool_request".into(), message: "Moby asked for something this app doesn't allow. Nothing was run.".into() },
                        });
                        return Ok(());
                    }
                    if rounds >= MAX_ROUNDS {
                        self.audit(&env, tool, None, "round_limit", None, "denied");
                        sink.emit(UiEvent::Error {
                            error: UiError { code: "tool_limit".into(), message: "Moby tried to do too many things in one go, so it stopped. Nothing more was run.".into() },
                        });
                        return Ok(());
                    }
                    rounds += 1;
                    let says = Some(text::short_line(&provisional, 300)).filter(|s| !s.is_empty());
                    let result = self.handle_call(&env, &call, says).await;
                    let canceled = matches!(
                        result,
                        ToolResult::Error {
                            code: ToolFailureCode::Canceled,
                            ..
                        }
                    ) && env.cancel.is_canceled();
                    messages.push(AgentMessage::ToolCall { call });
                    messages.push(AgentMessage::ToolResult { result });
                    if canceled {
                        return self.stopped(&env);
                    }
                }
            }
        }
    }

    fn stopped(&self, env: &Env<'_>) -> Result<(), Failure> {
        env.sink.emit(UiEvent::Error {
            error: UiError {
                code: "canceled".into(),
                message: "Stopped.".into(),
            },
        });
        Ok(())
    }

    /// Steps 2 to 7 for one valid request. Always produces a result for the model.
    async fn handle_call(
        &self,
        env: &Env<'_>,
        call: &ToolCall,
        says: Option<String>,
    ) -> ToolResult {
        let tool = tools::tool_name(&call.request);
        let id = call.call_id.clone();
        // Still the same sign-in, and nobody pressed Stop or signed out.
        let current = |env: &Env<'_>| {
            !env.cancel.is_canceled()
                && env.api.account_id().is_ok_and(|u| u == env.user)
                && env.api.epoch() == env.epoch
        };
        if !current(env) {
            return self.finish(
                env,
                id,
                tool,
                tool,
                None,
                "canceled",
                None,
                Err(ToolFailureCode::Canceled),
            );
        }
        let plan = tools::plan(
            env.store,
            &self.sessions,
            &env.user,
            env.epoch,
            &call.request,
            now_ms(),
        );
        let (action, decision) = match plan {
            Plan::Refuse {
                code,
                decision,
                resource_id,
            } => {
                return self.finish(
                    env,
                    id,
                    tool,
                    tool_label(tool),
                    resource_id.as_deref(),
                    decision,
                    None,
                    Err(code),
                );
            }
            Plan::Run { action, decision } => (action, decision),
            Plan::Confirm { action } => {
                let answer = self.ask(env, &action, says).await;
                let refusal = match answer {
                    Answer::Allowed => None,
                    Answer::Denied => Some(("denied", ToolFailureCode::Denied)),
                    Answer::Expired => Some(("expired", ToolFailureCode::Denied)),
                    Answer::Canceled => Some(("canceled", ToolFailureCode::Canceled)),
                };
                if let Some((decision, code)) = refusal {
                    return self.finish(
                        env,
                        id,
                        tool,
                        &action.describe(),
                        action.resource_id().as_deref(),
                        decision,
                        None,
                        Err(code),
                    );
                }
                // The world may have changed while the window was open: another sign-in, a Stop,
                // a removed folder. The answer counts only for exactly what was shown.
                if !current(env) {
                    return self.finish(
                        env,
                        id,
                        tool,
                        &action.describe(),
                        action.resource_id().as_deref(),
                        "canceled",
                        None,
                        Err(ToolFailureCode::Canceled),
                    );
                }
                let same = match tools::plan(
                    env.store,
                    &self.sessions,
                    &env.user,
                    env.epoch,
                    &call.request,
                    now_ms(),
                ) {
                    Plan::Confirm { action: again } | Plan::Run { action: again, .. } => {
                        again == action
                    }
                    Plan::Refuse { .. } => false,
                };
                if !same {
                    return self.finish(
                        env,
                        id,
                        tool,
                        &action.describe(),
                        action.resource_id().as_deref(),
                        "changed",
                        None,
                        Err(ToolFailureCode::Unavailable),
                    );
                }
                (action, "approved_once")
            }
        };
        // Last look before anything runs.
        if !current(env) {
            return self.finish(
                env,
                id,
                tool,
                &action.describe(),
                action.resource_id().as_deref(),
                "canceled",
                None,
                Err(ToolFailureCode::Canceled),
            );
        }
        let started = Instant::now();
        let result = self.run(env, &action).await;
        self.finish(
            env,
            id,
            tool,
            &action.describe(),
            action.resource_id().as_deref(),
            decision,
            Some(started),
            result,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn finish(
        &self,
        env: &Env<'_>,
        call_id: StableId,
        tool: &'static str,
        description: &str,
        resource: Option<&str>,
        decision: &str,
        started: Option<Instant>,
        result: Result<String, ToolFailureCode>,
    ) -> ToolResult {
        let code = match &result {
            Ok(_) => "ok",
            Err(code) => tools::code_str(*code),
        };
        self.audit(env, tool, resource, decision, started, code);
        let (outcome, note) = match &result {
            Ok(_) => ("ok", String::new()),
            Err(code) => (
                tools::code_str(*code),
                tools::failure_text(*code).to_owned(),
            ),
        };
        env.sink.emit(UiEvent::ToolResult {
            tool,
            description: text::short_line(description, 160),
            outcome,
            note,
        });
        match result {
            Ok(output) => ToolResult::Ok { call_id, output },
            Err(code) => ToolResult::Error { call_id, code },
        }
    }

    /// Asks the person. A launch is confirmed in a native window opened and answered by Rust, so
    /// the webview never sees an id it could answer. Only reminders use the in-page dialog.
    async fn ask(&self, env: &Env<'_>, action: &Action, says: Option<String>) -> Answer {
        if action.needs_native_confirmation() {
            return self.ask_native(env, action).await;
        }
        let approval_id = format!("apr_{}", Uuid::new_v4().hyphenated());
        let (tx, rx) = oneshot::channel();
        self.pending().insert(
            approval_id.clone(),
            Pending {
                user: env.user.clone(),
                epoch: env.epoch,
                tx,
            },
        );
        env.sink.emit(UiEvent::ApprovalRequired {
            approval_id: approval_id.clone(),
            tool: action.tool(),
            title: action.describe(),
            details: action.details(),
            moby_says: says,
            timeout_ms: u64::try_from(self.approval_timeout.as_millis()).unwrap_or(u64::MAX),
        });
        let answer = tokio::select! {
            decided = rx => match decided {
                Ok(Decision::Once) => Answer::Allowed,
                Ok(Decision::Deny) => Answer::Denied,
                Err(_) => Answer::Canceled,
            },
            () = tokio::time::sleep(self.approval_timeout) => Answer::Expired,
            () = env.cancel.cancelled() => Answer::Canceled,
        };
        self.pending().remove(&approval_id);
        env.sink.emit(UiEvent::ApprovalClosed {
            approval_id,
            outcome: match answer {
                Answer::Denied => "denied",
                Answer::Allowed => "approved",
                Answer::Expired => "expired",
                Answer::Canceled => "canceled",
            },
        });
        answer
    }

    async fn ask_native(&self, env: &Env<'_>, action: &Action) -> Answer {
        let mut lines = vec![action.describe()];
        lines.extend(action.details());
        // The model's own words are left out of the window altogether.
        let prompt = Prompt::new("Moby asks permission", &lines, "allow this once");
        env.sink.emit(UiEvent::NativePrompt {
            title: text::short_line(&action.describe(), 160),
        });
        let answer = tokio::select! {
            allowed = self.confirmer.confirm(prompt) => {
                if allowed { Answer::Allowed } else { Answer::Denied }
            }
            () = tokio::time::sleep(self.approval_timeout) => Answer::Expired,
            () = env.cancel.cancelled() => Answer::Canceled,
        };
        env.sink.emit(UiEvent::NativePromptClosed);
        answer
    }

    /// Step 5 and 6 with the time limit and cancellation around them.
    async fn run(&self, env: &Env<'_>, action: &Action) -> Result<String, ToolFailureCode> {
        if matches!(action, Action::Reminder { .. }) {
            let id = tools::save_reminder(env.store, &env.user, action)?;
            return Ok(format!("Reminder saved. ID: {id}"));
        }
        let stop = Arc::new(AtomicBool::new(false));
        let launched = Arc::new(AtomicBool::new(false));
        let started = Instant::now();
        let (work, flag, limit, seen) = (
            action.clone(),
            Arc::clone(&stop),
            self.tool_timeout,
            Arc::clone(&launched),
        );
        let runner = Arc::clone(&self.runner);
        let mut task =
            tokio::task::spawn_blocking(move || runner(&work, &flag, started, limit, &seen));
        let result = tokio::select! {
            done = &mut task => done.unwrap_or(Err(ToolFailureCode::Failed)),
            () = tokio::time::sleep(self.tool_timeout) => { stop.store(true, Ordering::SeqCst); Err(ToolFailureCode::Timeout) }
            () = env.cancel.cancelled() => { stop.store(true, Ordering::SeqCst); Err(ToolFailureCode::Canceled) }
        };
        settle(result, launched.load(Ordering::SeqCst))
    }
}

/// A launch that was cut off by a timeout or a cancel after the program had already started is
/// reported as started: the activity log and the model must not say nothing happened.
pub(crate) fn settle(
    result: Result<String, ToolFailureCode>,
    launched: bool,
) -> Result<String, ToolFailureCode> {
    match result {
        Err(ToolFailureCode::Timeout | ToolFailureCode::Canceled) if launched => {
            Ok("ok".to_owned())
        }
        other => other,
    }
}

/// Starts a sign-out in the only safe order: the sign-in epoch changes first, so any decision that
/// arrives afterwards is stale, and only then are the running turn and the dialogs dropped.
pub fn begin_sign_out(api: &Api, agent: &Agent) {
    api.set_account(None);
    agent.end_session();
}

enum Answer {
    Allowed,
    Denied,
    Expired,
    Canceled,
}

fn tool_label(tool: &str) -> &'static str {
    match tool {
        "git_status" => "git status",
        "search_files" => "file search",
        "open_app" => "opening a program",
        "open_project" => "opening a project",
        _ => "a reminder",
    }
}

/// The tool name of a rejected frame, for the activity log, if it is one we know. Never anything else.
fn rejected_tool(frame: &str) -> &'static str {
    let name = serde_json::from_str::<serde_json::Value>(frame)
        .ok()
        .and_then(|v| v["call"]["request"]["tool"].as_str().map(str::to_owned));
    match name.as_deref() {
        Some("git_status") => "git_status",
        Some("open_app") => "open_app",
        Some("open_project") => "open_project",
        Some("search_files") => "search_files",
        Some("create_reminder") => "create_reminder",
        _ => "unknown",
    }
}

#[tauri::command]
pub async fn agent_turn(
    api: State<'_, Api>,
    store: State<'_, Store>,
    agent: State<'_, Agent>,
    turn: TurnInput,
    on_event: Channel<UiEvent>,
) -> Result<(), Failure> {
    agent.run_turn(&api, &store, &on_event, turn).await
}

/// Answers a dialog that the app opened. The id comes from an `approval_required` event.
#[tauri::command]
pub async fn answer_approval(
    api: State<'_, Api>,
    agent: State<'_, Agent>,
    approval_id: String,
    decision: Decision,
) -> Result<(), Failure> {
    let user = api.account_id()?;
    agent.answer(&user, api.epoch(), &approval_id, decision)
}

#[tauri::command]
pub async fn cancel_turn(agent: State<'_, Agent>) -> Result<(), Failure> {
    agent.cancel_turn();
    Ok(())
}

#[cfg(test)]
#[path = "agent_tests.rs"]
mod tests;
