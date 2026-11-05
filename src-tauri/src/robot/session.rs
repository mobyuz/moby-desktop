//! The desktop's half of the robot session, as a state machine that does no I/O. Bytes and time
//! come in through `on_message`, `tick` and the pairing calls; messages to send come out of
//! `take_outgoing`. The supervisor connects it to a transport, and tests drive it with a clock
//! they move by hand.
//!
//! What the firmware does is the reference (`moby_session.c`, ADR 0001 in moby-firmware):
//! hello at once and every second, a heartbeat every 2 s once acknowledged, and a session ended
//! after about 8 s without any desktop frame. The protocol text does not say so yet, but a desktop
//! therefore has to send a frame at least every 2 s: when nothing else is due, an empty
//! `set_config`, whose `command_ok` is ignored.
//!
//! The hardware mute is the robot's and wins everywhere. This file only reads it; no message it
//! builds asks the robot to change it.
//!
//! The robot is untrusted input. A message that does not fit the state is counted and dropped.

use std::collections::VecDeque;
use std::sync::Arc;

use moby_protocol::usb::{
    ActionDurationMs, AudioStreamId, Cancel, CommandKind, ControlMessage, ControlVersion,
    DeviceConfig, ErrorCode, ErrorInfo, Hello, Nonce, PairBegin, PairKey, PairSetKey,
    PairTimeoutMs, Proof, SetExpression,
};
use moby_protocol::{Expression, PROTOCOL_VERSION, StableId};

use super::pairings::PairingStore;
use super::view::{DeviceView, PairFailure, PairStep, Phase, RobotError, Snapshot, default_name};

/// A desktop frame at least this often keeps the robot's session alive (it drops after ~8 s).
pub const KEEPALIVE_MS: u64 = 1_000;
/// The robot sends a heartbeat every 2 s. Three missed ones and a margin means it is gone.
pub const HEARTBEAT_LOSS_MS: u64 = 6_500;
/// A robot sends `hello` the moment the port opens. Waiting longer means it is not a Moby.
pub const HELLO_WAIT_MS: u64 = 5_000;
const COMMAND_TIMEOUT_MS: u64 = 3_000;
/// The longest pairing window the protocol allows.
pub const PAIR_WINDOW_MS: u32 = 30_000;
/// Slack on top of the window for the robot's `E_TIMEOUT` to arrive.
const PAIR_GRACE_MS: u64 = 2_000;
const ERROR_SHOWN_MS: u64 = 5_000;
/// A real robot repeats its hello once a second. A hello sooner than this after the last one that
/// was handled is dropped, so a flood costs no Credential Manager read and no new session.
const HELLO_MIN_GAP_MS: u64 = 500;
/// How long the robot has, after our `hello_ack`, to send a valid `session_proof` (protocol,
/// handshake step 5). The robot's own session timeout is about 8 s, so the desktop decides first.
pub const PROOF_WINDOW_MS: u64 = 3_000;
const MAX_PENDING: usize = 8;

/// Capabilities the desktop needs to talk to a robot at all. None: a robot without `usb_pcm` (the
/// default firmware image has no microphone path) can still be paired and watched, and the
/// interface says that voice is unavailable on it. See docs/adr/0007-voice-chain.md.
pub const REQUIRED_CAPABILITIES: &[&str] = &[];
const AUDIO_CAPABILITY: &str = "usb_pcm";

pub trait Random: Send {
    /// Fills the buffer with unpredictable bytes. False if the source failed.
    fn fill(&mut self, buf: &mut [u8]) -> bool;
}

/// The operating system's random generator (`BCryptGenRandom` on Windows).
pub struct OsRandom;

