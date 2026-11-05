//! The thread that runs the supervisor, and the handle the IPC commands use to reach it. State
//! changes go out to the interface through channels it passed in (no event permission is needed,
//! so the webview capability set stays empty).

use std::sync::mpsc::{self, RecvTimeoutError, TryRecvError};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use tauri::AppHandle;
use tauri::ipc::Channel;
use tokio::sync::oneshot;

use super::clock::{Clock, SystemClock};
use super::pairings::{DeviceRecord, Fault, PairingStore, SystemPairings};
use super::session::UserError;
use super::supervisor::{Command, Supervisor, Target};
use super::transport::PortInfo;
use super::view::Snapshot;

const MAX_SUBSCRIBERS: usize = 8;
const IDLE_WAIT: Duration = Duration::from_millis(250);

pub(super) enum Msg {
    Command(Command, oneshot::Sender<Result<(), UserError>>),
    Ports(oneshot::Sender<Vec<PortInfo>>),
    #[cfg(debug_assertions)]
    Dev(
        super::commands::DevAction,
        oneshot::Sender<Result<(), UserError>>,
    ),
}

/// Handle to the robot link. Managed as Tauri state.
pub struct Robot {
    tx: mpsc::Sender<Msg>,
    subscribers: Arc<Mutex<Vec<Channel<Snapshot>>>>,
    latest: Arc<Mutex<Snapshot>>,
    pairings: Arc<dyn PairingStore>,
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Robot {
    /// Starts the link thread. If a robot has been paired on this computer, it also starts
    /// looking for one; otherwise nothing touches a port until the person asks.
    pub fn start(app: AppHandle) -> Self {
        let clock: Arc<dyn Clock> = Arc::new(SystemClock::new());
        let pairings: Arc<dyn PairingStore> = Arc::new(SystemPairings::new(app));
        let subscribers: Arc<Mutex<Vec<Channel<Snapshot>>>> = Arc::new(Mutex::new(Vec::new()));
        let latest = Arc::new(Mutex::new(Snapshot::disconnected(None)));
        let (tx, rx) = mpsc::channel();

        let publish = {
            let subscribers = Arc::clone(&subscribers);
            let latest = Arc::clone(&latest);
            Box::new(move |snapshot: &Snapshot| {
                *lock(&latest) = snapshot.clone();
                lock(&subscribers).retain(|channel| channel.send(snapshot.clone()).is_ok());
            })
        };

        #[cfg(debug_assertions)]
        let (connector, sim) = super::commands::dev_connector(Arc::clone(&clock));
        #[cfg(not(debug_assertions))]
        let connector: Box<dyn super::transport::Connector> =
            Box::new(super::transport::SerialConnector);

        let supervisor = Supervisor::new(
            Arc::clone(&clock),
            connector,
            Arc::clone(&pairings),
            publish,
        );
        let paired_before = pairings.list().is_ok_and(|list| !list.is_empty());
        if paired_before {
            let (reply, _) = oneshot::channel();
            let _ = tx.send(Msg::Command(Command::Connect(Target::Auto), reply));
        }
        let spawned = std::thread::Builder::new()
            .name("moby-robot".into())
            .spawn(move || {
                #[cfg(debug_assertions)]
                run(rx, supervisor, &*clock, sim);
                #[cfg(not(debug_assertions))]
                run(rx, supervisor);
            });
        if spawned.is_err() {
            eprintln!("moby: robot link: the link thread could not start");
        }
        Self {
            tx,
            subscribers,
            latest,
            pairings,
        }
    }

    pub fn subscribe(&self, channel: Channel<Snapshot>) {
        let _ = channel.send(lock(&self.latest).clone());
        let mut subscribers = lock(&self.subscribers);
        if subscribers.len() >= MAX_SUBSCRIBERS {
            subscribers.remove(0);
        }
        subscribers.push(channel);
    }

    pub async fn command(&self, command: Command) -> Result<(), UserError> {
        let (reply, answer) = oneshot::channel();
        self.tx
            .send(Msg::Command(command, reply))
            .map_err(|_| unavailable())?;
        answer.await.map_err(|_| unavailable())?
    }

    pub async fn ports(&self) -> Result<Vec<PortInfo>, UserError> {
        let (reply, answer) = oneshot::channel();
        self.tx.send(Msg::Ports(reply)).map_err(|_| unavailable())?;
        answer.await.map_err(|_| unavailable())
    }

    pub fn devices(&self) -> Result<Vec<DeviceRecord>, Fault> {
        self.pairings.list()
    }

    #[cfg(debug_assertions)]
    pub async fn dev(&self, action: super::commands::DevAction) -> Result<(), UserError> {
        let (reply, answer) = oneshot::channel();
        self.tx
            .send(Msg::Dev(action, reply))
            .map_err(|_| unavailable())?;
        answer.await.map_err(|_| unavailable())?
    }
}

fn unavailable() -> UserError {
    UserError {
        code: "unavailable",
        message: "The robot link is not running.".to_owned(),
    }
}

#[cfg(debug_assertions)]
fn run(
    rx: mpsc::Receiver<Msg>,
    mut supervisor: Supervisor,
    clock: &dyn Clock,
    sim: super::commands::SimHub,
) {
    run_loop(&rx, &mut supervisor, |supervisor, message| match message {
        Msg::Dev(action, reply) => {
            let _ = reply.send(sim.apply(supervisor, clock, action));
        }
        other => handle(supervisor, other),
    });
}

#[cfg(not(debug_assertions))]
fn run(rx: mpsc::Receiver<Msg>, mut supervisor: Supervisor) {
    run_loop(&rx, &mut supervisor, handle);
}

fn handle(supervisor: &mut Supervisor, message: Msg) {
    match message {
        Msg::Command(command, reply) => {
            let _ = reply.send(supervisor.command(command));
        }
        Msg::Ports(reply) => {
            let _ = reply.send(supervisor.ports());
        }
        #[cfg(debug_assertions)]
        Msg::Dev(_, reply) => {
            let _ = reply.send(Err(unavailable()));
        }
    }
}

fn run_loop(
    rx: &mpsc::Receiver<Msg>,
    supervisor: &mut Supervisor,
    mut on_message: impl FnMut(&mut Supervisor, Msg),
) {
    loop {
        // With nothing to do, sleep until a command arrives (or a quarter second passes).
        let mut next = if supervisor.is_idle() {
            match rx.recv_timeout(IDLE_WAIT) {
                Ok(message) => Some(message),
                Err(RecvTimeoutError::Timeout) => None,
                Err(RecvTimeoutError::Disconnected) => return,
            }
        } else {
            None
        };
        loop {
            let message = match next.take() {
                Some(message) => message,
                None => match rx.try_recv() {
                    Ok(message) => message,
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => return,
                },
            };
            on_message(supervisor, message);
        }
        supervisor.step();
    }
}
