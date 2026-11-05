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
//! Audio is not here: it is stage D4, and nothing in this module starts a microphone.

mod clock;
mod commands;
mod link;
mod pairings;
mod service;
mod session;
#[cfg(any(test, debug_assertions))]
mod sim;
mod supervisor;
#[cfg(test)]
mod tests;
mod transport;
mod view;

pub use commands::{
    robot_begin_pairing, robot_cancel_pairing, robot_connect, robot_devices, robot_disconnect,
    robot_dev, robot_ports, robot_subscribe, robot_unpair,
};
pub use service::Robot;

/// Whether the simulator is part of this build. Release builds say no, and the check below makes
/// that a compile-time fact rather than a convention.
pub const SIMULATOR_COMPILED: bool = cfg!(any(test, debug_assertions));

#[cfg(not(any(test, debug_assertions)))]
const _: () = assert!(!SIMULATOR_COMPILED, "the simulator must not be in a release build");
