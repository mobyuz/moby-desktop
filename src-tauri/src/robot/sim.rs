//! A robot in software, for development builds and tests. It speaks the real wire protocol over
//! the same `Transport` the USB port uses, and follows what the firmware does
//! (`moby_session.c` in moby-firmware): hello at once and every second, a new attempt every 3 s,
//! a heartbeat every 2 s once acknowledged, the session ended after three heartbeats that no
//! desktop frame answered, and the pairing rules (idle and unpaired only, a talk press confirms,
//! the key is taken only after that, a long press wipes it) and, with the `usb_pcm` capability,
//! the push-to-talk stream of `claude/f4-ptt-audio` (ADR 0002): a press on an authenticated, unmuted,
//! idle robot opens a stream, frames of 640 bytes numbered from 0 follow in real time, and the
//! release, the mute switch, a `cancel`, an overrun or the limit end it. Faults can be injected
//! into the next stream. No hardware was used to check any of it.
//!
//! This whole file is compiled only for tests and for builds with debug assertions. A release
//! build contains none of it (`SIMULATOR_MARKER` below is searched for in the release binary).

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use moby_protocol::usb::{
    ControlMessage, Frame, FrameDecoder, FrameError, FrameKind, MAX_WIRE_LEN, Nonce, PairKey,
};
use moby_protocol::{Expression, PROTOCOL_VERSION, StableId, can_transition};
use serde_json::{Value, json};

use super::clock::Clock;
use super::transport::{Connector, OpenError, PortInfo, PortKind, Transport, TransportError};

/// A string that exists in the binary only if the simulator was compiled in.
pub const SIMULATOR_MARKER: &str = "moby-simulator-compiled-7f3a91c2";

const HELLO_RETRY_MS: u64 = 1_000;
const HANDSHAKE_MS: u64 = 3_000;
const HEARTBEAT_MS: u64 = 2_000;
const LIVENESS_MISSED: u32 = 3;
const ERROR_FACE_MS: u64 = 3_000;
const ERROR_GAP_MS: u64 = 500;
const MAX_OUT: usize = 16 * 1024;
const MAX_LOG: usize = 512;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Idle,
    Pairing,
    Listening,
    Thinking,
    Speaking,
}

/// A way for the next push-to-talk stream to go wrong. `at` counts frames from 0.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamFault {
    None,
    /// Frame `at` is never sent, so the sequence has a gap.
    SkipFrame(u32),
    /// Frame `at` is sent twice.
    DuplicateFrame(u32),
    /// Frames `at` and `at + 1` are sent in the wrong order.
    SwapFrames(u32),
    /// Frame `at` has half the announced size.
    WrongSize(u32),
    /// At frame `at` the robot's ring overflows: `E_AUDIO_OVERRUN`, then `audio_end` with `error`.
    Overrun(u32),
    /// After frame `at` the robot says nothing more: no frames, no `audio_end`.
    CutOff(u32),
    /// The robot never lets go: frames keep coming as fast as the link takes them.
    NeverEnds,
    /// `audio_end` claims one frame more than was sent.
    WrongCount,
    /// Frame `at` carries another stream's ID.
    WrongStream(u32),
    /// The mute switch goes on after frame `at`, as it would in a hand.
    MuteAt(u32),
    /// The mute switch goes on after frame `at`, and a misbehaving robot keeps sending frames and
    /// never sends `audio_end`.
    MuteAndKeepSending(u32),
    /// The robot cancels its own stream after frame `at`.
    RobotCancel(u32),
}

struct Talk {
    id: u16,
    started_at: u64,
    /// Frames made so far, whether or not they were sent.
    produced: u32,
    wire_seq: u32,
    released_at: Option<u32>,
    fault: StreamFault,
    dead: bool,
}

const FRAME_MS: u64 = 20;
const FRAME_BYTES: usize = 640;
/// Room the sim leaves in its output buffer before it makes more audio.
const AUDIO_ROOM: usize = 8 * 1024;
/// The firmware's own limit, 15 s of 20 ms frames.
const SIM_MAX_FRAMES: u32 = 750;
/// What the simulated microphone "hears", repeated: a pattern a test can search for.
pub const AUDIO_MARKER: [u8; 8] = [0x5A, 0xC3, 0x91, 0x7E, 0x2B, 0xD4, 0x08, 0xF6];

#[derive(Debug, Clone, Default)]
pub struct SimStats {
    pub streams_started: u32,
    pub frames_sent: u32,
    pub sessions_started: u32,
    pub sessions_dropped_for_silence: u32,
    pub heartbeats_sent: u32,
    pub hellos_sent: u32,
    pub desktop_frames: u32,
    pub bad_frames_seen: u32,
    pub audio_frames_rejected: u32,
}

