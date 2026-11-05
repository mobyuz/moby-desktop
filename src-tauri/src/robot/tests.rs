//! Tests for the robot link, run against the simulator on a clock the tests move by hand: a test
//! that covers a minute of robot time takes milliseconds, and nothing depends on real timing.

use std::sync::{Arc, Mutex, PoisonError};

use moby_protocol::usb::{ControlMessage, Frame, FrameKind, MAX_WIRE_LEN, PairKey};
use moby_protocol::{Expression, StableId};
use serde_json::json;

use super::audio::{AudioEvent, AudioSink};
use super::clock::{Clock, ManualClock};
use super::link::Framer;
use super::pairings::{MemoryPairings, PairingStore};
use super::session::{
    DropReason, HEARTBEAT_LOSS_MS, HELLO_WAIT_MS, Random, Session, incompatibility,
};
use super::sim::{SimConnector, SimRobot};
use super::supervisor::{Command, Supervisor, Target};
use super::view::{PairStep, Phase, Snapshot};

const DEVICE: &str = "dev_0123456789abcdef0123456789abcdef";

fn key(byte: u8) -> PairKey {
    PairKey::from_bytes([byte; 32])
}

/// Predictable "random" bytes, so a test knows what the desktop would have made.
struct SeqRandom(u8);

impl Random for SeqRandom {
    fn fill(&mut self, buf: &mut [u8]) -> bool {
        for byte in buf {
            self.0 = self.0.wrapping_add(1);
            *byte = self.0;
        }
        true
    }
}

struct Rig {
    clock: Arc<ManualClock>,
    robot: Arc<Mutex<SimRobot>>,
    sup: Supervisor,
    pairings: Arc<MemoryPairings>,
    logs: Arc<Mutex<Vec<String>>>,
    published: Arc<Mutex<Vec<Snapshot>>>,
}

fn rig(robot: SimRobot, pairings: MemoryPairings) -> Rig {
    rig_with_audio(robot, pairings, Arc::new(|_| {}))
}

fn rig_with_audio(robot: SimRobot, pairings: MemoryPairings, audio: AudioSink) -> Rig {
    let clock = Arc::new(ManualClock::new());
    let robot = Arc::new(Mutex::new(robot));
    let connector = SimConnector::new(Arc::clone(&robot), Arc::clone(&clock) as Arc<dyn Clock>);
    let pairings = Arc::new(pairings);
    let logs = Arc::new(Mutex::new(Vec::new()));
    let published = Arc::new(Mutex::new(Vec::new()));
    let sup = Supervisor::new(
        Arc::clone(&clock) as Arc<dyn Clock>,
        Box::new(connector),
        Arc::clone(&pairings) as Arc<dyn PairingStore>,
        {
            let published = Arc::clone(&published);
            Box::new(move |snapshot| {
                published
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .push(snapshot.clone());
            })
        },
    )
    .with_audio(audio)
    .with_random(Box::new(|| Box::new(SeqRandom(0))))
    .with_log({
        let logs = Arc::clone(&logs);
        Box::new(move |line| {
            logs.lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(line.to_owned());
        })
    });
    Rig {
        clock,
        robot,
        sup,
        pairings,
        logs,
        published,
    }
}

fn unpaired_rig() -> Rig {
    rig(SimRobot::new(DEVICE), MemoryPairings::default())
}

impl Rig {
    /// Runs the link for `ms` of robot time in 10 ms steps.
    fn run(&mut self, ms: u64) {
        for _ in 0..ms / 10 {
            self.sup.step();
            self.clock.advance(10);
        }
    }

    fn connect(&mut self) {
        self.sup.command(Command::Connect(Target::Auto)).unwrap();
    }

    fn snap(&self) -> Snapshot {
        self.sup.snapshot()
    }

    fn now(&self) -> u64 {
        self.clock.now_ms()
    }

    fn sim<R>(&self, f: impl FnOnce(&mut SimRobot) -> R) -> R {
        f(&mut self.robot.lock().unwrap_or_else(PoisonError::into_inner))
    }

    fn press(&self) {
        let now = self.now();
        self.sim(|robot| robot.press(now));
    }

    /// Everything a person or a log could see: published states, log lines and Debug output.
    fn visible_text(&self) -> String {
        let mut text = String::new();
        for snapshot in self
            .published
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
        {
            text.push_str(&serde_json::to_string(snapshot).unwrap());
            text.push_str(&format!("{snapshot:?}"));
        }
        text.push_str(&serde_json::to_string(&self.snap()).unwrap());
        text.push_str(
            &self
                .logs
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .join("\n"),
        );
        text.push_str(&format!("{:?}", self.pairings));
        text
    }

    fn received_names(&self) -> Vec<String> {
        self.sim(|robot| {
            robot
                .received
                .iter()
                .map(|(_, name)| name.clone())
                .collect()
        })
    }
}

fn paired_rig(robot_key: u8, desktop_key: Option<u8>) -> Rig {
    let pairings = match desktop_key {
        Some(byte) => MemoryPairings::with_key(DEVICE, key(byte)),
        None => MemoryPairings::default(),
    };
    rig(SimRobot::new(DEVICE).with_key(key(robot_key)), pairings)
}

/// Builds a robot message the way the wire carries it.
fn robot_message(value: serde_json::Value) -> ControlMessage {
    serde_json::from_value(value).expect("a valid robot message")
}

fn hello_json(id: &str, paired: bool, version: u32) -> serde_json::Value {
    json!({
        "v":1,"kind":"event","name":"hello","id":id,"ts_ms":1,
        "data":{"device_id":DEVICE,"hw_rev":"r1","fw_version":"0.3.0",
                "protocol_version":version,"capabilities":[],"mute":false,"paired":paired,
                "nonce":"00112233445566778899aabbccddeeff"}
    })
}

fn event_json(id: &str, name: &str, data: serde_json::Value) -> serde_json::Value {
    json!({"v":1,"kind":"event","name":name,"id":id,"ts_ms":1,"data":data})
}

// ---- handshake -----------------------------------------------------------------------------

#[test]
fn an_unpaired_robot_is_found_acknowledged_and_shown_as_unpaired() {
    let mut rig = unpaired_rig();
    rig.connect();
    rig.run(300);
    let snap = rig.snap();
    assert_eq!(snap.phase, Phase::Unpaired);
    assert!(snap.simulated);
    let device = snap.device.expect("device");
    assert_eq!(device.id, DEVICE);
    assert!(!device.paired_here && !device.paired_on_robot);
    assert!(!device.audio);
    assert!(rig.sim(|robot| robot.is_established()));
    assert_eq!(
        rig.received_names().first().map(String::as_str),
        Some("hello_ack")
    );
}

