//! The robot's push-to-talk stream, put back together on the desktop (stage D4).
//!
//! The firmware (`claude/f4-ptt-audio`, ADR 0002) sends `audio_start`, then frames of 640 bytes
//! (20 ms of 16 kHz mono `pcm_s16le`) numbered from 0 for each stream, then `audio_end`. The robot
//! is untrusted input, so this module is strict: any frame that is missing, repeated, out of order,
//! of the wrong size or for another stream ends the whole turn with a clear reason. Partial audio
//! is never passed on, and nothing here guesses what a lost frame contained.
//!
//! Limits are hard: one stream at a time, at most `MAX_UTTERANCE_MS` and `MAX_UTTERANCE_BYTES`
//! buffered, and a stream that goes quiet for `STALL_MS` is dropped. The buffer is zeroed when the
//! stream ends, however it ends, and it never touches the disk. `Pcm` has no `Debug` output that
//! shows audio and the module never logs.
//!
//! This is a pure state machine: bytes and time come in, `AudioEvent`s and cancel requests go out.

use std::sync::Arc;

use moby_protocol::usb::{
    AudioCodec, AudioEnd, AudioEndReason, AudioStart, AudioStartReason, ErrorCode,
};

/// The format moby-api takes and the firmware sends: 16 kHz, mono, 16-bit.
pub const SAMPLE_RATE: u32 = 16_000;
pub const BYTES_PER_MS: usize = 32;
/// The firmware stops at 15 s by default. A configured limit may be longer, but the API takes at
/// most 30 s, so the desktop's own hard limit sits between the two.
pub const MAX_UTTERANCE_MS: u32 = 20_000;
pub const MAX_UTTERANCE_BYTES: usize = MAX_UTTERANCE_MS as usize * BYTES_PER_MS;
/// Anything shorter is a bump of the button, not a sentence.
pub const MIN_UTTERANCE_MS: u32 = 200;
/// A stream with no frame for this long is cut off.
pub const STALL_MS: u64 = 1_500;
/// The frame length the firmware uses. Other lengths that fit a frame are accepted too.
const MAX_FRAME_BYTES: usize = 1024;

/// Raw audio. It is zeroed when dropped, shows only its length when printed, and has no way to be
/// written anywhere by this module.
pub struct Pcm(Vec<u8>);

impl Pcm {
    pub fn new(bytes: Vec<u8>) -> Self {
        Self(bytes)
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Overwrites the audio now instead of when the value is dropped.
    pub fn wipe(&mut self) {
        self.0.fill(0);
        self.0.clear();
    }
}

impl Drop for Pcm {
    fn drop(&mut self) {
        self.wipe();
    }
}

impl std::fmt::Debug for Pcm {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Pcm({} bytes)", self.0.len())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AbortReason {
    /// A frame was lost, repeated or out of order.
    BadSequence,
    /// A frame was not the announced size.
    BadFrameSize,
    /// The stream went past the hard limit.
    TooLong,
    /// The robot's ring buffer overflowed (`E_AUDIO_OVERRUN`).
    Overrun,
    /// The stream went quiet without an `audio_end`.
    Stalled,
    /// The hardware mute went on.
    Mute,
    /// The robot ended the stream with `cancel`.
    RobotCancel,
    /// The robot ended the stream with `error`.
    RobotError,
    /// `audio_end` named another number of frames than arrived.
    CountMismatch,
    /// No audio at all.
    Empty,
    /// Less than a fifth of a second.
    TooShort,
    /// Parameters this desktop does not take, or a wake-word start.
    Unsupported,
    /// A second stream started while one was open.
    Overlap,
    /// The robot could not start a stream now (not authenticated, muted, no capability).
    NotAllowed,
    /// The cable or the link went away.
    LinkLost,
    /// The person pressed Stop on the desktop.
    Canceled,
}

impl AbortReason {
    pub fn code(self) -> &'static str {
        match self {
            Self::BadSequence => "audio_sequence",
            Self::BadFrameSize => "audio_frame_size",
            Self::TooLong => "audio_too_long",
            Self::Overrun => "audio_overrun",
            Self::Stalled => "audio_stalled",
            Self::Mute => "audio_muted",
            Self::RobotCancel => "audio_canceled",
            Self::RobotError => "audio_robot_error",
            Self::CountMismatch => "audio_incomplete",
            Self::Empty => "audio_empty",
            Self::TooShort => "audio_short",
            Self::Unsupported => "audio_unsupported",
            Self::Overlap => "audio_overlap",
            Self::NotAllowed => "audio_not_allowed",
            Self::LinkLost => "audio_link_lost",
            Self::Canceled => "canceled",
        }
    }

