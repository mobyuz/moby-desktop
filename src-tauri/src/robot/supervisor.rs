//! Keeps one robot link alive: finds the port, opens it, runs the session over it, notices when it
//! is lost, and reconnects with a growing wait. It owns no thread; the service calls `step` in a
//! loop, and tests call it with a clock they move by hand.
//!
//! Open ports are chosen only from a fresh listing of USB serial ports. The webview can pick a
//! name from that listing, never type one that gets opened without being in it.

use std::sync::Arc;
use std::time::Duration;

use moby_protocol::usb::ControlMessage;
use moby_protocol::{Expression, StableId};

use super::audio::{Assembler, AudioSink};
use super::clock::Clock;
use super::link::{Framer, Incoming, RESYNC, Stats};
use super::pairings::PairingStore;
use super::session::{DropReason, OsRandom, Random, Session, UserError};
use super::transport::{Connector, OpenError, PortInfo, PortKind, Transport};
use super::view::{Phase, Snapshot};

const READ_CHUNK: usize = 512;
/// At most this many bytes are taken from the port per step, so a flood cannot starve the loop.
const MAX_READ_PER_STEP: usize = 4096;
const READ_WAIT: Duration = Duration::from_millis(15);
const BACKOFF_PAUSE: Duration = Duration::from_millis(40);
/// The interface hears about a change at most this often. A burst of bad frames is one update.
const PUBLISH_GAP_MS: u64 = 250;
const BACKOFF_MS: [u32; 6] = [500, 1_000, 2_000, 4_000, 8_000, 10_000];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    /// The only port that looks like a Moby.
    Auto,
    /// A port the person chose from the listing.
    Port(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Connect(Target),
    Disconnect,
    BeginPairing,
    CancelPairing,
    Unpair(String),
    /// Stop capturing and handling the current push-to-talk stream, and tell the robot.
    CancelCapture,
    /// Show a face on the robot for a while (thinking, speaking, happy, error).
    Face(Expression, u32),
}

struct Backoff {
    index: usize,
}

impl Backoff {
    fn reset(&mut self) {
        self.index = 0;
    }

    fn next(&mut self) -> u32 {
        let wait = BACKOFF_MS[self.index.min(BACKOFF_MS.len() - 1)];
        self.index += 1;
        wait
    }
}

struct Link {
    transport: Box<dyn Transport>,
    framer: Framer,
    session: Session,
    assembler: Assembler,
    port: PortInfo,
    reported_ready: bool,
}

enum State {
    Idle,
    Backoff { until: u64, wait_ms: u32 },
    Online(Box<Link>),
}

pub struct Supervisor {
    clock: Arc<dyn Clock>,
    connector: Box<dyn Connector>,
    pairings: Arc<dyn PairingStore>,
    random: Box<dyn Fn() -> Box<dyn Random> + Send>,
    publish: Box<dyn FnMut(&Snapshot) + Send>,
    log: Box<dyn Fn(&str) + Send>,
    audio: AudioSink,
    state: State,
    target: Option<Target>,
    backoff: Backoff,
    problem: Option<String>,
    attempts: u32,
    last_published: Option<(Snapshot, u64)>,
}

impl Supervisor {
    pub fn new(
        clock: Arc<dyn Clock>,
        connector: Box<dyn Connector>,
        pairings: Arc<dyn PairingStore>,
        publish: Box<dyn FnMut(&Snapshot) + Send>,
    ) -> Self {
        Self {
            clock,
            connector,
            pairings,
            random: Box::new(|| Box::new(OsRandom)),
            publish,
            log: Box::new(|line| eprintln!("moby: robot link: {line}")),
            audio: std::sync::Arc::new(|_| {}),
            state: State::Idle,
            target: None,
            backoff: Backoff { index: 0 },
            problem: None,
            attempts: 0,
            last_published: None,
        }
    }

    /// Where finished utterances and aborted streams go. Without one, audio is thrown away.
    pub fn with_audio(mut self, audio: AudioSink) -> Self {
        self.audio = audio;
        self
    }

    #[cfg(test)]
    pub fn with_random(mut self, random: Box<dyn Fn() -> Box<dyn Random> + Send>) -> Self {
        self.random = random;
        self
    }

    #[cfg(test)]
    pub fn with_log(mut self, log: Box<dyn Fn(&str) + Send>) -> Self {
        self.log = log;
        self
    }

    /// Nothing to do until a command arrives, so the caller may wait for one.
    pub fn is_idle(&self) -> bool {
        matches!(self.state, State::Idle)
    }

    pub fn ports(&mut self) -> Vec<PortInfo> {
        self.connector.ports()
    }