#[test]
fn with_the_stored_key_the_robot_accepts_the_proof_and_the_state_becomes_ready() {
    let mut rig = paired_rig(7, Some(7));
    rig.connect();
    rig.run(300);
    // The robot has accepted the session but nothing has confirmed it yet.
    assert_eq!(rig.snap().phase, Phase::Connecting);
    rig.run(1_500);
    assert_eq!(rig.snap().phase, Phase::Ready);
    assert!(rig.sim(|robot| robot.is_authenticated()));
    assert!(rig.snap().device.unwrap().paired_here);
}

#[test]
fn a_wrong_stored_key_is_rejected_and_explained() {
    let mut rig = paired_rig(7, Some(8));
    rig.connect();
    rig.run(1_500);
    let snap = rig.snap();
    assert_eq!(snap.phase, Phase::KeyRejected);
    assert!(snap.message.unwrap().contains("did not accept the key"));
    assert!(!rig.sim(|robot| robot.is_authenticated()));
}

#[test]
fn a_paired_robot_without_a_key_here_is_shown_as_paired_elsewhere() {
    let mut rig = paired_rig(7, None);
    rig.connect();
    rig.run(1_500);
    let snap = rig.snap();
    assert_eq!(snap.phase, Phase::ForeignPaired);
    assert!(
        snap.message
            .unwrap()
            .contains("hold its talk button for 10 seconds")
    );
    assert!(snap.device.unwrap().paired_on_robot);
}

#[test]
fn an_unreadable_key_store_is_reported_not_guessed() {
    let mut rig = paired_rig(7, Some(7));
    rig.pairings.fail_read(true);
    rig.connect();
    rig.run(500);
    assert_eq!(rig.snap().phase, Phase::VaultUnavailable);
}

#[test]
fn a_robot_on_another_protocol_version_gets_a_clear_message_and_is_refused() {
    let mut rig = rig(
        SimRobot::new(DEVICE).with_protocol_version(2),
        MemoryPairings::default(),
    );
    rig.connect();
    rig.run(3_000);
    let snap = rig.snap();
    assert_eq!(snap.phase, Phase::Incompatible);
    let message = snap.message.unwrap();
    assert!(
        message.contains("version 2") && message.contains("version 1"),
        "{message}"
    );
    assert!(!rig.sim(|robot| robot.is_established()));
    // It is refused, not abandoned: the link stays so the message stays.
    assert_eq!(rig.snap().phase, Phase::Incompatible);
}

#[test]
fn missing_capabilities_are_named_in_the_message() {
    let hello = robot_message(hello_json("e-1", false, 1));
    let ControlMessage::Hello { data, .. } = hello else {
        unreachable!()
    };
    assert_eq!(incompatibility(&data, &[]), None);
    let message = incompatibility(&data, &["usb_pcm"]).expect("incompatible");
    assert!(message.contains("usb_pcm"), "{message}");
}

#[test]
fn a_repeated_hello_gets_the_same_answer_without_restarting_the_session() {
    let pairings: Arc<dyn PairingStore> = Arc::new(MemoryPairings::default());
    let mut session = Session::new(pairings, Box::new(SeqRandom(0)), 1_000);
    let hello = robot_message(hello_json("e-1", false, 1));
    session.on_message(1_000, &hello);
    let first = session.take_outgoing(1_000);
    session.on_message(2_000, &hello);
    let second = session.take_outgoing(2_000);
    assert_eq!(first, second);
    assert!(session.established());
}

// ---- keepalive and liveness ----------------------------------------------------------------

#[test]
fn the_desktop_sends_a_frame_at_least_every_two_seconds_and_the_robot_never_drops_it() {
    let mut rig = unpaired_rig();
    rig.connect();
    rig.run(30_000);
    assert_eq!(rig.snap().phase, Phase::Unpaired);
    rig.sim(|robot| {
        assert_eq!(robot.stats.sessions_started, 1);
        assert_eq!(robot.stats.sessions_dropped_for_silence, 0);
        assert!(robot.stats.heartbeats_sent >= 14);
        let times: Vec<u64> = robot.received.iter().map(|(at, _)| *at).collect();
        assert!(times.len() > 25, "only {} frames", times.len());
        for pair in times.windows(2) {
            assert!(
                pair[1] - pair[0] <= 2_000,
                "gap of {} ms",
                pair[1] - pair[0]
            );
        }
        // With nothing else to say, the frame is an empty set_config.
        let names: Vec<&str> = robot
            .received
            .iter()
            .map(|(_, name)| name.as_str())
            .collect();
        assert!(names[1..].iter().all(|name| *name == "set_config"));
    });
    // The replies to those keepalives are ignored: nothing unexpected was counted.
    assert_eq!(rig.snap().stats.unexpected_messages, 0);
}

#[test]
fn the_simulated_robot_drops_a_desktop_that_stays_silent_about_eight_seconds() {
    // A check of the simulator against the firmware's rule, so the keepalive test above means something.
    let clock = Arc::new(ManualClock::new());
    let mut robot = SimRobot::new(DEVICE);
    robot.on_link_up(clock.now_ms());
    let mut framer = Framer::new();
    let hello = framer.push(&drain(&mut robot)).remove(0);
    let ack = hello
        .acknowledge_hello(Some(StableId::new("s-1").unwrap()), None, 5)
        .unwrap();
    robot.receive(clock.now_ms(), &framer.encode(&ack).unwrap());
    assert!(robot.is_established());
    let start = clock.now_ms();
    let mut dropped_at = None;
    for _ in 0..1_500 {
        clock.advance(10);
        robot.step(clock.now_ms());
        if robot.stats.sessions_dropped_for_silence > 0 {
            dropped_at = Some(clock.now_ms() - start);
            break;
        }
    }
    let dropped_at = dropped_at.expect("dropped");
    assert!((7_000..=9_000).contains(&dropped_at), "{dropped_at}");
}

fn drain(robot: &mut SimRobot) -> Vec<u8> {
    robot.take_output()
}