    /// One sentence for the person. Nothing was sent anywhere.
    pub fn message(self) -> &'static str {
        match self {
            Self::BadSequence | Self::BadFrameSize | Self::CountMismatch => {
                "Part of what Moby heard was lost on the way, so nothing was used. Please try again."
            }
            Self::TooLong => {
                "That was too long, so nothing was used. Please say it in a shorter sentence."
            }
            Self::Overrun => {
                "Moby could not keep up with the audio, so nothing was used. Please try again."
            }
            Self::Stalled => {
                "The audio from Moby stopped in the middle, so nothing was used. Please try again."
            }
            Self::Mute => "Moby's mute switch went on, so the audio was thrown away.",
            Self::RobotCancel => "Moby canceled the recording, so the audio was thrown away.",
            Self::RobotError => "Moby had a problem while recording, so the audio was thrown away.",
            Self::Empty => "Moby did not hear anything.",
            Self::TooShort => {
                "That was too short to understand. Hold the talk button while you speak."
            }
            Self::Unsupported => {
                "Moby sent audio in a form this app does not take, so it was thrown away."
            }
            Self::Overlap => {
                "Moby started a second recording before the first ended, so both were thrown away."
            }
            Self::NotAllowed => "Moby is not allowed to record right now, so nothing was used.",
            Self::LinkLost => {
                "The connection to Moby was lost while recording, so the audio was thrown away."
            }
            Self::Canceled => "Stopped.",
        }
    }
}

/// The stream IDs are for tests and for a reader of the code; the chain handles one stream at a time.
#[derive(Debug)]
#[allow(dead_code)]
pub enum AudioEvent {
    /// A stream began: the person is talking.
    Started { stream_id: u16 },
    /// The whole utterance, complete and checked.
    Utterance { stream_id: u16, pcm: Pcm },
    /// The stream ended without a usable utterance. The audio is already gone.
    Aborted {
        stream_id: Option<u16>,
        reason: AbortReason,
    },
}

pub type AudioSink = Arc<dyn Fn(AudioEvent) + Send + Sync>;

struct Open {
    stream_id: u16,
    frame_bytes: usize,
    next_sequence: u32,
    frames: u32,
    buf: Vec<u8>,
    last_activity: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AudioStats {
    pub streams_completed: u64,
    pub streams_aborted: u64,
    pub frames_ignored: u64,
    pub events_ignored: u64,
}

pub struct Assembler {
    sink: AudioSink,
    open: Option<Open>,
    cancels: Vec<u16>,
    pub stats: AudioStats,
}

impl Assembler {
    pub fn new(sink: AudioSink) -> Self {
        Self {
            sink,
            open: None,
            cancels: Vec::new(),
            stats: AudioStats::default(),
        }
    }

    #[cfg(test)]
    pub fn is_open(&self) -> bool {
        self.open.is_some()
    }

    /// Streams the robot should be told to cancel, as `cancel` commands.
    pub fn take_cancels(&mut self) -> Vec<u16> {
        std::mem::take(&mut self.cancels)
    }

    fn emit(&self, event: AudioEvent) {
        (self.sink)(event);
    }

    /// Ends the open stream with a reason, wipes the buffer and asks the robot to stop.
    fn abort(&mut self, reason: AbortReason, tell_robot: bool) {
        let Some(mut open) = self.open.take() else {
            return;
        };
        open.buf.fill(0);
        self.stats.streams_aborted += 1;
        if tell_robot {
            self.cancels.push(open.stream_id);
        }
        self.emit(AudioEvent::Aborted {
            stream_id: Some(open.stream_id),
            reason,
        });
    }

