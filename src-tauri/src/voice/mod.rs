//! The voice chain (stage D4): the robot's push-to-talk audio, then consent, speech to text, the
//! agent turn, text to speech, and playback on this computer's speakers.
//!
//! ```text
//!  robot mic -> robot/audio.rs --Utterance--> consent -> STT -> transcript --webview--> agent turn
//!                                                                                          |
//!                                    speakers <- playback <- WAV <- TTS <- speech_text <---+
//! ```
//!
//! * One turn is one future. Pressing Stop, the robot's `cancel`, a new button press, revoking
//!   consent or signing out cancels it, and dropping a request future drops the HTTP request.
//! * The agent turn is the existing one, unchanged: its tools, its confirmations and its limits.
//!   The transcript is handed to the webview only so that it is shown and stored in the conversation
//!   under the usual retention rules; the webview then continues the turn with `voice_turn`, and
//!   Rust checks that the newest message is exactly the transcript it holds. A voice turn can
//!   never answer an approval: nothing in this module touches `Agent::answer`.
//! * Raw audio and transcripts are never logged (`Voice::log` takes only `&'static str`), never
//!   written to the audit log, and never written to disk. Our own buffers are zeroed at the end of
//!   the turn; the HTTP stack's copies are freed, not zeroed.
//! * Tokens stay in Rust: every call goes through `Api`.
//! * The PC microphone (`mic.rs`) exists only where the simulator does (tests and debug builds).

pub(crate) mod client;
#[cfg(any(test, debug_assertions))]
pub(crate) mod mic;
pub(crate) mod play;
#[cfg(test)]
pub(crate) mod testing;
#[cfg(test)]
mod tests;
pub(crate) mod wav;

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use moby_protocol::Expression;
use serde::Serialize;
use tauri::State;
use tauri::ipc::Channel;
use tokio::sync::{mpsc, oneshot};

use crate::agent::{Agent, Cancel, Sink, TurnInput, UiError, UiEvent};
use crate::api::{Api, Failure};
use crate::confirm::{Confirmer, Prompt};
use crate::robot::Robot;
use crate::robot::audio::{AbortReason, AudioEvent, Pcm};
use crate::store::{Store, now_ms};

use play::{DeviceSpeaker, Speaker};

/// Whether the PC microphone is part of this build. That a release build says no is proved by
/// `scripts/check-release-excludes-dev.ps1`, which searches the built binary for the marker
/// string in `mic.rs`, the same way the simulator is checked (ADR 0006).
#[allow(dead_code)]
pub const PC_MIC_COMPILED: bool = cfg!(any(test, debug_assertions));

/// Below this the speech service was not sure what it heard, and the robot asks again.
/// A proposal from the plan, not a measured value.
pub const MIN_CONFIDENCE: f32 = 0.35;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Stage {
    Idle,
    /// The robot is sending a stream (or the development microphone is on).
    Listening,
    Transcribing,
    /// The words are known and shown; the app is about to continue the turn.
    AwaitingTurn,
    Thinking,
    Synthesizing,
    Speaking,
    Error,
}

/// The words Rust heard, waiting for the webview to put them in the conversation.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PendingTurn {
    pub id: String,
    pub text: String,
}

/// What the interface is told. Never audio.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct VoiceView {
    pub stage: Stage,
    /// One sentence written by this app, for a notice or an error.
    pub message: Option<String>,
    pub code: Option<String>,
    pub pending: Option<PendingTurn>,
    /// The development microphone on this computer is capturing. Always false in a release build.
    pub mic_on: bool,
    /// This build has the development microphone.
    pub dev_mic: bool,
}

impl VoiceView {
    fn new() -> Self {
        Self {
            stage: Stage::Idle,
            message: None,
            code: None,
            pending: None,
            mic_on: false,
            dev_mic: PC_MIC_COMPILED,
        }
    }
}

pub trait ViewSink: Send + Sync {
    /// False once the other side is gone.
    fn emit(&self, view: &VoiceView) -> bool;
}

impl ViewSink for Channel<VoiceView> {
    fn emit(&self, view: &VoiceView) -> bool {
        self.send(view.clone()).is_ok()
    }
}