#[test]
fn lost_heartbeats_end_the_link_and_it_reconnects_with_a_short_first_wait() {
    let mut rig = unpaired_rig();
    rig.connect();
    rig.run(500);
    assert_eq!(rig.snap().phase, Phase::Unpaired);
    rig.sim(|robot| robot.set_silent(true));
    let mut waited = 0;
    while rig.snap().phase != Phase::Reconnecting && waited < 20_000 {
        rig.run(10);
        waited += 10;
    }
    assert_eq!(rig.snap().phase, Phase::Reconnecting);
    assert!(
        (HEARTBEAT_LOSS_MS - 500..=HEARTBEAT_LOSS_MS + 2_500).contains(&waited),
        "{waited}"
    );
    assert_eq!(rig.snap().retry_in_ms, Some(500));
    assert!(rig.snap().message.unwrap().contains("stopped answering"));
    rig.sim(|robot| robot.set_silent(false));
    rig.run(1_500);
    assert_eq!(rig.snap().phase, Phase::Unpaired);
    assert_eq!(rig.snap().retry_in_ms, None);
}

#[test]
fn reconnect_waits_grow_up_to_ten_seconds_and_reset_after_a_good_session() {
    let mut rig = unpaired_rig();
    rig.sim(|robot| robot.unplug());
    rig.connect();
    let mut waits = Vec::new();
    for _ in 0..5_000 {
        rig.run(10);
        let wait = rig.snap().retry_in_ms;
        if wait.is_some() && waits.last() != Some(&wait) {
            waits.push(wait);
        }
    }
    assert_eq!(
        waits,
        [500, 1_000, 2_000, 4_000, 8_000, 10_000].map(Some).to_vec()
    );
    assert!(rig.snap().message.unwrap().contains("No Moby found"));
    // Plugged back in: it connects within the current wait, and the next loss starts short again.
    rig.sim(|robot| robot.plug());
    rig.run(11_000);
    assert_eq!(rig.snap().phase, Phase::Unpaired);
    rig.sim(|robot| robot.unplug());
    rig.run(300);
    assert_eq!(rig.snap().phase, Phase::Reconnecting);
    assert_eq!(rig.snap().retry_in_ms, Some(500));
}

#[test]
fn a_device_that_never_says_hello_is_dropped_and_retried() {
    let mut rig = unpaired_rig();
    rig.sim(|robot| robot.set_silent(true));
    rig.connect();
    // The simulator sends a hello at link-up even when silent, so use a session directly.
    let pairings: Arc<dyn PairingStore> = Arc::new(MemoryPairings::default());
    let mut session = Session::new(pairings, Box::new(SeqRandom(0)), 1_000);
    assert_eq!(session.tick(1_000 + HELLO_WAIT_MS), None);
    assert_eq!(
        session.tick(1_001 + HELLO_WAIT_MS),
        Some(DropReason::NoHello)
    );
    rig.run(100);
}

#[test]
fn a_pulled_cable_is_noticed_and_the_link_comes_back_when_it_is_plugged_in() {
    let mut rig = unpaired_rig();
    rig.connect();
    rig.run(500);
    rig.sim(|robot| robot.unplug());
    rig.run(100);
    assert_eq!(rig.snap().phase, Phase::Reconnecting);
    assert!(rig.snap().message.unwrap().contains("unplugged"));
    rig.sim(|robot| robot.plug());
    rig.run(3_000);
    assert_eq!(rig.snap().phase, Phase::Unpaired);
}

// ---- bad input -----------------------------------------------------------------------------

#[test]
fn a_corrupted_frame_is_counted_and_the_link_carries_on() {
    let mut rig = unpaired_rig();
    rig.connect();
    rig.run(300);
    rig.sim(|robot| robot.corrupt_next_frames(1));
    rig.run(5_000);
    let snap = rig.snap();
    assert_eq!(snap.phase, Phase::Unpaired);
    assert!(
        snap.stats.bad_crc + snap.stats.bad_frame >= 1,
        "{:?}",
        snap.stats
    );
    assert!(snap.stats.frames_ok >= 2);
}

#[test]
fn an_oversized_segment_is_dropped_and_counted() {
    let mut rig = unpaired_rig();
    rig.connect();
    rig.run(300);
    rig.sim(|robot| robot.inject_oversized());
    rig.run(300);
    assert_eq!(rig.snap().stats.oversized, 1);
    assert_eq!(rig.snap().phase, Phase::Unpaired);
}

#[test]
fn rom_boot_text_on_the_port_does_not_break_the_handshake() {
    let mut rig = unpaired_rig();
    rig.sim(|robot| robot.inject_garbage());
    rig.connect();
    rig.run(500);
    assert_eq!(rig.snap().phase, Phase::Unpaired);
}

#[test]
fn malformed_json_unknown_fields_and_unknown_names_are_counted_and_ignored() {
    let mut rig = unpaired_rig();
    rig.connect();
    rig.run(300);
    rig.sim(|robot| {
        robot.inject_payload(b"{ not json");
        robot.inject_payload(b"[1,2,3]");
        robot.inject_payload(
            &serde_json::to_vec(&event_json(
                "e-9",
                "heartbeat",
                json!({"state":"idle","mute":false,"uptime_ms":1,"surprise":true}),
            ))
            .unwrap(),
        );
        robot.inject_payload(
            &serde_json::to_vec(&event_json("e-9", "self_destruct", json!({}))).unwrap(),
        );
    });
    rig.run(300);
    let snap = rig.snap();
    assert_eq!(snap.stats.malformed_json, 4);
    assert_eq!(snap.phase, Phase::Unpaired);
}

#[test]
fn robot_audio_is_ignored_and_counted_and_never_answered() {
    let mut rig = unpaired_rig();
    rig.connect();
    rig.run(300);
    rig.sim(|robot| {
        robot.inject_audio_frame();
        robot.inject_payload(
            &serde_json::to_vec(&event_json(
                "e-8",
                "audio_start",
                json!({"stream_id":1,"reason":"wake","codec":"pcm_s16le","sample_rate":16000,
                       "channels":1,"frame_ms":20,"preroll_ms":0}),
            ))
            .unwrap(),
        );
    });
    rig.run(300);
    let stats = rig.snap().stats;
    assert_eq!(stats.audio_frames_ignored, 1);
    assert_eq!(stats.audio_events_ignored, 1);
    assert_eq!(stats.audio_streams, 0);
    // The desktop records nothing. It replies with keepalives and one `cancel` for the stream
    // that a robot with no audio capability had no business starting.
    let names = rig.received_names();
    assert!(
        names[1..]
            .iter()
            .all(|name| name == "set_config" || name == "cancel")
    );
    assert_eq!(names.iter().filter(|name| *name == "cancel").count(), 1);
}

// ---- mute ----------------------------------------------------------------------------------

