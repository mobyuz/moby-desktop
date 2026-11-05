//! The computer's own microphone, for development only: it lets the voice chain be tried without a
//! robot. The product takes voice from the robot's microphone and nowhere else (owner decision,
//! docs/adr/0007-voice-chain.md).
//!
//! This whole file is compiled only where the simulator is: tests and builds with debug
//! assertions. A release build contains none of it. `MIC_MARKER` below is a string that exists in
//! the binary only if this file was compiled in, and a test searches a release binary for it.
//! Capture starts only when a person asks in the development panel, and the interface shows
//! "mic on" for exactly as long as it runs. There is no wake word and no always-listening mode.
//! The audio stays in memory and is handed to the same chain as the robot's.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

use crate::robot::audio::{MAX_UTTERANCE_MS, Pcm};

/// A string that exists in the binary only if the PC microphone was compiled in.
pub const MIC_MARKER: &str = "moby-pc-microphone-compiled-3c9e51d7";

const TARGET_RATE: u32 = 16_000;

/// Interleaved device samples to 16 kHz mono 16-bit little-endian PCM: channels are averaged and
/// the rate is changed by linear interpolation. Plenty for speech in a development tool.
pub fn to_pcm16k_mono(samples: &[f32], rate: u32, channels: u16) -> Vec<u8> {
    if rate == 0 || channels == 0 {
        return Vec::new();
    }
    let channels = usize::from(channels);
    let mono: Vec<f32> = samples
        .chunks_exact(channels)
        .map(|frame| frame.iter().sum::<f32>() / channels as f32)
        .collect();
    if mono.is_empty() {
        return Vec::new();
    }
    let out_len = (mono.len() as u64 * u64::from(TARGET_RATE) / u64::from(rate)) as usize;
    let mut pcm = Vec::with_capacity(out_len * 2);
    for index in 0..out_len {
        let position = index as f64 * f64::from(rate) / f64::from(TARGET_RATE);
        let base = position.floor() as usize;
        let fraction = (position - base as f64) as f32;
        let a = mono[base.min(mono.len() - 1)];
        let b = mono[(base + 1).min(mono.len() - 1)];
        let value = (a + (b - a) * fraction).clamp(-1.0, 1.0);
        pcm.extend_from_slice(&((value * 32_767.0) as i16).to_le_bytes());
    }
    pcm
}

pub struct MicCapture {
    stop: Arc<AtomicBool>,
    thread: JoinHandle<Vec<u8>>,
}

impl MicCapture {
    /// Opens the default input device and starts capturing on a thread of its own.
    pub fn start() -> Result<Self, String> {
        let stop = Arc::new(AtomicBool::new(false));
        let (opened_tx, opened_rx) = std::sync::mpsc::channel::<Result<(), String>>();
        let thread = {
            let stop = Arc::clone(&stop);
            std::thread::Builder::new()
                .name("moby-dev-mic".into())
                .spawn(move || capture(&stop, &opened_tx))
                .map_err(|_| "the capture thread could not start".to_owned())?
        };
        match opened_rx.recv_timeout(Duration::from_secs(5)) {
            Ok(Ok(())) => Ok(Self { stop, thread }),
            Ok(Err(message)) => {
                let _ = thread.join();
                Err(message)
            }
            Err(_) => {
                stop.store(true, Ordering::SeqCst);
                Err("the microphone did not answer".to_owned())
            }
        }
    }

    /// Stops capturing and returns what was heard, at most the utterance limit.
    pub fn finish(self) -> Pcm {
        self.stop.store(true, Ordering::SeqCst);
        Pcm::new(self.thread.join().unwrap_or_default())
    }
}

fn capture(stop: &AtomicBool, opened: &std::sync::mpsc::Sender<Result<(), String>>) -> Vec<u8> {
    use rodio::Source;
    let opened_mic = rodio::microphone::MicrophoneBuilder::new()
        .default_device()
        .and_then(|builder| builder.default_config())
        .map_err(|error| error.to_string())
        .and_then(|builder| builder.open_stream().map_err(|error| error.to_string()));
    let mut mic = match opened_mic {
        Ok(mic) => mic,
        Err(message) => {
            let _ = opened.send(Err(message));
            return Vec::new();
        }
    };
    let _ = opened.send(Ok(()));
    let (rate, channels) = (mic.sample_rate().get(), mic.channels().get());
    let limit = rate as usize * usize::from(channels) * (MAX_UTTERANCE_MS as usize / 1000);
    let mut samples: Vec<f32> = Vec::new();
    while !stop.load(Ordering::SeqCst) && samples.len() < limit {
        match mic.next() {
            Some(sample) => samples.push(sample),
            None => break,
        }
    }
    let pcm = to_pcm16k_mono(&samples, rate, channels);
    samples.fill(0.0);
    pcm
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stereo_48k_becomes_mono_16k() {
        // 480 stereo frames at 48 kHz are 10 ms, so 160 samples at 16 kHz.
        let input: Vec<f32> = (0..960).map(|n| if n % 2 == 0 { 0.5 } else { -0.5 }).collect();
        let pcm = to_pcm16k_mono(&input, 48_000, 2);
        assert_eq!(pcm.len(), 160 * 2);
        assert!(pcm.chunks_exact(2).all(|pair| i16::from_le_bytes([pair[0], pair[1]]) == 0));
    }

    #[test]
    fn a_tone_keeps_its_level_and_clipping_is_bounded() {
        let loud = vec![2.0_f32; 1600];
        let pcm = to_pcm16k_mono(&loud, 16_000, 1);
        assert_eq!(pcm.len(), 3200);
        assert_eq!(i16::from_le_bytes([pcm[0], pcm[1]]), 32_767);
        assert!(to_pcm16k_mono(&[], 16_000, 1).is_empty());
        assert!(to_pcm16k_mono(&[0.1], 0, 1).is_empty());
        assert!(to_pcm16k_mono(&[0.1], 16_000, 0).is_empty());
    }
}
