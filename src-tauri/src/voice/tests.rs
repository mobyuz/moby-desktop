//! The voice chain against a stub moby-api on this computer, a recording robot link and a fake
//! speaker. Nothing here uses a real server, a speech provider, a robot or a sound device.

use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use serde_json::json;

use super::play::fake::FakeSpeaker;
use super::testing::{ANSWER, FakeRobot, Reply, Stub, Views};
use super::*;
use crate::agent::{Agent, Sink, TurnInput, UiEvent};
use crate::confirm::testing::Scripted;
use crate::paths::testing::{Scratch, helper_exe};
use crate::robot::audio::{AbortReason, AudioEvent, Pcm};
use crate::robot::sim::AUDIO_MARKER;

const USER: &str = "alice";
const TRANSCRIPT: &str = "git holatini ayt";

#[derive(Default)]
struct Events(Mutex<Vec<UiEvent>>);

impl Sink for Events {
    fn emit(&self, event: UiEvent) {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).push(event);
    }
}

struct World {
    scratch: Scratch,
    store: Store,
    api: Api,
    agent: Agent,
    stub: Stub,
    robot: FakeRobot,
    speaker: FakeSpeaker,
    voice: Voice,
    views: Arc<Views>,
    events: Arc<Events>,
    logs: Arc<Mutex<Vec<String>>>,
    confirmer: Arc<Scripted>,
}

fn marker_pcm(ms: usize) -> Pcm {
    Pcm::new(
        AUDIO_MARKER
            .iter()
            .copied()
            .cycle()
            .take(ms * 32)
            .collect(),
    )
}

impl World {
    fn new() -> Self {
        Self::build(true, Limits::default(), Duration::from_secs(30), &[false])
    }

    fn build(consent: bool, limits: Limits, approval: Duration, answers: &[bool]) -> Self {
        let scratch = Scratch::new();
        let store = Store::open(&scratch.path("data"));
        if consent {
            store
                .with(|db| db.set_voice_consent(Some((USER, "dev-1")), now_ms()))
                .unwrap();
        }
        let stub = Stub::new();
        let api = Api::signed_in_for_test(&stub.url, USER);
        let confirmer = Arc::new(Scripted::new(answers));
        let agent = Agent::with_timeouts(approval, Duration::from_secs(10), confirmer.clone());
        let logs = Arc::new(Mutex::new(Vec::new()));
        let voice = Voice::with_limits(limits).with_log({
            let logs = Arc::clone(&logs);
            Box::new(move |line| logs.lock().unwrap().push(line.to_owned()))
        });
        let views = Arc::new(Views::default());
        voice.subscribe(Box::new(Arc::clone(&views)));
        Self {
            scratch,
            store,
            api,
            agent,
            stub,
            robot: FakeRobot::default(),
            speaker: FakeSpeaker::default(),
            voice,
            views,
            events: Arc::new(Events::default()),
            logs,
            confirmer,
        }
    }

    fn deps(&self) -> Deps<'_> {
        Deps {
            api: &self.api,
            store: &self.store,
            agent: &self.agent,
            speaker: &self.speaker,
            robot: &self.robot,
        }
    }

    /// What the page does: show the words, then continue the turn with them.
    async fn page(&self) {
        let until = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(pending) = self.voice.view().pending {
                let sink: Arc<dyn Sink> = self.events.clone();
                let _ = self
                    .voice
                    .continue_turn(&pending.id, TurnInput::for_test(&pending.text), sink)
                    .await;
                return;
            }
            if Instant::now() > until || self.voice.view().stage == Stage::Idle && self.voice.view().code.is_some() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }

    /// A whole turn with a page that answers.
    async fn talk(&self) {
        let turn = self.voice.start_turn();
        let deps = self.deps();
        tokio::join!(self.voice.run_turn(&deps, turn, marker_pcm(1000)), self.page());
    }

    /// Starts a turn, runs `act` once `ready` is true, then waits for the turn to end.
    async fn talk_and(&self, with_page: bool, act: impl std::future::Future<Output = ()>) {
        let turn = self.voice.start_turn();
        let page = async {
            if with_page {
                self.page().await;
            }
        };
        let deps = self.deps();
        tokio::join!(self.voice.run_turn(&deps, turn, marker_pcm(1000)), page, act);
    }

    fn code(&self) -> Option<String> {
        self.voice.view().code
    }
}