#[test]
fn the_hardware_mute_is_shown_and_the_desktop_never_asks_to_change_it() {
    let mut rig = unpaired_rig();
    rig.connect();
    rig.run(500);
    assert!(!rig.snap().muted);
    let now = rig.now();
    rig.sim(|robot| robot.set_mute(now, true));
    rig.run(300);
    let snap = rig.snap();
    assert!(snap.muted);
    assert_eq!(snap.expression, Expression::Muted);
    // Pairing is refused while muted, without asking the robot.
    let error = rig.sup.command(Command::BeginPairing).unwrap_err();
    assert_eq!(error.code, "muted");
    let now = rig.now();
    rig.sim(|robot| robot.set_mute(now, false));
    rig.run(3_000);
    assert!(!rig.snap().muted);
    assert_eq!(rig.snap().expression, Expression::Idle);
    // Everything the desktop ever sent: an answer to hello and empty configuration frames.
    assert!(
        rig.received_names()
            .iter()
            .all(|name| name == "hello_ack" || name == "set_config")
    );
}

#[test]
fn robot_states_and_errors_are_reflected() {
    let mut rig = unpaired_rig();
    rig.connect();
    rig.run(500);
    let now = rig.now();
    rig.sim(|robot| robot.show(now, Expression::Listening));
    rig.run(2_500);
    assert_eq!(rig.snap().expression, Expression::Listening);
    // An error event from the robot is shown for a few seconds.
    rig.sim(|robot| {
        robot.inject_payload(
            &serde_json::to_vec(&event_json(
                "e-7",
                "error",
                json!({"code":"E_HW","detail":"mic fault"}),
            ))
            .unwrap(),
        )
    });
    rig.run(300);
    let error = rig.snap().error.expect("error");
    assert_eq!(
        (error.code.as_str(), error.detail.as_str()),
        ("E_HW", "mic fault")
    );
    rig.run(6_000);
    assert_eq!(rig.snap().error, None);
}

// ---- pairing -------------------------------------------------------------------------------

#[test]
fn pairing_happy_path_stores_the_key_once_and_the_next_connection_is_authenticated() {
    let mut rig = unpaired_rig();
    rig.connect();
    rig.run(300);
    rig.sup.command(Command::BeginPairing).unwrap();
    rig.run(300);
    let snap = rig.snap();
    assert_eq!(snap.phase, Phase::Pairing);
    assert_eq!(snap.pair_step, Some(PairStep::PressButton));
    assert!(rig.sim(|robot| robot.is_waiting_for_button()));
    // No key exists before the button is pressed.
    assert!(!rig.pairings.has_key(DEVICE));
    rig.press();
    rig.run(300);
    let snap = rig.snap();
    assert_eq!(snap.phase, Phase::Ready);
    assert_eq!(snap.pair_failure, None);
    assert!(snap.device.as_ref().unwrap().paired_here);
    // Both sides hold the same 32-byte key, kept in the store and nowhere else.
    let stored = rig.pairings.key_text(DEVICE).expect("stored key");
    assert_eq!(stored.len(), 64);
    assert_eq!(rig.sim(|robot| robot.key_text()), Some(stored.clone()));
    assert!(rig.pairings.has_record(DEVICE));
    assert!(!rig.visible_text().contains(&stored));
    // A new connection proves itself with that key.
    rig.sup.command(Command::Disconnect).unwrap();
    rig.run(100);
    rig.connect();
    rig.run(2_000);
    assert_eq!(rig.snap().phase, Phase::Ready);
    assert!(rig.sim(|robot| robot.is_authenticated()));
}

#[test]
fn if_nobody_presses_the_button_pairing_times_out_and_nothing_is_stored() {
    let mut rig = unpaired_rig();
    rig.connect();
    rig.run(300);
    rig.sup.command(Command::BeginPairing).unwrap();
    rig.run(33_000);
    let snap = rig.snap();
    assert_eq!(snap.phase, Phase::Unpaired);
    assert_eq!(snap.pair_failure.as_ref().map(|f| f.code), Some("timeout"));
    assert!(!rig.pairings.has_key(DEVICE) && !rig.pairings.has_record(DEVICE));
    assert!(!rig.sim(|robot| robot.is_paired()));
    // The person can try again.
    rig.sup.command(Command::BeginPairing).unwrap();
    rig.run(200);
    rig.press();
    rig.run(300);
    assert_eq!(rig.snap().phase, Phase::Ready);
}

#[test]
fn pairing_an_already_paired_robot_is_refused_by_the_robot_and_explained() {
    let mut rig = paired_rig(7, None);
    rig.connect();
    rig.run(500);
    rig.sup.command(Command::BeginPairing).unwrap();
    rig.run(300);
    let snap = rig.snap();
    assert_eq!(
        snap.pair_failure.as_ref().map(|f| f.code),
        Some("already_paired")
    );
    assert!(snap.pair_failure.unwrap().message.contains("reset"));
    assert!(!rig.pairings.has_key(DEVICE));
    assert_eq!(
        rig.sim(|robot| robot.key_text()),
        Some(key(7).as_str().to_owned())
    );
}

#[test]
fn a_second_begin_while_waiting_is_refused_here() {
    let mut rig = unpaired_rig();
    rig.connect();
    rig.run(300);
    rig.sup.command(Command::BeginPairing).unwrap();
    assert_eq!(
        rig.sup.command(Command::BeginPairing).unwrap_err().code,
        "busy"
    );
}

#[test]
fn a_cancelled_pairing_ignores_a_late_button_press() {
    let mut rig = unpaired_rig();
    rig.connect();
    rig.run(300);
    rig.sup.command(Command::BeginPairing).unwrap();
    rig.run(300);
    rig.sup.command(Command::CancelPairing).unwrap();
    rig.press();
    rig.run(500);
    // The robot's confirmation arrived with no pairing running: wrong order, so no key is made.
    assert_eq!(rig.snap().stats.unexpected_messages, 1);
    assert!(!rig.pairings.has_key(DEVICE));
    assert!(
        !rig.received_names()
            .iter()
            .any(|name| name == "pair_set_key")
    );
    assert_eq!(rig.snap().phase, Phase::Unpaired);
}

#[test]
fn muting_during_pairing_stops_it() {
    let mut rig = unpaired_rig();
    rig.connect();
    rig.run(300);
    rig.sup.command(Command::BeginPairing).unwrap();
    rig.run(300);
    let now = rig.now();
    rig.sim(|robot| robot.set_mute(now, true));
    rig.run(300);
    let snap = rig.snap();
    assert_eq!(snap.pair_failure.as_ref().map(|f| f.code), Some("muted"));
    assert_eq!(snap.phase, Phase::Unpaired);
    assert!(snap.muted);
}

