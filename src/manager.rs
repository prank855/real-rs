use std::collections::{BTreeMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use windows::core::PCWSTR;
use windows::Win32::Foundation::PROPERTYKEY;
use windows::Win32::Media::Audio::{
    EDataFlow, ERole, IMMDevice, IMMDeviceEnumerator, IMMNotificationClient,
    IMMNotificationClient_Impl, DEVICE_STATE,
};

use crate::audio::{self, AppResult, ComApartment, Props, Session, Target};
use crate::logger::Logger;

#[derive(Clone)]
pub struct EndpointView {
    pub id: String,
    pub name: String,
    pub role: String,
    pub properties: Props,
    pub applied: bool,
    pub paused: bool,
    pub status: String,
}

#[derive(Clone, Default)]
pub struct Snapshot {
    pub endpoints: Vec<EndpointView>,
    pub held: usize,
    pub open_failed: usize,
}

#[derive(Clone)]
pub struct Ui {
    control: Control,
    snapshot: Arc<Mutex<Snapshot>>,
}

impl Ui {
    pub fn snapshot(&self) -> Snapshot {
        self.snapshot.lock().unwrap().clone()
    }

    pub fn control(&self) -> Control {
        self.control.clone()
    }
}

#[derive(Clone)]
pub struct Control {
    commands: mpsc::Sender<Command>,
}

impl Control {
    pub fn stop_device(&self, device_id: &str) {
        let _ = self.commands.send(Command::Stop(device_id.to_string()));
    }

    pub fn start_device(&self, device_id: &str) {
        let _ = self.commands.send(Command::Start(device_id.to_string()));
    }

    fn quit(&self) {
        let _ = self.commands.send(Command::Quit);
    }
}

enum Command {
    Rescan,
    Stop(String),
    Start(String),
    Quit,
}

#[windows::core::implement(IMMNotificationClient)]
struct NotificationClient {
    commands: mpsc::Sender<Command>,
}

impl NotificationClient {
    fn notify(&self) {
        let _ = self.commands.send(Command::Rescan);
    }
}

impl IMMNotificationClient_Impl for NotificationClient_Impl {
    fn OnDeviceStateChanged(
        &self,
        _device_id: &PCWSTR,
        _new_state: DEVICE_STATE,
    ) -> windows::core::Result<()> {
        self.notify();
        Ok(())
    }

    fn OnDeviceAdded(&self, _device_id: &PCWSTR) -> windows::core::Result<()> {
        self.notify();
        Ok(())
    }

    fn OnDeviceRemoved(&self, _device_id: &PCWSTR) -> windows::core::Result<()> {
        self.notify();
        Ok(())
    }

    fn OnDefaultDeviceChanged(
        &self,
        _flow: EDataFlow,
        _role: ERole,
        _device_id: &PCWSTR,
    ) -> windows::core::Result<()> {
        self.notify();
        Ok(())
    }

    fn OnPropertyValueChanged(
        &self,
        _device_id: &PCWSTR,
        _key: &PROPERTYKEY,
    ) -> windows::core::Result<()> {
        self.notify();
        Ok(())
    }
}

pub struct Manager {
    stop: Arc<AtomicBool>,
    ui: Ui,
    thread: Option<JoinHandle<()>>,
}

impl Manager {
    pub fn start(logger: Arc<Logger>) -> AppResult<Self> {
        let stop = Arc::new(AtomicBool::new(false));
        let snapshot = Arc::new(Mutex::new(Snapshot::default()));
        let (commands, receiver) = mpsc::channel();
        let (report_sender, report_receiver) = mpsc::channel();

        let worker_stop = stop.clone();
        let worker_snapshot = snapshot.clone();
        let notifications = commands.clone();
        let thread = thread::Builder::new()
            .name("real-audio".into())
            .spawn(move || {
                worker(
                    logger,
                    receiver,
                    notifications,
                    worker_stop,
                    worker_snapshot,
                    report_sender,
                )
            })
            .map_err(|error| format!("could not start the audio monitoring thread: {error}"))?;

        match report_receiver.recv_timeout(Duration::from_secs(10)) {
            Ok(Ok(())) => Ok(Self {
                stop,
                ui: Ui {
                    control: Control { commands },
                    snapshot,
                },
                thread: Some(thread),
            }),
            Ok(Err(error)) => {
                let _ = thread.join();
                Err(error)
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                stop.store(true, Ordering::SeqCst);
                let _ = commands.send(Command::Quit);
                drop(thread);
                Err("the audio monitoring thread did not finish initializing in time".into())
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                let _ = thread.join();
                Err("the audio monitoring thread stopped before reporting its status".into())
            }
        }
    }

    pub fn ui(&self) -> Ui {
        self.ui.clone()
    }

    pub fn stop(mut self) {
        self.shutdown();
    }

    fn shutdown(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        self.ui.control.quit();

        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for Manager {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn worker(
    logger: Arc<Logger>,
    commands: mpsc::Receiver<Command>,
    notifications: mpsc::Sender<Command>,
    stop: Arc<AtomicBool>,
    snapshot: Arc<Mutex<Snapshot>>,
    report: mpsc::Sender<Result<(), String>>,
) {
    let _apartment = match ComApartment::initialize() {
        Ok(apartment) => apartment,
        Err(error) => {
            let _ = report.send(Err(error));
            return;
        }
    };

    let enumerator = match audio::create_enumerator() {
        Ok(enumerator) => enumerator,
        Err(error) => {
            let _ = report.send(Err(error));
            return;
        }
    };

    let notification_client: IMMNotificationClient = NotificationClient {
        commands: notifications,
    }
    .into();

    let subscribed =
        unsafe { enumerator.RegisterEndpointNotificationCallback(&notification_client) }.is_ok();
    if !subscribed {
        logger.line(
            "Warning: could not subscribe to Windows audio device notifications; \
             falling back to periodic rescans.",
        );
    }

    let mut state = State::new(logger.clone());
    state.refresh(&enumerator);
    publish(&snapshot, &state);

    let _ = report.send(Ok(()));

    let mut pending: VecDeque<Command> = VecDeque::new();

    while !stop.load(Ordering::SeqCst) {
        let command = match pending.pop_front() {
            Some(command) => Ok(command),
            None => commands.recv_timeout(Duration::from_secs(10)),
        };

        if stop.load(Ordering::SeqCst) {
            break;
        }

        match command {
            Ok(Command::Quit) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
            Ok(Command::Rescan) => {
                thread::sleep(Duration::from_millis(250));

                while let Ok(command) = commands.try_recv() {
                    pending.push_back(command);
                }

                pending.retain(|command| !matches!(command, Command::Rescan));

                state.refresh(&enumerator);
            }
            Ok(Command::Stop(device_id)) => state.pause(&device_id),
            Ok(Command::Start(device_id)) => {
                state.resume(&device_id);
                state.refresh(&enumerator);
            }
            Err(mpsc::RecvTimeoutError::Timeout) => state.refresh(&enumerator),
        }

        publish(&snapshot, &state);
    }

    if subscribed {
        let _ = unsafe { enumerator.UnregisterEndpointNotificationCallback(&notification_client) };
    }
}

fn publish(snapshot: &Mutex<Snapshot>, state: &State) {
    *snapshot.lock().unwrap() = state.snapshot();
}

#[derive(Clone, PartialEq, Eq)]
enum Status {
    Holding {
        requested: u32,
        current: Option<u32>,
    },
    Unsupported {
        lowest: u32,
    },
    OpenFailed(String),
    Paused,
}

impl Status {
    fn describe(&self, properties: &Props) -> String {
        match self {
            Self::Holding { requested, current } => {
                let mut text = format!(
                    "holding {:.2} ms ({} frames)",
                    properties.milliseconds(*requested),
                    requested
                );

                if let Some(current) = current {
                    text.push_str(&format!(
                        "; engine at {:.2} ms ({} frames)",
                        properties.milliseconds(*current),
                        current
                    ));
                }

                text
            }
            Self::Unsupported { lowest } => format!(
                "no lower shared-mode period (lowest {:.2} ms)",
                properties.milliseconds(*lowest)
            ),
            Self::OpenFailed(error) => {
                format!("could not open a low-latency stream: {error}")
            }
            Self::Paused => "paused by user".into(),
        }
    }
}

struct Endpoint {
    name: String,
    role: String,
    properties: Props,
    session: Option<Session>,
    status: Status,
}

struct State {
    logger: Arc<Logger>,
    endpoints: BTreeMap<String, Endpoint>,
    paused: HashSet<String>,
}

impl State {
    fn new(logger: Arc<Logger>) -> Self {
        Self {
            logger,
            endpoints: BTreeMap::new(),
            paused: HashSet::new(),
        }
    }

    fn snapshot(&self) -> Snapshot {
        let mut held = 0;
        let mut open_failed = 0;

        let endpoints = self
            .endpoints
            .iter()
            .map(|(id, endpoint)| {
                if endpoint.session.is_some() {
                    held += 1;
                }

                if matches!(&endpoint.status, Status::OpenFailed(_)) {
                    open_failed += 1;
                }

                EndpointView {
                    id: id.clone(),
                    name: endpoint.name.clone(),
                    role: endpoint.role.clone(),
                    properties: endpoint.properties.clone(),
                    applied: endpoint.session.is_some(),
                    paused: matches!(&endpoint.status, Status::Paused),
                    status: endpoint.status.describe(&endpoint.properties),
                }
            })
            .collect();

        Snapshot {
            endpoints,
            held,
            open_failed,
        }
    }

    fn pause(&mut self, device_id: &str) {
        self.paused.insert(device_id.to_string());

        if let Some(endpoint) = self.endpoints.get_mut(device_id) {
            endpoint.session = None;
            endpoint.status = Status::Paused;
        }
    }

    fn resume(&mut self, device_id: &str) {
        self.paused.remove(device_id);
        self.endpoints.remove(device_id);
    }

    fn release(&mut self, device_id: &str, message: &str) {
        self.paused.remove(device_id);

        if let Some(endpoint) = self.endpoints.remove(device_id) {
            self.logger.line(format!("{message} {}.", endpoint.name));
        }
    }

    fn refresh(&mut self, enumerator: &IMMDeviceEnumerator) {
        let targets = audio::collect_targets(enumerator);

        self.release_stale(&targets);

        for target in targets {
            self.refresh_target(enumerator, target);
        }
    }

    fn release_stale(&mut self, targets: &[Target]) {
        let desired: HashSet<&str> = targets.iter().map(|target| target.id.as_str()).collect();
        let stale: Vec<String> = self
            .endpoints
            .keys()
            .filter(|device_id| !desired.contains(device_id.as_str()))
            .cloned()
            .collect();

        for device_id in stale {
            self.release(&device_id, "Released low-latency stream on");
        }
    }

    fn refresh_target(&mut self, enumerator: &IMMDeviceEnumerator, target: Target) {
        let device = match get_device(enumerator, &target.id) {
            Ok(device) => device,
            Err(_) => {
                self.release(&target.id, "Audio endpoint disappeared:");
                return;
            }
        };

        let activation = match audio::query(&device) {
            Ok(activation) => activation,
            Err(error) => {
                self.logger.line(format!(
                    "Could not query audio endpoint {}: {}",
                    target.id, error
                ));
                return;
            }
        };

        if !self.paused.contains(&target.id) {
            if let Some(existing) = self.endpoints.get(&target.id) {
                if !needs_reopen(existing, activation.properties()) {
                    return;
                }
            }
        }

        let endpoint = self.create_endpoint(&target, activation, audio::friendly_name(&device));

        let was_known = self.endpoints.contains_key(&target.id);
        let status_changed = self
            .endpoints
            .get(&target.id)
            .map(|existing| existing.status != endpoint.status)
            .unwrap_or(true);
        let line = describe_endpoint(&endpoint, was_known);

        self.endpoints.insert(target.id, endpoint);

        if status_changed {
            self.logger.line(line);
        }
    }

    fn create_endpoint(
        &self,
        target: &Target,
        activation: audio::Activation,
        name: String,
    ) -> Endpoint {
        let properties = activation.properties().clone();
        let enabled = !self.paused.contains(&target.id);
        let mut session = None;
        let mut status = if enabled {
            Status::Unsupported {
                lowest: properties.default_period,
            }
        } else {
            Status::Paused
        };

        if enabled && can_reduce(&properties) {
            match Session::start(activation) {
                Ok(started) => {
                    status = Status::Holding {
                        requested: started.requested_period(),
                        current: started.current_period().ok(),
                    };
                    session = Some(started);
                }
                Err(error) => status = Status::OpenFailed(error),
            }
        }

        Endpoint {
            name,
            role: format!(
                "{}, {}",
                target.role,
                if target.capture { "capture" } else { "render" }
            ),
            properties,
            session,
            status,
        }
    }
}

fn get_device(enumerator: &IMMDeviceEnumerator, device_id: &str) -> AppResult<IMMDevice> {
    let wide = audio::wide_null(device_id);
    unsafe { enumerator.GetDevice(audio::pcwstr(&wide)) }
        .map_err(|error| audio::error_text("IMMDeviceEnumerator::GetDevice", error))
}

fn can_reduce(properties: &Props) -> bool {
    properties.low_latency_period().is_some()
}

fn needs_reopen(endpoint: &Endpoint, properties: &Props) -> bool {
    if endpoint.properties != *properties {
        return true;
    }

    match &endpoint.session {
        Some(session) => !matches!(
            session.current_period(),
            Ok(current) if current <= session.requested_period()
        ),
        None => can_reduce(properties),
    }
}

fn describe_endpoint(endpoint: &Endpoint, was_known: bool) -> String {
    let properties = &endpoint.properties;
    let note = if was_known { " (re-opened)" } else { "" };
    let color = if endpoint.session.is_some() { 32 } else { 31 };

    let details = format!(
        "{}  [{}]{}\n  {} Hz, {} ch, {}-bit\n  \
         periods: default {:.2} ms ({} frames), min {:.2} ms ({} frames), max {:.2} ms ({} frames)\n  \
         status: {}",
        endpoint.name,
        endpoint.role,
        note,
        properties.sample_rate,
        properties.channels,
        properties.bits_per_sample,
        properties.milliseconds(properties.default_period),
        properties.default_period,
        properties.milliseconds(properties.minimum_period),
        properties.minimum_period,
        properties.milliseconds(properties.maximum_period),
        properties.maximum_period,
        endpoint.status.describe(properties)
    );

    format!("\n\x1b[{color}m●\x1b[0m {details}")
}
