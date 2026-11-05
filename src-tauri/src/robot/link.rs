//! Framing between bytes and control messages. All framing is the `moby-protocol` codec: nothing
//! here builds or splits a frame by hand. The robot is untrusted, so every outcome of bad input is
//! a counted drop. Nothing here panics, and memory is bounded by the decoder's fixed buffers.

use moby_protocol::usb::{
    ControlMessage, DELIMITER, Frame, FrameDecoder, FrameError, FrameKind, MAX_PAYLOAD_LEN,
    MAX_WIRE_LEN, Peer,
};
use serde::Serialize;

/// Counts of what the link dropped or ignored. Counts only, never content.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Stats {
    pub frames_ok: u64,
    pub bad_crc: u64,
    pub bad_frame: u64,
    pub oversized: u64,
    pub bad_frame_version: u64,
    pub malformed_json: u64,
    /// A control message that only the desktop may send.
    pub wrong_sender: u64,
    pub sequence_gaps: u64,
    /// Robot audio frames received and thrown away. Audio belongs to a later stage (D4).
    pub audio_frames_ignored: u64,
    /// `audio_start` and `audio_end` events, ignored for the same reason.
    pub audio_events_ignored: u64,
    /// Messages that did not fit the session's state, for example a reply nobody asked for.
    pub unexpected_messages: u64,
}

pub struct Framer {
    decoder: Box<FrameDecoder>,
    next_tx: u32,
    next_rx: Option<u32>,
    pub stats: Stats,
}

impl Framer {
    pub fn new() -> Self {
        Self {
            decoder: Box::default(),
            next_tx: 0,
            next_rx: None,
            stats: Stats::default(),
        }
    }

    /// Feeds bytes from the port and returns every robot control message they completed.
    /// A damaged or oversized segment is dropped through the next delimiter and counted.
    pub fn push(&mut self, bytes: &[u8]) -> Vec<ControlMessage> {
        let mut messages = Vec::new();
        for &byte in bytes {
            let Some(result) = self.decoder.push(byte) else {
                continue;
            };
            match result {
                Ok(frame) => Self::accept(&mut self.stats, &mut self.next_rx, frame, &mut messages),
                Err(FrameError::BadCrc) => self.stats.bad_crc += 1,
                Err(FrameError::FrameTooLarge) => self.stats.oversized += 1,
                Err(FrameError::UnsupportedVersion(_)) => self.stats.bad_frame_version += 1,
                Err(FrameError::BadFrame | FrameError::OutputTooSmall) => self.stats.bad_frame += 1,
            }
        }
        messages
    }

    fn accept(
        stats: &mut Stats,
        next_rx: &mut Option<u32>,
        frame: Frame<'_>,
        messages: &mut Vec<ControlMessage>,
    ) {
        match frame.kind {
            FrameKind::Control => {
                if next_rx.is_some_and(|expected| expected != frame.sequence) {
                    stats.sequence_gaps += 1;
                }
                *next_rx = Some(frame.sequence.wrapping_add(1));
                match serde_json::from_slice::<ControlMessage>(frame.payload) {
                    Ok(message) if message.sender() == Peer::Robot => {
                        stats.frames_ok += 1;
                        messages.push(message);
                    }
                    Ok(_) => stats.wrong_sender += 1,
                    // Unknown names, unknown fields, wrong kinds and bad numbers all end here.
                    Err(_) => stats.malformed_json += 1,
                }
            }
            FrameKind::RobotAudio => stats.audio_frames_ignored += 1,
            // Only the desktop sends this kind.
            FrameKind::DesktopAudio => stats.wrong_sender += 1,
        }
    }

