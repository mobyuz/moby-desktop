//! IPC commands for the robot link. Each one is narrow, and every input is checked here before it
//! reaches the link. The webview never names a path, and a port name it sends is opened only if
//! it appears in a fresh listing of USB serial ports. No key, proof or nonce ever crosses this
//! boundary: pairing is a command with no arguments, and the key is made and kept in Rust.

use moby_protocol::StableId;
use serde::Serialize;
use tauri::State;
use tauri::ipc::Channel;

use super::service::Robot;
use super::session::UserError;
use super::supervisor::{Command, Target};
use super::transport::{PortInfo, PortKind, plausible_port_name};
use super::view::Snapshot;
use crate::api::Failure;
use crate::confirm::{Confirmer, Prompt};

impl From<UserError> for Failure {
    fn from(error: UserError) -> Self {
        Failure::new(error.code, &error.message)
    }
}

#[derive(Serialize)]
pub struct DeviceEntry {
    id: String,
    name: String,
    created_at: i64,
}

/// Streams the link state: the current one at once, then every change.
#[tauri::command]
pub async fn robot_subscribe(
    robot: State<'_, Robot>,
    on_event: Channel<Snapshot>,
) -> Result<(), Failure> {
    robot.subscribe(on_event);
    Ok(())
}

/// USB serial ports, with the likely Moby ones marked.
#[tauri::command]
pub async fn robot_ports(robot: State<'_, Robot>) -> Result<Vec<PortInfo>, Failure> {
    Ok(robot.ports().await?)
}

/// Connects to the one port that looks like a Moby, or to the port the person chose.
#[tauri::command]
pub async fn robot_connect(
    robot: State<'_, Robot>,
    agent: State<'_, crate::agent::Agent>,
    port: Option<String>,
) -> Result<(), Failure> {
    let target = match port {
        None => Target::Auto,
        Some(name) if plausible_port_name(&name) => {
            let ports = robot.ports().await?;
            authorize_port(&ports, &name, agent.confirmer().as_ref()).await?;
            Target::Port(name)
        }
        Some(_) => return Err(Failure::new("invalid_input", "That isn't a port name.")),
    };
    Ok(robot.command(Command::Connect(target)).await?)
}