#[test]
fn if_the_key_cannot_be_saved_nothing_is_sent_to_the_robot() {
    let mut rig = unpaired_rig();
    rig.pairings.fail_save(true);
    rig.connect();
    rig.run(300);
    rig.sup.command(Command::BeginPairing).unwrap();
    rig.run(300);
    rig.press();
    rig.run(500);
    let snap = rig.snap();
    assert_eq!(snap.pair_failure.as_ref().map(|f| f.code), Some("storage"));
    assert!(
        !rig.received_names()
            .iter()
            .any(|name| name == "pair_set_key")
    );
    assert!(!rig.sim(|robot| robot.is_paired()));
}

#[test]
fn a_refused_key_is_taken_back_out_of_the_store() {
    let pairings: Arc<MemoryPairings> = Arc::new(MemoryPairings::default());
    let mut session = Session::new(
        Arc::clone(&pairings) as Arc<dyn PairingStore>,
        Box::new(SeqRandom(0)),
        1_000,
    );
    session.on_message(1_000, &robot_message(hello_json("e-1", false, 1)));
    session.take_outgoing(1_000);
    session.begin_pairing(1_100).unwrap();
    let begin = session.take_outgoing(1_100).remove(0);
    session.on_message(
        1_150,
        &robot_message(json!({
            "v":1,"kind":"response","name":"command_ok","id":begin.id().as_str(),"ts_ms":2,"data":{}
        })),
    );
    session.on_message(
        1_200,
        &robot_message(event_json("e-2", "pair_button_confirmed", json!({}))),
    );
    assert!(pairings.has_key(DEVICE));
    let set_key = session.take_outgoing(1_200).remove(0);
    assert_eq!(set_key.name(), "pair_set_key");
    session.on_message(
        1_250,
        &robot_message(json!({
            "v":1,"kind":"response","name":"command_error","id":set_key.id().as_str(),"ts_ms":3,
            "data":{"code":"E_STATE","detail":"pairing was not confirmed"}
        })),
    );
    assert!(!pairings.has_key(DEVICE) && !pairings.has_record(DEVICE));
    let snap = session.snapshot();
    assert_eq!(snap.pair_failure.map(|f| f.code), Some("not_confirmed"));
    assert_eq!(snap.phase, Phase::Unpaired);
}

#[test]
fn a_button_confirmation_nobody_asked_for_makes_no_key() {
    let pairings: Arc<MemoryPairings> = Arc::new(MemoryPairings::default());
    let mut session = Session::new(
        Arc::clone(&pairings) as Arc<dyn PairingStore>,
        Box::new(SeqRandom(0)),
        1_000,
    );
    session.on_message(1_000, &robot_message(hello_json("e-1", false, 1)));
    session.take_outgoing(1_000);
    session.on_message(
        1_100,
        &robot_message(event_json("e-2", "pair_button_confirmed", json!({}))),
    );
    assert!(session.take_outgoing(1_100).is_empty());
    assert!(!pairings.has_key(DEVICE));
    assert_eq!(session.unexpected, 1);
}

#[test]
fn a_robot_that_never_answers_pair_begin_ends_the_attempt() {
    let pairings: Arc<dyn PairingStore> = Arc::new(MemoryPairings::default());
    let mut session = Session::new(pairings, Box::new(SeqRandom(0)), 1_000);
    session.on_message(1_000, &robot_message(hello_json("e-1", false, 1)));
    session.take_outgoing(1_000);
    session.begin_pairing(1_100).unwrap();
    session.take_outgoing(1_100);
    session.on_message(
        1_200,
        &robot_message(event_json(
            "e-2",
            "heartbeat",
            json!({"state":"idle","mute":false,"uptime_ms":1}),
        )),
    );
    assert_eq!(session.tick(1_200 + 3_000), None);
    assert_eq!(
        session.snapshot().pair_failure.map(|f| f.code),
        Some("no_answer")
    );
}

#[test]
fn the_pairing_window_has_its_own_deadline_if_the_robot_never_reports_a_timeout() {
    let pairings: Arc<dyn PairingStore> = Arc::new(MemoryPairings::default());
    let mut session = Session::new(pairings, Box::new(SeqRandom(0)), 1_000);
    session.on_message(1_000, &robot_message(hello_json("e-1", false, 1)));
    session.take_outgoing(1_000);
    session.begin_pairing(1_000).unwrap();
    let begin = session.take_outgoing(1_000).remove(0);
    session.on_message(
        1_010,
        &robot_message(json!({
            "v":1,"kind":"response","name":"command_ok","id":begin.id().as_str(),"ts_ms":2,"data":{}
        })),
    );
    let keep_alive = |session: &mut Session, at: u64| {
        session.on_message(
            at,
            &robot_message(event_json(
                "e-3",
                "heartbeat",
                json!({"state":"idle","mute":false,"uptime_ms":1}),
            )),
        );
    };
    keep_alive(&mut session, 20_000);
    session.tick(20_000);
    assert_eq!(session.snapshot().pair_step, Some(PairStep::PressButton));
    keep_alive(&mut session, 33_900);
    session.tick(33_900);
    assert_eq!(
        session.snapshot().pair_failure.map(|f| f.code),
        Some("timeout")
    );
}

#[test]
fn pair_reset_keeps_the_key_and_asks_the_person() {
    let mut rig = paired_rig(7, Some(7));
    rig.connect();
    rig.run(1_500);
    assert_eq!(rig.snap().phase, Phase::Ready);
    let now = rig.now();
    rig.sim(|robot| robot.long_press(now));
    rig.run(300);
    let snap = rig.snap();
    assert_eq!(snap.phase, Phase::PairingReset);
    assert!(snap.message.unwrap().contains("Unpair"));
    assert!(snap.device.unwrap().paired_here, "the key is still here");
    assert!(rig.pairings.has_key(DEVICE) && rig.pairings.has_record(DEVICE));
    assert!(!rig.sim(|robot| robot.is_paired()));
    // Only an explicit unpair removes it.
    rig.sup.command(Command::Unpair(DEVICE.to_owned())).unwrap();
    assert!(!rig.pairings.has_key(DEVICE) && !rig.pairings.has_record(DEVICE));
    assert_eq!(rig.snap().phase, Phase::Unpaired);
}

