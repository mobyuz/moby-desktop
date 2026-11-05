//! The link to the Moby robot over USB (stage D3).
//!
//! ```text
//!  webview  <-- Channel<Snapshot> --  service (thread)  --  Supervisor  --  Session (no I/O)
//!     |                                                         |
//!  narrow commands                                         Transport: USB serial port,
//!     |                                                    or the simulator (dev builds, tests)
//!  commands.rs
//! ```
//!
//! * `link.rs` frames and parses bytes with the `moby-protocol` codec and counts everything it
//!   drops. The robot is untrusted input.
//! * `session.rs` is the handshake, keepalive, pairing and state tracking, as a state machine.
//! * `supervisor.rs` finds the port, reconnects with backoff, and publishes state changes.
//! * `pairings.rs` keeps the key in Windows Credential Manager and the device in SQLite.
//! * `sim.rs` is a robot in software that follows the firmware. It exists only in development
//!   builds and tests.
//!
//! * `audio.rs` puts the robot's push-to-talk stream back together, strictly (stage D4). The robot
//!   is the only microphone this module knows. Nothing here records on the computer.

pub(crate) mod audio;
mod clock;
pub(crate) mod commands;
mod link;
mod pairings;
mod service;
mod session;
#[cfg(any(test, debug_assertions))]
pub(crate) mod sim;
mod supervisor;
#[cfg(test)]
mod tests;
mod transport;
mod view;

pub use service::Robot;

/// Whether the simulator is part of this build. That a release build says no is proved by
/// `scripts/check-release-excludes-dev.ps1`, which searches the built binary for the simulator's
/// marker string (a compile-time assert on this constant would only repeat the `cfg`).
#[allow(dead_code)]
pub const SIMULATOR_COMPILED: bool = cfg!(any(test, debug_assertions));
