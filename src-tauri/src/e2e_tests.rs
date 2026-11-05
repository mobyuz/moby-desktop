//! The desktop's real API client and agent loop against a real, locally running moby-api (mock LLM
//! and mock speech providers, nothing paid). Ignored by default; it runs only when asked for and
//! only when `MOBY_E2E_API_URL` is set. See "Running against a real moby-api" in the README.
//!
//! What is real: `Api` (sign-up, sign-in, refresh, sign-out, account deletion), `Agent::run_turn`,
//! the tool code, the local store, `Voice` (consent, STT, TTS) and the API's own answers.
//! What is injected: the confirmer (a script that says no), the speaker (records, plays nothing),
//! the robot link (records). Nothing opens a window, a microphone or a sound device.
//!
//! The test goes on after a mismatch and lists every one at the end, because finding where the
//! real API differs from the stub the other tests use is the point of the run. Nothing secret is
//! printed: the test account is generated at run time and only exists in the local database.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::process::Command;

use moby_protocol::{AgentMessage, AgentTurnRequest, ToolCall};
use reqwest::Method;
use serde_json::{Value, json};

use super::*;
use crate::api::{INSTALLATION_ID, REFRESH_TOKEN, read_json, vault_clear, vault_get};
use crate::paths::testing::{Scratch, helper_exe};
use crate::robot::audio::Pcm;
use crate::voice::play::fake::FakeSpeaker;
use crate::voice::testing::{FakeRobot, STT_OK, tone_wav};
use crate::voice::{Deps, MIN_CONFIDENCE, Stage, Voice, client};

const OTHER_ACCOUNT: &str = "7f3a90c2-1111-4222-8333-444455556666";

// ---- a recording relay between the client and the API ------------------------------------------

#[derive(Clone, Debug)]
struct Seen {
    method: String,
    path: String,
    body: String,
    status: u16,
}

struct Relay {
    url: String,
    seen: Arc<Mutex<Vec<Seen>>>,
}

impl Relay {
    fn all(&self) -> Vec<Seen> {
        self.seen.lock().unwrap().clone()
    }

    fn to(&self, method: &str, path: &str) -> Vec<Seen> {
        self.all()
            .into_iter()
            .filter(|s| s.method == method && s.path.split('?').next() == Some(path))
            .collect()
    }

    fn agent_bodies(&self) -> Vec<Value> {
        self.to("POST", "/v1/agent/turn")
            .iter()
            .map(|s| serde_json::from_str(&s.body).unwrap_or(Value::Null))
            .collect()
    }
}

/// Forwards every request to the real API untouched (one connection per request) and writes down
/// what went out and what status came back. It is how the test sees what the client really sent.
fn relay(upstream: &str) -> Relay {
    let target = upstream
        .trim_start_matches("http://")
        .trim_end_matches('/')
        .to_owned();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
    let seen = Arc::new(Mutex::new(Vec::<Seen>::new()));
    let log = Arc::clone(&seen);
    std::thread::spawn(move || {
        for connection in listener.incoming() {
            let Ok(stream) = connection else { continue };
            let (target, log) = (target.clone(), Arc::clone(&log));
            std::thread::spawn(move || forward(stream, &target, &log));
        }
    });
    Relay { url, seen }
}

fn forward(mut client: TcpStream, target: &str, log: &Mutex<Vec<Seen>>) {
    let mut raw = Vec::new();
    let mut chunk = [0_u8; 16384];
    let (head_end, length) = loop {
        let n = client.read(&mut chunk).unwrap_or(0);
        if n == 0 {
            return;
        }
        raw.extend_from_slice(&chunk[..n]);
        if let Some(pos) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
            let head = String::from_utf8_lossy(&raw[..pos]).to_lowercase();
            let length = head
                .lines()
                .find_map(|l| l.strip_prefix("content-length:"))
                .and_then(|v| v.trim().parse::<usize>().ok())
                .unwrap_or(0);
            break (pos + 4, length);
        }
    };
    while raw.len() < head_end + length {
        let n = client.read(&mut chunk).unwrap_or(0);
        if n == 0 {
            break;
        }
        raw.extend_from_slice(&chunk[..n]);
    }
    let head = String::from_utf8_lossy(&raw[..head_end]).to_string();
    let mut lines = head.split("\r\n");
    let request_line = lines.next().unwrap_or("").to_owned();
    let mut parts = request_line.split_whitespace();
    let (method, path) = (
        parts.next().unwrap_or("").to_owned(),
        parts.next().unwrap_or("").to_owned(),
    );
    let mut out = format!("{request_line}\r\n");
    for line in lines.filter(|l| !l.is_empty()) {
        if !line.to_ascii_lowercase().starts_with("connection:") {
            out.push_str(line);
            out.push_str("\r\n");
        }
    }
    out.push_str("Connection: close\r\n\r\n");
    let Ok(mut upstream) = TcpStream::connect(target) else {
        return;
    };
    let _ = upstream.write_all(out.as_bytes());
    let _ = upstream.write_all(&raw[head_end..]);
    let mut response = Vec::new();
    let _ = upstream.read_to_end(&mut response);
    let status = String::from_utf8_lossy(&response[..response.len().min(16)])
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    log.lock().unwrap().push(Seen {
        method,
        path,
        body: String::from_utf8_lossy(&raw[head_end..]).into_owned(),
        status,
    });
    let _ = client.write_all(&response);
}

