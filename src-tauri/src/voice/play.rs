//! Playback on the computer's speakers. The chain talks to the `Speaker` trait, so tests inject a
//! fake and need no sound device. The real one uses `rodio` (pinned exactly, playback only), on a
//! thread of its own: the sound device handle is opened, used and dropped there, and never crosses
//! into async code. All of it is in Rust. The webview gets no audio permission and no way to play
//! anything.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// Playback format: what moby-api returns.
pub const SAMPLE_RATE: u32 = 16_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlayError {
    /// No output device, or it could not be opened.
    NoDevice,
}

/// Sound that is playing. Dropping it stops the sound.
pub trait Playing: Send {
    fn is_done(&self) -> bool;
    fn stop(&self);
}

pub trait Speaker: Send + Sync {
    /// Starts playing 16 kHz mono samples and returns at once.
    fn play(&self, samples: Vec<i16>) -> Result<Box<dyn Playing>, PlayError>;
}

/// The real speakers, through `rodio`.
pub struct DeviceSpeaker;

struct DevicePlaying {
    stop: Arc<AtomicBool>,
    done: Arc<AtomicBool>,
}

impl Playing for DevicePlaying {
    fn is_done(&self) -> bool {
        self.done.load(Ordering::SeqCst)
    }

    fn stop(&self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

impl Drop for DevicePlaying {
    fn drop(&mut self) {
        self.stop();
    }
}

impl Speaker for DeviceSpeaker {
    fn play(&self, samples: Vec<i16>) -> Result<Box<dyn Playing>, PlayError> {
        let stop = Arc::new(AtomicBool::new(false));
        let done = Arc::new(AtomicBool::new(false));
        let (opened_tx, opened_rx) = std::sync::mpsc::channel();
        let thread = {
            let (stop, done) = (Arc::clone(&stop), Arc::clone(&done));
            std::thread::Builder::new()
                .name("moby-speaker".into())
                .spawn(move || {
                    // Either way the other side hears about it exactly once.
                    let Ok(mut sink) = rodio::DeviceSinkBuilder::open_default_sink() else {
                        let _ = opened_tx.send(false);
                        done.store(true, Ordering::SeqCst);
                        return;
                    };
                    sink.log_on_drop(false);
                    let player = rodio::Player::connect_new(sink.mixer());
                    let floats: Vec<f32> = samples
                        .iter()
                        .map(|&sample| f32::from(sample) / 32_768.0)
                        .collect();
                    if let (Some(channels), Some(rate)) = (
                        std::num::NonZero::new(1_u16),
                        std::num::NonZero::new(SAMPLE_RATE),
                    ) {
                        player.append(rodio::buffer::SamplesBuffer::new(channels, rate, floats));
                    }
                    let _ = opened_tx.send(true);
                    while !stop.load(Ordering::SeqCst) && !player.empty() {
                        std::thread::sleep(Duration::from_millis(20));
                    }
                    player.stop();
                    done.store(true, Ordering::SeqCst);
                })
        };
        if thread.is_err() || !opened_rx.recv_timeout(Duration::from_secs(5)).unwrap_or(false) {
            stop.store(true, Ordering::SeqCst);
            return Err(PlayError::NoDevice);
        }
        Ok(Box::new(DevicePlaying { stop, done }))
    }
}

/// A speaker for tests: remembers what it was given and plays for a time a test sets.
#[cfg(test)]
pub mod fake {
    use std::sync::{Arc, Mutex, PoisonError};
    use std::time::{Duration, Instant};

    use super::{PlayError, Playing, Speaker};

    #[derive(Default)]
    pub struct FakeSpeaker {
        pub played: Mutex<Vec<Vec<i16>>>,
        pub stopped: Arc<Mutex<u32>>,
        pub duration: Duration,
        pub fail: bool,
    }

    struct FakePlaying {
        until: Instant,
        stopped: Arc<Mutex<u32>>,
        was_stopped: std::sync::atomic::AtomicBool,
    }

    impl Playing for FakePlaying {
        fn is_done(&self) -> bool {
            Instant::now() >= self.until
        }

        fn stop(&self) {
            if !self.was_stopped.swap(true, std::sync::atomic::Ordering::SeqCst) {
                *self.stopped.lock().unwrap_or_else(PoisonError::into_inner) += 1;
            }
        }
    }

    impl Drop for FakePlaying {
        fn drop(&mut self) {
            if !self.is_done() {
                self.stop();
            }
        }
    }

    impl Speaker for FakeSpeaker {
        fn play(&self, samples: Vec<i16>) -> Result<Box<dyn Playing>, PlayError> {
            if self.fail {
                return Err(PlayError::NoDevice);
            }
            self.played
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(samples);
            Ok(Box::new(FakePlaying {
                until: Instant::now() + self.duration,
                stopped: Arc::clone(&self.stopped),
                was_stopped: std::sync::atomic::AtomicBool::new(false),
            }))
        }
    }
}