async fn wait_stage(world: &World, stage: Stage) {
    let until = Instant::now() + Duration::from_secs(10);
    while world.voice.view().stage != stage {
        assert!(Instant::now() < until, "never reached {stage:?}");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

// ---- happy path and consent ------------------------------------------------------------

#[tokio::test]
async fn the_happy_path_goes_through_every_stage_and_plays_the_answer() {
    let world = World::new();
    world.talk().await;
    let stt = world.stub.to("/v1/voice/stt");
    assert_eq!(stt.len(), 1);
    assert!(stt[0].body.starts_with(b"RIFF"), "a WAV goes up");
    assert!(stt[0].body.windows(8).any(|w| w == AUDIO_MARKER));
    assert_eq!(stt[0].body.len(), 44 + 32_000);
    let agent = world.stub.to("/v1/agent/turn");
    assert_eq!(agent.len(), 1);
    assert!(String::from_utf8_lossy(&agent[0].body).contains(TRANSCRIPT));
    let tts = world.stub.to("/v1/voice/tts");
    assert_eq!(tts.len(), 1);
    let body: serde_json::Value = serde_json::from_slice(&tts[0].body).unwrap();
    assert_eq!(body["text"], "Hammasi joyida.");
    assert_eq!(body["language"], "uz");
    assert_eq!(body["script"], "latn");
    assert_eq!(world.speaker.played.lock().unwrap().len(), 1);
    assert_eq!(
        world.views.stages(),
        [
            Stage::Idle,
            Stage::Transcribing,
            Stage::AwaitingTurn,
            Stage::Thinking,
            Stage::Synthesizing,
            Stage::Speaking,
            Stage::Idle
        ]
    );
    assert_eq!(
        world.robot.faces(),
        [Expression::Thinking, Expression::Speaking, Expression::Happy]
    );
    // The words reached the page; Rust itself wrote no history.
    assert!(world.events.0.lock().unwrap().iter().any(|e| matches!(e, UiEvent::Completed { .. })));
}

#[tokio::test]
async fn without_consent_nothing_is_uploaded_and_the_reason_is_shown() {
    let world = World::build(false, Limits::default(), Duration::from_secs(30), &[]);
    world.talk().await;
    assert!(world.stub.requests().is_empty(), "no request of any kind");
    assert_eq!(world.code().as_deref(), Some("voice_consent_required"));
    assert!(world.voice.view().message.unwrap().contains("Voice is off"));
    // A stream that starts without consent is cut at the robot at once.
    world.voice.on_capture_started(&world.deps());
    assert_eq!(world.robot.cancels(), 1);
    assert_eq!(world.voice.view().stage, Stage::Idle);
}

#[tokio::test]
async fn consent_of_another_account_does_not_count() {
    let world = World::build(false, Limits::default(), Duration::from_secs(30), &[]);
    world
        .store
        .with(|db| db.set_voice_consent(Some(("bob", "dev-1")), now_ms()))
        .unwrap();
    world.talk().await;
    assert!(world.stub.requests().is_empty());
}

#[tokio::test]
async fn accepting_records_the_version_on_the_server_and_here_and_revoking_is_immediate() {
    let world = World::build(false, Limits::default(), Duration::from_secs(30), &[]);
    assert!(!world.voice.consent_status(&world.api, &world.store).unwrap().accepted);
    let status = world.voice.accept_consent(&world.api, &world.store).await.unwrap();
    assert_eq!(status.version.as_deref(), Some("dev-1"));
    let posts = world.stub.to("/v1/consents");
    let sent: serde_json::Value = serde_json::from_slice(&posts[1].body).unwrap();
    assert_eq!(sent, json!({"type":"cloud_voice","version":"dev-1","accepted":true}));

    world.talk().await;
    assert_eq!(world.stub.to("/v1/voice/stt").len(), 1);

    let status = world
        .voice
        .revoke_consent(&world.api, &world.store, &world.robot)
        .await
        .unwrap();
    assert!(!status.accepted);
    let last = world.stub.to("/v1/consents").pop().unwrap();
    let sent: serde_json::Value = serde_json::from_slice(&last.body).unwrap();
    assert_eq!(sent["accepted"], false);
    world.talk().await;
    assert_eq!(world.stub.to("/v1/voice/stt").len(), 1, "nothing more goes up");
}

#[tokio::test]
async fn a_refused_acceptance_stores_nothing() {
    let world = World::build(false, Limits::default(), Duration::from_secs(30), &[]);
    world.stub.set(|c| {
        c.consents_post = Reply::Json(400, r#"{"code":"invalid_input","message":"x","request_id":"r"}"#.into());
    });
    assert!(world.voice.accept_consent(&world.api, &world.store).await.is_err());
    assert!(!world.voice.consent_status(&world.api, &world.store).unwrap().accepted);
}

#[tokio::test]
async fn revoking_during_speech_to_text_drops_the_request_and_nothing_else_is_sent() {
    let world = World::new();
    world.stub.set(|c| c.stt = Reply::Hang);
    world
        .talk_and(false, async {
            world.stub.wait_for("/v1/voice/stt", 1).await;
            world
                .voice
                .revoke_consent(&world.api, &world.store, &world.robot)
                .await
                .unwrap();
        })
        .await;
    world.stub.wait_dropped("/v1/voice/stt", 0).await;
    assert!(world.stub.to("/v1/voice/tts").is_empty());
    assert!(world.stub.to("/v1/agent/turn").is_empty());
    assert!(world.speaker.played.lock().unwrap().is_empty());
}

#[tokio::test]
async fn consent_withdrawn_while_the_agent_works_stops_the_answer_from_being_spoken() {
    let world = World::new();
    world.stub.set(|c| c.agent = vec![Reply::Sse(vec![ANSWER.to_owned()])]);
    world
        .store
        .with(|db| db.set_voice_consent(Some((USER, "dev-1")), now_ms()))
        .unwrap();
    let turn = world.voice.start_turn();
    let revoker = async {
        wait_stage(&world, Stage::AwaitingTurn).await;
        world.store.with(|db| db.set_voice_consent(None, now_ms())).unwrap();
    };
    let deps = world.deps();
    tokio::join!(world.voice.run_turn(&deps, turn, marker_pcm(1000)), world.page(), revoker);
    assert!(world.stub.to("/v1/voice/tts").is_empty());
}

// ---- cancel at each stage ---------------------------------------------------------------

#[tokio::test]
async fn cancel_while_listening_stops_the_capture_at_the_robot() {
    let world = World::new();
    world.voice.on_capture_started(&world.deps());
    assert_eq!(world.voice.view().stage, Stage::Listening);
    world.voice.cancel(&world.robot);
    assert_eq!(world.robot.cancels(), 1);
    assert_eq!(world.voice.view().stage, Stage::Idle);
    assert_eq!(world.code().as_deref(), Some("canceled"));
}

#[tokio::test]
async fn cancel_during_speech_to_text_drops_the_request() {
    let world = World::new();
    world.stub.set(|c| c.stt = Reply::Hang);
    world
        .talk_and(false, async {
            world.stub.wait_for("/v1/voice/stt", 1).await;
            world.voice.cancel(&world.robot);
        })
        .await;
    world.stub.wait_dropped("/v1/voice/stt", 0).await;
    assert!(world.stub.to("/v1/agent/turn").is_empty());
    assert_eq!(world.voice.view().stage, Stage::Idle);
}

#[tokio::test]
async fn cancel_after_the_words_are_shown_never_starts_the_agent() {
    let world = World::new();
    world
        .talk_and(false, async {
            wait_stage(&world, Stage::AwaitingTurn).await;
            world.voice.cancel(&world.robot);
        })
        .await;
    assert!(world.stub.to("/v1/agent/turn").is_empty());
    assert!(world.stub.to("/v1/voice/tts").is_empty());
    assert_eq!(world.voice.view().stage, Stage::Idle);
}

#[tokio::test]
async fn cancel_during_the_agent_turn_ends_it_and_nothing_is_spoken() {
    let world = World::new();
    world.stub.set(|c| c.agent = vec![Reply::Hang]);
    world
        .talk_and(true, async {
            world.stub.wait_for("/v1/agent/turn", 1).await;
            world.voice.cancel(&world.robot);
        })
        .await;
    world.stub.wait_dropped("/v1/agent/turn", 0).await;
    assert!(world.stub.to("/v1/voice/tts").is_empty());
    assert!(world.speaker.played.lock().unwrap().is_empty());
}

#[tokio::test]
async fn cancel_during_text_to_speech_drops_the_request() {
    let world = World::new();
    world.stub.set(|c| c.tts = Reply::Hang);
    world
        .talk_and(true, async {
            world.stub.wait_for("/v1/voice/tts", 1).await;
            world.voice.cancel(&world.robot);
        })
        .await;
    world.stub.wait_dropped("/v1/voice/tts", 0).await;
    assert!(world.speaker.played.lock().unwrap().is_empty());
}

#[tokio::test]
async fn cancel_during_playback_stops_the_sound() {
    let mut world = World::new();
    world.speaker.duration = Duration::from_secs(30);
    let started = Instant::now();
    world
        .talk_and(true, async {
            wait_stage(&world, Stage::Speaking).await;
            world.voice.cancel(&world.robot);
        })
        .await;
    assert!(started.elapsed() < Duration::from_secs(10));
    assert_eq!(*world.speaker.stopped.lock().unwrap(), 1);
    assert_eq!(world.voice.view().stage, Stage::Idle);
    world.speaker.duration = Duration::ZERO;
}

#[tokio::test]
async fn a_new_button_press_cancels_the_turn_that_is_running() {
    let world = World::new();
    world.stub.set(|c| c.stt = Reply::Hang);
    world
        .talk_and(false, async {
            world.stub.wait_for("/v1/voice/stt", 1).await;
            world.voice.on_capture_started(&world.deps());
        })
        .await;
    world.stub.wait_dropped("/v1/voice/stt", 0).await;
    assert_eq!(world.voice.view().stage, Stage::Listening);
}

// ---- errors and timeouts ----------------------------------------------------------------

#[tokio::test]
async fn speech_to_text_errors_are_shown_in_the_apps_words() {
    for (status, code) in [
        (429, "voice_quota"),
        (503, "voice_unavailable"),
        (502, "voice_provider"),
        (504, "voice_provider"),
        (413, "voice_too_long"),
        (500, "voice_failed"),
    ] {
        let world = World::new();
        let body = r#"{"code":"provider_secret_sk-123","message":"leaky","request_id":"r"}"#;
        world.stub.set(|c| c.stt = Reply::Json(status, body.to_owned()));
        world.talk().await;
        assert_eq!(world.code().as_deref(), Some(code), "{status}");
        assert_eq!(world.voice.view().stage, Stage::Error);
        let message = world.voice.view().message.unwrap();
        assert!(!message.contains("leaky") && !message.contains("sk-123"));
        assert!(world.robot.faces().contains(&Expression::Error));
        assert!(world.stub.to("/v1/agent/turn").is_empty());
    }
}

#[tokio::test]
async fn a_403_from_the_server_turns_voice_off_here() {
    let world = World::new();
    world.stub.set(|c| {
        c.stt = Reply::Json(403, r#"{"code":"voice_consent_required","message":"m","request_id":"r"}"#.into());
    });
    world.talk().await;
    assert_eq!(world.code().as_deref(), Some("voice_consent_required"));
    assert!(!world.voice.consent_status(&world.api, &world.store).unwrap().accepted);
}

#[tokio::test]
async fn timeouts_end_the_stage_and_drop_the_request() {
    let limits = Limits {
        stt_timeout: Duration::from_millis(250),
        tts_timeout: Duration::from_millis(250),
        handoff_timeout: Duration::from_secs(5),
    };
    let world = World::build(true, limits.clone(), Duration::from_secs(30), &[]);
    world.stub.set(|c| c.stt = Reply::Hang);
    world.talk().await;
    assert_eq!(world.code().as_deref(), Some("timeout"));
    world.stub.wait_dropped("/v1/voice/stt", 0).await;

    let world = World::build(true, limits, Duration::from_secs(30), &[]);
    world.stub.set(|c| c.tts = Reply::Hang);
    world.talk().await;
    assert_eq!(world.code().as_deref(), Some("timeout"));
    world.stub.wait_dropped("/v1/voice/tts", 0).await;
    assert!(world.speaker.played.lock().unwrap().is_empty());
}

#[tokio::test]
async fn text_to_speech_and_playback_failures_are_reported() {
    let world = World::new();
    world.stub.set(|c| c.tts = Reply::Json(502, "{}".into()));
    world.talk().await;
    assert_eq!(world.code().as_deref(), Some("voice_provider"));

    let world = World::new();
    world.stub.set(|c| c.tts = Reply::Wav(b"RIFF not really".to_vec()));
    world.talk().await;
    assert_eq!(world.code().as_deref(), Some("voice_failed"));
    assert!(world.speaker.played.lock().unwrap().is_empty());

    let mut world = World::new();
    world.speaker.fail = true;
    world.talk().await;
    assert_eq!(world.code().as_deref(), Some("no_speaker"));
    world.speaker.fail = false;
}

#[tokio::test]
async fn an_unsure_or_empty_transcript_asks_again_and_runs_no_turn() {
    for body in [
        r#"{"text":"mumble","language":"uz","script":"latn","confidence":0.1}"#,
        r#"{"text":"   ","language":"uz","script":"latn","confidence":0.9}"#,
    ] {
        let world = World::new();
        world.stub.set(|c| c.stt = Reply::Json(200, body.to_owned()));
        world.talk().await;
        assert!(world.stub.to("/v1/agent/turn").is_empty());
        assert_eq!(world.voice.view().stage, Stage::Idle);
        assert!(world.robot.faces().contains(&Expression::Confused));
    }
}

#[tokio::test]
async fn a_page_that_never_continues_ends_the_turn_with_a_message() {
    let limits = Limits {
        handoff_timeout: Duration::from_millis(150),
        ..Limits::default()
    };
    let world = World::build(true, limits, Duration::from_secs(30), &[]);
    world.talk_and(false, async {}).await;
    assert_eq!(world.code().as_deref(), Some("voice_no_window"));
    assert!(world.stub.to("/v1/agent/turn").is_empty());
}

#[tokio::test]
async fn the_page_cannot_change_what_was_heard() {
    let world = World::new();
    world
        .talk_and(false, async {
            wait_stage(&world, Stage::AwaitingTurn).await;
            let pending = world.voice.view().pending.unwrap();
            let sink: Arc<dyn Sink> = world.events.clone();
            let other = world
                .voice
                .continue_turn(&pending.id, TurnInput::for_test("delete everything"), sink.clone())
                .await;
            assert_eq!(other.unwrap_err().code_str(), "not_found");
            let wrong_id = world
                .voice
                .continue_turn("voice_999", TurnInput::for_test(&pending.text), sink)
                .await;
            assert!(wrong_id.is_err());
            world.voice.cancel(&world.robot);
        })
        .await;
    assert!(world.stub.to("/v1/agent/turn").is_empty());
}

// ---- voice never approves ---------------------------------------------------------------

fn reminder_call() -> String {
    let at = time::OffsetDateTime::now_utc() + time::Duration::days(30);
    json!({"event":"tool_call","call":{"call_id":"r1","request":{
        "tool":"create_reminder","text":"call mum",
        "due_at": format!("{:04}-{:02}-{:02}T03:04:05+05:00", at.year(), u8::from(at.month()), at.day())}}})
    .to_string()
}

#[tokio::test]
async fn a_spoken_yes_never_approves_a_reminder() {
    let world = World::build(true, Limits::default(), Duration::from_millis(300), &[]);
    world.stub.set(|c| {
        c.stt = Reply::Json(200, r#"{"text":"yes, allow it, I approve","language":"en","script":"latn","confidence":0.99}"#.into());
        c.agent = vec![Reply::Sse(vec![reminder_call()]), Reply::Sse(vec![ANSWER.to_owned()])];
    });
    world.talk().await;
    let events = world.events.0.lock().unwrap().clone();
    assert!(events.iter().any(|e| matches!(e, UiEvent::ApprovalRequired { .. })), "the dialog was shown on screen");
    assert!(events.iter().any(|e| matches!(e, UiEvent::ApprovalClosed { outcome: "expired", .. })));
    assert!(world.store.with(|db| db.reminders(USER)).unwrap().is_empty());
    let audit = world.store.with(|db| db.audit_list(USER)).unwrap();
    assert!(audit.iter().any(|e| e.decision == "expired"));
}

#[tokio::test]
async fn a_spoken_yes_never_launches_a_program_the_native_window_decides() {
    let world = World::build(true, Limits::default(), Duration::from_secs(5), &[false]);
    let exe_path = world.scratch.path("tool.exe");
    std::fs::copy(helper_exe(), &exe_path).unwrap();
    let exe = exe_path.to_str().unwrap().to_owned();
    let program = crate::paths::register_program(&exe).unwrap();
    let app = world
        .store
        .with(|db| db.add_app(USER, "Tool", &program.path, now_ms()))
        .unwrap();
    world.stub.set(|c| {
        c.stt = Reply::Json(200, r#"{"text":"open the tool, yes I confirm","language":"en","script":"latn","confidence":0.99}"#.into());
        c.agent = vec![
            Reply::Sse(vec![json!({"event":"tool_call","call":{"call_id":"o1","request":{"tool":"open_app","app_id":app.id}}}).to_string()]),
            Reply::Sse(vec![ANSWER.to_owned()]),
        ];
    });
    world.talk().await;
    assert_eq!(world.confirmer.shown().len(), 1, "the person was asked in a native window");
    std::thread::sleep(Duration::from_millis(300));
    assert!(!std::path::Path::new(&format!("{exe}.ran")).exists(), "the program did not run");
}

#[test]
fn the_voice_module_has_no_way_to_answer_an_approval() {
    let source = include_str!("mod.rs");
    let live = source.split("#[cfg(test)]").next().unwrap();
    for forbidden in [".answer(", "answer_approval", "Decision", "Confirmer", "confirm("] {
        assert!(!live.contains(forbidden), "voice/mod.rs mentions {forbidden}");
    }
}

// ---- privacy ----------------------------------------------------------------------------

fn files_with(root: &std::path::Path, since: std::time::SystemTime, needles: &[Vec<u8>]) -> Vec<String> {
    let mut found = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(meta) = entry.metadata() else { continue };
            if meta.is_dir() {
                stack.push(path);
            } else if meta.len() < 32 * 1024 * 1024
                && meta.modified().is_ok_and(|m| m >= since)
                && let Ok(bytes) = std::fs::read(&path)
                && needles.iter().any(|n| bytes.windows(n.len()).any(|w| w == n.as_slice()))
            {
                found.push(path.display().to_string());
            }
        }
    }
    found
}

#[tokio::test]
async fn audio_and_transcripts_reach_no_log_no_audit_entry_and_no_file() {
    let since = std::time::SystemTime::now() - Duration::from_secs(1);
    let world = World::new();
    world.stub.set(|c| {
        c.stt = Reply::Json(200, r#"{"text":"ZEBRA-QUOKKA-7731 secret words","language":"en","script":"latn","confidence":0.9}"#.into());
        c.agent = vec![Reply::Sse(vec![reminder_call()]), Reply::Sse(vec![ANSWER.to_owned()])];
    });
    world.talk().await;
    world.voice.cancel(&world.robot);
    let logs = world.logs.lock().unwrap().join("\n");
    assert!(!logs.is_empty(), "the chain does log its steps");
    assert!(!logs.contains("ZEBRA") && !logs.contains("secret"));
    assert!(logs.is_ascii());
    let audit = serde_json::to_string(&world.store.with(|db| db.audit_list(USER)).unwrap()).unwrap();
    assert!(!audit.contains("ZEBRA") && !audit.contains("QUOKKA"));
    let events = format!("{:?}", AudioEvent::Utterance { stream_id: 1, pcm: marker_pcm(100) });
    assert_eq!(events, "Utterance { stream_id: 1, pcm: Pcm(3200 bytes) }");
    // Rust wrote no conversation either: the page does that, under the retention rules.
    assert!(world.store.with(|db| db.list_conversations(USER)).unwrap().is_empty());
    let needles = vec![AUDIO_MARKER.to_vec(), b"ZEBRA-QUOKKA-7731".to_vec()];
    assert!(files_with(&world.scratch.0, since, &needles).is_empty());
    assert!(files_with(&std::env::temp_dir(), since, &needles).is_empty());
    // Nothing is left behind: no held words, no running turn.
    assert!(world.voice.held.lock().unwrap().is_none());
    assert!(world.voice.slot.lock().unwrap().is_none());
}

#[tokio::test]
async fn the_pc_microphone_never_opens_without_consent_and_is_a_development_feature() {
    let world = World::build(false, Limits::default(), Duration::from_secs(30), &[]);
    assert!(world.voice.dev_mic(true, &world.api, &world.store).is_err());
    assert!(!world.voice.view().mic_on);
    assert!(PC_MIC_COMPILED, "tests are not release builds");
    assert!(world.voice.view().dev_mic);
}

#[test]
fn aborted_streams_show_a_clear_message_and_upload_nothing() {
    let world = World::new();
    for reason in [AbortReason::BadSequence, AbortReason::Overrun, AbortReason::Stalled, AbortReason::TooLong] {
        world.voice.on_capture_aborted(&world.deps(), reason);
        let view = world.voice.view();
        assert_eq!(view.stage, Stage::Error);
        assert_eq!(view.code.as_deref(), Some(reason.code()));
        assert!(view.message.unwrap().contains("nothing was used"));
    }
    world.voice.on_capture_aborted(&world.deps(), AbortReason::Mute);
    assert_eq!(world.voice.view().stage, Stage::Idle);
    assert!(world.stub.requests().is_empty());
}