pub struct SimRobot {
    device_id: String,
    protocol_version: u32,
    capabilities: Vec<&'static str>,
    key: Option<PairKey>,
    muted: bool,
    link_up: bool,
    unplugged: bool,
    silent: bool,
    established: bool,
    authenticated: bool,
    hello_open: bool,
    hello_rejected: bool,
    hello_id: String,
    nonce: [u8; 16],
    hello_sent_at: u64,
    attempt_at: u64,
    session_id: String,
    unanswered: u32,
    next_heartbeat: u64,
    state: State,
    face: Expression,
    pair_confirmed: bool,
    pair_deadline: u64,
    error_until: Option<u64>,
    expression_until: Option<u64>,
    press_announced: bool,
    event_counter: u32,
    rng: u64,
    decoder: Box<FrameDecoder>,
    tx_seq: u32,
    out: VecDeque<u8>,
    error_sent_at: Option<u64>,
    corrupt_next: u32,
    talk: Option<Talk>,
    next_fault: StreamFault,
    next_stream_id: u16,
    now: u64,
    pub stats: SimStats,
    /// When each desktop control frame arrived, and its name. Bounded.
    pub received: VecDeque<(u64, String)>,
}

impl SimRobot {
    pub fn new(device_id: &str) -> Self {
        // Keeps the marker in every binary that contains the simulator (tests too), so the
        // release check has something to find in a binary that must have it.
        std::hint::black_box(SIMULATOR_MARKER);
        Self {
            device_id: device_id.to_owned(),
            protocol_version: PROTOCOL_VERSION,
            capabilities: Vec::new(),
            key: None,
            muted: false,
            link_up: false,
            unplugged: false,
            silent: false,
            established: false,
            authenticated: false,
            hello_open: false,
            hello_rejected: false,
            hello_id: String::new(),
            nonce: [0; 16],
            hello_sent_at: 0,
            attempt_at: 0,
            session_id: String::new(),
            unanswered: 0,
            next_heartbeat: 0,
            state: State::Idle,
            face: Expression::Idle,
            pair_confirmed: false,
            pair_deadline: 0,
            error_until: None,
            expression_until: None,
            press_announced: false,
            event_counter: 1,
            rng: 0x9E37_79B9_7F4A_7C15,
            decoder: Box::default(),
            tx_seq: 0,
            out: VecDeque::new(),
            error_sent_at: None,
            corrupt_next: 0,
            talk: None,
            next_fault: StreamFault::None,
            next_stream_id: 1,
            now: 0,
            stats: SimStats::default(),
            received: VecDeque::new(),
        }
    }

    // ---- controls for tests and the development panel ------------------------------------

    pub fn with_key(mut self, key: PairKey) -> Self {
        self.key = Some(key);
        self
    }