#[test]
fn a_forged_pair_reset_cannot_erase_a_pairing_or_start_audio() {
    let mut rig = paired_rig(7, Some(7));
    rig.connect();
    rig.run(1_500);
    rig.sim(|robot| {
        robot.inject_payload(
            &serde_json::to_vec(&event_json("e-66", "pair_reset", json!({}))).unwrap(),
        );
    });
    rig.run(300);
    assert_eq!(rig.snap().phase, Phase::PairingReset);
    assert!(
        rig.pairings.has_key(DEVICE),
        "a message anyone can send did not delete the key"
    );
    assert!(rig.sim(|robot| robot.is_paired()));
}

#[test]
fn pairing_again_after_a_reset_replaces_the_key() {
    let mut rig = paired_rig(7, Some(7));
    rig.connect();
    rig.run(1_500);
    let now = rig.now();
    rig.sim(|robot| robot.long_press(now));
    rig.run(300);
    rig.sup.command(Command::BeginPairing).unwrap();
    rig.run(300);
    rig.press();
    rig.run(500);
    assert!(rig.pairings.has_key(DEVICE));
    assert!(rig.sim(|robot| robot.is_paired()));
}

#[test]
fn unpairing_removes_the_key_and_the_record_and_the_robot_is_then_foreign() {
    let mut rig = paired_rig(7, Some(7));
    rig.connect();
    rig.run(1_500);
    rig.sup.command(Command::Unpair(DEVICE.to_owned())).unwrap();
    assert!(!rig.pairings.has_key(DEVICE) && !rig.pairings.has_record(DEVICE));
    assert_eq!(rig.snap().phase, Phase::ForeignPaired);
    assert_eq!(
        rig.sup
            .command(Command::Unpair("Not An Id".into()))
            .unwrap_err()
            .code,
        "invalid_input"
    );
}

#[test]
fn the_key_is_in_no_snapshot_log_or_debug_output() {
    let mut rig = unpaired_rig();
    rig.connect();
    rig.run(300);
    rig.sup.command(Command::BeginPairing).unwrap();
    rig.run(300);
    rig.press();
    rig.run(1_500);
    assert_eq!(rig.snap().phase, Phase::Ready);
    let stored = rig.pairings.key_text(DEVICE).unwrap();
    let text = rig.visible_text();
    assert!(!text.contains(&stored));
    // Not even a part of it that would help: the proof and the raw key bytes in decimal.
    let key = PairKey::try_from(stored.clone()).unwrap();
    assert_eq!(format!("{key:?}"), "PairKey(<redacted>)");
    let message: ControlMessage = robot_message_for_key(&key);
    assert!(!format!("{message:?}").contains(&stored));
    assert!(format!("{message:?}").contains("redacted"));
}

fn robot_message_for_key(key: &PairKey) -> ControlMessage {
    serde_json::from_value(json!({
        "v":1,"kind":"command","name":"pair_set_key","id":"c-1","ts_ms":1,
        "data":{"pair_key": key.as_str()}
    }))
    .unwrap()
}

// ---- shared protocol vectors, through the desktop's link layer -------------------------------

fn hex(value: &str) -> Vec<u8> {
    (0..value.len())
        .step_by(2)
        .map(|offset| u8::from_str_radix(&value[offset..offset + 2], 16).unwrap())
        .collect()
}

