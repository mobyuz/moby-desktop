//! What the interface is told about the robot. These types are the whole of what crosses the IPC
//! boundary: states, short messages the app wrote, and counters. Never a key, a proof, a nonce, a
//! raw frame or a path. Text that came from the robot is cut to protocol bounds and shown as text.

use moby_protocol::Expression;
use serde::Serialize;

use super::link::Stats;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    /// No link, and none wanted.
    Disconnected,
    /// Opening the port, or waiting for the robot to say hello.
    Connecting,
    /// The link dropped. Trying again.
    Reconnecting,
    /// The robot speaks another protocol version.
    Incompatible,
    /// A robot that holds no key. It can be paired.
    Unpaired,
    /// A pairing is in progress.
    Pairing,
    /// The robot holds a key that this computer does not have.
    ForeignPaired,
    /// The robot did not accept the key stored here.
    KeyRejected,
    /// Windows Credential Manager could not be read, so the key is unavailable.
    VaultUnavailable,
    /// Connected and authenticated.
    Ready,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PairStep {
    /// `pair_begin` is on its way.
    Starting,
    /// Waiting for a press of the talk button on the robot.
    PressButton,
    /// The button was pressed. The key is being saved and sent.
    Saving,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DeviceView {
    pub id: String,
    pub name: String,
    pub hw_rev: String,
    pub fw_version: String,
    /// This computer holds a key for the robot.
    pub paired_here: bool,
    /// The robot says it holds a key.
    pub paired_on_robot: bool,
    /// The robot offers audio over USB. Audio itself is a later stage.
    pub audio: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PairFailure {
    pub code: &'static str,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RobotError {
    pub code: String,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Snapshot {
    pub phase: Phase,
    /// One sentence for people, written by this app, when the phase needs explaining.
    pub message: Option<String>,
    /// The hardware mute switch, as the robot reports it. This app can only show it.
    pub muted: bool,
    pub expression: Expression,
    pub device: Option<DeviceView>,
    pub pair_step: Option<PairStep>,
    pub pair_failure: Option<PairFailure>,
    pub error: Option<RobotError>,
    /// Set while waiting to reconnect: how long the wait is.
    pub retry_in_ms: Option<u32>,
    pub port: Option<String>,
    pub simulated: bool,
    pub stats: Stats,
}

impl Snapshot {
    pub fn disconnected(message: Option<String>) -> Self {
        Self {
            phase: Phase::Disconnected,
            message,
            muted: false,
            expression: Expression::Disconnected,
            device: None,
            pair_step: None,
            pair_failure: None,
            error: None,
            retry_in_ms: None,
            port: None,
            simulated: false,
            stats: Stats::default(),
        }
    }
}

/// A short readable name for a robot, built from the end of its ID. The robot has no name field.
pub fn default_name(device_id: &str) -> String {
    let tail: String = device_id
        .chars()
        .rev()
        .take(4)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    format!("Moby {tail}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_come_from_the_end_of_the_id() {
        assert_eq!(default_name("dev_0123456789abcdef"), "Moby cdef");
        assert_eq!(default_name("ab"), "Moby ab");
    }
}