    /// One control message as wire bytes, including the zero delimiter. `None` if it cannot be
    /// framed, which no message the session builds can cause.
    pub fn encode(&mut self, message: &ControlMessage) -> Option<Vec<u8>> {
        let payload = serde_json::to_vec(message).ok()?;
        if payload.len() > MAX_PAYLOAD_LEN {
            return None;
        }
        let frame = Frame {
            kind: FrameKind::Control,
            stream_id: 0,
            sequence: self.next_tx,
            payload: &payload,
        };
        let mut wire = vec![0; MAX_WIRE_LEN];
        let written = frame.encode(&mut wire).ok()?;
        wire.truncate(written);
        self.next_tx = self.next_tx.wrapping_add(1);
        Some(wire)
    }
}

/// The first byte on a new connection. The ROM may have printed boot text on the port, so a
/// lone delimiter ends whatever partial segment the robot's decoder holds.
pub const RESYNC: [u8; 1] = [DELIMITER];

#[cfg(test)]
mod tests {
    use super::*;
    use moby_protocol::usb::{MAX_ENCODED_LEN, Nonce};
    use serde_json::json;

    fn wire_for(message: &serde_json::Value, sequence: u32) -> Vec<u8> {
        let payload = serde_json::to_vec(message).unwrap();
        let mut wire = vec![0; MAX_WIRE_LEN];
        let n = Frame {
            kind: FrameKind::Control,
            stream_id: 0,
            sequence,
            payload: &payload,
        }
        .encode(&mut wire)
        .unwrap();
        wire.truncate(n);
        wire
    }

    fn heartbeat(id: &str) -> serde_json::Value {
        json!({"v":1,"kind":"event","name":"heartbeat","id":id,"ts_ms":5,
               "data":{"state":"idle","mute":false,"uptime_ms":5}})
    }