    pub fn command(&mut self, command: Command) -> Result<(), UserError> {
        let now = self.clock.now_ms();
        match command {
            Command::Connect(target) => {
                if matches!(self.state, State::Online(_)) && self.target.as_ref() == Some(&target) {
                    return Ok(());
                }
                self.close("switching");
                self.target = Some(target);
                self.problem = None;
                self.attempts = 0;
                self.backoff.reset();
                self.state = State::Backoff {
                    until: now,
                    wait_ms: 0,
                };
                Ok(())
            }
            Command::Disconnect => {
                self.close("disconnected by the user");
                self.target = None;
                self.problem = None;
                self.state = State::Idle;
                Ok(())
            }
            Command::BeginPairing => match &mut self.state {
                State::Online(link) => link.session.begin_pairing(now),
                _ => Err(UserError {
                    code: "not_connected",
                    message: "Connect to Moby first.".to_owned(),
                }),
            },
            Command::CancelPairing => {
                if let State::Online(link) = &mut self.state {
                    link.session.cancel_pairing();
                }
                Ok(())
            }
            Command::CancelCapture => {
                if let State::Online(link) = &mut self.state {
                    link.assembler.cancel();
                    link.flush_cancels(now);
                }
                Ok(())
            }
            Command::Face(expression, hold_ms) => {
                if let State::Online(link) = &mut self.state {
                    link.session.show_face(now, expression, hold_ms);
                }
                Ok(())
            }
            Command::Unpair(id) => {
                let id = StableId::new(id).map_err(|_| UserError {
                    code: "invalid_input",
                    message: "That isn't a Moby.".to_owned(),
                })?;
                self.pairings.forget(&id).map_err(|_| UserError {
                    code: "storage",
                    message: "The pairing could not be removed. Windows Credential Manager or the history file is not available."
                        .to_owned(),
                })?;
                if let State::Online(link) = &mut self.state {
                    link.session.key_removed(&id);
                }
                Ok(())
            }
        }
    }

    /// One turn of the loop: at most a few milliseconds of work plus one short wait.
    pub fn step(&mut self) {
        let now = self.clock.now_ms();
        match std::mem::replace(&mut self.state, State::Idle) {
            State::Idle => {}
            State::Backoff { until, wait_ms } => {
                if now >= until {
                    self.try_open(now);
                } else {
                    self.state = State::Backoff { until, wait_ms };
                    self.clock.pause(BACKOFF_PAUSE);
                }
            }
            State::Online(mut link) => {
                let outcome = link.run(now, self.clock.as_ref());
                if link.session.established() && !link.reported_ready {
                    link.reported_ready = true;
                    self.backoff.reset();
                    self.attempts = 0;
                }
                match outcome {
                    Ok(()) if link.session.take_restart() => {
                        // A pairing just finished. That session had no robot proof, so the link
                        // starts over at once and the next session is a proven one.
                        drop(link);
                        self.state = State::Backoff {
                            until: now,
                            wait_ms: 0,
                        };
                    }
                    Ok(()) => self.state = State::Online(link),
                    Err(reason) => self.lost(reason, now),
                }
            }
        }
        self.publish_if_changed(self.clock.now_ms());
    }

    fn try_open(&mut self, now: u64) {
        self.attempts += 1;
        let ports = self.connector.ports();
        let chosen = match &self.target {
            Some(Target::Port(name)) => ports.iter().find(|port| &port.name == name).cloned(),
            Some(Target::Auto) => {
                let mut likely = ports.iter().filter(|port| port.kind == PortKind::Moby);
                match (likely.next(), likely.next()) {
                    (Some(only), None) => Some(only.clone()),
                    (Some(_), Some(_)) => {
                        // Two candidates: guessing could open somebody's other board.
                        self.problem = Some(
                            "More than one possible Moby is connected. Choose the port to use."
                                .to_owned(),
                        );
                        self.target = None;
                        self.state = State::Idle;
                        return;
                    }
                    _ => None,
                }
            }
            None => {
                self.state = State::Idle;
                return;
            }
        };
        let Some(port) = chosen else {
            self.problem = Some(
                "No Moby found. Plug it into a USB port of this computer with a data cable."
                    .to_owned(),
            );
            self.schedule_retry(now);
            return;
        };
        match self.connector.open(&port.name) {
            Ok(mut transport) => {
                self.problem = None;
                // A lone delimiter first: the robot's ROM may have printed text on the port.
                if transport.write_all(&RESYNC).is_err() {
                    self.problem = Some("The port closed as it opened. Trying again.".to_owned());
                    self.schedule_retry(now);
                    return;
                }
                let random = (self.random)();
                self.state = State::Online(Box::new(Link {
                    transport,
                    framer: Framer::new(),
                    session: Session::new(Arc::clone(&self.pairings), random, now),
                    assembler: Assembler::new(Arc::clone(&self.audio)),
                    port,
                    reported_ready: false,
                }));
            }
            Err(error) => {
                self.problem = Some(
                    match error {
                        OpenError::Busy => {
                            "Another program is using that port. Close it, then Moby will connect."
                        }
                        OpenError::NotFound => "That port went away. Looking for it again.",
                        OpenError::Failed => "The port could not be opened. Trying again.",
                    }
                    .to_owned(),
                );
                self.schedule_retry(now);
            }
        }
    }

    fn schedule_retry(&mut self, now: u64) {
        let wait_ms = self.backoff.next();
        self.state = State::Backoff {
            until: now + u64::from(wait_ms),
            wait_ms,
        };
    }