/// What the chain asks of the robot link. Fire and forget: the chain never waits for it.
pub trait RobotControl: Send + Sync {
    /// Stop the push-to-talk stream that is open, if any, and throw its audio away.
    fn cancel_capture(&self);
    /// Show a face on the robot for `hold_ms` (0 holds it until the next one).
    fn face(&self, expression: Expression, hold_ms: u32);
}

impl RobotControl for Robot {
    fn cancel_capture(&self) {
        Robot::cancel_capture(self);
    }

    fn face(&self, expression: Expression, hold_ms: u32) {
        Robot::face(self, expression, hold_ms);
    }
}

/// Everything a turn borrows. Tests build one from stubs.
pub struct Deps<'a> {
    pub api: &'a Api,
    pub store: &'a Store,
    pub agent: &'a Agent,
    pub speaker: &'a dyn Speaker,
    pub robot: &'a dyn RobotControl,
}

#[derive(Debug, Clone)]
pub struct Limits {
    pub stt_timeout: Duration,
    pub tts_timeout: Duration,
    /// How long the app has to continue a turn after the transcript is shown.
    pub handoff_timeout: Duration,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            stt_timeout: Duration::from_secs(20),
            tts_timeout: Duration::from_secs(20),
            handoff_timeout: Duration::from_secs(15),
        }
    }
}

struct Active {
    id: u64,
    cancel: Cancel,
}

type Slot = Arc<Mutex<Option<Active>>>;

/// The right to run one turn. Dropping it frees the slot, however the turn ended.
pub struct Turn {
    id: u64,
    cancel: Cancel,
    slot: Slot,
}

impl Drop for Turn {
    fn drop(&mut self) {
        let mut slot = lock(&self.slot);
        if slot.as_ref().is_some_and(|active| active.id == self.id) {
            *slot = None;
        }
    }
}

struct Continue {
    input: TurnInput,
    sink: Arc<dyn Sink>,
    done: oneshot::Sender<Result<(), Failure>>,
}

struct Held {
    turn_id: String,
    text: String,
    tx: oneshot::Sender<Continue>,
}

/// Passes events on and remembers the two things the chain needs from them.
struct Tap {
    inner: Arc<dyn Sink>,
    answer: Mutex<Option<(String, Expression)>>,
    error: Mutex<Option<UiError>>,
}