    /// An `audio_start`. `allowed` is the session's verdict: established, authenticated, not muted,
    /// and the robot offers `usb_pcm`.
    pub fn on_start(&mut self, now: u64, start: &AudioStart, allowed: bool) {
        let stream_id = start.stream_id.get();
        if self.open.is_some() {
            // Two streams at once cannot both be right. Drop both.
            self.abort(AbortReason::Overlap, true);
            self.cancels.push(stream_id);
            self.stats.streams_aborted += 1;
            self.emit(AudioEvent::Aborted {
                stream_id: Some(stream_id),
                reason: AbortReason::Overlap,
            });
            return;
        }
        let frame_bytes = start.frame_bytes();
        let reject = |this: &mut Self, reason: AbortReason| {
            this.cancels.push(stream_id);
            this.stats.streams_aborted += 1;
            this.emit(AudioEvent::Aborted {
                stream_id: Some(stream_id),
                reason,
            });
        };
        if !allowed {
            self.stats.events_ignored += 1;
            reject(self, AbortReason::NotAllowed);
            return;
        }
        // Only a button press starts a stream at this stage: there is no wake word. The format is
        // the one moby-api takes, and a frame must fit the protocol's payload limit exactly.
        let valid = start.reason == AudioStartReason::Button
            && start.codec == AudioCodec::PcmS16le
            && start.sample_rate.get() == SAMPLE_RATE
            && start.channels.get() == 1
            && frame_bytes > 0
            && frame_bytes <= MAX_FRAME_BYTES
            && frame_bytes.is_multiple_of(2)
            && start.preroll_ms.get() == 0;
        if !valid {
            reject(self, AbortReason::Unsupported);
            return;
        }
        self.open = Some(Open {
            stream_id,
            frame_bytes,
            next_sequence: 0,
            frames: 0,
            buf: Vec::with_capacity(frame_bytes * 64),
            last_activity: now,
        });
        self.emit(AudioEvent::Started { stream_id });
    }

    /// One frame of robot audio.
    pub fn on_frame(&mut self, now: u64, stream_id: u16, sequence: u32, payload: &[u8]) {
        let Some(open) = &mut self.open else {
            // Late frames after an abort, or frames nobody asked for.
            self.stats.frames_ignored += 1;
            return;
        };
        if open.stream_id != stream_id {
            self.abort(AbortReason::BadSequence, true);
            return;
        }
        if sequence != open.next_sequence {
            self.abort(AbortReason::BadSequence, true);
            return;
        }
        if payload.len() != open.frame_bytes {
            self.abort(AbortReason::BadFrameSize, true);
            return;
        }
        if open.buf.len() + payload.len() > MAX_UTTERANCE_BYTES {
            self.abort(AbortReason::TooLong, true);
            return;
        }
        open.buf.extend_from_slice(payload);
        open.next_sequence = open.next_sequence.wrapping_add(1);
        open.frames += 1;
        open.last_activity = now;
    }

    /// An `audio_end`. The only ending that can produce an utterance.
    pub fn on_end(&mut self, end: &AudioEnd) {
        let stream_id = end.stream_id.get();
        match &self.open {
            Some(open) if open.stream_id == stream_id => {}
            _ => {
                self.stats.events_ignored += 1;
                return;
            }
        }
        match end.reason {
            AudioEndReason::Mute => return self.abort(AbortReason::Mute, false),
            AudioEndReason::Cancel => return self.abort(AbortReason::RobotCancel, false),
            AudioEndReason::Error => return self.abort(AbortReason::RobotError, false),
            AudioEndReason::VadSilence
            | AudioEndReason::MaxDuration
            | AudioEndReason::ButtonRelease => {}
        }
        let Some(mut open) = self.open.take() else {
            return;
        };
        let verdict = if open.frames != end.frames {
            Some(AbortReason::CountMismatch)
        } else if open.buf.is_empty() {
            Some(AbortReason::Empty)
        } else if u32::try_from(open.buf.len() / BYTES_PER_MS).unwrap_or(u32::MAX)
            < MIN_UTTERANCE_MS
        {
            Some(AbortReason::TooShort)
        } else {
            None
        };
        if let Some(reason) = verdict {
            open.buf.fill(0);
            self.stats.streams_aborted += 1;
            // The robot already ended it. Nothing to cancel.
            self.emit(AudioEvent::Aborted {
                stream_id: Some(stream_id),
                reason,
            });
            return;
        }
        self.stats.streams_completed += 1;
        let pcm = Pcm::new(std::mem::take(&mut open.buf));
        self.emit(AudioEvent::Utterance { stream_id, pcm });
    }

    /// The robot reported an error. An overrun ends the stream, because the robot already dropped
    /// audio and what is left has a hole in it.
    pub fn on_robot_error(&mut self, code: ErrorCode) {
        if code == ErrorCode::AudioOverrun {
            self.abort(AbortReason::Overrun, true);
        }
    }

    /// The hardware mute is on: the stream is over and the buffer is gone, whatever the robot says
    /// next.
    pub fn on_mute(&mut self) {
        self.abort(AbortReason::Mute, false);
    }

    /// The person pressed Stop.
    pub fn cancel(&mut self) {
        self.abort(AbortReason::Canceled, true);
    }