    fn lost(&mut self, reason: LinkEnd, now: u64) {
        (self.log)(match reason {
            LinkEnd::PortClosed => "port closed",
            LinkEnd::Dropped(DropReason::NoHello) => "no hello from the device",
            LinkEnd::Dropped(DropReason::HeartbeatLost) => "heartbeats lost",
        });
        self.problem = Some(
            match reason {
                LinkEnd::PortClosed => "Moby was unplugged or the port closed.",
                LinkEnd::Dropped(DropReason::NoHello) => {
                    "The device on that port did not answer like a Moby."
                }
                LinkEnd::Dropped(DropReason::HeartbeatLost) => {
                    "Moby stopped answering. Reconnecting."
                }
            }
            .to_owned(),
        );
        self.close("lost");
        self.schedule_retry(now);
    }

    fn close(&mut self, _why: &str) {
        // Dropping the link closes the port.
        if matches!(self.state, State::Online(_)) {
            self.state = State::Idle;
        }
    }

    pub fn snapshot(&self) -> Snapshot {
        match &self.state {
            State::Idle => Snapshot::disconnected(self.problem.clone()),
            State::Backoff { wait_ms, .. } => {
                let mut snapshot = Snapshot::disconnected(self.problem.clone());
                snapshot.phase = if *wait_ms > 0 {
                    Phase::Reconnecting
                } else {
                    Phase::Connecting
                };
                snapshot.retry_in_ms = (*wait_ms > 0).then_some(*wait_ms);
                snapshot
            }
            State::Online(link) => {
                let mut snapshot = link.session.snapshot();
                let mut stats: Stats = link.framer.stats.clone();
                stats.unexpected_messages = link.session.unexpected;
                stats.audio_frames_ignored = link.assembler.stats.frames_ignored;
                stats.audio_events_ignored = link.assembler.stats.events_ignored;
                stats.audio_streams = link.assembler.stats.streams_completed;
                stats.audio_aborted = link.assembler.stats.streams_aborted;
                snapshot.stats = stats;
                snapshot.port = Some(link.port.name.clone());
                snapshot.simulated = link.port.simulated;
                snapshot
            }
        }
    }

    fn publish_if_changed(&mut self, now: u64) {
        let snapshot = self.snapshot();
        let due = match &self.last_published {
            None => true,
            Some((last, at)) => *last != snapshot && now.saturating_sub(*at) >= PUBLISH_GAP_MS,
        };
        if due {
            (self.publish)(&snapshot);
            self.last_published = Some((snapshot, now));
        }
    }
}

enum LinkEnd {
    PortClosed,
    Dropped(DropReason),
}

impl Link {
    /// Routes one decoded frame. Audio frames and stream events go to the assembler first, so a
    /// stream is judged by the state the session was in when it arrived.
    fn handle(&mut self, now: u64, item: Incoming) {
        match item {
            Incoming::Audio {
                stream_id,
                sequence,
                payload,
            } => self.assembler.on_frame(now, stream_id, sequence, &payload),
            Incoming::Control(message) => {
                match &message {
                    ControlMessage::AudioStart { data, .. } => {
                        self.assembler
                            .on_start(now, data, self.session.audio_allowed());
                    }
                    ControlMessage::AudioEnd { data, .. } => self.assembler.on_end(data),
                    ControlMessage::Error { data, .. } => {
                        self.assembler.on_robot_error(data.code);
                    }
                    _ => {}
                }
                self.session.on_message(now, &message);
                // The mute switch ends a stream and discards what was buffered, whichever message
                // brought the news. A new session or a lost key does the same.
                if self.session.muted() {
                    self.assembler.on_mute();
                } else {
                    self.assembler.check_allowed(self.session.audio_allowed());
                }
            }
        }
        self.flush_cancels(now);
    }

    fn flush_cancels(&mut self, now: u64) {
        for stream_id in self.assembler.take_cancels() {
            self.session.cancel_stream(now, stream_id);
        }
    }

    fn run(&mut self, now: u64, clock: &dyn Clock) -> Result<(), LinkEnd> {
        let mut buf = [0_u8; READ_CHUNK];
        let mut total = 0;
        let mut wait = READ_WAIT;
        while total < MAX_READ_PER_STEP {
            let count = self
                .transport
                .read(&mut buf, wait)
                .map_err(|_| LinkEnd::PortClosed)?;
            if count == 0 {
                break;
            }
            total += count;
            wait = Duration::ZERO;
            for item in self.framer.push_all(&buf[..count]) {
                self.handle(now, item);
            }
        }
        let now = now.max(clock.now_ms());
        self.assembler.tick(now);
        self.flush_cancels(now);
        let dropped = self.session.tick(now);
        for message in self.session.take_outgoing(now) {
            if let Some(wire) = self.framer.encode(&message) {
                self.transport
                    .write_all(&wire)
                    .map_err(|_| LinkEnd::PortClosed)?;
            }
        }
        dropped.map_or(Ok(()), |reason| Err(LinkEnd::Dropped(reason)))
    }
}