    /// Capabilities the hello advertises. `usb_pcm` turns on the push-to-talk stream.
    pub fn with_capabilities(mut self, capabilities: &[&'static str]) -> Self {
        self.capabilities = capabilities.to_vec();
        self
    }

    pub fn with_protocol_version(mut self, version: u32) -> Self {
        self.protocol_version = version;
        self
    }

    /// Everything the robot has written so far, as a test would read it from the port.
    #[cfg(test)]
    pub fn take_output(&mut self) -> Vec<u8> {
        self.out.drain(..).collect()
    }

    #[cfg(test)]
    pub fn is_paired(&self) -> bool {
        self.key.is_some()
    }

    #[cfg(test)]
    pub fn key_text(&self) -> Option<String> {
        self.key.as_ref().map(|key| key.as_str().to_owned())
    }

    #[cfg(test)]
    pub fn is_established(&self) -> bool {
        self.established
    }

    #[cfg(test)]
    pub fn is_authenticated(&self) -> bool {
        self.authenticated
    }

    #[cfg(test)]
    pub fn is_waiting_for_button(&self) -> bool {
        self.state == State::Pairing
    }

    /// The hardware mute switch.
    pub fn set_mute(&mut self, now: u64, muted: bool) {
        self.step(now);
        self.apply_mute(muted);
    }

    fn apply_mute(&mut self, muted: bool) {
        if muted == self.muted {
            return;
        }
        let was_pairing = self.state == State::Pairing;
        self.muted = muted;
        self.pair_confirmed = false;
        self.expression_until = None;
        self.error_until = None;
        self.face = Expression::Idle;
        let streaming = self.talk.is_some();
        if self.established {
            self.state = State::Idle;
            self.event("mute_changed", json!({"muted": muted, "source": "switch"}));
            if muted && streaming {
                self.end_stream("mute");
            }
            if muted && was_pairing {
                self.emit_error("E_MUTED", "pairing cancelled by mute");
            }
        }
    }

    /// A short press of the talk button.
    pub fn press(&mut self, now: u64) {
        self.step(now);
        if !self.established || self.muted {
            return;
        }
        if self.state == State::Pairing && !self.pair_confirmed {
            self.pair_confirmed = true;
            self.event("pair_button_confirmed", json!({}));
        } else {
            self.event("button_event", json!({"button": "talk", "action": "press"}));
            self.press_announced = true;
            self.maybe_start_stream(now);
        }
    }

    /// The firmware's rule: a stream starts on a press edge, only on an authenticated, unmuted,
    /// idle robot that offers `usb_pcm`, and never while another is open.
    fn maybe_start_stream(&mut self, now: u64) {
        if !self.capabilities.contains(&"usb_pcm")
            || !self.authenticated
            || self.muted
            || self.state != State::Idle
            || self.talk.is_some()
        {
            return;
        }
        let id = self.next_stream_id;
        self.next_stream_id = self.next_stream_id.checked_add(1).unwrap_or(1);
        self.stats.streams_started += 1;
        self.state = State::Listening;
        self.face = Expression::Listening;
        self.expression_until = None;
        let fault = std::mem::replace(&mut self.next_fault, StreamFault::None);
        self.talk = Some(Talk {
            id,
            started_at: now,
            produced: 0,
            wire_seq: 0,
            released_at: None,
            fault,
            dead: false,
        });
        self.event(
            "audio_start",
            json!({"stream_id": id, "reason": "button", "codec": "pcm_s16le",
                   "sample_rate": 16000, "channels": 1, "frame_ms": 20, "preroll_ms": 0}),
        );
    }

    /// The next stream will go wrong in this way.
    pub fn inject_stream_fault(&mut self, fault: StreamFault) {
        self.next_fault = fault;
    }

    /// The face the robot shows right now.
    #[cfg(test)]
    pub fn current_face(&self) -> Expression {
        self.effective()
    }

    #[cfg(test)]
    pub fn is_streaming(&self) -> bool {
        self.talk.is_some()
    }

    fn audio_out(&mut self, stream_id: u16, sequence: u32, size: usize) {
        if !self.link_up || self.silent {
            return;
        }
        let payload: Vec<u8> = AUDIO_MARKER.iter().copied().cycle().take(size).collect();
        let frame = Frame {
            kind: FrameKind::RobotAudio,
            stream_id,
            sequence,
            payload: &payload,
        };
        let mut wire = vec![0; MAX_WIRE_LEN];
        let Ok(written) = frame.encode(&mut wire) else {
            return;
        };
        for byte in &wire[..written] {
            if self.out.len() >= MAX_OUT {
                return;
            }
            self.out.push_back(*byte);
        }
        self.stats.frames_sent += 1;
    }

    /// Ends the open stream the way the firmware does: capture is off already, `audio_end` says
    /// why, and `frames` counts what was sent.
    fn end_stream(&mut self, reason: &str) {
        let Some(talk) = self.talk.take() else {
            return;
        };
        if self.state == State::Listening {
            self.state = State::Idle;
            self.face = Expression::Idle;
        }
        if talk.dead {
            return;
        }
        let mut frames = talk.produced;
        if talk.fault == StreamFault::WrongCount {
            frames += 1;
        }
        self.event(
            "audio_end",
            json!({"stream_id": talk.id, "reason": reason, "frames": frames,
                   "duration_ms": frames * 20}),
        );
    }

    /// Makes the audio that is due, in real time, as long as the link has room.
    fn advance_stream(&mut self, now: u64) {
        while self.out.len() < AUDIO_ROOM {
            let Some(talk) = &mut self.talk else {
                return;
            };
            if talk.dead {
                return;
            }
            let elapsed =
                u32::try_from(now.saturating_sub(talk.started_at) / FRAME_MS).unwrap_or(u32::MAX);
            let target = match (talk.released_at, talk.fault) {
                (Some(at), _) => at,
                (None, StreamFault::NeverEnds) => u32::MAX,
                (None, StreamFault::MuteAndKeepSending(_)) => elapsed,
                (None, _) => elapsed.min(SIM_MAX_FRAMES),
            };
            if talk.produced >= target {
                if talk.released_at.is_some() {
                    self.end_stream("button_release");
                } else if talk.produced >= SIM_MAX_FRAMES && talk.fault != StreamFault::NeverEnds {
                    self.end_stream("max_duration");
                }
                return;
            }
            let (id, index, seq, fault) = (talk.id, talk.produced, talk.wire_seq, talk.fault);
            talk.produced += 1;
            talk.wire_seq += 1;
            match fault {
                StreamFault::SkipFrame(at) if at == index => {}
                StreamFault::DuplicateFrame(at) if at == index => {
                    self.audio_out(id, seq, FRAME_BYTES);
                    self.audio_out(id, seq, FRAME_BYTES);
                }
                StreamFault::SwapFrames(at) if at == index => {
                    // Frame `at + 1` goes first.
                    if let Some(talk) = &mut self.talk {
                        talk.produced += 1;
                        talk.wire_seq += 1;
                    }
                    self.audio_out(id, seq + 1, FRAME_BYTES);
                    self.audio_out(id, seq, FRAME_BYTES);
                }
                StreamFault::WrongSize(at) if at == index => {
                    self.audio_out(id, seq, FRAME_BYTES / 2);
                }
                StreamFault::WrongStream(at) if at == index => {
                    self.audio_out(id.wrapping_add(1).max(1), seq, FRAME_BYTES);
                }
                StreamFault::Overrun(at) if at == index => {
                    self.emit_error("E_AUDIO_OVERRUN", "capture ring overflowed");
                    self.end_stream("error");
                    return;
                }
                StreamFault::CutOff(at) if at == index => {
                    if let Some(talk) = &mut self.talk {
                        talk.dead = true;
                    }
                    return;
                }
                _ => self.audio_out(id, seq, FRAME_BYTES),
            }
            match fault {
                StreamFault::MuteAt(at) if at == index => {
                    self.apply_mute(true);
                    return;
                }
                StreamFault::MuteAndKeepSending(at) if at == index => {
                    // The switch goes on, the event goes out, and this robot carries on anyway.
                    self.muted = true;
                    self.event("mute_changed", json!({"muted": true, "source": "switch"}));
                }
                StreamFault::RobotCancel(at) if at == index => {
                    self.end_stream("cancel");
                    return;
                }
                _ => {}
            }
        }
    }

    pub fn release(&mut self, now: u64) {
        self.step(now);
        if let Some(talk) = &mut self.talk
            && talk.released_at.is_none()
        {
            talk.released_at = Some(
                u32::try_from(now.saturating_sub(talk.started_at) / FRAME_MS)
                    .unwrap_or(u32::MAX)
                    .min(SIM_MAX_FRAMES)
                    .max(talk.produced),
            );
        }
        if self.press_announced {
            self.press_announced = false;
            self.event(
                "button_event",
                json!({"button": "talk", "action": "release"}),
            );
        }
    }

    /// The ten second hold, without waiting ten seconds: the button event and the wipe.
    pub fn long_press(&mut self, now: u64) {
        self.step(now);
        if self.established {
            self.event(
                "button_event",
                json!({"button": "talk", "action": "long_press"}),
            );
        }
        self.key = None;
        self.authenticated = false;
        self.pair_confirmed = false;
        self.end_stream("cancel");
        if self.state == State::Pairing {
            self.state = State::Idle;
        }
        if self.established {
            self.event("pair_reset", json!({}));
        }
    }

    /// Pretends the robot is in a state the desktop would see in a heartbeat.
    pub fn show(&mut self, now: u64, face: Expression) {
        self.step(now);
        if self.established && !self.muted && self.state != State::Pairing {
            self.face = face;
            self.state = match face {
                Expression::Listening => State::Listening,
                Expression::Thinking => State::Thinking,
                Expression::Speaking => State::Speaking,
                _ => State::Idle,
            };
        }
    }

    pub fn set_silent(&mut self, silent: bool) {
        self.silent = silent;
    }

    pub fn unplug(&mut self) {
        self.unplugged = true;
    }

    pub fn plug(&mut self) {
        self.unplugged = false;
    }

    /// Damages the next `count` frames the robot sends.
    pub fn corrupt_next_frames(&mut self, count: u32) {
        self.corrupt_next = count;
    }

    /// A segment longer than any frame, then a delimiter.
    pub fn inject_oversized(&mut self) {
        for _ in 0..1_500 {
            self.out.push_back(0xAA);
        }
        self.out.push_back(0);
    }

    /// Bytes that are not a frame, like the ROM's boot text.
    pub fn inject_garbage(&mut self) {
        for byte in b"ESP-ROM:esp32s3-20210327\r\nBuild:Mar 27 2021\r\n" {
            self.out.push_back(*byte);
        }
        self.out.push_back(0);
    }

    /// A frame of robot audio, which a robot without the capability would never send.
    #[cfg(test)]
    pub fn inject_audio_frame(&mut self) {
        let frame = Frame {
            kind: FrameKind::RobotAudio,
            stream_id: 1,
            sequence: 0,
            payload: &[0; 640],
        };
        let mut wire = vec![0; MAX_WIRE_LEN];
        if let Ok(written) = frame.encode(&mut wire) {
            for byte in &wire[..written] {
                self.out.push_back(*byte);
            }
        }
    }

    /// Sends one raw control payload in a valid frame, for malformed-message tests.
    #[cfg(test)]
    pub fn inject_payload(&mut self, payload: &[u8]) {
        self.frame_out(payload);
    }

    // ---- the link ------------------------------------------------------------------------

    pub fn on_link_up(&mut self, now: u64) {
        self.now = now;
        self.link_up = true;
        self.decoder.reset();
        self.out.clear();
        self.end_quietly();
        self.start_attempt();
    }

    pub fn on_link_down(&mut self) {
        self.link_up = false;
        self.out.clear();
        self.end_quietly();
    }

    fn end_quietly(&mut self) {
        self.established = false;
        self.authenticated = false;
        self.session_id.clear();
        self.pair_confirmed = false;
        self.error_until = None;
        self.expression_until = None;
        self.face = Expression::Idle;
        self.unanswered = 0;
        self.hello_open = false;
        self.hello_rejected = false;
        self.press_announced = false;
        self.nonce = [0; 16];
        self.state = State::Idle;
        self.talk = None;
    }

    fn effective(&self) -> Expression {
        if self.muted {
            Expression::Muted
        } else if !self.established {
            Expression::Disconnected
        } else if self.error_until.is_some() {
            Expression::Error
        } else {
            self.face
        }
    }

    /// Advances the robot's own timers to `now`.
    pub fn step(&mut self, now: u64) {
        self.now = now;
        if self.unplugged || !self.link_up || self.silent {
            return;
        }
        if self.error_until.is_some_and(|until| now >= until) {
            self.error_until = None;
        }
        if self.expression_until.is_some_and(|until| now >= until) {
            self.expression_until = None;
            self.face = Expression::Idle;
            if self.state != State::Pairing {
                self.state = State::Idle;
            }
        }
        if self.state == State::Pairing && now >= self.pair_deadline {
            self.state = State::Idle;
            self.pair_confirmed = false;
            self.raise_error("E_TIMEOUT", "pairing window expired");
        }
        if !self.established {
            if !self.hello_open {
                if now >= self.hello_sent_at + HELLO_RETRY_MS {
                    self.start_attempt();
                }
            } else if now >= self.attempt_at + HANDSHAKE_MS {
                self.start_attempt();
            } else if !self.hello_rejected && now >= self.hello_sent_at + HELLO_RETRY_MS {
                self.send_hello();
            }
            return;
        }
        self.advance_stream(now);
        if now >= self.next_heartbeat {
            if self.unanswered >= LIVENESS_MISSED {
                self.stats.sessions_dropped_for_silence += 1;
                self.end_quietly();
                self.out.clear();
                self.start_attempt();
                return;
            }
            let state = self.effective();
            self.event(
                "heartbeat",
                json!({
                    "state": state,
                    "mute": self.muted,
                    "uptime_ms": u32::try_from(now & 0xFFFF_FFFF).unwrap_or(0),
                }),
            );
            self.stats.heartbeats_sent += 1;
            self.unanswered += 1;
            self.next_heartbeat = now + HEARTBEAT_MS;
        }
    }

    fn random_bytes<const N: usize>(&mut self) -> [u8; N] {
        let mut bytes = [0_u8; N];
        for byte in &mut bytes {
            self.rng ^= self.rng << 13;
            self.rng ^= self.rng >> 7;
            self.rng ^= self.rng << 17;
            *byte = (self.rng >> 24) as u8;
        }
        bytes
    }

    fn next_event_id(&mut self) -> String {
        let id = format!("e-{:04}", self.event_counter % 10_000);
        self.event_counter += 1;
        id
    }

    fn start_attempt(&mut self) {
        self.hello_sent_at = self.now;
        self.hello_open = false;
        self.hello_rejected = false;
        self.nonce = self.random_bytes();
        self.hello_id = self.next_event_id();
        self.hello_open = true;
        self.attempt_at = self.now;
        self.send_hello();
    }

    fn send_hello(&mut self) {
        self.hello_sent_at = self.now;
        self.stats.hellos_sent += 1;
        let nonce = Nonce::from_bytes(self.nonce);
        let value = json!({
            "v": 1, "kind": "event", "name": "hello", "id": self.hello_id,
            "ts_ms": self.ts(),
            "data": {
                "device_id": self.device_id, "hw_rev": "sim-r1", "fw_version": "0.0.0-sim",
                "protocol_version": self.protocol_version, "capabilities": self.capabilities,
                "mute": self.muted, "paired": self.key.is_some(), "nonce": nonce.as_str(),
            }
        });
        self.send_value(&value);
    }

    fn ts(&self) -> u32 {
        u32::try_from(self.now & 0xFFFF_FFFF).unwrap_or(0)
    }

    fn event(&mut self, name: &str, data: Value) {
        if !self.link_up || self.silent {
            return;
        }
        let id = self.next_event_id();
        let value = json!({"v":1,"kind":"event","name":name,"id":id,"ts_ms":self.ts(),"data":data});
        self.send_value(&value);
    }

    fn respond(&mut self, id: &str, error: Option<(&str, &str)>) {
        if !self.link_up || self.silent {
            return;
        }
        let value = match error {
            None => {
                json!({"v":1,"kind":"response","name":"command_ok","id":id,"ts_ms":self.ts(),"data":{}})
            }
            Some((code, detail)) => json!({
                "v":1,"kind":"response","name":"command_error","id":id,"ts_ms":self.ts(),
                "data":{"code":code,"detail":detail}
            }),
        };
        self.send_value(&value);
    }

    fn emit_error(&mut self, code: &str, detail: &str) {
        self.event("error", json!({"code": code, "detail": detail}));
    }

    /// An error the robot raises itself. It also shows the error face for a moment.
    fn raise_error(&mut self, code: &str, detail: &str) {
        self.emit_error(code, detail);
        if !self.muted && self.established {
            self.error_until = Some(self.now + ERROR_FACE_MS);
        }
    }

    /// Protocol errors from the wire go out at most once per 500 ms.
    fn protocol_error(&mut self, code: &str, detail: &str) {
        if self
            .error_sent_at
            .is_some_and(|at| self.now.saturating_sub(at) < ERROR_GAP_MS)
        {
            return;
        }
        self.error_sent_at = Some(self.now);
        self.emit_error(code, detail);
    }

    fn send_value(&mut self, value: &Value) {
        if let Ok(payload) = serde_json::to_vec(value) {
            self.frame_out(&payload);
        }
    }

    fn frame_out(&mut self, payload: &[u8]) {
        let frame = Frame {
            kind: FrameKind::Control,
            stream_id: 0,
            sequence: self.tx_seq,
            payload,
        };
        self.tx_seq = self.tx_seq.wrapping_add(1);
        let mut wire = vec![0; MAX_WIRE_LEN];
        let Ok(written) = frame.encode(&mut wire) else {
            return;
        };
        wire.truncate(written);
        if self.corrupt_next > 0 {
            self.corrupt_next -= 1;
            let index = wire.len() / 2;
            wire[index] ^= 0x5A;
        }
        for byte in wire {
            if self.out.len() >= MAX_OUT {
                return;
            }
            self.out.push_back(byte);
        }
    }

    // ---- bytes from the desktop ------------------------------------------------------------

    pub fn receive(&mut self, now: u64, bytes: &[u8]) {
        self.step(now);
        if !self.link_up {
            return;
        }
        for &byte in bytes {
            let Some(result) = self.decoder.push(byte) else {
                continue;
            };
            match result {
                Ok(frame) => match frame.kind {
                    FrameKind::Control => {
                        let payload = frame.payload.to_vec();
                        self.on_frame(&payload);
                    }
                    FrameKind::DesktopAudio => {
                        self.stats.audio_frames_rejected += 1;
                        self.touch();
                    }
                    FrameKind::RobotAudio => {
                        self.stats.bad_frames_seen += 1;
                        self.protocol_error("E_BAD_FRAME", "unexpected frame kind");
                    }
                },
                Err(FrameError::BadCrc) => {
                    self.stats.bad_frames_seen += 1;
                    self.protocol_error("E_CRC", "frame checksum mismatch");
                }
                Err(_) => {
                    self.stats.bad_frames_seen += 1;
                    self.protocol_error("E_BAD_FRAME", "invalid frame");
                }
            }
        }
    }

    /// Any CRC-valid desktop frame counts as proof of life.
    fn touch(&mut self) {
        self.stats.desktop_frames += 1;
        if self.established {
            self.unanswered = 0;
        }
    }

    fn on_frame(&mut self, payload: &[u8]) {
        self.touch();
        let Ok(message) = serde_json::from_slice::<ControlMessage>(payload) else {
            self.protocol_error("E_BAD_FRAME", "invalid control message");
            return;
        };
        if self.received.len() >= MAX_LOG {
            self.received.pop_front();
        }
        self.received
            .push_back((self.now, message.name().to_owned()));
        match &message {
            ControlMessage::HelloAck { id, data, .. } => self.on_hello_ack(id, data),
            _ if message.is_command() => self.on_command(&message),
            // Robot-side messages and unknown senders are ignored.
            _ => {}
        }
    }

    fn on_hello_ack(&mut self, id: &StableId, ack: &moby_protocol::usb::HelloAck) {
        if self.established || !self.hello_open || id.as_str() != self.hello_id {
            return;
        }
        if !ack.accepted {
            self.hello_rejected = true;
            return;
        }
        let Some(session_id) = &ack.session_id else {
            self.protocol_error("E_BAD_FRAME", "hello_ack without session_id");
            return;
        };
        let proven = match &self.key {
            None => true,
            Some(key) => match (&ack.proof, StableId::new(self.device_id.clone())) {
                (Some(proof), Ok(device)) => key.verify_session_proof(
                    &Nonce::from_bytes(self.nonce),
                    &device,
                    session_id,
                    proof,
                ),
                _ => false,
            },
        };
        self.authenticated = self.key.is_some() && proven;
        self.established = true;
        self.hello_open = false;
        self.session_id = session_id.as_str().to_owned();
        self.nonce = [0; 16];
        self.unanswered = 0;
        self.next_heartbeat = self.now + HEARTBEAT_MS;
        self.face = Expression::Idle;
        self.state = State::Idle;
        self.stats.sessions_started += 1;
        self.event(
            "mute_changed",
            json!({"muted": self.muted, "source": "boot"}),
        );
        if !proven {
            self.raise_error("E_STATE", "authentication failed");
        }
    }

    fn on_command(&mut self, message: &ControlMessage) {
        let id = message.id().as_str().to_owned();
        if !self.established {
            self.respond(&id, Some(("E_STATE", "no session")));
            return;
        }
        // A missing capability is checked before authentication, like the firmware does.
        match message {
            ControlMessage::AudioPlayStart { .. } | ControlMessage::AudioPlayEnd { .. } => {
                let has = self.capabilities.contains(&"usb_pcm");
                self.respond(
                    &id,
                    Some(if has {
                        ("E_STATE", "audio playback not implemented")
                    } else {
                        ("E_UNSUPPORTED", "audio unsupported")
                    }),
                );
                return;
            }
            ControlMessage::ServoPose { .. } => {
                let has = self.capabilities.contains(&"servo_pose");
                self.respond(
                    &id,
                    Some(if has {
                        ("E_STATE", "servo not implemented")
                    } else {
                        ("E_UNSUPPORTED", "servo unsupported")
                    }),
                );
                return;
            }
            _ => {}
        }
        if self.key.is_some() && !self.authenticated {
            self.respond(&id, Some(("E_STATE", "not authenticated")));
            return;
        }
        match message {
            ControlMessage::Expression { data, .. } => {
                self.on_expression(&id, data);
            }
            ControlMessage::DisplayText { .. } => {
                if self.muted {
                    self.respond(&id, Some(("E_MUTED", "hardware mute is on")));
                } else {
                    self.respond(&id, None);
                }
            }
            ControlMessage::Cancel { data, .. } => {
                if self
                    .talk
                    .as_ref()
                    .is_some_and(|talk| talk.id == data.stream_id.get())
                {
                    // Capture stops, buffered audio is dropped, `audio_end` says `cancel`.
                    self.end_stream("cancel");
                    self.respond(&id, None);
                } else {
                    self.respond(&id, Some(("E_STATE", "no such stream")));
                }
            }
            ControlMessage::SetConfig { .. } => self.respond(&id, None),
            ControlMessage::PairBegin { data, .. } => {
                self.on_pair_begin(&id, data.timeout_ms.get())
            }
            ControlMessage::PairSetKey { data, .. } => self.on_pair_set_key(&id, &data.pair_key),
            _ => self.respond(&id, Some(("E_UNSUPPORTED", "unsupported command"))),
        }
    }

    fn on_expression(&mut self, id: &str, data: &moby_protocol::usb::SetExpression) {
        let target = data.state;
        if self.muted {
            self.respond(id, Some(("E_MUTED", "hardware mute is on")));
        } else if self.state == State::Pairing {
            self.respond(id, Some(("E_BUSY", "pairing in progress")));
        } else if matches!(target, Expression::Muted | Expression::Disconnected) {
            self.respond(id, Some(("E_STATE", "the robot owns this state")));
        } else if !matches!(data.animation.as_str(), "none" | "blink") {
            self.respond(id, Some(("E_UNSUPPORTED", "unknown animation")));
        } else if !can_transition(self.effective(), target) {
            self.respond(id, Some(("E_STATE", "transition not allowed")));
        } else {
            self.error_until = None;
            self.face = target;
            let duration = u64::from(data.duration_ms.get());
            self.expression_until =
                (duration != 0 && target != Expression::Idle).then_some(self.now + duration);
            self.state = match target {
                Expression::Listening => State::Listening,
                Expression::Thinking => State::Thinking,
                Expression::Speaking => State::Speaking,
                _ => State::Idle,
            };
            self.respond(id, None);
        }
    }

    fn on_pair_begin(&mut self, id: &str, timeout_ms: u32) {
        if self.muted {
            self.respond(id, Some(("E_MUTED", "hardware mute is on")));
        } else if self.key.is_some() {
            self.respond(id, Some(("E_STATE", "already paired")));
        } else if self.state == State::Pairing {
            self.respond(id, Some(("E_BUSY", "pairing already waiting")));
        } else if self.state != State::Idle {
            self.respond(id, Some(("E_STATE", "robot is not idle")));
        } else {
            self.pair_confirmed = false;
            self.pair_deadline = self.now + u64::from(timeout_ms);
            self.face = Expression::Idle;
            self.expression_until = None;
            self.state = State::Pairing;
            self.respond(id, None);
        }
    }

    fn on_pair_set_key(&mut self, id: &str, key: &PairKey) {
        if self.key.is_some() || self.state != State::Pairing || !self.pair_confirmed {
            self.respond(id, Some(("E_STATE", "pairing was not confirmed")));
        } else {
            self.key = Some(key.clone());
            self.authenticated = true;
            self.pair_confirmed = false;
            self.state = State::Idle;
            self.respond(id, None);
        }
    }
}

// ---- transport ----------------------------------------------------------------------------

fn lock(robot: &Mutex<SimRobot>) -> MutexGuard<'_, SimRobot> {
    robot.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The robot's end of a link. Dropping it is the cable being pulled.
pub struct SimTransport {
    robot: Arc<Mutex<SimRobot>>,
    clock: Arc<dyn Clock>,
}

impl Transport for SimTransport {
    fn read(&mut self, buf: &mut [u8], wait: Duration) -> Result<usize, TransportError> {
        let count = {
            let mut robot = lock(&self.robot);
            if robot.unplugged {
                return Err(TransportError::Closed);
            }
            robot.step(self.clock.now_ms());
            let count = buf.len().min(robot.out.len());
            for slot in buf.iter_mut().take(count) {
                *slot = robot.out.pop_front().unwrap_or(0);
            }
            count
        };
        if count == 0 {
            self.clock.pause(wait.min(Duration::from_millis(10)));
        }
        Ok(count)
    }