fn rows(text: &'static str) -> Vec<Vec<&'static str>> {
    text.lines()
        .filter(|line| !line.starts_with('#') && !line.is_empty())
        .map(|line| line.split('\t').collect())
        .collect()
}

#[test]
fn the_protocol_frame_vectors_decode_the_same_way_in_the_desktop_link_layer() {
    for fields in rows(include_str!(
        "../../../../moby-protocol/tests/fixtures/usb-vectors.tsv"
    )) {
        let [name, result, kind, _stream, _sequence, _payload, wire] = fields[..] else {
            panic!("bad row {fields:?}");
        };
        let mut framer = Framer::new();
        let items = framer.push_all(&hex(wire));
        let stats = framer.stats.clone();
        assert!(
            items
                .iter()
                .all(|item| !matches!(item, super::link::Incoming::Control(_))),
            "{name}: no vector here is a robot control message"
        );
        match (result, kind) {
            ("ok", "2") => assert_eq!(items.len(), 1, "{name}: one robot audio frame"),
            // Desktop audio never comes from a robot.
            ("ok", "3") => assert_eq!(stats.wrong_sender, 1, "{name}"),
            ("bad_crc", _) => assert_eq!(stats.bad_crc, 1, "{name}"),
            ("unsupported_version", _) => assert_eq!(stats.bad_frame_version, 1, "{name}"),
            ("bad_frame", _) => assert_eq!(stats.bad_frame, 1, "{name}"),
            other => panic!("{name}: unexpected vector {other:?}"),
        }
    }
}

#[test]
fn the_protocol_control_vectors_are_accepted_or_dropped_as_the_protocol_says() {
    let mut accepted = 0;
    let mut desktop_only = 0;
    let mut rejected = 0;
    for fields in rows(include_str!(
        "../../../../moby-protocol/tests/fixtures/control-vectors.tsv"
    )) {
        let [name, verdict, _sequence, payload, wire] = fields[..] else {
            panic!("bad row {fields:?}");
        };
        let mut framer = Framer::new();
        let messages = framer.push(&hex(wire));
        let parsed = serde_json::from_slice::<ControlMessage>(&hex(payload));
        match verdict {
            "valid" => {
                let message = parsed.unwrap_or_else(|error| panic!("{name}: {error}"));
                if message.sender() == moby_protocol::usb::Peer::Robot {
                    assert_eq!(messages.len(), 1, "{name}");
                    assert_eq!(messages[0], message, "{name}");
                    accepted += 1;
                } else {
                    assert!(messages.is_empty(), "{name}");
                    assert_eq!(framer.stats.wrong_sender, 1, "{name}");
                    desktop_only += 1;
                }
            }
            "invalid" => {
                assert!(parsed.is_err(), "{name}");
                assert!(messages.is_empty(), "{name}");
                rejected += 1;
            }
            other => panic!("{name}: verdict {other}"),
        }
    }
    assert!(accepted >= 10 && desktop_only >= 5 && rejected >= 30);
}

#[test]
fn every_message_the_desktop_builds_is_a_valid_desktop_frame() {
    // Run a whole pairing and check each frame the robot received decoded cleanly: the simulator
    // counts a frame it could not decode.
    let mut rig = unpaired_rig();
    rig.connect();
    rig.run(300);
    rig.sup.command(Command::BeginPairing).unwrap();
    rig.run(200);
    rig.press();
    rig.run(3_000);
    rig.sim(|robot| {
        assert_eq!(robot.stats.bad_frames_seen, 0);
        let names: Vec<&str> = robot
            .received
            .iter()
            .map(|(_, name)| name.as_str())
            .collect();
        assert!(names.contains(&"pair_begin") && names.contains(&"pair_set_key"));
    });
    let mut wire = vec![0; MAX_WIRE_LEN];
    let frame = Frame {
        kind: FrameKind::Control,
        stream_id: 0,
        sequence: 0,
        payload: b"{}",
    };
    assert!(frame.encode(&mut wire).is_ok());
}

// ---- push-to-talk audio (D4) ---------------------------------------------------------------

use super::sim::{AUDIO_MARKER, StreamFault};

type Heard = Arc<Mutex<Vec<AudioEvent>>>;

/// A paired, authenticated robot that offers `usb_pcm`, and everything the link hands on.
fn audio_rig() -> (Rig, Heard) {
    let heard: Heard = Arc::new(Mutex::new(Vec::new()));
    let sink: AudioSink = {
        let heard = Arc::clone(&heard);
        Arc::new(move |event| {
            heard
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(event)
        })
    };
    let robot = SimRobot::new(DEVICE)
        .with_key(key(7))
        .with_capabilities(&["usb_pcm"]);
    let mut rig = rig_with_audio(robot, MemoryPairings::with_key(DEVICE, key(7)), sink);
    rig.connect();
    rig.run(2_500);
    assert_eq!(rig.snap().phase, Phase::Ready);
    assert!(rig.snap().device.unwrap().audio);
    (rig, heard)
}

fn summary(heard: &Heard) -> Vec<String> {
    heard
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .iter()
        .map(|event| match event {
            AudioEvent::Started { .. } => "started".to_owned(),
            AudioEvent::Utterance { pcm, .. } => format!("utterance:{}", pcm.len()),
            AudioEvent::Aborted { reason, .. } => format!("aborted:{}", reason.code()),
        })
        .collect()
}

fn talk(rig: &mut Rig, ms: u64) {
    let now = rig.now();
    rig.sim(|robot| robot.press(now));
    rig.run(ms);
    let now = rig.now();
    rig.sim(|robot| robot.release(now));
    rig.run(300);
}

#[test]
fn a_held_button_becomes_one_checked_utterance() {
    let (mut rig, heard) = audio_rig();
    talk(&mut rig, 1_000);
    let events = summary(&heard);
    assert_eq!(events.len(), 2, "{events:?}");
    assert_eq!(events[0], "started");
    let size: usize = events[1]
        .strip_prefix("utterance:")
        .unwrap()
        .parse()
        .unwrap();
    assert!(
        (45 * 640..=55 * 640).contains(&size) && size.is_multiple_of(640),
        "{size}"
    );
    if let AudioEvent::Utterance { pcm, .. } = &heard.lock().unwrap()[1] {
        assert!(pcm.as_bytes().windows(8).any(|w| w == AUDIO_MARKER));
    }
    assert_eq!(rig.snap().stats.audio_streams, 1);
    assert_eq!(rig.snap().stats.audio_aborted, 0);
}

#[test]
fn every_stream_fault_ends_the_turn_with_its_own_reason_and_no_audio_is_passed_on() {
    let cases = [
        (StreamFault::SkipFrame(3), "aborted:audio_sequence", 1_000),
        (
            StreamFault::DuplicateFrame(3),
            "aborted:audio_sequence",
            1_000,
        ),
        (StreamFault::SwapFrames(3), "aborted:audio_sequence", 1_000),
        (StreamFault::WrongStream(3), "aborted:audio_sequence", 1_000),
        (StreamFault::WrongSize(3), "aborted:audio_frame_size", 1_000),
        (StreamFault::Overrun(3), "aborted:audio_overrun", 1_000),
        (StreamFault::CutOff(10), "aborted:audio_stalled", 3_000),
        (StreamFault::NeverEnds, "aborted:audio_too_long", 6_000),
        (StreamFault::WrongCount, "aborted:audio_incomplete", 1_000),
        (StreamFault::MuteAt(10), "aborted:audio_muted", 1_000),
        (
            StreamFault::MuteAndKeepSending(10),
            "aborted:audio_muted",
            1_000,
        ),
        (
            StreamFault::RobotCancel(10),
            "aborted:audio_canceled",
            1_000,
        ),
    ];
    for (fault, expected, ms) in cases {
        let (mut rig, heard) = audio_rig();
        rig.sim(|robot| robot.inject_stream_fault(fault));
        talk(&mut rig, ms);
        let events = summary(&heard);
        assert_eq!(events, ["started", expected], "{fault:?}");
        assert_eq!(rig.snap().stats.audio_streams, 0, "{fault:?}");
        // A new, healthy turn still works after any fault (the mute cases need the switch opened).
        let now = rig.now();
        rig.sim(|robot| robot.set_mute(now, false));
        rig.run(300);
        talk(&mut rig, 600);
        assert!(
            summary(&heard).last().unwrap().starts_with("utterance:"),
            "{fault:?}: {:?}",
            summary(&heard)
        );
    }
}

#[test]
fn a_bad_stream_makes_the_desktop_tell_the_robot_to_cancel_it() {
    let (mut rig, _heard) = audio_rig();
    rig.sim(|robot| robot.inject_stream_fault(StreamFault::SkipFrame(3)));
    talk(&mut rig, 500);
    assert_eq!(
        rig.received_names()
            .iter()
            .filter(|n| *n == "cancel")
            .count(),
        1
    );
    assert!(!rig.sim(|robot| robot.is_streaming()));
}

#[test]
fn mute_mid_stream_discards_the_audio_on_the_desktop_even_if_the_robot_keeps_talking() {
    let (mut rig, heard) = audio_rig();
    rig.sim(|robot| robot.inject_stream_fault(StreamFault::MuteAndKeepSending(10)));
    talk(&mut rig, 1_500);
    assert_eq!(summary(&heard), ["started", "aborted:audio_muted"]);
    assert!(
        rig.snap().stats.audio_frames_ignored > 0,
        "frames after the mute were thrown away"
    );
    assert!(rig.snap().muted);
}

#[test]
fn stop_in_the_app_ends_capture_handling_and_cancels_the_robots_stream() {
    let (mut rig, heard) = audio_rig();
    let now = rig.now();
    rig.sim(|robot| robot.press(now));
    rig.run(400);
    rig.sup.command(Command::CancelCapture).unwrap();
    rig.run(300);
    assert_eq!(summary(&heard), ["started", "aborted:canceled"]);
    assert!(rig.received_names().contains(&"cancel".to_owned()));
    assert!(!rig.sim(|robot| robot.is_streaming()));
}

#[test]
fn the_robot_face_follows_what_the_desktop_asks_for() {
    let (mut rig, _heard) = audio_rig();
    rig.sup
        .command(Command::Face(Expression::Thinking, 0))
        .unwrap();
    rig.run(300);
    assert_eq!(rig.sim(|robot| robot.current_face()), Expression::Thinking);
    rig.sup
        .command(Command::Face(Expression::Speaking, 0))
        .unwrap();
    rig.run(300);
    assert_eq!(rig.sim(|robot| robot.current_face()), Expression::Speaking);
    rig.sup
        .command(Command::Face(Expression::Happy, 1_000))
        .unwrap();
    rig.run(300);
    assert_eq!(rig.sim(|robot| robot.current_face()), Expression::Happy);
    // The mute switch outranks it: nothing is sent while muted.
    let now = rig.now();
    rig.sim(|robot| robot.set_mute(now, true));
    rig.run(300);
    let count = |rig: &Rig| {
        rig.received_names()
            .iter()
            .filter(|n| *n == "expression")
            .count()
    };
    let before = count(&rig);
    rig.sup
        .command(Command::Face(Expression::Thinking, 0))
        .unwrap();
    rig.run(300);
    assert_eq!(before, count(&rig));
}

#[test]
fn a_robot_that_may_not_stream_is_refused_and_told_to_stop() {
    let (mut rig, heard) = audio_rig();
    let now = rig.now();
    rig.sim(|robot| robot.set_mute(now, true));
    rig.run(300);
    rig.sim(|robot| {
        robot.inject_payload(
            &serde_json::to_vec(&event_json(
                "e-77",
                "audio_start",
                json!({"stream_id":9,"reason":"button","codec":"pcm_s16le","sample_rate":16000,
                       "channels":1,"frame_ms":20,"preroll_ms":0}),
            ))
            .unwrap(),
        );
    });
    rig.run(300);
    assert_eq!(summary(&heard), ["aborted:audio_not_allowed"]);
    assert!(rig.received_names().contains(&"cancel".to_owned()));
}

#[test]
fn the_whole_chain_runs_from_the_simulated_robot_through_stub_speech_services() {
    use crate::agent::{Agent, Sink, TurnInput, UiEvent};
    use crate::confirm::testing::Scripted;
    use crate::voice::play::fake::FakeSpeaker;
    use crate::voice::testing::{FakeRobot, Stub};
    use crate::voice::{Deps, Voice};

    struct Quiet;
    impl Sink for Quiet {
        fn emit(&self, _: UiEvent) {}
    }

    let (mut rig, heard) = audio_rig();
    talk(&mut rig, 1_000);
    let utterance = heard.lock().unwrap().remove(1);
    assert!(matches!(utterance, AudioEvent::Utterance { .. }));

    let scratch = crate::paths::testing::Scratch::new();
    let store = crate::store::Store::open(&scratch.path("data"));
    store
        .with(|db| db.set_voice_consent(Some(("alice", "dev-1")), 1))
        .unwrap();
    let stub = Stub::new();
    let api = crate::api::Api::signed_in_for_test(&stub.url, "alice");
    let agent = Agent::with_timeouts(
        std::time::Duration::from_secs(5),
        std::time::Duration::from_secs(5),
        Arc::new(Scripted::new(&[])),
    );
    let (robot, speaker, voice) = (FakeRobot::default(), FakeSpeaker::default(), Voice::new());
    let deps = Deps {
        api: &api,
        store: &store,
        agent: &agent,
        speaker: &speaker,
        robot: &robot,
    };
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let page = async {
            loop {
                if let Some(pending) = voice.view().pending {
                    let _ = voice
                        .continue_turn(
                            &pending.id,
                            TurnInput::for_test(&pending.text),
                            Arc::new(Quiet),
                        )
                        .await;
                    return;
                }
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        };
        tokio::join!(voice.handle_event(&deps, utterance), page);
    });
    assert_eq!(stub.to("/v1/voice/stt").len(), 1);
    assert_eq!(speaker.played.lock().unwrap().len(), 1);
    assert_eq!(
        robot.faces(),
        [
            Expression::Thinking,
            Expression::Speaking,
            Expression::Happy
        ]
    );
    // The faces are then sent over the link, and the simulated robot shows the last one.
    let faces: Vec<_> = robot.faces.lock().unwrap().clone();
    for (expression, hold) in faces {
        rig.sup.command(Command::Face(expression, hold)).unwrap();
        rig.run(200);
    }
    assert_eq!(rig.sim(|robot| robot.current_face()), Expression::Happy);
}