/// A port that does not look like a Moby is opened only after the person confirmed it in a native
/// window that shows its name and USB IDs. Opening one means writing to it, and some boards react
/// to a stray byte or a line change. Nothing is written before this returns.
pub(crate) async fn authorize_port(
    ports: &[PortInfo],
    name: &str,
    confirmer: &dyn Confirmer,
) -> Result<(), Failure> {
    let Some(port) = ports.iter().find(|port| port.name == name) else {
        return Err(Failure::new(
            "not_found",
            "That port is not connected any more.",
        ));
    };
    if port.kind == PortKind::Moby || port.simulated {
        return Ok(());
    }
    let ids = match (port.vid, port.pid) {
        (Some(vid), Some(pid)) => format!("USB ID {vid:04X}:{pid:04X}"),
        _ => "USB ID unknown".to_owned(),
    };
    let prompt = Prompt::new(
        "Open a device that does not look like Moby?",
        &[
            format!("Port: {}", port.name),
            ids,
            format!("The device calls itself: {}", port.label),
            "Moby will send it data to see whether it is a Moby. Only continue if you plugged this device in and want to try it.".to_owned(),
        ],
        "open this port",
    );
    if confirmer.confirm(prompt).await {
        Ok(())
    } else {
        Err(Failure::new("canceled", "The port was not opened."))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::confirm::testing::Scripted;

    fn port(name: &str, kind: PortKind) -> PortInfo {
        PortInfo {
            name: name.to_owned(),
            kind,
            label: "Some\u{202E}Board".to_owned(),
            simulated: false,
            vid: Some(0x10C4),
            pid: Some(0xEA60),
        }
    }

    #[tokio::test]
    async fn a_port_that_is_not_a_moby_needs_a_native_yes_that_shows_its_ids() {
        let ports = [
            port("COM3", PortKind::OtherUsb),
            port("COM4", PortKind::Moby),
        ];
        let no = Scripted::new(&[false]);
        let error = authorize_port(&ports, "COM3", &no).await.unwrap_err();
        assert_eq!(error.code_str(), "canceled");
        let shown = no.shown();
        assert_eq!(shown.len(), 1);
        assert!(shown[0].body.contains("COM3") && shown[0].body.contains("10C4:EA60"));
        assert!(!shown[0].body.contains('\u{202E}'));
        let yes = Scripted::new(&[true]);
        authorize_port(&ports, "COM3", &yes).await.unwrap();
        // A port that looks like a Moby needs no window, and an unknown one is refused outright.
        let none = Scripted::new(&[]);
        authorize_port(&ports, "COM4", &none).await.unwrap();
        assert!(none.shown().is_empty());
        assert_eq!(
            authorize_port(&ports, "COM9", &none)
                .await
                .unwrap_err()
                .code_str(),
            "not_found"
        );
    }
}

#[tauri::command]
pub async fn robot_disconnect(robot: State<'_, Robot>) -> Result<(), Failure> {
    Ok(robot.command(Command::Disconnect).await?)
}

/// Starts pairing. The interface then shows "press the talk button"; the key never leaves Rust.
#[tauri::command]
pub async fn robot_begin_pairing(robot: State<'_, Robot>) -> Result<(), Failure> {
    Ok(robot.command(Command::BeginPairing).await?)
}

#[tauri::command]
pub async fn robot_cancel_pairing(robot: State<'_, Robot>) -> Result<(), Failure> {
    Ok(robot.command(Command::CancelPairing).await?)
}

/// Robots paired with this computer: ID and name only.
#[tauri::command]
pub async fn robot_devices(robot: State<'_, Robot>) -> Result<Vec<DeviceEntry>, Failure> {
    let list = robot.devices().map_err(|_| {
        Failure::new(
            "store_unavailable",
            "Moby can't read its list of paired robots.",
        )
    })?;
    Ok(list
        .into_iter()
        .map(|record| DeviceEntry {
            id: record.id,
            name: record.name,
            created_at: record.created_at,
        })
        .collect())
}

/// Removes the pairing from this computer: the key and the list entry. The robot keeps its copy
/// until its talk button is held for 10 seconds.
#[tauri::command]
pub async fn robot_unpair(robot: State<'_, Robot>, device_id: String) -> Result<(), Failure> {
    if StableId::new(device_id.as_str()).is_err() {
        return Err(Failure::new("invalid_input", "That isn't a Moby."));
    }
    Ok(robot.command(Command::Unpair(device_id)).await?)
}

// ---- development panel ---------------------------------------------------------------------

#[cfg(debug_assertions)]
pub use dev::{DevAction, SimHub, dev_connector};

/// Drives the simulated robot. Development builds only: a release build answers `not_available`
/// and contains no simulator.
#[cfg(debug_assertions)]
#[tauri::command]
pub async fn robot_dev(robot: State<'_, Robot>, action: DevAction) -> Result<(), Failure> {
    Ok(robot.dev(action).await?)
}

#[cfg(not(debug_assertions))]
#[tauri::command]
pub async fn robot_dev() -> Result<(), Failure> {
    Err(Failure::new(
        "not_available",
        "The robot simulator is not part of this build.",
    ))
}

#[cfg(debug_assertions)]
mod dev {
    use std::sync::{Arc, Mutex, PoisonError};

    use moby_protocol::Expression;
    use moby_protocol::usb::PairKey;
    use serde::Deserialize;

    use super::super::clock::Clock;
    use super::super::session::{OsRandom, Random, UserError};
    use super::super::sim::{
        CombinedConnector, SIMULATOR_MARKER, SimConnector, SimRobot, StreamFault,
    };
    use super::super::supervisor::{Command, Supervisor, Target};
    use super::super::transport::{Connector, SerialConnector};

    const SIM_DEVICE: &str = "dev_51515151515151515151515151515151";

    #[derive(Debug, Deserialize)]
    #[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
    pub enum DevAction {
        /// Plugs in the simulated robot and connects to it. A fresh one starts from the given
        /// settings; otherwise the previous robot, with its pairing, comes back.
        Attach {
            fresh: bool,
            paired_elsewhere: bool,
            incompatible: bool,
        },
        Detach,
        Press,
        Release,
        LongPress,
        Mute {
            muted: bool,
        },
        Show {
            expression: Expression,
        },
        Silence {
            on: bool,
        },
        Unplug,
        Plug,
        CorruptFrames,
        Oversize,
        Garbage,
        /// The next push-to-talk stream goes wrong in the named way.
        StreamFault {
            fault: DevFault,
        },
    }

    #[derive(Debug, Clone, Copy, Deserialize)]
    #[serde(rename_all = "snake_case")]
    pub enum DevFault {
        None,
        BadSequence,
        Duplicate,
        Swap,
        WrongSize,
        Overrun,
        CutOff,
        NeverEnds,
        WrongCount,
        WrongStream,
        MuteMidStream,
        MuteAndKeepSending,
        RobotCancel,
    }

    pub struct SimHub {
        robot: Arc<Mutex<SimRobot>>,
        enabled: Arc<Mutex<bool>>,
    }

    pub fn dev_connector(clock: Arc<dyn Clock>) -> (Box<dyn Connector>, SimHub) {
        // Keeps the marker in development binaries, so the same search can show it is absent
        // from a release binary.
        std::hint::black_box(SIMULATOR_MARKER);
        let robot = Arc::new(Mutex::new(
            SimRobot::new(SIM_DEVICE).with_capabilities(&["usb_pcm"]),
        ));
        let enabled = Arc::new(Mutex::new(false));
        let connector = CombinedConnector {
            real: Box::new(SerialConnector),
            sim: SimConnector::new(Arc::clone(&robot), clock),
            sim_enabled: Arc::clone(&enabled),
        };
        (Box::new(connector), SimHub { robot, enabled })
    }

    impl SimHub {
        pub fn apply(
            &self,
            supervisor: &mut Supervisor,
            clock: &dyn Clock,
            action: DevAction,
        ) -> Result<(), UserError> {
            let now = clock.now_ms();
            let mut robot = self.robot.lock().unwrap_or_else(PoisonError::into_inner);
            match action {
                DevAction::Attach {
                    fresh,
                    paired_elsewhere,
                    incompatible,
                } => {
                    drop(robot);
                    supervisor.command(Command::Disconnect)?;
                    let mut robot = self.robot.lock().unwrap_or_else(PoisonError::into_inner);
                    if fresh {
                        let mut next = SimRobot::new(SIM_DEVICE).with_capabilities(&["usb_pcm"]);
                        if paired_elsewhere {
                            let mut bytes = [0_u8; PairKey::BYTES];
                            if OsRandom.fill(&mut bytes) {
                                next = next.with_key(PairKey::from_bytes(bytes));
                            }
                        }
                        if incompatible {
                            next = next.with_protocol_version(2);
                        }
                        *robot = next;
                    }
                    robot.plug();
                    drop(robot);
                    *self.enabled.lock().unwrap_or_else(PoisonError::into_inner) = true;
                    supervisor.command(Command::Connect(Target::Port("SIMULATOR".to_owned())))
                }
                DevAction::Detach => {
                    drop(robot);
                    supervisor.command(Command::Disconnect)?;
                    *self.enabled.lock().unwrap_or_else(PoisonError::into_inner) = false;
                    Ok(())
                }
                DevAction::Press => {
                    robot.press(now);
                    Ok(())
                }
                DevAction::Release => {
                    robot.release(now);
                    Ok(())
                }
                DevAction::LongPress => {
                    robot.long_press(now);
                    Ok(())
                }
                DevAction::Mute { muted } => {
                    robot.set_mute(now, muted);
                    Ok(())
                }
                DevAction::Show { expression } => {
                    robot.show(now, expression);
                    Ok(())
                }
                DevAction::Silence { on } => {
                    robot.set_silent(on);
                    Ok(())
                }
                DevAction::Unplug => {
                    robot.unplug();
                    Ok(())
                }
                DevAction::Plug => {
                    robot.plug();
                    Ok(())
                }
                DevAction::CorruptFrames => {
                    robot.corrupt_next_frames(3);
                    Ok(())
                }
                DevAction::Oversize => {
                    robot.inject_oversized();
                    Ok(())
                }
                DevAction::Garbage => {
                    robot.inject_garbage();
                    Ok(())
                }
                DevAction::StreamFault { fault } => {
                    // Each fault strikes the fourth frame, 60 ms in: early enough to see at once.
                    robot.inject_stream_fault(match fault {
                        DevFault::None => StreamFault::None,
                        DevFault::BadSequence => StreamFault::SkipFrame(3),
                        DevFault::Duplicate => StreamFault::DuplicateFrame(3),
                        DevFault::Swap => StreamFault::SwapFrames(3),
                        DevFault::WrongSize => StreamFault::WrongSize(3),
                        DevFault::Overrun => StreamFault::Overrun(3),
                        DevFault::CutOff => StreamFault::CutOff(3),
                        DevFault::NeverEnds => StreamFault::NeverEnds,
                        DevFault::WrongCount => StreamFault::WrongCount,
                        DevFault::WrongStream => StreamFault::WrongStream(3),
                        DevFault::MuteMidStream => StreamFault::MuteAt(3),
                        DevFault::MuteAndKeepSending => StreamFault::MuteAndKeepSending(3),
                        DevFault::RobotCancel => StreamFault::RobotCancel(3),
                    });
                    Ok(())
                }
            }
        }
    }
}