// ---- reporting ----------------------------------------------------------------------------------

#[derive(Default)]
struct Report {
    bad: Vec<String>,
}

impl Report {
    fn check(&mut self, ok: bool, what: &str, detail: impl std::fmt::Display) {
        if ok {
            println!("  ok        {what}");
        } else {
            println!("  MISMATCH  {what}: {detail}");
            self.bad.push(format!("{what}: {detail}"));
        }
    }

    fn info(&self, what: &str, detail: impl std::fmt::Display) {
        println!("  info      {what}: {detail}");
    }

    fn step(&self, name: &str) {
        println!("\n== {name}");
    }
}

fn code_of<T>(result: &Result<T, Failure>) -> String {
    match result {
        Ok(_) => "ok".to_owned(),
        Err(failure) => failure.code_str().to_owned(),
    }
}

// ---- helpers ------------------------------------------------------------------------------------

#[derive(Default)]
struct Events(Mutex<Vec<UiEvent>>);

impl Events {
    fn all(&self) -> Vec<UiEvent> {
        self.0.lock().unwrap().clone()
    }

    fn completed(&self) -> Option<(String, Expression)> {
        self.all().into_iter().find_map(|e| match e {
            UiEvent::Completed {
                speech_text,
                expression_hint,
                ..
            } => Some((speech_text, expression_hint)),
            _ => None,
        })
    }

    fn error_code(&self) -> Option<String> {
        self.all().into_iter().find_map(|e| match e {
            UiEvent::Error { error } => Some(error.code),
            _ => None,
        })
    }

    fn tool_results(&self) -> Vec<(&'static str, &'static str)> {
        self.all()
            .into_iter()
            .filter_map(|e| match e {
                UiEvent::ToolResult { tool, outcome, .. } => Some((tool, outcome)),
                _ => None,
            })
            .collect()
    }

    fn speech_deltas(&self) -> usize {
        self.all()
            .iter()
            .filter(|e| matches!(e, UiEvent::SpeechDelta { .. }))
            .count()
    }

    fn prompts(&self) -> usize {
        self.all()
            .iter()
            .filter(|e| {
                matches!(
                    e,
                    UiEvent::ApprovalRequired { .. } | UiEvent::NativePrompt { .. }
                )
            })
            .count()
    }
}

impl Sink for Events {
    fn emit(&self, event: UiEvent) {
        self.0.lock().unwrap().push(event);
    }
}