// ---- review fixes ----------------------------------------------------------------------

#[test]
fn a_hello_flood_costs_one_vault_read_and_hellos_alone_cannot_keep_a_link_alive() {
    let pairings = Arc::new(MemoryPairings::with_key(DEVICE, key(7)));
    let store: Arc<dyn PairingStore> = pairings.clone();
    let mut session = Session::new(store, Box::new(SeqRandom(0)), 1_000);
    for n in 0..500_u64 {
        session.on_message(
            1_000 + n / 50,
            &robot_message(hello_json(&format!("e-{n}"), true, 1)),
        );
    }
    assert_eq!(pairings.reads(), 1, "one Credential Manager read, not 500");
    assert!(session.unexpected >= 490);
    // Hellos at the rate of a real robot, and nothing else: the link still ends.
    for (n, at) in (2_000..8_000).step_by(600).enumerate() {
        session.on_message(at, &robot_message(hello_json(&format!("h-{n}"), true, 1)));
    }
    assert_eq!(session.tick(8_000), Some(DropReason::HeartbeatLost));
}

#[test]
fn text_from_the_robot_loses_bidi_and_zero_width_characters() {
    let pairings: Arc<dyn PairingStore> = Arc::new(MemoryPairings::default());
    let mut session = Session::new(pairings, Box::new(SeqRandom(0)), 1_000);
    session.on_message(1_000, &robot_message(hello_json("e-1", false, 1)));
    session.on_message(
        1_100,
        &robot_message(event_json(
            "e-2",
            "error",
            json!({"code":"E_HW","detail":"fine\u{202E}gnp.exe\u{200B}\u{2066}x"}),
        )),
    );
    let error = session.snapshot().error.unwrap();
    assert_eq!(error.detail, "finegnp.exex");
    assert_eq!(
        super::transport::clean_label("Mo\u{202E}by\u{200B} board"),
        "Moby board"
    );
}