impl Sink for Tap {
    fn emit(&self, event: UiEvent) {
        match &event {
            UiEvent::Completed {
                speech_text,
                expression_hint,
                ..
            } => *lock(&self.answer) = Some((speech_text.clone(), *expression_hint)),
            UiEvent::Error { error } => *lock(&self.error) = Some(error.clone()),
            _ => {}
        }
        self.inner.emit(event);
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

pub struct Voice {
    view: Mutex<VoiceView>,
    sinks: Mutex<Vec<Box<dyn ViewSink>>>,
    slot: Slot,
    held: Mutex<Option<Held>>,
    /// Set the moment consent is withdrawn and cleared when it is given again. It outranks the
    /// stored consent, so voice stays off even if the settings file cannot be written.
    revoked: AtomicBool,
    next: AtomicU64,
    limits: Limits,
    log: Box<dyn Fn(&'static str) + Send + Sync>,
    events: Mutex<Option<mpsc::UnboundedSender<AudioEvent>>>,
    #[cfg(any(test, debug_assertions))]
    mic: Mutex<Option<mic::MicCapture>>,
}

impl Voice {
    pub fn new() -> Self {
        Self::with_limits(Limits::default())
    }

    pub fn with_limits(limits: Limits) -> Self {
        Self {
            view: Mutex::new(VoiceView::new()),
            sinks: Mutex::new(Vec::new()),
            slot: Arc::new(Mutex::new(None)),
            held: Mutex::new(None),
            revoked: AtomicBool::new(false),
            next: AtomicU64::new(1),
            limits,
            // Only fixed strings can be logged: a transcript or audio cannot be passed here.
            log: Box::new(|line| eprintln!("moby: voice: {line}")),
            events: Mutex::new(None),
            #[cfg(any(test, debug_assertions))]
            mic: Mutex::new(None),
        }
    }

    #[cfg(test)]
    pub fn with_log(mut self, log: Box<dyn Fn(&'static str) + Send + Sync>) -> Self {
        self.log = log;
        self
    }

    /// Where the development microphone sends what it heard.
    pub fn set_event_sender(&self, sender: mpsc::UnboundedSender<AudioEvent>) {
        *lock(&self.events) = Some(sender);
    }

    pub fn view(&self) -> VoiceView {
        lock(&self.view).clone()
    }

    pub fn subscribe(&self, sink: Box<dyn ViewSink>) {
        let _ = sink.emit(&self.view());
        let mut sinks = lock(&self.sinks);
        if sinks.len() >= 8 {
            sinks.remove(0);
        }
        sinks.push(sink);
    }

    fn log(&self, line: &'static str) {
        (self.log)(line);
    }

    fn publish(&self, change: impl FnOnce(&mut VoiceView)) {
        let snapshot = {
            let mut view = lock(&self.view);
            change(&mut view);
            view.clone()
        };
        lock(&self.sinks).retain(|sink| sink.emit(&snapshot));
    }

    fn current(&self, id: u64) -> bool {
        lock(&self.slot)
            .as_ref()
            .is_some_and(|active| active.id == id)
    }

    fn set_stage(&self, id: u64, stage: Stage) {
        if self.current(id) {
            self.publish(|view| {
                view.stage = stage;
                view.message = None;
                view.code = None;
            });
        }
    }

    /// Ends a turn in a resting state with a sentence for the person.
    fn finish(&self, id: u64, stage: Stage, code: Option<&str>, message: Option<&str>) {
        if self.current(id) {
            self.publish(|view| {
                view.stage = stage;
                view.code = code.map(str::to_owned);
                view.message = message.map(str::to_owned);
                view.pending = None;
            });
        }
    }

    // ---- consent -------------------------------------------------------------------------

    /// Voice is on only for the account that accepted, and only until it is revoked. Checked
    /// before audio leaves the computer, and again before the answer is spoken.
    pub fn require_consent(&self, api: &Api, store: &Store) -> Result<(), Failure> {
        let user = api.account_id()?;
        let stored = if self.revoked.load(Ordering::SeqCst) {
            None
        } else {
            store.with(|db| db.voice_consent())?
        };
        match stored {
            Some((account, _)) if account == user => Ok(()),
            _ => Err(Failure::new(
                "voice_consent_required",
                "Voice is off. Turn it on in Settings to talk to Moby. Until then nothing you say is sent anywhere.",
            )),
        }
    }

    pub fn consent_status(&self, api: &Api, store: &Store) -> Result<ConsentStatus, Failure> {
        let user = api.account_id()?;
        let stored = if self.revoked.load(Ordering::SeqCst) {
            None
        } else {
            store.with(|db| db.voice_consent())?
        };
        Ok(match stored {
            Some((account, version)) if account == user => ConsentStatus {
                accepted: true,
                version: Some(version),
            },
            _ => ConsentStatus {
                accepted: false,
                version: None,
            },
        })
    }

    /// Turning cloud voice on is confirmed in a native window that Rust opens, so a compromised
    /// page cannot do it by calling this command. The window says plainly that audio leaves the
    /// computer, and shows the consent version (and the server's consent text, if it sends one).
    /// The consent is recorded on the server and, only if that worked, here. Cancel records nothing.
    pub async fn accept_consent(
        &self,
        api: &Api,
        store: &Store,
        confirmer: &dyn Confirmer,
    ) -> Result<ConsentStatus, Failure> {
        let user = api.account_id()?;
        let offer = client::current_consent_offer(api).await?;
        let mut lines = vec![
            "Turn on cloud voice?".to_owned(),
            "What you say will leave this computer as audio and be sent to Moby's server, which turns it into text. The text of Moby's answer is sent too, to be turned into speech. The server does not keep either."
                .to_owned(),
            "Moby only listens while you hold the talk button on the robot.".to_owned(),
            format!("Consent version: {}", offer.version),
        ];
        if let Some(text) = &offer.text {
            lines.push(format!("From the server: {text}"));
        }
        let prompt = Prompt::new("Send your voice to Moby's server?", &lines, "turn voice on");
        if !confirmer.confirm(prompt).await {
            return Err(Failure::new("canceled", "Voice stays off."));
        }
        client::set_consent(api, &offer.version, true).await?;
        store.with(|db| db.set_voice_consent(Some((&user, &offer.version)), now_ms()))?;
        self.revoked.store(false, Ordering::SeqCst);
        self.log("consent accepted");
        self.consent_status(api, store)
    }

    /// Voice is off here at once, before the server is told. A turn in progress is canceled, so
    /// no audio leaves after this call returns.
    pub async fn revoke_consent(
        &self,
        api: &Api,
        store: &Store,
        robot: &dyn RobotControl,
    ) -> Result<ConsentStatus, Failure> {
        // First of all: nothing more leaves, whatever happens to the settings file below.
        self.revoked.store(true, Ordering::SeqCst);
        self.stop_everything(robot);
        let user = api.account_id()?;
        let version = store
            .with(|db| db.voice_consent())
            .ok()
            .flatten()
            .filter(|(account, _)| *account == user)
            .map(|(_, version)| version);
        store.with(|db| db.set_voice_consent(None, now_ms()))?;
        self.log("consent revoked");
        // Best effort: it is already off on this computer, so a failure here is only reported.
        let told = match version {
            Some(version) => client::set_consent(api, &version, false).await,
            None => match client::current_consent_version(api).await {
                Ok(version) => client::set_consent(api, &version, false).await,
                Err(error) => Err(error),
            },
        };
        told.map_err(|_| {
            Failure::new(
                "consent_not_synced",
                "Voice is off on this computer, but Moby's server could not be told. Check your connection and turn it off again.",
            )
        })?;
        self.consent_status(api, store)
    }

    // ---- turns ---------------------------------------------------------------------------

    /// Takes the right to run a turn. Whatever was running is canceled first.
    pub fn start_turn(&self) -> Turn {
        let id = self.next.fetch_add(1, Ordering::SeqCst);
        let cancel = Cancel::new();
        let previous = lock(&self.slot).replace(Active {
            id,
            cancel: cancel.clone(),
        });
        if let Some(previous) = previous {
            previous.cancel.cancel();
        }
        Turn {
            id,
            cancel,
            slot: Arc::clone(&self.slot),
        }
    }

    /// Cancels the turn that is running, and the capture if one is open. Safe at any stage.
    pub fn cancel(&self, robot: &dyn RobotControl) {
        self.stop_everything(robot);
        self.log("canceled");
    }

    fn stop_everything(&self, robot: &dyn RobotControl) {
        if let Some(active) = lock(&self.slot).as_ref() {
            active.cancel.cancel();
        }
        self.drop_held();
        #[cfg(any(test, debug_assertions))]
        self.stop_mic();
        robot.cancel_capture();
        let listening = self.view().stage == Stage::Listening;
        if listening || self.view().mic_on {
            self.publish(|view| {
                view.stage = Stage::Idle;
                view.message = Some("Stopped.".to_owned());
                view.code = Some("canceled".to_owned());
                view.mic_on = false;
            });
        }
    }

    fn drop_held(&self) {
        *lock(&self.held) = None;
    }

    /// A stream began. If there is no consent the robot is told to stop and nothing is kept.
    pub fn on_capture_started(&self, deps: &Deps<'_>) {
        if let Err(failure) = self.require_consent(deps.api, deps.store) {
            deps.robot.cancel_capture();
            self.publish(|view| {
                view.stage = Stage::Idle;
                view.code = Some(failure.code_str().to_owned());
                view.message = Some(failure.message_str().to_owned());
            });
            self.log("capture refused: no consent");
            return;
        }
        // A new button press cancels whatever stage the last turn is in (plan: "Bekor qilish").
        if let Some(active) = lock(&self.slot).take() {
            active.cancel.cancel();
        }
        self.drop_held();
        self.publish(|view| {
            view.stage = Stage::Listening;
            view.message = None;
            view.code = None;
            view.pending = None;
        });
    }

    /// A stream ended without a usable utterance. The audio is already gone.
    pub fn on_capture_aborted(&self, deps: &Deps<'_>, reason: AbortReason) {
        let quiet = matches!(
            reason,
            AbortReason::Canceled
                | AbortReason::Mute
                | AbortReason::Empty
                | AbortReason::TooShort
                | AbortReason::RobotCancel
        );
        self.publish(|view| {
            view.stage = if quiet { Stage::Idle } else { Stage::Error };
            view.code = Some(reason.code().to_owned());
            view.message = Some(reason.message().to_owned());
            view.pending = None;
        });
        if !quiet {
            deps.robot.face(Expression::Error, 3_000);
        }
        self.log("capture ended without audio");
    }

    /// One event from the robot link (or the development microphone).
    pub async fn handle_event(&self, deps: &Deps<'_>, event: AudioEvent) {
        match event {
            AudioEvent::Started { .. } => self.on_capture_started(deps),
            AudioEvent::Aborted { reason, .. } => self.on_capture_aborted(deps, reason),
            AudioEvent::Utterance { pcm, .. } => {
                let turn = self.start_turn();
                self.run_turn(deps, turn, pcm).await;
            }
        }
    }

    fn fail(&self, deps: &Deps<'_>, turn: &Turn, failure: &Failure) {
        // A turn that was replaced by a newer one says nothing: the newer one owns the screen.
        if self.current(turn.id) {
            self.finish(
                turn.id,
                Stage::Error,
                Some(failure.code_str()),
                Some(failure.message_str()),
            );
            deps.robot.face(Expression::Error, 3_000);
        }
    }

    fn stopped(&self, deps: &Deps<'_>, turn: &Turn) {
        if self.current(turn.id) {
            self.finish(turn.id, Stage::Idle, Some("canceled"), Some("Stopped."));
            deps.robot.face(Expression::Idle, 0);
        }
        self.log("turn stopped");
    }

    /// Consent, speech to text, the agent turn, text to speech, playback. Returns when the turn
    /// is over, whatever way it ended. The audio is zeroed first thing: only the WAV copy lives on,
    /// and it is zeroed when this function returns.
    pub async fn run_turn(&self, deps: &Deps<'_>, turn: Turn, pcm: Pcm) {
        let id = turn.id;
        let cancel = turn.cancel.clone();

        // 1. Consent. Without it the audio is thrown away and nothing is sent.
        if let Err(failure) = self.require_consent(deps.api, deps.store) {
            drop(pcm);
            self.finish(
                id,
                Stage::Idle,
                Some(failure.code_str()),
                Some(failure.message_str()),
            );
            self.log("audio dropped: no consent");
            return;
        }
        let Some(wave) = wav::encode(pcm.as_bytes()).map(Pcm::new) else {
            drop(pcm);
            self.fail(
                deps,
                &turn,
                &Failure::new("audio_unusable", "That audio could not be used."),
            );
            return;
        };
        drop(pcm);

        // 2. Speech to text. Dropping the future on cancel drops the request.
        self.set_stage(id, Stage::Transcribing);
        deps.robot.face(Expression::Thinking, 0);
        self.log("speech to text");
        if cancel.is_canceled() {
            drop(wave);
            return self.stopped(deps, &turn);
        }
        // `biased`: select! otherwise picks at random when both are ready, and a canceled turn
        // must not start its request.
        let heard = tokio::select! {
            biased;
            () = cancel.cancelled() => {
                drop(wave);
                return self.stopped(deps, &turn);
            }
            result = client::transcribe(deps.api, wave.as_bytes().to_vec(), self.limits.stt_timeout) => result,
        };
        drop(wave);
        let transcript = match heard {
            Ok(transcript) => transcript,
            Err(failure) => {
                if failure.code_str() == "voice_consent_required" {
                    let _ = deps.store.with(|db| db.set_voice_consent(None, now_ms()));
                }
                self.log("speech to text failed");
                return self.fail(deps, &turn, &failure);
            }
        };
        if cancel.is_canceled() {
            return self.stopped(deps, &turn);
        }
        if transcript.text.is_empty() {
            self.finish(
                id,
                Stage::Idle,
                Some("nothing_heard"),
                Some("Moby did not catch any words. Hold the talk button and try again."),
            );
            deps.robot.face(Expression::Confused, 2_000);
            return;
        }
        if transcript.confidence < MIN_CONFIDENCE {
            self.finish(
                id,
                Stage::Idle,
                Some("low_confidence"),
                Some("Moby was not sure what you said. Please say it again."),
            );
            deps.robot.face(Expression::Confused, 2_000);
            return;
        }

        // 3. Hand the words to the app, which shows them and stores them in the conversation.
        let (tx, rx) = oneshot::channel();
        let turn_id = format!("voice_{id}");
        *lock(&self.held) = Some(Held {
            turn_id: turn_id.clone(),
            text: transcript.text.clone(),
            tx,
        });
        if self.current(id) {
            self.publish(|view| {
                view.stage = Stage::AwaitingTurn;
                view.message = None;
                view.code = None;
                view.pending = Some(PendingTurn {
                    id: turn_id.clone(),
                    text: transcript.text.clone(),
                });
            });
        }
        let handoff = tokio::select! {
            biased;
            () = cancel.cancelled() => None,
            result = rx => result.ok(),
            () = tokio::time::sleep(self.limits.handoff_timeout) => None,
        };
        self.drop_held();
        let Some(Continue { input, sink, done }) = handoff else {
            if cancel.is_canceled() {
                return self.stopped(deps, &turn);
            }
            return self.fail(
                deps,
                &turn,
                &Failure::new(
                    "voice_no_window",
                    "Moby heard you, but the window did not take the words. Open Moby and try again.",
                ),
            );
        };

        // 4. The agent turn, exactly as for typed text. Only the language is the speaker's own.
        self.set_stage(id, Stage::Thinking);
        let tap = Arc::new(Tap {
            inner: sink,
            answer: Mutex::new(None),
            error: Mutex::new(None),
        });
        if cancel.is_canceled() {
            let _ = done.send(Err(Failure::new("canceled", "Stopped.")));
            return self.stopped(deps, &turn);
        }
        let mut run = std::pin::pin!(deps.agent.run_turn(
            deps.api,
            deps.store,
            tap.as_ref(),
            input.with_language(transcript.language)
        ));
        let mut asked_agent_to_stop = false;
        let outcome = loop {
            tokio::select! {
                biased;
                () = cancel.cancelled(), if !asked_agent_to_stop => {
                    // The agent ends its own turn cleanly: pending approvals close, tools stop.
                    asked_agent_to_stop = true;
                    deps.agent.cancel_turn();
                }
                result = &mut run => break result,
            }
        };
        let _ = done.send(outcome.clone());
        if let Err(failure) = outcome {
            return self.fail(deps, &turn, &failure);
        }
        if cancel.is_canceled() {
            return self.stopped(deps, &turn);
        }
        let Some((speech, hint)) = lock(&tap.answer).take() else {
            // An error or a refusal came instead of an answer. The page already shows it.
            let error = lock(&tap.error).take();
            let (code, message) = match error {
                Some(error) => (error.code, error.message),
                None => ("no_answer".to_owned(), "Moby had no answer.".to_owned()),
            };
            self.finish(id, Stage::Error, Some(&code), Some(&message));
            deps.robot.face(Expression::Error, 3_000);
            return;
        };

        // 5. Text to speech. Consent may have been withdrawn while the agent was busy.
        if let Err(failure) = self.require_consent(deps.api, deps.store) {
            self.finish(
                id,
                Stage::Idle,
                Some(failure.code_str()),
                Some(failure.message_str()),
            );
            self.log("answer not spoken: no consent");
            return;
        }
        let words = client::speakable(&speech);
        if words.is_empty() {
            self.finish(id, Stage::Idle, None, None);
            deps.robot.face(hint, 2_000);
            return;
        }
        self.set_stage(id, Stage::Synthesizing);
        self.log("text to speech");
        if cancel.is_canceled() {
            return self.stopped(deps, &turn);
        }
        let spoken = tokio::select! {
            biased;
            () = cancel.cancelled() => return self.stopped(deps, &turn),
            result = client::synthesize(deps.api, &words, transcript.language, transcript.script, self.limits.tts_timeout) => result,
        };
        let wave = match spoken {
            Ok(wave) => Pcm::new(wave),
            Err(failure) => {
                if failure.code_str() == "voice_consent_required" {
                    let _ = deps.store.with(|db| db.set_voice_consent(None, now_ms()));
                }
                self.log("text to speech failed");
                return self.fail(deps, &turn, &failure);
            }
        };
        let samples = match wav::decode(wave.as_bytes()) {
            Ok(samples) => samples,
            Err(_) => {
                return self.fail(
                    deps,
                    &turn,
                    &Failure::new(
                        "voice_failed",
                        "Moby's voice came back in a form this app cannot play.",
                    ),
                );
            }
        };
        drop(wave);

        // 6. Playback on this computer's speakers. Stopping it is dropping it.
        self.set_stage(id, Stage::Speaking);
        deps.robot.face(Expression::Speaking, 0);
        if cancel.is_canceled() {
            return self.stopped(deps, &turn);
        }
        let playing = match deps.speaker.play(samples) {
            Ok(playing) => playing,
            Err(_) => {
                return self.fail(
                    deps,
                    &turn,
                    &Failure::new(
                        "no_speaker",
                        "Moby could not use the speakers of this computer.",
                    ),
                );
            }
        };
        loop {
            if playing.is_done() {
                break;
            }
            tokio::select! {
                biased;
                () = cancel.cancelled() => {
                    playing.stop();
                    return self.stopped(deps, &turn);
                }
                () = tokio::time::sleep(Duration::from_millis(20)) => {}
            }
        }
        drop(playing);
        self.finish(id, Stage::Idle, None, None);
        deps.robot.face(hint, 2_000);
        self.log("turn finished");
    }

    /// The webview continues a turn whose transcript it was shown. The text must be exactly what
    /// Rust heard: the page cannot make a voice turn say something else.
    pub async fn continue_turn(
        &self,
        turn_id: &str,
        input: TurnInput,
        sink: Arc<dyn Sink>,
    ) -> Result<(), Failure> {
        let held = {
            let mut held = lock(&self.held);
            match held.as_ref() {
                Some(current)
                    if current.turn_id == turn_id
                        && input.last_user_text() == Some(current.text.as_str()) =>
                {
                    held.take()
                }
                _ => None,
            }
        };
        let Some(held) = held else {
            return Err(Failure::new(
                "not_found",
                "That voice turn is no longer waiting.",
            ));
        };
        let (done, finished) = oneshot::channel();
        held.tx
            .send(Continue { input, sink, done })
            .map_err(|_| Failure::new("canceled", "Stopped."))?;
        finished
            .await
            .map_err(|_| Failure::new("canceled", "Stopped."))?
    }

    // ---- the development microphone ------------------------------------------------------

    /// Starts or stops the PC microphone. Development builds only. The audio goes through the same
    /// chain as the robot's, and `mic_on` is true for exactly as long as it is capturing.
    #[cfg(any(test, debug_assertions))]
    pub fn dev_mic(&self, on: bool, api: &Api, store: &Store) -> Result<(), Failure> {
        self.dev_mic_with(on, api, store, mic::MicCapture::start)
    }

    /// open is what actually touches the sound device. A test passes one that only records that
    /// it was called, so "the microphone never opens without consent" can be checked.
    #[cfg(any(test, debug_assertions))]
    pub(crate) fn dev_mic_with(
        &self,
        on: bool,
        api: &Api,
        store: &Store,
        open: impl FnOnce() -> Result<mic::MicCapture, String>,
    ) -> Result<(), Failure> {
        if on {
            std::hint::black_box(mic::MIC_MARKER);
            self.require_consent(api, store)?;
            let mut mic = lock(&self.mic);
            if mic.is_some() {
                return Ok(());
            }
            *mic = Some(open().map_err(|message| {
                Failure::new(
                    "no_microphone",
                    &format!("The microphone could not be opened: {message}"),
                )
            })?);
            drop(mic);
            if let Some(active) = lock(&self.slot).take() {
                active.cancel.cancel();
            }
            self.publish(|view| {
                view.mic_on = true;
                view.stage = Stage::Listening;
                view.message = None;
                view.code = None;
            });
            Ok(())
        } else {
            let capture = lock(&self.mic).take();
            self.publish(|view| view.mic_on = false);
            if let Some(capture) = capture {
                let pcm = capture.finish();
                match lock(&self.events).as_ref() {
                    Some(sender) => {
                        let _ = sender.send(AudioEvent::Utterance { stream_id: 0, pcm });
                    }
                    None => drop(pcm),
                }
            }
            Ok(())
        }
    }

    /// Stops the microphone and throws its audio away.
    #[cfg(any(test, debug_assertions))]
    fn stop_mic(&self) {
        let capture = lock(&self.mic).take();
        if let Some(capture) = capture {
            drop(capture.finish());
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ConsentStatus {
    pub accepted: bool,
    pub version: Option<String>,
}

// ---- commands ----------------------------------------------------------------------------

#[tauri::command]
pub async fn voice_subscribe(
    voice: State<'_, Voice>,
    on_event: Channel<VoiceView>,
) -> Result<(), Failure> {
    voice.subscribe(Box::new(on_event));
    Ok(())
}

#[tauri::command]
pub async fn voice_status(
    voice: State<'_, Voice>,
    api: State<'_, Api>,
    store: State<'_, Store>,
) -> Result<ConsentStatus, Failure> {
    voice.consent_status(&api, &store)
}

#[tauri::command]
pub async fn voice_accept_consent(
    voice: State<'_, Voice>,
    api: State<'_, Api>,
    store: State<'_, Store>,
    agent: State<'_, Agent>,
) -> Result<ConsentStatus, Failure> {
    voice
        .accept_consent(&api, &store, agent.confirmer().as_ref())
        .await
}

#[tauri::command]
pub async fn voice_revoke_consent(
    voice: State<'_, Voice>,
    api: State<'_, Api>,
    store: State<'_, Store>,
    robot: State<'_, Robot>,
) -> Result<ConsentStatus, Failure> {
    voice.revoke_consent(&api, &store, &*robot).await
}

/// Stop: ends capture handling, the speech request, the agent turn, the speech request and
/// playback, whichever is running.
#[tauri::command]
pub async fn voice_cancel(voice: State<'_, Voice>, robot: State<'_, Robot>) -> Result<(), Failure> {
    voice.cancel(&*robot);
    Ok(())
}

/// Continues a voice turn after the page showed the transcript. The page sends the conversation as
/// for typed text; Rust refuses it unless the newest message is the transcript it holds.
#[tauri::command]
pub async fn voice_turn(
    voice: State<'_, Voice>,
    turn_id: String,
    turn: TurnInput,
    on_event: Channel<UiEvent>,
) -> Result<(), Failure> {
    voice
        .continue_turn(&turn_id, turn, Arc::new(on_event))
        .await
}

#[cfg(debug_assertions)]
#[tauri::command]
pub async fn voice_dev_mic(
    voice: State<'_, Voice>,
    api: State<'_, Api>,
    store: State<'_, Store>,
    on: bool,
) -> Result<(), Failure> {
    voice.dev_mic(on, &api, &store)
}

#[cfg(not(debug_assertions))]
#[tauri::command]
pub async fn voice_dev_mic() -> Result<(), Failure> {
    Err(Failure::new(
        "not_available",
        "The development microphone is not part of this build.",
    ))
}

/// Runs the chain for every event the robot link produces. Utterances run on their own tasks, so
/// a new button press (an event on this loop) can cancel the turn that is still running.
pub fn start_dispatcher(app: tauri::AppHandle, mut events: mpsc::UnboundedReceiver<AudioEvent>) {
    use tauri::Manager;
    tauri::async_runtime::spawn(async move {
        while let Some(event) = events.recv().await {
            let app = app.clone();
            if let AudioEvent::Utterance { .. } = &event {
                // The right to run is taken here, in order, before the task starts.
                let turn = app.state::<Voice>().start_turn();
                let AudioEvent::Utterance { pcm, .. } = event else {
                    continue;
                };
                tauri::async_runtime::spawn(async move {
                    let (voice, api, store, agent, robot) = (
                        app.state::<Voice>(),
                        app.state::<Api>(),
                        app.state::<Store>(),
                        app.state::<Agent>(),
                        app.state::<Robot>(),
                    );
                    let deps = Deps {
                        api: &api,
                        store: &store,
                        agent: &agent,
                        speaker: &DeviceSpeaker,
                        robot: &*robot,
                    };
                    voice.run_turn(&deps, turn, pcm).await;
                });
            } else {
                let (voice, api, store, agent, robot) = (
                    app.state::<Voice>(),
                    app.state::<Api>(),
                    app.state::<Store>(),
                    app.state::<Agent>(),
                    app.state::<Robot>(),
                );
                let deps = Deps {
                    api: &api,
                    store: &store,
                    agent: &agent,
                    speaker: &DeviceSpeaker,
                    robot: &*robot,
                };
                voice.handle_event(&deps, event).await;
            }
        }
    });
}