    #[test]
    fn a_good_frame_becomes_a_message() {
        let mut framer = Framer::new();
        let messages = framer.push(&wire_for(&heartbeat("e-0001"), 0));
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].name(), "heartbeat");
        assert_eq!(framer.stats.frames_ok, 1);
    }

    #[test]
    fn corrupted_crc_is_counted_and_the_next_frame_still_decodes() {
        let mut framer = Framer::new();
        let mut bad = wire_for(&heartbeat("e-0001"), 0);
        let middle = bad.len() / 2;
        bad[middle] ^= 0x55;
        let mut stream = bad;
        stream.extend(wire_for(&heartbeat("e-0002"), 1));
        let messages = framer.push(&stream);
        assert_eq!(messages.len(), 1);
        assert_eq!(framer.stats.bad_crc + framer.stats.bad_frame, 1);
    }

    #[test]
    fn oversized_segments_are_dropped_through_the_delimiter_and_counted() {
        let mut framer = Framer::new();
        let mut stream = vec![0xAA; MAX_ENCODED_LEN + 500];
        stream.push(DELIMITER);
        stream.extend(wire_for(&heartbeat("e-0001"), 0));
        let messages = framer.push(&stream);
        assert_eq!(messages.len(), 1);
        assert_eq!(framer.stats.oversized, 1);
    }

    #[test]
    fn arbitrary_noise_never_panics_and_never_yields_a_message() {
        let mut framer = Framer::new();
        let mut seed = 0x1234_5678_u32;
        let mut noise = Vec::new();
        for _ in 0..20_000 {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            noise.push((seed >> 24) as u8);
        }
        assert!(framer.push(&noise).is_empty());
    }

    #[test]
    fn malformed_json_unknown_fields_and_unknown_names_are_counted() {
        let mut framer = Framer::new();
        let mut cases: Vec<Vec<u8>> = vec![b"{not json".to_vec(), b"[]".to_vec(), vec![0xff, 0xfe]];
        for value in [
            json!({"v":1,"kind":"event","name":"heartbeat","id":"e-1","ts_ms":1,
                   "data":{"state":"idle","mute":false,"uptime_ms":1,"extra":1}}),
            json!({"v":1,"kind":"event","name":"made_up","id":"e-1","ts_ms":1,"data":{}}),
            json!({"v":2,"kind":"event","name":"heartbeat","id":"e-1","ts_ms":1,
                   "data":{"state":"idle","mute":false,"uptime_ms":1}}),
            json!({"v":1,"kind":"command","name":"heartbeat","id":"e-1","ts_ms":1,
                   "data":{"state":"idle","mute":false,"uptime_ms":1}}),
            json!({"v":1,"kind":"event","name":"heartbeat","id":"E 1","ts_ms":1,
                   "data":{"state":"idle","mute":false,"uptime_ms":1}}),
            json!({"v":1,"kind":"event","name":"heartbeat","id":"e-1","ts_ms":4294967296_u64,
                   "data":{"state":"idle","mute":false,"uptime_ms":1}}),
        ] {
            cases.push(serde_json::to_vec(&value).unwrap());
        }
        let count = cases.len() as u64;
        for (index, payload) in cases.iter().enumerate() {
            let mut wire = vec![0; MAX_WIRE_LEN];
            let n = Frame {
                kind: FrameKind::Control,
                stream_id: 0,
                sequence: index as u32,
                payload,
            }
            .encode(&mut wire)
            .unwrap();
            assert!(framer.push(&wire[..n]).is_empty());
        }
        assert_eq!(framer.stats.malformed_json, count);
    }

    #[test]
    fn a_message_only_the_desktop_may_send_is_dropped() {
        let mut framer = Framer::new();
        let ack = json!({"v":1,"kind":"response","name":"hello_ack","id":"e-1","ts_ms":1,
                         "data":{"accepted":false,"config":{}}});
        assert!(framer.push(&wire_for(&ack, 0)).is_empty());
        assert_eq!(framer.stats.wrong_sender, 1);
    }

    #[test]
    fn robot_audio_is_counted_and_dropped() {
        let mut framer = Framer::new();
        let mut wire = vec![0; MAX_WIRE_LEN];
        let n = Frame {
            kind: FrameKind::RobotAudio,
            stream_id: 7,
            sequence: 0,
            payload: &[1, 2, 3, 4],
        }
        .encode(&mut wire)
        .unwrap();
        assert!(framer.push(&wire[..n]).is_empty());
        assert_eq!(framer.stats.audio_frames_ignored, 1);
    }

    #[test]
    fn a_skipped_sequence_number_is_counted() {
        let mut framer = Framer::new();
        framer.push(&wire_for(&heartbeat("e-0001"), 0));
        framer.push(&wire_for(&heartbeat("e-0002"), 5));
        assert_eq!(framer.stats.sequence_gaps, 1);
    }

    #[test]
    fn encoded_messages_round_trip_through_the_decoder() {
        // The desktop's own frames decode with the same codec: a hello_ack for a robot hello.
        let hello: ControlMessage = serde_json::from_value(json!({
            "v":1,"kind":"event","name":"hello","id":"e-0001","ts_ms":1,
            "data":{"device_id":"dev_0123","hw_rev":"r1","fw_version":"0.3.0",
                    "protocol_version":1,"capabilities":[],"mute":false,"paired":false,
                    "nonce":"00112233445566778899aabbccddeeff"}
        }))
        .unwrap();
        let ack = hello
            .acknowledge_hello(Some(moby_protocol::StableId::new("s-1").unwrap()), None, 2)
            .unwrap();
        let mut framer = Framer::new();
        let wire = framer.encode(&ack).unwrap();
        let mut decoder = FrameDecoder::default();
        let mut decoded = None;
        for &byte in &wire {
            if let Some(result) = decoder.push(byte) {
                decoded = Some(result.unwrap().payload.to_vec());
            }
        }
        let value: serde_json::Value = serde_json::from_slice(&decoded.unwrap()).unwrap();
        assert_eq!(value["name"], "hello_ack");
        assert_eq!(value["id"], "e-0001");
        let _ = Nonce::BYTES;
    }
}