    /// Time passes. A stream that goes quiet, or runs past the hard time limit, is dropped.
    pub fn tick(&mut self, now: u64) {
        let Some(open) = &self.open else {
            return;
        };
        if now.saturating_sub(open.last_activity) > STALL_MS {
            self.abort(AbortReason::Stalled, true);
        }
    }

    /// Whether the session says this stream may exist at all. Called after every message.
    pub fn check_allowed(&mut self, allowed: bool) {
        if !allowed {
            self.abort(AbortReason::NotAllowed, false);
        }
    }
}

impl Drop for Assembler {
    fn drop(&mut self) {
        // The link is gone (cable, switch of port, disconnect). Nobody is left to cancel.
        self.abort(AbortReason::LinkLost, false);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    fn start(stream: u16) -> AudioStart {
        serde_json::from_value(serde_json::json!({
            "stream_id": stream, "reason": "button", "codec": "pcm_s16le",
            "sample_rate": 16000, "channels": 1, "frame_ms": 20, "preroll_ms": 0
        }))
        .unwrap()
    }

    fn end(stream: u16, reason: &str, frames: u32) -> AudioEnd {
        serde_json::from_value(serde_json::json!({
            "stream_id": stream, "reason": reason, "frames": frames, "duration_ms": frames * 20
        }))
        .unwrap()
    }

    fn rig() -> (Assembler, Arc<Mutex<Vec<String>>>) {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink: AudioSink = {
            let seen = Arc::clone(&seen);
            Arc::new(move |event| {
                let text = match event {
                    AudioEvent::Started { stream_id } => format!("started {stream_id}"),
                    AudioEvent::Utterance { stream_id, pcm } => {
                        format!("utterance {stream_id} {}", pcm.len())
                    }
                    AudioEvent::Aborted { reason, .. } => format!("aborted {}", reason.code()),
                };
                seen.lock().unwrap().push(text);
            })
        };
        (Assembler::new(sink), seen)
    }

    fn feed(assembler: &mut Assembler, stream: u16, frames: u32) {
        for sequence in 0..frames {
            assembler.on_frame(10, stream, sequence, &[7; 640]);
        }
    }

    #[test]
    fn a_whole_stream_becomes_one_utterance() {
        let (mut a, seen) = rig();
        a.on_start(0, &start(3), true);
        feed(&mut a, 3, 25);
        a.on_end(&end(3, "button_release", 25));
        assert_eq!(*seen.lock().unwrap(), ["started 3", "utterance 3 16000"]);
        assert!(!a.is_open());
        assert_eq!(a.stats.streams_completed, 1);
    }

    #[test]
    fn a_gap_a_repeat_or_a_swap_ends_the_turn_and_asks_the_robot_to_stop() {
        for order in [[0, 1, 3], [0, 1, 1], [0, 2, 1], [1, 2, 3]] {
            let (mut a, seen) = rig();
            a.on_start(0, &start(3), true);
            for sequence in order {
                a.on_frame(1, 3, sequence, &[1; 640]);
            }
            a.on_end(&end(3, "button_release", 3));
            let seen = seen.lock().unwrap();
            assert_eq!(seen.len(), 2, "{order:?}: {seen:?}");
            assert_eq!(seen[1], "aborted audio_sequence", "{order:?}");
            assert_eq!(a.take_cancels(), [3], "{order:?}");
        }
    }

    #[test]
    fn frame_sizes_must_match_the_announcement() {
        let (mut a, seen) = rig();
        a.on_start(0, &start(3), true);
        a.on_frame(1, 3, 0, &[0; 640]);
        a.on_frame(1, 3, 1, &[0; 320]);
        assert_eq!(seen.lock().unwrap()[1], "aborted audio_frame_size");
    }

    #[test]
    fn the_buffer_has_a_hard_byte_and_duration_bound() {
        let (mut a, seen) = rig();
        a.on_start(0, &start(9), true);
        let frames_allowed = MAX_UTTERANCE_BYTES / 640;
        feed(&mut a, 9, u32::try_from(frames_allowed).unwrap());
        assert!(a.is_open(), "exactly at the limit is still fine");
        a.on_frame(10, 9, u32::try_from(frames_allowed).unwrap(), &[0; 640]);
        assert!(!a.is_open());
        assert_eq!(seen.lock().unwrap()[1], "aborted audio_too_long");
    }

    #[test]
    fn a_wrong_frame_count_or_an_empty_or_tiny_stream_is_never_used() {
        for (frames, claimed, expect) in [
            (5, 6, "aborted audio_incomplete"),
            (0, 0, "aborted audio_empty"),
            (5, 5, "aborted audio_short"),
        ] {
            let (mut a, seen) = rig();
            a.on_start(0, &start(2), true);
            feed(&mut a, 2, frames);
            a.on_end(&end(2, "vad_silence", claimed));
            assert_eq!(seen.lock().unwrap()[1], expect);
        }
    }

    #[test]
    fn the_robot_ending_a_stream_for_mute_cancel_or_error_discards_it() {
        for (reason, expect) in [
            ("mute", "aborted audio_muted"),
            ("cancel", "aborted audio_canceled"),
            ("error", "aborted audio_robot_error"),
        ] {
            let (mut a, seen) = rig();
            a.on_start(0, &start(2), true);
            feed(&mut a, 2, 50);
            a.on_end(&end(2, reason, 50));
            assert_eq!(seen.lock().unwrap()[1], expect);
            assert!(a.take_cancels().is_empty(), "the robot already ended it");
        }
    }

    #[test]
    fn overrun_mute_stop_and_stall_all_end_a_stream() {
        let (mut a, seen) = rig();
        a.on_start(0, &start(2), true);
        feed(&mut a, 2, 50);
        a.on_robot_error(ErrorCode::AudioOverrun);
        a.on_start(0, &start(3), true);
        a.on_mute();
        a.on_start(0, &start(4), true);
        a.cancel();
        a.on_start(0, &start(5), true);
        a.tick(1 + STALL_MS + 10);
        let seen = seen.lock().unwrap().clone();
        let ended: Vec<&str> = seen
            .iter()
            .filter(|line| line.starts_with("aborted"))
            .map(String::as_str)
            .collect();
        assert_eq!(
            ended,
            [
                "aborted audio_overrun",
                "aborted audio_muted",
                "aborted canceled",
                "aborted audio_stalled"
            ]
        );
        assert!(!a.is_open());
    }

    #[test]
    fn a_stream_that_is_not_allowed_or_is_odd_is_refused_and_cancelled() {
        let (mut a, seen) = rig();
        a.on_start(0, &start(2), false);
        let mut wake = start(3);
        wake.reason = AudioStartReason::Wake;
        a.on_start(0, &wake, true);
        let mut rate = start(4);
        rate.sample_rate = moby_protocol::usb::SampleRate::try_from(8000).unwrap();
        a.on_start(0, &rate, true);
        let mut stereo = start(5);
        stereo.channels = moby_protocol::usb::ChannelCount::try_from(2).unwrap();
        a.on_start(0, &stereo, true);
        assert_eq!(
            *seen.lock().unwrap(),
            [
                "aborted audio_not_allowed",
                "aborted audio_unsupported",
                "aborted audio_unsupported",
                "aborted audio_unsupported"
            ]
        );
        assert_eq!(a.take_cancels(), [2, 3, 4, 5]);
        assert!(!a.is_open());
    }

    #[test]
    fn a_second_start_drops_both_streams() {
        let (mut a, seen) = rig();
        a.on_start(0, &start(2), true);
        a.on_start(0, &start(3), true);
        assert_eq!(a.stats.streams_aborted, 2);
        assert!(!a.is_open());
        assert!(
            seen.lock()
                .unwrap()
                .iter()
                .filter(|s| s.contains("overlap"))
                .count()
                == 2
        );
    }

    #[test]
    fn frames_with_no_stream_are_counted_and_dropped() {
        let (mut a, seen) = rig();
        a.on_frame(0, 1, 0, &[0; 640]);
        a.on_end(&end(1, "button_release", 1));
        assert_eq!(a.stats.frames_ignored, 1);
        assert_eq!(a.stats.events_ignored, 1);
        assert!(seen.lock().unwrap().is_empty());
    }

    #[test]
    fn dropping_the_link_mid_stream_reports_it_and_the_audio_is_zeroed() {
        let (mut a, seen) = rig();
        a.on_start(0, &start(2), true);
        feed(&mut a, 2, 10);
        drop(a);
        assert_eq!(seen.lock().unwrap()[1], "aborted audio_link_lost");
    }

    #[test]
    fn pcm_wipes_itself_and_never_prints_its_contents() {
        let mut pcm = Pcm::new(vec![0xAB; 64]);
        assert_eq!(format!("{pcm:?}"), "Pcm(64 bytes)");
        pcm.wipe();
        assert_eq!(pcm.len(), 0);
    }
}