impl Random for OsRandom {
    fn fill(&mut self, buf: &mut [u8]) -> bool {
        getrandom::fill(buf).is_ok()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DropReason {
    /// The port opened but nothing like a Moby spoke.
    NoHello,
    /// The robot stopped sending heartbeats.
    HeartbeatLost,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserError {
    pub code: &'static str,
    pub message: String,
}

impl UserError {
    fn new(code: &'static str, message: &str) -> Self {
        Self {
            code,
            message: message.to_owned(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
enum Stage {
    Waiting,
    Incompatible(String),
    Established,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Auth {
    /// The robot holds no key.
    Unpaired,
    /// The robot holds a key that this computer lacks.
    NoKey,
    /// The key could not be read from Credential Manager.
    VaultFault,
    /// A proof was sent. The robot has not answered yet.
    Proving,
    Authenticated,
    Rejected,
    /// The robot said `pair_reset`, or said `paired: false` while a key for it is saved here.
    /// Nothing proves it was the robot, so the key stays and the session is not trusted (no audio)
    /// until the person forgets the key or pairs again.
    Reset,
    /// Nothing proved that the device holds the key: no valid `session_proof` in time, a wrong
    /// one, one that should not exist, or a session that came from `pair_set_key`. No audio.
    Unproven,
}

/// What a valid `session_proof` has to match: everything the proof covers, from this session only.
/// It exists only while a `desktop_nonce` was sent and no proof has been judged yet, so only the
/// first `session_proof` counts.
struct ProofWait {
    key: PairKey,
    desktop_nonce: Nonce,
    robot_nonce: Nonce,
    device: StableId,
    session: StableId,
    deadline: u64,
}

/// Checks the robot's proof with the protocol's constant-time helper.
pub(super) fn robot_proof_is_valid(
    key: &PairKey,
    desktop_nonce: &Nonce,
    robot_nonce: &Nonce,
    device: &StableId,
    session: &StableId,
    proof: &Proof,
) -> bool {
    key.verify_robot_session_proof(desktop_nonce, robot_nonce, device, session, proof)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Pair {
    Idle,
    /// `pair_begin` sent.
    Starting {
        since: u64,
    },
    /// The robot is waiting for its talk button.
    WaitButton {
        deadline: u64,
    },
    /// `pair_set_key` sent.
    Saving {
        since: u64,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PendingKind {
    Keepalive,
    PairBegin,
    PairSetKey,
    /// A face change or a cancel. The answer is read and dropped: the robot may refuse it (muted,
    /// busy, or the stream already ended) and nothing depends on that.
    Fire,
}

#[derive(Debug, Clone)]
struct Pending {
    id: StableId,
    kind: PendingKind,
    sent_at: u64,
}

#[derive(Debug, Clone)]
struct Device {
    id: StableId,
    hw_rev: String,
    fw_version: String,
    audio: bool,
    paired_on_robot: bool,
    key_here: bool,
}

pub struct Session {
    pairings: Arc<dyn PairingStore>,
    random: Box<dyn Random>,
    opened_at: u64,
    stage: Stage,
    auth: Auth,
    pair: Pair,
    proof_wait: Option<ProofWait>,
    /// Set after `pair_set_key`: the session has no robot proof, so the link starts over.
    restart: bool,
    pair_failure: Option<PairFailure>,
    device: Option<Device>,
    hello_id: Option<StableId>,
    last_ack: Option<ControlMessage>,
    out: VecDeque<ControlMessage>,
    pending: Vec<Pending>,
    next_command: u64,
    session_counter: u64,
    last_tx: u64,
    last_heartbeat: u64,
    last_hello_at: Option<u64>,
    muted: bool,
    expression: Expression,
    error: Option<(RobotError, u64)>,
    pub unexpected: u64,
}

fn ts(now: u64) -> u32 {
    u32::try_from(now & 0xFFFF_FFFF).unwrap_or(0)
}

pub(super) fn incompatibility(hello: &Hello, required: &[&str]) -> Option<String> {
    if !hello.supports_protocol() {
        return Some(format!(
            "This Moby speaks protocol version {}, and this app speaks version {PROTOCOL_VERSION}. Update the app or Moby's firmware so that they match.",
            hello.protocol_version
        ));
    }
    let missing: Vec<&str> = required
        .iter()
        .copied()
        .filter(|needed| {
            !hello
                .capabilities
                .as_slice()
                .iter()
                .any(|have| have.as_str() == *needed)
        })
        .collect();
    if missing.is_empty() {
        None
    } else {
        Some(format!(
            "This Moby's software does not offer what this app needs ({}). Update Moby's firmware.",
            missing.join(", ")
        ))
    }
}

impl Session {
    pub fn new(pairings: Arc<dyn PairingStore>, random: Box<dyn Random>, now: u64) -> Self {
        Self {
            pairings,
            random,
            opened_at: now,
            stage: Stage::Waiting,
            auth: Auth::Unpaired,
            pair: Pair::Idle,
            proof_wait: None,
            restart: false,
            pair_failure: None,
            device: None,
            hello_id: None,
            last_ack: None,
            out: VecDeque::new(),
            pending: Vec::new(),
            next_command: 0,
            session_counter: 0,
            last_tx: now,
            last_heartbeat: now,
            last_hello_at: None,
            muted: false,
            expression: Expression::Disconnected,
            error: None,
            unexpected: 0,
        }
    }

    pub fn established(&self) -> bool {
        self.stage == Stage::Established
    }

    /// True once after a pairing: the new key is saved and the robot holds it, but that session
    /// carried no `desktop_nonce`, so the robot cannot be proven on it. The caller starts the
    /// handshake over (protocol step 8).
    pub fn take_restart(&mut self) -> bool {
        std::mem::take(&mut self.restart)
    }

    /// The hardware mute, as the robot last reported it.
    pub fn muted(&self) -> bool {
        self.stage == Stage::Established && self.muted
    }

    /// Whether a push-to-talk stream may exist now: the session is up and authenticated, the mute
    /// switch is open, and this robot offers `usb_pcm`. A stream at any other time is refused.
    pub fn audio_allowed(&self) -> bool {
        self.stage == Stage::Established
            && self.auth == Auth::Authenticated
            && !self.muted
            && self.device.as_ref().is_some_and(|device| device.audio)
    }

    // ---- input -------------------------------------------------------------------------

    pub fn on_message(&mut self, now: u64, message: &ControlMessage) {
        match message {
            ControlMessage::Hello { data, .. } => self.on_hello(now, message, data),
            _ if self.stage != Stage::Established => self.unexpected += 1,
            ControlMessage::Heartbeat { data, .. } => {
                self.last_heartbeat = now;
                self.muted = data.mute;
                self.expression = if data.mute {
                    Expression::Muted
                } else {
                    data.state
                };
            }
            ControlMessage::MuteChanged { data, .. } => {
                self.muted = data.muted;
                self.expression = if data.muted {
                    Expression::Muted
                } else {
                    Expression::Idle
                };
                if data.muted && self.pair != Pair::Idle {
                    self.fail_pairing("muted");
                }
            }
            ControlMessage::SessionProof { data, .. } => self.on_session_proof(&data.proof),
            ControlMessage::Error { data, .. } => self.on_robot_error(now, data),
            ControlMessage::PairButtonConfirmed { .. } => self.on_button_confirmed(now),
            ControlMessage::PairReset { .. } => self.on_pair_reset(),
            ControlMessage::CommandOk { id, .. } => self.on_response(now, id, None),
            ControlMessage::CommandError { id, data, .. } => {
                self.on_response(now, id, Some(data));
            }
            // The stream itself is put together in `audio.rs`, which the link feeds directly.
            ControlMessage::AudioStart { .. } | ControlMessage::AudioEnd { .. } => {}
            // Nothing acts on a wake word or a button press: a stream starts when the robot says so.
            ControlMessage::WakeDetected { .. } | ControlMessage::ButtonEvent { .. } => {}
            // Messages only the desktop sends never get here (the framer drops them).
            _ => self.unexpected += 1,
        }
    }

    fn on_hello(&mut self, now: u64, message: &ControlMessage, hello: &Hello) {
        if self
            .last_hello_at
            .is_some_and(|before| now.saturating_sub(before) < HELLO_MIN_GAP_MS)
        {
            self.unexpected += 1;
            return;
        }
        self.last_hello_at = Some(now);
        let was_established = self.stage == Stage::Established;
        // The robot repeats a hello until it hears our answer. Answer a repeat the same way,
        // without starting over.
        if self.stage == Stage::Established
            && self.hello_id.as_ref() == Some(message.id())
            && let Some(ack) = self.last_ack.clone()
        {
            self.out.push_back(ack);
            return;
        }
        // A new hello is a new session, whether the robot restarted or dropped us.
        if self.pair != Pair::Idle {
            self.fail_pairing("interrupted");
        }
        self.pair = Pair::Idle;
        self.pending.clear();
        self.out.clear();
        self.error = None;
        self.hello_id = Some(message.id().clone());
        self.last_ack = None;

        if let Some(reason) = incompatibility(hello, REQUIRED_CAPABILITIES) {
            if let Some(ack) = message.acknowledge_hello(None, None, ts(now)) {
                self.out.push_back(ack);
            }
            self.device = Some(Device {
                id: hello.device_id.clone(),
                hw_rev: crate::text::short_line(hello.hw_rev.as_str(), 32),
                fw_version: hello.fw_version.as_str().to_owned(),
                audio: false,
                paired_on_robot: hello.paired,
                key_here: false,
            });
            self.stage = Stage::Incompatible(reason);
            self.expression = Expression::Disconnected;
            return;
        }

        let (key, vault_fault) = match self.pairings.key(&hello.device_id) {
            Ok(key) => (key, false),
            Err(_) => (None, true),
        };
        let session_id = self.new_session_id();
        self.proof_wait = None;
        self.restart = false;
        // The desktop proof and a fresh `desktop_nonce` go out together, only to a robot that
        // reported `paired` and only when we hold the key. The nonce comes from the operating
        // system's generator for each ack and is never reused.
        let mut proving = None;
        if hello.paired
            && let Some(key) = &key
        {
            let mut bytes = [0_u8; Nonce::BYTES];
            if self.random.fill(&mut bytes) {
                proving = Some((key.clone(), Nonce::from_bytes(bytes)));
            }
            bytes.fill(0);
        }
        let ack = message.acknowledge_hello(
            Some(session_id.clone()),
            proving.as_ref().map(|(key, nonce)| (key, nonce.clone())),
            ts(now),
        );
        self.auth = if !hello.paired && key.is_some() {
            // A device that says it is unpaired, with a key for its ID saved here: stale, not new.
            Auth::Reset
        } else if !hello.paired {
            Auth::Unpaired
        } else if proving.is_some() {
            Auth::Proving
        } else if key.is_some() {
            // We hold the key but could not make a nonce: nothing can be proven.
            Auth::Unproven
        } else if vault_fault {
            Auth::VaultFault
        } else {
            Auth::NoKey
        };
        if let Some((key, desktop_nonce)) = proving {
            self.proof_wait = Some(ProofWait {
                key,
                desktop_nonce,
                robot_nonce: hello.nonce.clone(),
                device: hello.device_id.clone(),
                session: session_id,
                deadline: now + PROOF_WINDOW_MS,
            });
        }
        self.device = Some(Device {
            id: hello.device_id.clone(),
            hw_rev: hello.hw_rev.as_str().to_owned(),
            fw_version: hello.fw_version.as_str().to_owned(),
            audio: hello
                .capabilities
                .as_slice()
                .iter()
                .any(|capability| capability.as_str() == AUDIO_CAPABILITY),
            paired_on_robot: hello.paired,
            key_here: key.is_some(),
        });
        self.stage = Stage::Established;
        self.muted = hello.mute;
        self.expression = if hello.mute {
            Expression::Muted
        } else {
            Expression::Idle
        };
        // A hello is not a heartbeat: only the first of a link counts as proof of life, so hellos
        // alone cannot keep a link up.
        if !was_established {
            self.last_heartbeat = now;
        }
        if let Some(ack) = ack {
            self.last_ack = Some(ack.clone());
            self.out.push_back(ack);
        }
    }

    fn new_session_id(&mut self) -> StableId {
        self.session_counter += 1;
        let mut bytes = [0_u8; 8];
        let value = if self.random.fill(&mut bytes) {
            u64::from_le_bytes(bytes)
        } else {
            // A session ID is not secret, and the nonce already makes each proof fresh.
            self.session_counter
        };
        StableId::new(format!("s-{value:016x}")).expect("a valid ID")
    }

    fn on_robot_error(&mut self, now: u64, info: &ErrorInfo) {
        self.error = Some((
            RobotError {
                code: error_code_text(info.code).to_owned(),
                // The robot's words are untrusted: bidirectional and zero-width characters go too.
                detail: crate::text::short_line(info.detail.as_str(), 128),
            },
            now,
        ));
        match (info.code, self.pair) {
            (ErrorCode::Timeout, Pair::Starting { .. } | Pair::WaitButton { .. }) => {
                self.fail_pairing("timeout");
            }
            (ErrorCode::Muted, Pair::Starting { .. } | Pair::WaitButton { .. }) => {
                self.fail_pairing("muted");
            }
            (ErrorCode::State, _) if self.auth == Auth::Proving => self.auth = Auth::Rejected,
            _ => {}
        }
    }

    /// The robot's `session_proof`. Only the first one counts, and only on a session where a
    /// `desktop_nonce` was sent. Anything else is a protocol violation: the device is unproven.
    fn on_session_proof(&mut self, proof: &Proof) {
        let Some(wait) = self.proof_wait.take() else {
            self.unexpected += 1;
            // A second proof on a proven session is ignored; one where none was asked for, or
            // after the first was judged bad, leaves the device unproven.
            if self.auth != Auth::Authenticated {
                self.auth = Auth::Unproven;
            }
            return;
        };
        if robot_proof_is_valid(
            &wait.key,
            &wait.desktop_nonce,
            &wait.robot_nonce,
            &wait.device,
            &wait.session,
            proof,
        ) {
            self.grant_authenticated();
        } else {
            self.auth = Auth::Unproven;
        }
    }

    /// The only place `Auth::Authenticated` is set, called only after a valid `session_proof`.
    fn grant_authenticated(&mut self) {
        self.auth = Auth::Authenticated;
    }

    fn on_response(&mut self, now: u64, id: &StableId, error: Option<&ErrorInfo>) {
        let Some(position) = self.pending.iter().position(|pending| &pending.id == id) else {
            self.unexpected += 1;
            return;
        };
        let pending = self.pending.remove(position);
        match (pending.kind, error) {
            // A `command_ok` proves nothing: any device can say it. Only a valid `session_proof`
            // authenticates the robot.
            (PendingKind::Keepalive, None) => {}
            (PendingKind::Keepalive, Some(info)) => {
                if info.code == ErrorCode::State
                    && matches!(self.auth, Auth::Proving | Auth::Authenticated)
                {
                    self.auth = Auth::Rejected;
                }
            }
            (PendingKind::PairBegin, None) => {
                if matches!(self.pair, Pair::Starting { .. }) {
                    self.pair = Pair::WaitButton {
                        deadline: now + u64::from(PAIR_WINDOW_MS) + PAIR_GRACE_MS,
                    };
                }
            }
            (PendingKind::PairBegin, Some(info)) => {
                let code = match info.code {
                    ErrorCode::State => "already_paired",
                    ErrorCode::Busy => "busy",
                    ErrorCode::Muted => "muted",
                    _ => "refused",
                };
                self.fail_pairing(code);
            }
            (PendingKind::Fire, _) => {}
            (PendingKind::PairSetKey, None) => {
                if matches!(self.pair, Pair::Saving { .. }) {
                    // The robot holds the new key and calls this session authenticated, but the
                    // session had no `desktop_nonce`, so the robot never proved itself. Unproven
                    // for audio, and the link starts over so the next session is a proven one.
                    self.pair = Pair::Idle;
                    self.auth = Auth::Unproven;
                    self.proof_wait = None;
                    self.restart = true;
                    if let Some(device) = &mut self.device {
                        device.paired_on_robot = true;
                        device.key_here = true;
                    }
                }
            }
            (PendingKind::PairSetKey, Some(_)) => {
                // The robot refused, so it did not store the key. Take ours back.
                self.drop_saved_key();
                self.fail_pairing("not_confirmed");
            }
        }
    }

    fn on_button_confirmed(&mut self, now: u64) {
        let Pair::WaitButton { .. } = self.pair else {
            // A confirmation nobody asked for. No key is made.
            self.unexpected += 1;
            return;
        };
        let Some(device) = self.device.as_ref().map(|device| device.id.clone()) else {
            self.unexpected += 1;
            return;
        };
        let mut bytes = [0_u8; PairKey::BYTES];
        if !self.random.fill(&mut bytes) {
            self.fail_pairing("random");
            return;
        }
        let key = PairKey::from_bytes(bytes);
        bytes.fill(0);
        // Saved before it is sent, so a robot never holds a key that this computer lost.
        if self
            .pairings
            .save(&device, &default_name(device.as_str()), &key)
            .is_err()
        {
            self.fail_pairing("storage");
            return;
        }
        let id = self.next_command_id();
        self.push_pending(&id, PendingKind::PairSetKey, now);
        self.out.push_back(ControlMessage::PairSetKey {
            v: ControlVersion::V1,
            kind: CommandKind::Command,
            id,
            ts_ms: ts(now),
            data: PairSetKey { pair_key: key },
        });
        self.pair = Pair::Saving { since: now };
    }

    /// Protocol step 7 says the desktop forgets its copy when the robot resets. But `pair_reset`
    /// carries no proof: any device on the port can send it, and forgetting a key on its say-so
    /// would let it erase a pairing. So the key stays here. The session stops being trusted, and
    /// the interface offers the choice to forget it. Only an explicit unpair or a new pairing
    /// removes the key (docs/adr/0006-robot-link.md).
    fn on_pair_reset(&mut self) {
        if self.pair != Pair::Idle {
            self.fail_pairing("interrupted");
        }
        self.proof_wait = None;
        if let Some(device) = &mut self.device {
            device.paired_on_robot = false;
            if device.key_here {
                self.auth = Auth::Reset;
            } else {
                self.auth = Auth::Unpaired;
            }
        }
    }

    fn drop_saved_key(&mut self) {
        if let Some(device) = &mut self.device {
            let id = device.id.clone();
            let _ = self.pairings.forget(&id);
            device.key_here = false;
        }
    }

    fn fail_pairing(&mut self, code: &'static str) {
        self.pair = Pair::Idle;
        self.pending
            .retain(|pending| !matches!(pending.kind, PendingKind::PairBegin));
        self.pair_failure = Some(PairFailure {
            code,
            message: pairing_failure_text(code).to_owned(),
        });
    }

    // ---- user actions ------------------------------------------------------------------

    pub fn begin_pairing(&mut self, now: u64) -> Result<(), UserError> {
        if self.stage != Stage::Established {
            return Err(UserError::new("not_connected", "Connect to Moby first."));
        }
        if self.pair != Pair::Idle {
            return Err(UserError::new("busy", "Pairing is already running."));
        }
        if self.muted {
            return Err(UserError::new(
                "muted",
                "Moby's mute switch is on. Turn it off, then try again.",
            ));
        }
        self.pair_failure = None;
        let id = self.next_command_id();
        self.push_pending(&id, PendingKind::PairBegin, now);
        self.out.push_back(ControlMessage::PairBegin {
            v: ControlVersion::V1,
            kind: CommandKind::Command,
            id,
            ts_ms: ts(now),
            data: PairBegin {
                timeout_ms: PairTimeoutMs::try_from(PAIR_WINDOW_MS).expect("within the range"),
            },
        });
        self.pair = Pair::Starting { since: now };
        Ok(())
    }

    /// Stops waiting. The protocol has no way to cancel on the robot, which keeps waiting for its
    /// own timeout; a button press in that time is ignored here.
    pub fn cancel_pairing(&mut self) {
        if self.pair != Pair::Idle {
            self.pair = Pair::Idle;
            self.pending
                .retain(|pending| pending.kind == PendingKind::Keepalive);
        }
        self.pair_failure = None;
    }

    /// Tells the robot to stop the stream it is sending. The robot answers `command_ok`, or
    /// `E_STATE` when the stream already ended; both are fine.
    pub fn cancel_stream(&mut self, now: u64, stream_id: u16) {
        if self.stage != Stage::Established {
            return;
        }
        let Ok(stream_id) = AudioStreamId::try_from(stream_id) else {
            return;
        };
        let id = self.next_command_id();
        self.push_pending(&id, PendingKind::Fire, now);
        self.out.push_back(ControlMessage::Cancel {
            v: ControlVersion::V1,
            kind: CommandKind::Command,
            id,
            ts_ms: ts(now),
            data: Cancel { stream_id },
        });
    }

    /// Asks the robot to show a face (thinking, speaking, happy, error). The robot owns
    /// listening, muted and disconnected, and the mute switch outranks everything, so this is
    /// not sent while muted.
    pub fn show_face(&mut self, now: u64, state: Expression, hold_ms: u32) {
        if self.stage != Stage::Established
            || self.muted
            || self.auth != Auth::Authenticated
            || matches!(
                state,
                Expression::Listening | Expression::Muted | Expression::Disconnected
            )
        {
            return;
        }
        let (Ok(duration_ms), Ok(animation)) =
            (ActionDurationMs::try_from(hold_ms), StableId::new("none"))
        else {
            return;
        };
        let id = self.next_command_id();
        self.push_pending(&id, PendingKind::Fire, now);
        self.out.push_back(ControlMessage::Expression {
            v: ControlVersion::V1,
            kind: CommandKind::Command,
            id,
            ts_ms: ts(now),
            data: SetExpression {
                state,
                animation,
                duration_ms,
            },
        });
    }

    /// The key for this robot was removed from this computer.
    pub fn key_removed(&mut self, device_id: &StableId) {
        if let Some(device) = &mut self.device
            && &device.id == device_id
        {
            device.key_here = false;
            if self.auth == Auth::Reset {
                self.auth = Auth::Unpaired;
            } else if device.paired_on_robot {
                self.auth = Auth::NoKey;
            }
        }
    }

    // ---- time --------------------------------------------------------------------------

    pub fn tick(&mut self, now: u64) -> Option<DropReason> {
        if self
            .error
            .as_ref()
            .is_some_and(|(_, at)| now.saturating_sub(*at) >= ERROR_SHOWN_MS)
        {
            self.error = None;
        }
        match self.stage {
            Stage::Waiting => {
                return (now.saturating_sub(self.opened_at) > HELLO_WAIT_MS)
                    .then_some(DropReason::NoHello);
            }
            Stage::Incompatible(_) => return None,
            Stage::Established => {}
        }
        if now.saturating_sub(self.last_heartbeat) > HEARTBEAT_LOSS_MS {
            return Some(DropReason::HeartbeatLost);
        }
        if self
            .proof_wait
            .as_ref()
            .is_some_and(|wait| now >= wait.deadline)
        {
            // No valid session_proof within the window: the device did not prove the key.
            self.proof_wait = None;
            self.auth = Auth::Unproven;
        }
        self.expire_pending(now);
        match self.pair {
            Pair::WaitButton { deadline } if now >= deadline => self.fail_pairing("timeout"),
            _ => {}
        }
        if self.out.is_empty() && now.saturating_sub(self.last_tx) >= KEEPALIVE_MS {
            let id = self.next_command_id();
            self.push_pending(&id, PendingKind::Keepalive, now);
            self.out.push_back(ControlMessage::SetConfig {
                v: ControlVersion::V1,
                kind: CommandKind::Command,
                id,
                ts_ms: ts(now),
                data: DeviceConfig::default(),
            });
        }
        None
    }

    fn expire_pending(&mut self, now: u64) {
        let mut expired = Vec::new();
        self.pending.retain(|pending| {
            let stale = now.saturating_sub(pending.sent_at) >= COMMAND_TIMEOUT_MS;
            if stale {
                expired.push(pending.kind);
            }
            !stale
        });
        for kind in expired {
            match kind {
                PendingKind::Keepalive | PendingKind::Fire => {}
                PendingKind::PairBegin => {
                    if matches!(self.pair, Pair::Starting { .. }) {
                        self.fail_pairing("no_answer");
                    }
                }
                PendingKind::PairSetKey => {
                    if matches!(self.pair, Pair::Saving { .. }) {
                        // Unknown whether the robot stored the key. Ours stays, so that if it did
                        // the next connection authenticates. If it did not, nothing is lost.
                        self.pair = Pair::Idle;
                        self.pair_failure = Some(PairFailure {
                            code: "unconfirmed",
                            message: pairing_failure_text("unconfirmed").to_owned(),
                        });
                    }
                }
            }
        }
    }

    // ---- output ------------------------------------------------------------------------

    pub fn take_outgoing(&mut self, now: u64) -> Vec<ControlMessage> {
        if self.out.is_empty() {
            return Vec::new();
        }
        self.last_tx = now;
        self.out.drain(..).collect()
    }

    fn next_command_id(&mut self) -> StableId {
        self.next_command += 1;
        StableId::new(format!("c-{}", self.next_command)).expect("a valid ID")
    }

    fn push_pending(&mut self, id: &StableId, kind: PendingKind, now: u64) {
        if self.pending.len() >= MAX_PENDING {
            self.pending.remove(0);
        }
        self.pending.push(Pending {
            id: id.clone(),
            kind,
            sent_at: now,
        });
    }

    // ---- what to show ------------------------------------------------------------------

    pub fn snapshot(&self) -> Snapshot {
        let (phase, message) = self.phase();
        let device = self.device.as_ref().map(|device| DeviceView {
            id: device.id.as_str().to_owned(),
            name: default_name(device.id.as_str()),
            hw_rev: device.hw_rev.clone(),
            fw_version: device.fw_version.clone(),
            paired_here: device.key_here,
            paired_on_robot: device.paired_on_robot,
            audio: device.audio,
        });
        let established = self.stage == Stage::Established;
        Snapshot {
            phase,
            message,
            muted: established && self.muted,
            expression: if established {
                self.expression
            } else {
                Expression::Disconnected
            },
            device,
            pair_step: match self.pair {
                Pair::Idle => None,
                Pair::Starting { .. } => Some(PairStep::Starting),
                Pair::WaitButton { .. } => Some(PairStep::PressButton),
                Pair::Saving { .. } => Some(PairStep::Saving),
            },
            pair_failure: self.pair_failure.clone(),
            error: self.error.as_ref().map(|(error, _)| error.clone()),
            retry_in_ms: None,
            port: None,
            simulated: false,
            stats: super::link::Stats::default(),
        }
    }

    fn phase(&self) -> (Phase, Option<String>) {
        match &self.stage {
            Stage::Waiting => (Phase::Connecting, None),
            Stage::Incompatible(reason) => (Phase::Incompatible, Some(reason.clone())),
            Stage::Established if self.pair != Pair::Idle => (Phase::Pairing, None),
            Stage::Established => match self.auth {
                Auth::Unpaired => (Phase::Unpaired, None),
                Auth::NoKey => (
                    Phase::ForeignPaired,
                    Some(
                        "This Moby holds a key that this computer does not have. To pair it here, hold its talk button for 10 seconds to reset it, then pair again."
                            .to_owned(),
                    ),
                ),
                Auth::VaultFault => (
                    Phase::VaultUnavailable,
                    Some(
                        "Windows Credential Manager is not available, so the key for this Moby cannot be read."
                            .to_owned(),
                    ),
                ),
                Auth::Proving => (Phase::Connecting, None),
                Auth::Rejected => (
                    Phase::KeyRejected,
                    Some(
                        "Moby did not accept the key saved on this computer. Remove it here, hold Moby's talk button for 10 seconds to reset it, then pair again."
                            .to_owned(),
                    ),
                ),
                Auth::Reset => (
                    Phase::PairingReset,
                    Some(
                        "This device says it is not paired (or that its pairing was reset), but a key for this Moby is saved on this computer. Anything plugged into the port can say that, so the key is kept and nothing it sends is used. If you reset Moby yourself, choose Unpair below to forget the key, then pair again. Otherwise leave it."
                            .to_owned(),
                    ),
                ),
                Auth::Unproven if self.restart => (Phase::Connecting, None),
                Auth::Unproven => (
                    Phase::Unproven,
                    Some(
                        "This device could not prove that it holds the pairing key, so Moby is not listening to it and nothing it sends is used. If this is your Moby, unplug it and plug it in again. If it keeps happening, unpair it and pair it again."
                            .to_owned(),
                    ),
                ),
                Auth::Authenticated => (Phase::Ready, None),
            },
        }
    }
}

fn error_code_text(code: ErrorCode) -> &'static str {
    match code {
        ErrorCode::ProtoVersion => "E_PROTO_VERSION",
        ErrorCode::BadFrame => "E_BAD_FRAME",
        ErrorCode::Crc => "E_CRC",
        ErrorCode::State => "E_STATE",
        ErrorCode::Muted => "E_MUTED",
        ErrorCode::Busy => "E_BUSY",
        ErrorCode::AudioOverrun => "E_AUDIO_OVERRUN",
        ErrorCode::Timeout => "E_TIMEOUT",
        ErrorCode::Unsupported => "E_UNSUPPORTED",
        ErrorCode::Hw => "E_HW",
    }
}

fn pairing_failure_text(code: &str) -> &'static str {
    match code {
        "timeout" => "The talk button was not pressed in time. Try again.",
        "muted" => "Moby's mute switch is on, so pairing stopped. Turn it off and try again.",
        "already_paired" => {
            "This Moby is already paired. To pair it with this computer, hold its talk button for 10 seconds to reset it first."
        }
        "busy" => "Moby is already waiting for a button press. Wait a moment, then try again.",
        "not_confirmed" => "Moby did not accept the key. Nothing was saved. Try again.",
        "no_answer" => "Moby did not answer. Check the cable and try again.",
        "storage" => {
            "The key could not be saved in Windows Credential Manager, so Moby was left unchanged. Try again."
        }
        "random" => "This computer could not make a secure key, so Moby was left unchanged.",
        "interrupted" => "The connection to Moby restarted, so pairing stopped. Try again.",
        "unconfirmed" => "Moby did not confirm the key. If it still asks to be paired, try again.",
        _ => "Moby refused to pair. Try again.",
    }
}