    fn write_all(&mut self, data: &[u8]) -> Result<(), TransportError> {
        let mut robot = lock(&self.robot);
        if robot.unplugged {
            return Err(TransportError::Closed);
        }
        robot.receive(self.clock.now_ms(), data);
        Ok(())
    }
}

impl Drop for SimTransport {
    fn drop(&mut self) {
        lock(&self.robot).on_link_down();
    }
}

/// A fake serial port that leads to a `SimRobot`.
pub struct SimConnector {
    pub robot: Arc<Mutex<SimRobot>>,
    pub clock: Arc<dyn Clock>,
    pub port: String,
}

impl SimConnector {
    pub fn new(robot: Arc<Mutex<SimRobot>>, clock: Arc<dyn Clock>) -> Self {
        Self {
            robot,
            clock,
            port: "SIMULATOR".to_owned(),
        }
    }
}

impl Connector for SimConnector {
    fn ports(&mut self) -> Vec<PortInfo> {
        if lock(&self.robot).unplugged {
            return Vec::new();
        }
        vec![PortInfo {
            name: self.port.clone(),
            kind: PortKind::Moby,
            label: "Moby simulator".to_owned(),
            simulated: true,
            vid: None,
            pid: None,
        }]
    }

    fn open(&mut self, name: &str) -> Result<Box<dyn Transport>, OpenError> {
        if name != self.port {
            return Err(OpenError::NotFound);
        }
        {
            let mut robot = lock(&self.robot);
            if robot.unplugged {
                return Err(OpenError::NotFound);
            }
            robot.on_link_up(self.clock.now_ms());
        }
        Ok(Box::new(SimTransport {
            robot: Arc::clone(&self.robot),
            clock: Arc::clone(&self.clock),
        }))
    }
}

/// Real ports plus the simulator's, for development builds.
pub struct CombinedConnector {
    pub real: Box<dyn Connector>,
    pub sim: SimConnector,
    pub sim_enabled: Arc<Mutex<bool>>,
}

impl Connector for CombinedConnector {
    fn ports(&mut self) -> Vec<PortInfo> {
        let mut ports = self.real.ports();
        if *self
            .sim_enabled
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
        {
            ports.extend(self.sim.ports());
        }
        ports
    }

    fn open(&mut self, name: &str) -> Result<Box<dyn Transport>, OpenError> {
        if name == self.sim.port {
            if !*self
                .sim_enabled
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
            {
                return Err(OpenError::NotFound);
            }
            self.sim.open(name)
        } else {
            self.real.open(name)
        }
    }
}