fn git(dir: &Path, args: &[&str]) {
    let out = Command::new("git")
        .current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", "NUL")
        .env("GIT_CONFIG_SYSTEM", "NUL")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .args([
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@example.com",
            "-c",
            "commit.gpgsign=false",
            "-c",
            "core.autocrlf=false",
        ])
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// The one-time token in the verification mail, read from the local mail catcher.
async fn verification_token(http: &reqwest::Client, mail: &str, email: &str) -> Option<String> {
    for _ in 0..40 {
        let found: Value = http
            .get(format!(
                "{mail}/api/v1/search?query=to%3A{}",
                email.replace('@', "%40")
            ))
            .send()
            .await
            .ok()?
            .json()
            .await
            .ok()?;
        let id = found["messages"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|m| m["Subject"] == "Verify your Moby email")
            .and_then(|m| m["ID"].as_str().map(str::to_owned));
        if let Some(id) = id {
            let message: Value = http
                .get(format!("{mail}/api/v1/message/{id}"))
                .send()
                .await
                .ok()?
                .json()
                .await
                .ok()?;
            let text = message["Text"].as_str()?;
            let at = text.find("#token=")? + "#token=".len();
            let token: String = text[at..].chars().take(64).collect();
            return (token.len() == 64 && token.chars().all(|c| c.is_ascii_hexdigit()))
                .then_some(token);
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    None
}

fn is_uuid(value: &str) -> bool {
    value.len() == 36 && value.chars().all(|c| c.is_ascii_hexdigit() || c == '-')
}

fn local_time_shape(value: &str) -> bool {
    // 2026-11-05T19:03:00+05:00
    let b = value.as_bytes();
    value.len() == 25
        && b[4] == b'-'
        && b[7] == b'-'
        && b[10] == b'T'
        && b[13] == b':'
        && b[16] == b':'
        && (b[19] == b'+' || b[19] == b'-')
        && b[22] == b':'
}

/// Removes the test entries from the Credential Manager, however the test ends.
struct VaultGuard;

impl Drop for VaultGuard {
    fn drop(&mut self) {
        let _ = vault_clear(REFRESH_TOKEN);
        let _ = vault_clear(INSTALLATION_ID);
    }
}

fn wave_for(ms: usize) -> Vec<u8> {
    let pcm: Vec<u8> = (0..(16 * ms) as i16)
        .flat_map(|n| ((n % 50) * 100).to_le_bytes())
        .collect();
    crate::voice::wav::encode(&pcm).expect("a valid clip")
}

fn pcm_for(ms: usize) -> Pcm {
    Pcm::new(
        (0..(16 * ms) as i16)
            .flat_map(|n| ((n % 50) * 100).to_le_bytes())
            .collect(),
    )
}

// ---- the run ------------------------------------------------------------------------------------

#[tokio::test]
#[ignore = "needs a running moby-api with mock providers; see the README"]
async fn real_api_end_to_end() {
    let Ok(api_url) = std::env::var("MOBY_E2E_API_URL") else {
        panic!("set MOBY_E2E_API_URL, for example http://127.0.0.1:8080 (see the README)");
    };
    let mail = std::env::var("MOBY_E2E_MAIL_URL").unwrap_or_else(|_| "http://127.0.0.1:8025".into());
    let mut r = Report::default();
    let _vault = VaultGuard;
    let _ = vault_clear(REFRESH_TOKEN);
    let _ = vault_clear(INSTALLATION_ID);

    let scratch = Scratch::new();
    let store = Store::open(&scratch.path("data"));
    let relay = relay(&api_url);
    let http = reqwest::Client::builder().no_proxy().build().unwrap();
    let confirmer = Arc::new(crate::confirm::testing::Scripted::new(&[false, false]));
    let agent = Agent::with_timeouts(
        Duration::from_secs(30),
        Duration::from_secs(10),
        confirmer.clone(),
    );
    let voice = Voice::new();
    let robot = FakeRobot::default();
    let speaker = FakeSpeaker::default();

    let suffix = Uuid::new_v4().simple().to_string();
    let email = format!("e2e-{suffix}@example.test");
    // Generated now, kept in memory, never printed or written down.
    let password = format!("Pw-{}-{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());

    // ---- account -------------------------------------------------------------------------------
    r.step("register, verify by mail, sign in, restore, sign out");
    let api = Api::new_with(&relay.url);
    let registered = crate::api::register_with(&api, &email, &password).await;
    r.check(registered.is_ok(), "register succeeds", code_of(&registered));
    r.check(
        relay.to("POST", "/v1/auth/register").last().map(|s| s.status) == Some(202),
        "register answers 202 (the stub tests assume any 2xx)",
        format!("{:?}", relay.to("POST", "/v1/auth/register").last().map(|s| s.status)),
    );
    let early = crate::api::sign_in_with(&api, &email, &password).await;
    r.check(
        early.is_err(),
        "sign-in before the mail is confirmed is refused",
        "it succeeded",
    );
    r.info("code for sign-in before confirmation", code_of(&early));

    let token = verification_token(&http, &mail, &email).await;
    r.check(token.is_some(), "verification mail reaches the local mail catcher", "none after 10 s");
    let token = token.expect("no verification mail");
    let verified = http
        .post(format!("{api_url}/v1/auth/verify-email"))
        .json(&json!({"token": token, "password": password}))
        .send()
        .await
        .unwrap();
    r.check(
        verified.status().as_u16() == 204,
        "verify-email answers 204",
        verified.status(),
    );

    let profile = crate::api::sign_in_with(&api, &email, &password)
        .await
        .expect("sign in");
    let account = api.account_id().expect("the account id is kept");
    r.check(is_uuid(&account), "profile.id is a UUID the store accepts", &account);
    r.check(
        profile["email"] == email.as_str(),
        "profile.email is the address that signed up",
        "different",
    );
    let installation = profile["installation_id"].as_str().unwrap_or("").to_owned();
    r.check(
        is_uuid(&installation),
        "profile.installation_id is a UUID",
        &installation,
    );
    let mut keys: Vec<_> = profile.as_object().unwrap().keys().cloned().collect();
    keys.sort();
    r.info("profile fields", keys.join(","));
    r.check(
        vault_get(REFRESH_TOKEN).ok().flatten().is_some()
            && vault_get(INSTALLATION_ID).ok().flatten().as_deref() == Some(installation.as_str()),
        "refresh token and installation id were saved",
        "missing",
    );
    let first_refresh = vault_get(REFRESH_TOKEN).ok().flatten().unwrap_or_default();

    // A fresh client is what a new launch of the app has: no access token, only the vault.
    let api = Api::new_with(&relay.url);
    let restored = crate::api::restore_session_of(&api).await;
    r.check(
        matches!(&restored, Ok(Some(p)) if p["id"] == account.as_str()),
        "restore_session finds the same account from the vault alone",
        code_of(&restored),
    );
    r.check(
        relay.to("POST", "/v1/auth/refresh").last().map(|s| s.status) == Some(200),
        "refresh answers 200",
        format!("{:?}", relay.to("POST", "/v1/auth/refresh").last().map(|s| s.status)),
    );
    let second_refresh = vault_get(REFRESH_TOKEN).ok().flatten().unwrap_or_default();
    r.check(
        !second_refresh.is_empty() && second_refresh != first_refresh,
        "the refresh token rotated and the new one was saved",
        "unchanged",
    );

    let out = crate::api::sign_out_of(&api, &agent, &voice, &robot).await;
    r.check(out.is_ok(), "sign_out succeeds", code_of(&out));
    r.check(
        api.account_id().is_err() && vault_get(REFRESH_TOKEN).ok().flatten().is_none(),
        "after sign-out there is no account and no saved refresh token",
        "still signed in",
    );
    r.check(
        relay.to("POST", "/v1/auth/logout").last().map(|s| s.status) == Some(204),
        "logout answers 204",
        format!("{:?}", relay.to("POST", "/v1/auth/logout").last().map(|s| s.status)),
    );
    let stale = http
        .post(format!("{api_url}/v1/auth/refresh"))
        .json(&json!({"refresh_token": second_refresh}))
        .send()
        .await
        .unwrap();
    r.check(
        stale.status().as_u16() == 401,
        "the signed-out refresh token is dead on the server (401)",
        stale.status(),
    );
    let none = crate::api::restore_session_of(&Api::new_with(&relay.url)).await;
    r.check(matches!(none, Ok(None)), "restore_session after sign-out is None", code_of(&none));

    let api = Api::new_with(&relay.url);
    let again = crate::api::sign_in_with(&api, &email, &password)
        .await
        .expect("sign in again");
    r.check(
        again["installation_id"] == installation.as_str(),
        "signing in again reuses the saved installation id",
        "a new installation was created",
    );

    let usage = read_json(api.send(Method::GET, "/v1/usage", None).await.expect("usage")).await;
    match &usage {
        Ok(usage) => {
            r.check(
                usage["llm_tokens_remaining"].is_number() && usage["reset_at"].is_string(),
                "usage has the two fields the UI reads",
                usage,
            );
            let mut keys: Vec<_> = usage.as_object().unwrap().keys().cloned().collect();
            keys.sort();
            r.info("usage fields", keys.join(","));
        }
        Err(failure) => r.check(false, "usage", failure.code_str()),
    }

    let user = account.clone();
    let epoch = api.epoch();

    // ---- a text turn -----------------------------------------------------------------------------
    r.step("a text turn");
    let events = Events::default();
    let done = agent
        .run_turn(&api, &store, &events, TurnInput::for_test("hello"))
        .await;
    r.check(done.is_ok(), "the turn runs", code_of(&done));
    r.check(
        events.speech_deltas() >= 1,
        "speech_delta frames arrive and parse",
        events.speech_deltas(),
    );
    let completed = events.completed();
    r.check(
        completed
            .as_ref()
            .is_some_and(|(text, hint)| text == "Hello! Moby is running in test mode." && *hint == Expression::Happy),
        "the turn completes with the mock's English greeting and a happy face",
        format!("{completed:?} / error {:?}", events.error_code()),
    );
    let first = relay.agent_bodies();
    let sent = first.first().cloned().unwrap_or(Value::Null);
    let tools: Vec<&str> = sent["available_tools"]
        .as_array()
        .map(|a| a.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    r.check(
        tools == ["create_reminder"],
        "with nothing registered only create_reminder is offered",
        format!("{tools:?}"),
    );
    r.check(
        sent["language"] == "en" && sent.get("script").is_none_or(Value::is_null),
        "English sends language en and no script",
        format!("{} / {}", sent["language"], sent["script"]),
    );

    // ---- local_time --------------------------------------------------------------------------------
    r.step("local_time");
    let stamp = sent["local_time"].as_str().unwrap_or("").to_owned();
    r.check(
        local_time_shape(&stamp),
        "the request carries local_time as YYYY-MM-DDThh:mm:ss+hh:mm",
        &stamp,
    );
    r.check(
        relay.to("POST", "/v1/agent/turn").last().map(|s| s.status) == Some(200),
        "the real API accepts the request that carries local_time (200, not 400)",
        format!("{:?}", relay.to("POST", "/v1/agent/turn").last().map(|s| s.status)),
    );
    let probe = |value: Value| {
        let api = &api;
        async move {
            let body = json!({
                "messages": [{"kind": "user", "text": "hello"}],
                "available_tools": [], "resources": [],
                "language": "en", "local_time": value,
            });
            let response = api
                .send(Method::POST, "/v1/agent/turn", Some(&body))
                .await
                .expect("probe");
            let status = response.status().as_u16();
            let _ = response.bytes().await;
            status
        }
    };
    for good in [
        "2026-11-05T19:03:00+05:00",
        "2026-11-05T19:03:00-03:30",
        "2026-11-05T14:03:00+00:00",
    ] {
        let status = probe(json!(good)).await;
        r.check(status == 200, "API accepts the offsets this app can produce", format!("{good} -> {status}"));
    }
    let status = probe(json!("yesterday")).await;
    r.info("API answer to a malformed local_time", status);
    r.check(status == 400, "a malformed local_time is rejected with 400", status);

    // ---- git status through the tool loop ------------------------------------------------------------
    r.step("the mock's git status flow with a registered test repo");
    let repo = scratch.dir(&format!("repo-{suffix}"));
    git(&repo, &["init", "-q", "-b", "main"]);
    std::fs::write(repo.join("a.txt"), "one\n").unwrap();
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-q", "-m", "first"]);
    std::fs::write(repo.join("a.txt"), "two\n").unwrap();
    std::fs::write(repo.join("untracked-e2e.txt"), "n").unwrap();
    let dir = crate::paths::register_dir(repo.to_str().unwrap()).unwrap();
    let project = store
        .with(|db| db.add_project(&user, "demo", &dir.path, now_ms()))
        .unwrap();
    store
        .with(|db| db.grant_read(&user, "git_status", &project.id, None, now_ms()))
        .unwrap();

    let before = relay.agent_bodies().len();
    let events = Events::default();
    let done = agent
        .run_turn(&api, &store, &events, TurnInput::for_test("git status"))
        .await;
    r.check(done.is_ok(), "the turn runs", code_of(&done));
    r.check(
        events.tool_results() == [("git_status", "ok")],
        "the git_status tool ran and succeeded",
        format!("{:?} / error {:?}", events.tool_results(), events.error_code()),
    );
    r.check(
        events.prompts() == 0,
        "a granted read tool asks nobody",
        events.prompts(),
    );
    r.check(
        events
            .completed()
            .is_some_and(|(text, _)| text == "The tool result was received."),
        "the turn ends with the mock's tool-result answer",
        format!("{:?}", events.completed()),
    );
    let bodies = relay.agent_bodies();
    r.check(
        bodies.len() == before + 2,
        "two requests: the question, then the history with the result",
        bodies.len() - before,
    );
    let (ask, back) = (
        bodies.get(before).cloned().unwrap_or(Value::Null),
        bodies.get(before + 1).cloned().unwrap_or(Value::Null),
    );
    let offered: Vec<&str> = ask["available_tools"]
        .as_array()
        .map(|a| a.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    r.check(
        offered.contains(&"git_status") && offered.contains(&"open_project"),
        "git_status and open_project are offered for the registered project",
        format!("{offered:?}"),
    );
    let resource = &ask["resources"][0];
    r.check(
        resource["kind"] == "project" && resource["display_name"] == "demo",
        "the project goes out as an id and a display name only",
        resource,
    );
    let history = back["messages"].as_array().cloned().unwrap_or_default();
    let kinds: Vec<&str> = history.iter().filter_map(|m| m["kind"].as_str()).collect();
    r.check(
        kinds == ["user", "tool_call", "tool_result"],
        "the second request is user, tool_call, tool_result",
        format!("{kinds:?}"),
    );
    r.check(
        history.get(1).map(|m| m["call"]["call_id"].clone()) == history.get(2).map(|m| m["result"]["call_id"].clone()),
        "the result carries the call_id the API chose",
        "different",
    );
    r.check(
        history.get(2).is_some_and(|m| m["result"]["status"] == "ok"),
        "the result is status ok",
        history.get(2).cloned().unwrap_or(Value::Null),
    );
    let output = history
        .get(2)
        .and_then(|m| m["result"]["output"].as_str())
        .unwrap_or("");
    r.check(
        output.contains("untracked-e2e.txt") && output.contains("main"),
        "the output names the branch and the changed file, relative to the repo",
        output.len(),
    );
    let base = scratch
        .0
        .file_name()
        .map_or_else(|| "moby-paths".to_owned(), |n| n.to_string_lossy().to_lowercase());
    let leaked: Vec<String> = relay
        .all()
        .iter()
        .filter(|s| s.path.starts_with("/v1/agent"))
        .filter(|s| {
            let body = s.body.to_lowercase();
            body.contains(&format!("repo-{suffix}"))
                || body.contains(&base)
                || body.contains("\\\\") // an escaped Windows path
                || body.contains(":/")
        })
        .map(|s| s.path.clone())
        .collect();
    r.check(
        leaked.is_empty(),
        "no agent request contains the repo path, the scratch folder or any drive path",
        format!("{leaked:?}"),
    );
    let audit = store.with(|db| db.audit_list(&user)).unwrap();
    r.check(
        audit
            .iter()
            .any(|a| a.tool == "git_status" && a.result_code.as_deref() == Some("ok")),
        "the activity log has the git_status run",
        audit.len(),
    );

    // ---- a denied launch -------------------------------------------------------------------------------
    r.step("a denied launch through the injected confirmer");
    println!("  note      the mock only asks for git_status, so the launch call is built here and sent through the real Agent::handle_call; the API is then given the history with the denial");
    let exe_path = scratch.path("tool.exe");
    std::fs::copy(helper_exe(), &exe_path).unwrap();
    let exe = exe_path.to_str().unwrap().to_owned();
    let program = crate::paths::register_program(&exe).unwrap();
    let app = store
        .with(|db| db.add_app(&user, "Tool", &program.path, now_ms()))
        .unwrap();
    let offer = build_offer(&store, &agent.sessions, &user, epoch, now_ms()).unwrap();
    let call = ToolCall {
        call_id: StableId::new("e2e-open-1").unwrap(),
        request: ToolRequest::OpenApp {
            app_id: StableId::new(app.id.clone()).unwrap(),
        },
    };
    r.check(
        offer.allows(&call.request),
        "the offer for this turn allows open_app for the registered program",
        "not offered",
    );
    let events = Events::default();
    let env = Env {
        api: &api,
        store: &store,
        sink: &events,
        user: user.clone(),
        epoch,
        cancel: Cancel::new(),
    };
    let result = agent.handle_call(&env, &call, None).await;
    r.check(
        matches!(result, ToolResult::Error { code: ToolFailureCode::Denied, .. }),
        "the denial comes back as an error result with code denied",
        format!("{result:?}"),
    );
    r.check(
        confirmer.shown().len() == 1,
        "the confirmer was asked exactly once",
        confirmer.shown().len(),
    );
    tokio::time::sleep(Duration::from_millis(600)).await;
    r.check(
        !Path::new(&format!("{exe}.ran")).exists(),
        "the marker file is absent: the program never ran",
        "it ran",
    );
    r.check(
        events.tool_results() == [("open_app", "denied")],
        "the page is told: denied",
        format!("{:?}", events.tool_results()),
    );
    let audit = store.with(|db| db.audit_list(&user)).unwrap();
    r.check(
        audit
            .iter()
            .any(|a| a.tool == "open_app" && a.decision == "denied"),
        "the activity log records the denial",
        audit.len(),
    );
    let request = AgentTurnRequest {
        messages: vec![
            AgentMessage::User { text: "open the tool".into() },
            AgentMessage::ToolCall { call },
            AgentMessage::ToolResult { result },
        ],
        available_tools: offer.tools.clone(),
        resources: offer.resources.clone(),
        language: Some(Language::En),
        script: None,
        local_time: local_time(),
    };
    let body = serde_json::to_value(&request).unwrap();
    let stream = open_agent_stream(&api, &body).await;
    r.check(stream.is_ok(), "the API accepts a history with an open_app call and a denied result", code_of(&stream));
    if let Ok(response) = stream {
        let mut frames = Frames::new(response);
        let mut last = None;
        while let Ok(Some(frame)) = frames.next().await {
            last = serde_json::from_str::<AgentTurnResponse>(&frame).ok();
        }
        r.check(
            matches!(&last, Some(AgentTurnResponse::Completed { speech_text, expression_hint, .. })
                if speech_text == "The tool could not complete the action." && *expression_hint == Expression::Confused),
            "the mock answers a denied result with its failure text and a confused face",
            format!("{last:?}"),
        );
    }

    // ---- voice ------------------------------------------------------------------------------------------
    r.step("voice: consent, speech to text, text to speech, revoke, refusal");
    let status = |p: &str| relay.to("POST", p).last().map(|s| s.status);
    r.check(
        voice.require_consent(&api, &store).is_err(),
        "no consent on this computer yet",
        "consent present",
    );
    let refused = client::transcribe(&api, tone_wav(), Duration::from_secs(20)).await;
    r.check(
        code_of(&refused) == "voice_consent_required" && status("/v1/voice/stt") == Some(403),
        "without consent the server refuses speech to text (403 voice_consent_required)",
        format!("{} / {:?}", code_of(&refused), status("/v1/voice/stt")),
    );
    let refused = client::synthesize(&api, "Hello", Language::En, Script::Latn, Duration::from_secs(20)).await;
    r.check(
        code_of(&refused) == "voice_consent_required" && status("/v1/voice/tts") == Some(403),
        "without consent the server refuses text to speech (403)",
        format!("{} / {:?}", code_of(&refused), status("/v1/voice/tts")),
    );

    let accepted = voice.accept_consent(&api, &store).await;
    r.check(
        matches!(&accepted, Ok(c) if c.accepted && c.version.as_deref() == Some("dev-1")),
        "accept_consent records version dev-1 (read from GET /v1/consents)",
        format!("{accepted:?}"),
    );
    r.info("POST /v1/consents status (the stub answers 204)", format!("{:?}", status("/v1/consents")));
    r.check(
        status("/v1/consents") == Some(204),
        "POST /v1/consents answers 204 like the stub",
        format!("{:?}", status("/v1/consents")),
    );
    r.check(
        relay.to("GET", "/v1/consents").last().map(|s| s.status) == Some(200),
        "GET /v1/consents answers 200 with current_versions.cloud_voice",
        "not 200",
    );

    let heard = client::transcribe(&api, wave_for(1000), Duration::from_secs(20)).await;
    match &heard {
        Ok(t) => {
            r.info(
                "mock transcript",
                format!("{:?} {:?}/{:?} confidence {}", t.text, t.language, t.script, t.confidence),
            );
            let stub: Value = serde_json::from_str(STT_OK).unwrap();
            r.check(
                t.text == stub["text"].as_str().unwrap()
                    && t.confidence == stub["confidence"].as_f64().unwrap() as f32,
                "the mock's transcript equals the stub's (text and confidence)",
                format!("{:?} {}", t.text, t.confidence),
            );
            r.check(
                t.confidence >= MIN_CONFIDENCE && !t.text.is_empty(),
                "the mock's confidence passes the chain's threshold",
                t.confidence,
            );
        }
        Err(failure) => r.check(false, "speech to text", failure.code_str()),
    }
    let spoken = client::synthesize(&api, "Salom, Mobi.", Language::Uz, Script::Latn, Duration::from_secs(20)).await;
    match &spoken {
        Ok(bytes) => {
            let samples = crate::voice::wav::decode(bytes);
            r.check(
                samples.as_ref().is_ok_and(|s| !s.is_empty()),
                "the mock's WAV decodes with this app's own decoder",
                format!("{} bytes, {:?}", bytes.len(), samples.as_ref().map(Vec::len)),
            );
            r.info("mock TTS", format!("{} bytes, {:?} samples", bytes.len(), samples.map(|s| s.len())));
        }
        Err(failure) => r.check(false, "text to speech", failure.code_str()),
    }

    // The whole chain: audio in, transcript to the page, agent turn, speech out, playback.
    let stt_before = relay.to("POST", "/v1/voice/stt").len();
    let events = Arc::new(Events::default());
    let turn = voice.start_turn();
    let deps = Deps {
        api: &api,
        store: &store,
        agent: &agent,
        speaker: &speaker,
        robot: &robot,
    };
    let page = async {
        let until = Instant::now() + Duration::from_secs(15);
        while Instant::now() < until {
            if let Some(pending) = voice.view().pending {
                let sink: Arc<dyn Sink> = events.clone();
                let _ = voice
                    .continue_turn(&pending.id, TurnInput::for_test(&pending.text), sink)
                    .await;
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    };
    tokio::join!(voice.run_turn(&deps, turn, pcm_for(1000)), page);
    let view = voice.view();
    r.check(
        view.stage == Stage::Idle && view.code.is_none(),
        "the voice turn ends idle without an error",
        format!("{:?} {:?} {:?}", view.stage, view.code, view.message),
    );
    r.check(
        events.completed().is_some() && speaker.played.lock().unwrap().len() == 1,
        "the answer was spoken: one clip reached the (fake) speaker",
        format!("completed {:?}, clips {}", events.completed(), speaker.played.lock().unwrap().len()),
    );
    r.check(
        relay.to("POST", "/v1/voice/stt").len() == stt_before + 1,
        "exactly one speech-to-text upload for the turn",
        relay.to("POST", "/v1/voice/stt").len() - stt_before,
    );
    r.info("answer the mock gave to the transcript", format!("{:?}", events.completed().map(|c| c.0)));

    let revoked = voice.revoke_consent(&api, &store, &robot).await;
    r.check(
        matches!(&revoked, Ok(c) if !c.accepted),
        "revoke_consent turns voice off here and on the server",
        format!("{revoked:?}"),
    );
    let consents: Value = read_json(
        api.send(Method::GET, "/v1/consents", None).await.unwrap(),
    )
    .await
    .unwrap_or(Value::Null);
    r.check(
        consents["records"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|c| c["type"] == "cloud_voice" && !c["revoked_at"].is_null()),
        "the server shows the cloud_voice record as revoked",
        consents["records"].to_string().len(),
    );
    let stt_before = relay.to("POST", "/v1/voice/stt").len();
    let turn = voice.start_turn();
    voice.run_turn(&deps, turn, pcm_for(1000)).await;
    r.check(
        relay.to("POST", "/v1/voice/stt").len() == stt_before
            && voice.view().code.as_deref() == Some("voice_consent_required"),
        "after revoking, the chain refuses locally and nothing is uploaded",
        format!("{:?}", voice.view().code),
    );
    let refused = client::transcribe(&api, tone_wav(), Duration::from_secs(20)).await;
    r.check(
        code_of(&refused) == "voice_consent_required" && status("/v1/voice/stt") == Some(403),
        "an upload that does get sent after revoking is refused by the server (403)",
        format!("{} / {:?}", code_of(&refused), status("/v1/voice/stt")),
    );

    // ---- deleting the account ----------------------------------------------------------------------------
    r.step("account deletion clears this account's local data, and only this account's");
    let conversation = store
        .with(|db| db.create_conversation(&user, "e2e", now_ms()))
        .unwrap();
    store
        .with(|db| {
            db.append_message(
                &user,
                &conversation.id,
                &serde_json::from_value(json!({
                    "id": Uuid::new_v4().hyphenated().to_string(),
                    "role": "user",
                    "text": "hello",
                }))
                .unwrap(),
                now_ms(),
            )
        })
        .unwrap();
    let other = store
        .with(|db| db.create_conversation(OTHER_ACCOUNT, "someone else", now_ms()))
        .unwrap();
    let counts = |who: &str| {
        store
            .with(|db| {
                Ok((
                    db.list_conversations(who)?.len(),
                    db.projects(who)?.len(),
                    db.apps(who)?.len(),
                    db.audit_list(who)?.len(),
                    db.grants(who, now_ms())?.len(),
                ))
            })
            .unwrap()
    };
    r.info("local rows before (conversations, projects, apps, activity, grants)", format!("{:?}", counts(&user)));

    let wrong = crate::api::delete_account_of(&api, &store, &agent, &voice, &robot, "not the password").await;
    r.info("delete with a wrong password: code", code_of(&wrong));
    r.check(
        code_of(&wrong) == "wrong_password",
        "a wrong password gives wrong_password",
        code_of(&wrong),
    );
    r.info("DELETE /v1/me status for a wrong password", format!("{:?}", relay.to("DELETE", "/v1/me").last().map(|s| s.status)));
    r.check(
        api.account_id().is_ok() && counts(&user).0 == 1 && counts(&user).1 == 1,
        "a refused deletion keeps the session and the local data",
        format!("{:?}", counts(&user)),
    );

    let deleted = crate::api::delete_account_of(&api, &store, &agent, &voice, &robot, &password).await;
    r.check(deleted.is_ok(), "deleting with the right password succeeds", code_of(&deleted));
    r.check(
        relay.to("DELETE", "/v1/me").last().map(|s| s.status) == Some(204),
        "DELETE /v1/me answers 204",
        format!("{:?}", relay.to("DELETE", "/v1/me").last().map(|s| s.status)),
    );
    r.check(
        counts(&user) == (0, 0, 0, 0, 0)
            && store.with(|db| db.voice_consent()).unwrap().is_none(),
        "this account's history, folders, programs, activity, grants and voice consent are gone",
        format!("{:?}", counts(&user)),
    );
    r.check(
        counts(OTHER_ACCOUNT).0 == 1 && other.id.len() > 8,
        "another account's history is untouched",
        format!("{:?}", counts(OTHER_ACCOUNT)),
    );
    r.check(
        api.account_id().is_err() && vault_get(REFRESH_TOKEN).ok().flatten().is_none(),
        "the session ended and the saved refresh token is gone",
        "still signed in",
    );
    let login = http
        .post(format!("{api_url}/v1/auth/login"))
        .json(&json!({"email": email, "password": password}))
        .send()
        .await
        .unwrap();
    r.check(
        login.status().as_u16() == 401,
        "the deleted account can no longer sign in (401)",
        login.status(),
    );

    // ---- result --------------------------------------------------------------------------------------------
    println!("\n== result");
    if r.bad.is_empty() {
        println!("  the real API matched everything the stub tests assume");
    } else {
        for line in &r.bad {
            println!("  MISMATCH  {line}");
        }
    }
    assert!(r.bad.is_empty(), "{} mismatch(es), listed above", r.bad.len());
}
