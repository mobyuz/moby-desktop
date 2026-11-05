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
use super::transport::{PortInfo, plausible_port_name};
use super::view::Snapshot;
use crate::api::Failure;

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
pub async fn robot_connect(robot: State<'_, Robot>, port: Option<String>) -> Result<(), Failure> {
    let target = match port {
        None => Target::Auto,
        Some(name) if plausible_port_name(&name) => Target::Port(name),
        Some(_) => return Err(Failure::new("invalid_input", "That isn't a port name.")),
    };
    Ok(robot.command(Command::Connect(target)).await?)
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
    use super::super::sim::{CombinedConnector, SIMULATOR_MARKER, SimConnector, SimRobot};
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
    }

    pub struct SimHub {
        robot: Arc<Mutex<SimRobot>>,
        enabled: Arc<Mutex<bool>>,
    }

    pub fn dev_connector(clock: Arc<dyn Clock>) -> (Box<dyn Connector>, SimHub) {
        // Keeps the marker in development binaries, so the same search can show it is absent
        // from a release binary.
        std::hint::black_box(SIMULATOR_MARKER);
        let robot = Arc::new(Mutex::new(SimRobot::new(SIM_DEVICE)));
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
                        let mut next = SimRobot::new(SIM_DEVICE);
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
            }
        }
    }
}
