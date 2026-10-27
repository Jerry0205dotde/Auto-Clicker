//! End-to-end worker protocol tests on a private bus, without real pointer input.
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Duration,
};

use ashpd::zbus::{
    self,
    zvariant::{OwnedObjectPath, OwnedValue, Value},
};
use klickmeister::{
    model::{ClickSettings, ClickType, MonitorGeometry, MouseButton},
    state::RunState,
    worker::{Command, HotkeyPhase, StartRequest, WorkerEvent, WorkerHandle},
};
use tokio::{
    sync::{Notify, mpsc},
    time::{sleep, timeout},
};

type Options = HashMap<String, OwnedValue>;
type TestResult = Result<(), Box<dyn std::error::Error>>;
const PATH: &str = "/org/freedesktop/portal/desktop";
static BUS_TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

#[derive(Clone)]
struct FakeMonitor {
    node_id: u32,
    position: Option<(i32, i32)>,
    size: Option<(i32, i32)>,
}

#[derive(Default)]
struct Observed {
    registered_apps: HashMap<String, String>,
    require_registration: bool,
    reject_registration: bool,
    hotkey_session: String,
    hotkey_created: usize,
    /// `preferred_trigger` of every BindShortcuts call.
    preferred_triggers: Vec<Option<String>>,
    reject_hotkey: bool,
    empty_hotkey_trigger: bool,
    hotkey_version: Option<u32>,
    remote_session: Option<OwnedObjectPath>,
    revoked_session: Option<OwnedObjectPath>,
    buttons: Vec<(i32, u32)>,
    button_failures: [usize; 2],
    closed: usize,
    stall_close: bool,
    close_releases: Vec<Arc<Notify>>,
    remote_owner: String,
    hotkey_owner: String,
    fixed_stream: bool,
    multiple_sources: bool,
    multi_monitor_layout: bool,
    reverse_monitor_streams: bool,
    monitor_streams: Option<Vec<FakeMonitor>>,
    motions: Vec<(u32, f64, f64)>,
    select_devices_response: u32,
    select_sources_response: u32,
    select_sources_calls: usize,
    remote_start_calls: usize,
    delay_hotkey: bool,
    /// Never send the Response to a hotkey CreateSession request.
    hold_hotkey_session: bool,
    close_after_hotkey_response: bool,
    hotkey_seen: Arc<Notify>,
    hotkey_release: Arc<Notify>,
    hotkey_responded: Arc<Notify>,
    configure_calls: usize,
    configure_seen: Arc<Notify>,
    configure_release: Arc<Notify>,
    delay_start: bool,
    start_seen: Arc<Notify>,
    start_release: Arc<Notify>,
    delay_button: bool,
    button_seen: Arc<Notify>,
    button_release: Arc<Notify>,
    version_reads: usize,
    hold_version: bool,
    version_seen: Arc<Notify>,
    version_release: Arc<Notify>,
}
type Shared = Arc<Mutex<Observed>>;

struct FakeRegistry(Shared);
#[zbus::interface(name = "org.freedesktop.host.portal.Registry", crate = "ashpd::zbus")]
impl FakeRegistry {
    #[zbus(property, name = "version")]
    fn version(&self) -> u32 {
        1
    }

    fn register(
        &self,
        app_id: String,
        _options: Options,
        #[zbus(header)] header: zbus::message::Header<'_>,
    ) -> zbus::fdo::Result<()> {
        let sender = header.sender().ok_or_else(|| failed("missing sender"))?;
        let mut state = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if state.reject_registration {
            return Err(zbus::fdo::Error::AccessDenied("registration denied".into()));
        }
        if app_id != "io.github.jerry0205.klickmeister"
            || state.registered_apps.contains_key(sender.as_str())
        {
            return Err(failed("invalid or repeated registration"));
        }
        state.registered_apps.insert(sender.to_string(), app_id);
        Ok(())
    }
}

fn failed(error: impl std::fmt::Display) -> zbus::fdo::Error {
    zbus::fdo::Error::Failed(error.to_string())
}

fn handle(
    header: &zbus::message::Header<'_>,
    options: &Options,
    key: &str,
    kind: &str,
) -> zbus::fdo::Result<OwnedObjectPath> {
    let sender = header
        .sender()
        .ok_or_else(|| failed("missing sender"))?
        .as_str()
        .trim_start_matches(':')
        .replace('.', "_");
    let token = options
        .get(key)
        .and_then(|v| <&str>::try_from(v).ok())
        .ok_or_else(|| failed("missing token"))?;
    OwnedObjectPath::try_from(format!("{PATH}/{kind}/{sender}/{token}")).map_err(failed)
}

async fn respond(
    connection: &zbus::Connection,
    header: &zbus::message::Header<'_>,
    options: &Options,
    results: Options,
) -> zbus::fdo::Result<OwnedObjectPath> {
    respond_with_status(connection, header, options, 0, results).await
}

async fn respond_with_status(
    connection: &zbus::Connection,
    header: &zbus::message::Header<'_>,
    options: &Options,
    status: u32,
    results: Options,
) -> zbus::fdo::Result<OwnedObjectPath> {
    let path = handle(header, options, "handle_token", "request")?;
    connection
        .emit_signal(
            header.sender().cloned(),
            &path,
            "org.freedesktop.portal.Request",
            "Response",
            &(status, results),
        )
        .await
        .map_err(failed)?;
    Ok(path)
}

struct FakeSession(Shared);
#[zbus::interface(name = "org.freedesktop.portal.Session", crate = "ashpd::zbus")]
impl FakeSession {
    /// Record each close and optionally hold its reply until the test releases it.
    async fn close(&self) {
        let release = {
            let mut state = self.0.lock().unwrap_or_else(|e| e.into_inner());
            state.closed += 1;
            if state.stall_close {
                let release = Arc::new(Notify::new());
                state.close_releases.push(release.clone());
                Some(release)
            } else {
                None
            }
        };
        if let Some(release) = release {
            release.notified().await;
        }
    }
    #[zbus(property)]
    fn version(&self) -> u32 {
        1
    }
}

async fn create(
    connection: &zbus::Connection,
    header: &zbus::message::Header<'_>,
    options: &Options,
    observed: &Shared,
    hotkey: bool,
) -> zbus::fdo::Result<OwnedObjectPath> {
    {
        let state = observed.lock().unwrap_or_else(|e| e.into_inner());
        let sender = header.sender().ok_or_else(|| failed("missing sender"))?;
        if state.require_registration && !state.registered_apps.contains_key(sender.as_str()) {
            return Err(failed("An app id is required"));
        }
    }
    let session = handle(header, options, "session_handle_token", "session")?;
    connection
        .object_server()
        .at(session.clone(), FakeSession(observed.clone()))
        .await
        .map_err(failed)?;
    if hotkey {
        let mut state = observed.lock().unwrap_or_else(|e| e.into_inner());
        state.hotkey_session = session.to_string();
        state.hotkey_created += 1;
    } else {
        observed
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remote_session = Some(session.clone());
    }
    {
        let mut state = observed.lock().unwrap_or_else(|e| e.into_inner());
        let owner = header
            .sender()
            .ok_or_else(|| failed("missing owner"))?
            .to_string();
        if hotkey {
            state.hotkey_owner = owner;
        } else {
            state.remote_owner = owner;
        }
    }
    let results = HashMap::from([(
        "session_handle".into(),
        Value::from(session).try_into().map_err(failed)?,
    )]);
    respond(connection, header, options, results).await
}

struct FakeHotkeys(Shared);
#[zbus::interface(name = "org.freedesktop.portal.GlobalShortcuts", crate = "ashpd::zbus")]
impl FakeHotkeys {
    /// Count reads, so a test can tell when the worker has asked for it,
    /// and optionally hold the reply until the test releases it.
    #[zbus(property, name = "version")]
    async fn version(&self) -> u32 {
        let hold = {
            let mut state = self.0.lock().unwrap_or_else(|e| e.into_inner());
            state.version_reads += 1;
            state
                .hold_version
                .then(|| (state.version_seen.clone(), state.version_release.clone()))
        };
        if let Some((seen, release)) = hold {
            seen.notify_one();
            release.notified().await;
        }
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .hotkey_version
            .unwrap_or(2)
    }

    async fn create_session(
        &self,
        options: Options,
        #[zbus(connection)] connection: &zbus::Connection,
        #[zbus(header)] header: zbus::message::Header<'_>,
    ) -> zbus::fdo::Result<OwnedObjectPath> {
        let hold = {
            let state = self.0.lock().unwrap_or_else(|e| e.into_inner());
            state.hold_hotkey_session.then(|| state.hotkey_seen.clone())
        };
        if let Some(seen) = hold {
            seen.notify_one();
            return handle(&header, &options, "handle_token", "request");
        }
        create(connection, &header, &options, &self.0, true).await
    }

    async fn bind_shortcuts(
        &self,
        _session: OwnedObjectPath,
        shortcuts: Vec<(String, Options)>,
        _parent: String,
        options: Options,
        #[zbus(connection)] connection: &zbus::Connection,
        #[zbus(header)] header: zbus::message::Header<'_>,
    ) -> zbus::fdo::Result<OwnedObjectPath> {
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .preferred_triggers
            .extend(shortcuts.iter().map(|(_, shortcut)| {
                shortcut
                    .get("preferred_trigger")
                    .and_then(|value| <&str>::try_from(value).ok())
                    .map(str::to_owned)
            }));
        let (delay, seen, release) = {
            let state = self.0.lock().unwrap_or_else(|e| e.into_inner());
            (
                state.delay_hotkey,
                state.hotkey_seen.clone(),
                state.hotkey_release.clone(),
            )
        };
        if delay {
            seen.notify_one();
            release.notified().await;
        }
        if self
            .0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .reject_hotkey
        {
            let path = handle(&header, &options, "handle_token", "request")?;
            connection
                .emit_signal(
                    header.sender().cloned(),
                    &path,
                    "org.freedesktop.portal.Request",
                    "Response",
                    &(1_u32, Options::new()),
                )
                .await
                .map_err(failed)?;
            return Ok(path);
        }
        let trigger = if self
            .0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .empty_hotkey_trigger
        {
            ""
        } else {
            "Pause"
        };
        let shortcuts = vec![(
            "toggle-clicking",
            HashMap::from([
                ("description", Value::from("Start/Stop")),
                ("trigger_description", Value::from(trigger)),
            ]),
        )];
        let results = HashMap::from([(
            "shortcuts".into(),
            Value::new(shortcuts).try_into().map_err(failed)?,
        )]);
        let response = respond(connection, &header, &options, results).await;
        let (close_after_response, responded) = {
            let state = self.0.lock().unwrap_or_else(|e| e.into_inner());
            (
                state.close_after_hotkey_response,
                state.hotkey_responded.clone(),
            )
        };
        if close_after_response {
            connection
                .emit_signal(
                    header.sender().cloned(),
                    &_session,
                    "org.freedesktop.portal.Session",
                    "Closed",
                    &(Options::new(),),
                )
                .await
                .map_err(failed)?;
        }
        responded.notify_one();
        response
    }

    async fn configure_shortcuts(
        &self,
        _session: OwnedObjectPath,
        _parent: String,
        _options: Options,
    ) {
        let (seen, release) = {
            let mut state = self.0.lock().unwrap_or_else(|e| e.into_inner());
            state.configure_calls += 1;
            (
                state.configure_seen.clone(),
                state.configure_release.clone(),
            )
        };
        seen.notify_one();
        release.notified().await;
    }
}

struct FakeRemote(Shared);
#[zbus::interface(name = "org.freedesktop.portal.RemoteDesktop", crate = "ashpd::zbus")]
impl FakeRemote {
    #[zbus(property)]
    fn version(&self) -> u32 {
        2
    }
    #[zbus(property)]
    fn available_device_types(&self) -> u32 {
        2
    }

    async fn create_session(
        &self,
        options: Options,
        #[zbus(connection)] connection: &zbus::Connection,
        #[zbus(header)] header: zbus::message::Header<'_>,
    ) -> zbus::fdo::Result<OwnedObjectPath> {
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .fixed_stream = false;
        create(connection, &header, &options, &self.0, false).await
    }
    async fn select_devices(
        &self,
        _session: OwnedObjectPath,
        options: Options,
        #[zbus(connection)] connection: &zbus::Connection,
        #[zbus(header)] header: zbus::message::Header<'_>,
    ) -> zbus::fdo::Result<OwnedObjectPath> {
        assert_eq!(
            options.get("types").and_then(|v| u32::try_from(v).ok()),
            Some(2)
        );
        let status = self
            .0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .select_devices_response;
        respond_with_status(connection, &header, &options, status, Options::new()).await
    }
    async fn start(
        &self,
        _session: OwnedObjectPath,
        _parent: String,
        options: Options,
        #[zbus(connection)] connection: &zbus::Connection,
        #[zbus(header)] header: zbus::message::Header<'_>,
    ) -> zbus::fdo::Result<OwnedObjectPath> {
        let (fixed, multiple, multi_layout, reverse, override_streams, delay, seen, release) = {
            let mut state = self.0.lock().unwrap_or_else(|e| e.into_inner());
            state.remote_start_calls += 1;
            (
                state.fixed_stream,
                state.multiple_sources,
                state.multi_monitor_layout,
                state.reverse_monitor_streams,
                state.monitor_streams.clone(),
                state.delay_start,
                state.start_seen.clone(),
                state.start_release.clone(),
            )
        };
        let mut results = HashMap::from([("devices".into(), OwnedValue::from(2_u32))]);
        if fixed {
            let mut stream = vec![(
                42_u32,
                HashMap::from([
                    ("source_type", Value::new(1_u32)),
                    ("position", Value::new((-1920_i32, 0_i32))),
                    ("size", Value::new((1920_i32, 1080_i32))),
                ]),
            )];
            if multi_layout {
                if multiple {
                    stream.insert(
                        0,
                        (
                            99_u32,
                            HashMap::from([
                                ("source_type", Value::new(1_u32)),
                                ("position", Value::new((0_i32, 0_i32))),
                                ("size", Value::new((1920_i32, 1080_i32))),
                            ]),
                        ),
                    );
                    if reverse {
                        stream.reverse();
                    }
                } else {
                    // KDE combines the workspace when multiple=false. Its
                    // metadata has a size but no per-monitor position.
                    stream = vec![(
                        99_u32,
                        HashMap::from([
                            ("size", Value::new((3840_i32, 1080_i32))),
                            ("source_type", Value::new(1_u32)),
                        ]),
                    )];
                }
            }
            if let Some(monitors) = override_streams {
                stream = monitors
                    .into_iter()
                    .map(|monitor| {
                        let mut properties = HashMap::new();
                        if let Some(position) = monitor.position {
                            properties.insert("position", Value::new(position));
                        }
                        if let Some(size) = monitor.size {
                            properties.insert("size", Value::new(size));
                        }
                        (monitor.node_id, properties)
                    })
                    .collect();
            }
            results.insert(
                "streams".into(),
                Value::new(stream).try_into().map_err(failed)?,
            );
        }
        if delay {
            // Return the request handle now, but keep its Response pending.
            let path = handle(&header, &options, "handle_token", "request")?;
            let destination = header.sender().map(ToString::to_string);
            let connection = connection.clone();
            let response_path = path.clone();
            tokio::spawn(async move {
                seen.notify_one();
                release.notified().await;
                let _ = connection
                    .emit_signal(
                        destination.as_deref(),
                        &response_path,
                        "org.freedesktop.portal.Request",
                        "Response",
                        &(0_u32, results),
                    )
                    .await;
            });
            Ok(path)
        } else {
            respond(connection, &header, &options, results).await
        }
    }
    async fn notify_pointer_button(
        &self,
        session: OwnedObjectPath,
        _options: Options,
        button: i32,
        state: u32,
    ) -> zbus::fdo::Result<()> {
        let delay = {
            let mut observed = self.0.lock().unwrap_or_else(|e| e.into_inner());
            if observed.revoked_session.as_ref() == Some(&session) {
                return Err(failed("session revoked"));
            }
            observed.buttons.push((button, state));
            if observed.button_failures[state as usize] > 0 {
                observed.button_failures[state as usize] -= 1;
                return Err(failed("simulated button failure"));
            }
            (state == 1 && observed.delay_button).then(|| {
                (
                    observed.button_seen.clone(),
                    observed.button_release.clone(),
                )
            })
        };
        // Hold the press reply, so the worker stays inside this click.
        if let Some((seen, release)) = delay {
            seen.notify_one();
            release.notified().await;
        }
        Ok(())
    }

    async fn notify_pointer_motion_absolute(
        &self,
        _session: OwnedObjectPath,
        _options: Options,
        stream: u32,
        x: f64,
        y: f64,
    ) {
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .motions
            .push((stream, x, y));
    }
}

struct FakeScreencast(Shared);
#[zbus::interface(name = "org.freedesktop.portal.ScreenCast", crate = "ashpd::zbus")]
impl FakeScreencast {
    #[zbus(property)]
    fn version(&self) -> u32 {
        5
    }

    async fn select_sources(
        &self,
        _session: OwnedObjectPath,
        options: Options,
        #[zbus(connection)] connection: &zbus::Connection,
        #[zbus(header)] header: zbus::message::Header<'_>,
    ) -> zbus::fdo::Result<OwnedObjectPath> {
        assert_eq!(
            options.get("types").and_then(|v| u32::try_from(v).ok()),
            Some(1)
        );
        let multiple = options
            .get("multiple")
            .and_then(|v| bool::try_from(v).ok())
            .ok_or_else(|| failed("missing multiple option"))?;
        let status = {
            let mut state = self.0.lock().unwrap_or_else(|e| e.into_inner());
            state.select_sources_calls += 1;
            state.fixed_stream = state.select_sources_response == 0;
            state.multiple_sources = multiple;
            state.select_sources_response
        };
        respond_with_status(connection, &header, &options, status, Options::new()).await
    }
}

async fn event(
    events: &mut mpsc::UnboundedReceiver<WorkerEvent>,
    matches: impl Fn(&WorkerEvent) -> bool,
) -> TestResult {
    timeout(Duration::from_secs(3), async {
        while let Some(event) = events.recv().await {
            if matches(&event) {
                return Ok(());
            }
            if let WorkerEvent::Error(error) = event {
                return Err(error.into());
            }
        }
        Err("worker event stream ended".into())
    })
    .await?
}

/// Hold Qt's reply deliberately, without blocking the worker or its private bus.
async fn start_request(
    events: &mut mpsc::UnboundedReceiver<WorkerEvent>,
) -> Result<StartRequest, Box<dyn std::error::Error>> {
    timeout(Duration::from_secs(3), async {
        while let Some(event) = events.recv().await {
            match event {
                WorkerEvent::StartRequested(request) => return Ok(request),
                WorkerEvent::Error(error) => return Err(error.into()),
                _ => {}
            }
        }
        Err("worker event stream ended".into())
    })
    .await?
}

/// Collect events up to the first match so skipped events can be checked too.
async fn events_until(
    events: &mut mpsc::UnboundedReceiver<WorkerEvent>,
    matches: impl Fn(&WorkerEvent) -> bool,
) -> Result<Vec<WorkerEvent>, Box<dyn std::error::Error>> {
    timeout(Duration::from_secs(3), async {
        let mut seen = Vec::new();
        while let Some(event) = events.recv().await {
            let found = matches(&event);
            seen.push(event);
            if found {
                return Ok(seen);
            }
        }
        Err("worker event stream ended".into())
    })
    .await?
}

fn run_states(events: &[WorkerEvent]) -> Vec<RunState> {
    events
        .iter()
        .filter_map(|e| match e {
            WorkerEvent::State(state) => Some(*state),
            _ => None,
        })
        .collect()
}

/// Hotkey problems without an active run must only change the hotkey phase.
fn assert_hotkey_only(events: &[WorkerEvent]) {
    assert!(
        events
            .iter()
            .any(|e| matches!(e, WorkerEvent::HotkeyPhase(HotkeyPhase::Unavailable)))
    );
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, WorkerEvent::State(state) if *state != RunState::Ready)),
        "an idle hotkey failure must not change the run state: {events:?}"
    );
}

async fn shortcuts_changed(service: &zbus::Connection, session: &str, trigger: &str) -> TestResult {
    let session = OwnedObjectPath::try_from(session)?;
    let shortcuts: Vec<(String, Options)> = vec![(
        "toggle-clicking".to_owned(),
        HashMap::from([
            (
                "description".to_owned(),
                Value::from("Start/Stop").try_into()?,
            ),
            (
                "trigger_description".to_owned(),
                Value::from(trigger).try_into()?,
            ),
        ]),
    )];
    service
        .emit_signal(
            None::<&str>,
            PATH,
            "org.freedesktop.portal.GlobalShortcuts",
            "ShortcutsChanged",
            &(session, shortcuts),
        )
        .await?;
    Ok(())
}

/// Revocation must be observed while BindShortcuts is pending and across its
/// successful response, before a registration can be handed to the worker.
async fn revoked_hotkey_registration(after_response: bool, stall_close: bool) -> TestResult {
    let _serial = BUS_TEST_LOCK.lock().await;
    let observed = Shared::default();
    let (seen, release, responded) = {
        let mut state = observed.lock().unwrap_or_else(|e| e.into_inner());
        state.close_after_hotkey_response = after_response;
        state.delay_hotkey = !after_response;
        state.stall_close = stall_close;
        (
            state.hotkey_seen.clone(),
            state.hotkey_release.clone(),
            state.hotkey_responded.clone(),
        )
    };
    let service = fake_portal(&observed).await?;
    let (tx, mut events) = mpsc::unbounded_channel();
    let worker = WorkerHandle::spawn(move |event| {
        let _ = tx.send(event);
    });
    let result: TestResult = async {
        if !after_response {
            timeout(Duration::from_secs(3), seen.notified()).await?;
            service.emit_signal(
                None::<&str>, hotkey_session(&observed)?,
                "org.freedesktop.portal.Session", "Closed", &(Options::new(),),
            ).await?;
        }
        let phases = events_until(&mut events, |e| matches!(e,
            WorkerEvent::HotkeyPhase(HotkeyPhase::Ready | HotkeyPhase::Unavailable)
        )).await?;
        let incorrectly_ready = phases.iter().any(|e| matches!(e,
            WorkerEvent::HotkeyPhase(HotkeyPhase::Ready)
        ));
        if incorrectly_ready {
            worker.send(Command::Start(continuous()))?;
            event(&mut events, |e| matches!(e, WorkerEvent::State(RunState::Clicking))).await?;
            timeout(Duration::from_secs(1), async {
                while presses(&observed) == 0 { sleep(Duration::from_millis(5)).await; }
            }).await?;
        }
        ensure(!incorrectly_ready && presses(&observed) == 0,
            &format!("revoked hotkey was reported Ready and allowed {} pointer presses", presses(&observed)))?;
        assert_hotkey_only(&phases);
        let failed = events_until(&mut events, |e| matches!(e,
            WorkerEvent::Error(message) if message.contains("Freigabe für das Tastenkürzel")
        )).await?;
        ensure(run_states(&failed).is_empty(), "idle revocation changed the run state")?;
        let (old_session, owner, closed) = {
            let state = observed.lock().unwrap_or_else(|e| e.into_inner());
            (state.hotkey_session.clone(), state.hotkey_owner.clone(), state.closed)
        };
        ensure(closed == 1, "revocation must close the failed registration session")?;
        let bus = zbus::fdo::DBusProxy::new(&service).await?;
        ensure(!bus.name_has_owner(owner.as_str().try_into()?).await?,
            "revocation must disconnect the registration owner, even if Close stalls")?;
        worker.send(Command::Start(continuous()))?;
        event(&mut events, |e| matches!(e, WorkerEvent::Error(message) if message.contains("Tastenkürzel für Start und Stopp"))).await?;
        ensure(observed.lock().unwrap_or_else(|e| e.into_inner()).remote_session.is_none(),
            "a revoked registration must not request pointer permission")?;

        // A successful response arriving after cancellation belongs to the old
        // connection. It must neither publish Ready nor poison a fresh retry.
        if !after_response {
            release.notify_one();
        }
        timeout(Duration::from_secs(3), responded.notified()).await?;
        {
            let mut state = observed.lock().unwrap_or_else(|e| e.into_inner());
            state.close_after_hotkey_response = false;
            state.delay_hotkey = false;
            state.stall_close = false;
            for close_release in std::mem::take(&mut state.close_releases) {
                close_release.notify_one();
            }
        }
        ensure(events.try_recv().is_err(), "a late bind response published a worker event")?;
        worker.send(Command::ConfigureHotkey)?;
        event(&mut events, |e| matches!(e, WorkerEvent::HotkeyPhase(HotkeyPhase::Registering))).await?;
        event(&mut events, |e| matches!(e, WorkerEvent::HotkeyPhase(HotkeyPhase::Ready))).await?;
        {
            let state = observed.lock().unwrap_or_else(|e| e.into_inner());
            ensure(state.hotkey_created == 2 && state.hotkey_session != old_session,
                "retry must create a fresh hotkey session")?;
        }
        worker.send(Command::Start(ClickSettings { repeat: Some(1), ..continuous() }))?;
        event(&mut events, |e| matches!(e, WorkerEvent::State(RunState::Stopped))).await?;
        ensure(presses(&observed) == 1, "healthy retry must allow exactly its requested click")?;

        // The watcher that protected binding must remain active at runtime.
        service.emit_signal(
            None::<&str>, hotkey_session(&observed)?,
            "org.freedesktop.portal.Session", "Closed", &(Options::new(),),
        ).await?;
        assert_hotkey_only(&events_until(&mut events, |e| matches!(e,
            WorkerEvent::Error(message) if message.contains("Freigabe für das Tastenkürzel")
        )).await?);
        Ok(())
    }.await;
    release.notify_one();
    for close_release in std::mem::take(
        &mut observed
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .close_releases,
    ) {
        close_release.notify_one();
    }
    tokio::task::spawn_blocking(move || worker.shutdown()).await?;
    service.close().await?;
    result
}

#[tokio::test]
#[ignore = "requires a private D-Bus session via dbus-run-session"]
async fn hotkey_closed_right_after_bind_response_blocks_start_and_allows_retry() -> TestResult {
    revoked_hotkey_registration(true, false).await
}

#[tokio::test]
#[ignore = "requires a private D-Bus session via dbus-run-session"]
async fn hotkey_closed_during_bind_cleans_up_and_ignores_late_response() -> TestResult {
    for stall_close in [false, true] {
        revoked_hotkey_registration(false, stall_close).await?;
    }
    Ok(())
}

#[tokio::test]
#[ignore = "requires a private D-Bus session via dbus-run-session"]
async fn hotkey_can_be_registered_again_after_rejection_and_session_loss() -> TestResult {
    let _serial = BUS_TEST_LOCK.lock().await;
    let observed = Shared::default();
    observed
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .reject_hotkey = true;
    let service = zbus::connection::Builder::session()?
        .name("org.freedesktop.portal.Desktop")?
        .serve_at(PATH, FakeHotkeys(observed.clone()))?
        .serve_at(PATH, FakeRemote(observed.clone()))?
        .serve_at(PATH, FakeScreencast(observed.clone()))?
        .build()
        .await?;
    let settings = ClickSettings {
        interval_ms: 100,
        button: MouseButton::Left,
        click_type: ClickType::Single,
        repeat: Some(1),
        position: None,
        monitor: None,
    };
    let (tx, mut events) = mpsc::unbounded_channel();
    let worker = WorkerHandle::spawn(move |event| {
        let _ = tx.send(event);
    });
    let result: TestResult = async {
        assert_hotkey_only(&events_until(&mut events, |e| matches!(e, WorkerEvent::Error(message) if message.contains("Tastenkürzel wurde nicht freigegeben"))).await?);
        worker.send(Command::Start(settings.clone()))?;
        event(&mut events, |e| matches!(e, WorkerEvent::Error(message) if message.contains("Tastenkürzel für Start und Stopp"))).await?;

        let (seen, release) = {
            let mut state = observed.lock().unwrap_or_else(|e| e.into_inner());
            state.reject_hotkey = false;
            state.delay_hotkey = true;
            (state.hotkey_seen.clone(), state.hotkey_release.clone())
        };
        worker.send(Command::ConfigureHotkey)?;
        event(&mut events, |e| matches!(e, WorkerEvent::HotkeyPhase(HotkeyPhase::Registering))).await?;
        timeout(Duration::from_secs(3), seen.notified()).await?;
        worker.send(Command::ConfigureHotkey)?;
        worker.send(Command::Start(settings.clone()))?;
        event(&mut events, |e| matches!(e, WorkerEvent::Error(message) if message.contains("Tastenkürzel für Start und Stopp"))).await?;
        assert_eq!(observed.lock().unwrap_or_else(|e| e.into_inner()).hotkey_created, 2);
        release.notify_one();
        event(&mut events, |e| matches!(e, WorkerEvent::HotkeyPhase(HotkeyPhase::Ready))).await?;
        let old_session = observed.lock().unwrap_or_else(|e| e.into_inner()).hotkey_session.clone();

        service
            .emit_signal(
                None::<&str>,
                old_session.as_str(),
                "org.freedesktop.portal.Session",
                "Closed",
                &(Options::new(),),
            )
            .await?;
        assert_hotkey_only(&events_until(&mut events, |e| matches!(e, WorkerEvent::Error(message) if message.contains("Freigabe für das Tastenkürzel"))).await?);
        worker.send(Command::Start(settings.clone()))?;
        event(&mut events, |e| matches!(e, WorkerEvent::Error(message) if message.contains("Tastenkürzel für Start und Stopp"))).await?;

        {
            let mut state = observed.lock().unwrap_or_else(|e| e.into_inner());
            state.delay_hotkey = false;
            state.empty_hotkey_trigger = true;
        }
        worker.send(Command::ConfigureHotkey)?;
        event(&mut events, |e| matches!(e, WorkerEvent::HotkeyPhase(HotkeyPhase::Unavailable))).await?;
        event(&mut events, |e| matches!(e, WorkerEvent::Error(message) if message.contains("Kein Tastenkürzel für Start und Stopp"))).await?;
        {
            let state = observed.lock().unwrap_or_else(|e| e.into_inner());
            assert_eq!(state.hotkey_created, 3);
            assert_ne!(state.hotkey_session, old_session);
        }
        let session = observed.lock().unwrap_or_else(|e| e.into_inner()).hotkey_session.clone();
        let (seen, release) = {
            let mut state = observed.lock().unwrap_or_else(|e| e.into_inner());
            state.empty_hotkey_trigger = false;
            (state.configure_seen.clone(), state.configure_release.clone())
        };
        worker.send(Command::ConfigureHotkey)?;
        event(&mut events, |e| matches!(e, WorkerEvent::HotkeyPhase(HotkeyPhase::Configuring(false)))).await?;
        timeout(Duration::from_secs(3), seen.notified()).await?;
        shortcuts_changed(&service, &session, "Pause").await?;
        event(&mut events, |e| matches!(e, WorkerEvent::HotkeyPhase(HotkeyPhase::Configuring(true)))).await?;
        release.notify_one();
        event(&mut events, |e| matches!(e, WorkerEvent::HotkeyPhase(HotkeyPhase::Ready))).await?;
        event(&mut events, |e| matches!(e, WorkerEvent::Status(status) if status == "Bereit")).await?;
        let (seen, release) = {
            let state = observed.lock().unwrap_or_else(|e| e.into_inner());
            (state.configure_seen.clone(), state.configure_release.clone())
        };
        worker.send(Command::ConfigureHotkey)?;
        event(&mut events, |e| matches!(e, WorkerEvent::HotkeyPhase(HotkeyPhase::Configuring(true)))).await?;
        timeout(Duration::from_secs(3), seen.notified()).await?;
        worker.send(Command::ConfigureHotkey)?;
        worker.send(Command::Start(settings.clone()))?;
        event(&mut events, |e| matches!(e, WorkerEvent::Error(message) if message.contains("Tastenkürzel für Start und Stopp"))).await?;
        release.notify_one();
        event(&mut events, |e| matches!(e, WorkerEvent::HotkeyPhase(HotkeyPhase::Ready))).await?;
        assert_eq!(observed.lock().unwrap_or_else(|e| e.into_inner()).configure_calls, 2);
        worker.send(Command::Start(settings.clone()))?;
        event(&mut events, |e| matches!(e, WorkerEvent::State(RunState::Starting))).await?;
        event(&mut events, |e| matches!(e, WorkerEvent::State(RunState::Clicking))).await?;
        event(&mut events, |e| matches!(e, WorkerEvent::State(RunState::Stopped))).await?;
        let (session, created_before, calls_before) = {
            let state = observed.lock().unwrap_or_else(|e| e.into_inner());
            (state.hotkey_session.clone(), state.hotkey_created, state.configure_calls)
        };
        shortcuts_changed(&service, &session, "").await?;
        assert_hotkey_only(&events_until(&mut events, |e| matches!(e, WorkerEvent::Error(message) if message.contains("entfernt"))).await?);
        worker.send(Command::Start(settings))?;
        event(&mut events, |e| matches!(e, WorkerEvent::Error(message) if message.contains("Tastenkürzel für Start und Stopp"))).await?;
        worker.send(Command::ConfigureHotkey)?;
        event(&mut events, |e| matches!(e, WorkerEvent::HotkeyPhase(HotkeyPhase::Configuring(false)))).await?;
        let (seen, release) = {
            let state = observed.lock().unwrap_or_else(|e| e.into_inner());
            (state.configure_seen.clone(), state.configure_release.clone())
        };
        timeout(Duration::from_secs(3), seen.notified()).await?;
        shortcuts_changed(&service, &session, "Pause").await?;
        event(&mut events, |e| matches!(e, WorkerEvent::HotkeyPhase(HotkeyPhase::Configuring(true)))).await?;
        release.notify_one();
        event(&mut events, |e| matches!(e, WorkerEvent::HotkeyPhase(HotkeyPhase::Ready))).await?;
        let state = observed.lock().unwrap_or_else(|e| e.into_inner());
        assert_eq!(state.hotkey_created, created_before);
        assert_eq!(state.configure_calls, calls_before + 1);
        Ok(())
    }
    .await;
    tokio::task::spawn_blocking(move || worker.shutdown()).await?;
    result?;
    service.close().await?;
    Ok(())
}

#[tokio::test]
#[ignore = "requires a private D-Bus session via dbus-run-session"]
async fn unbound_v1_hotkey_can_be_retried_without_configure_shortcuts() -> TestResult {
    let _serial = BUS_TEST_LOCK.lock().await;
    let observed = Shared::default();
    {
        let mut state = observed.lock().unwrap_or_else(|e| e.into_inner());
        state.hotkey_version = Some(1);
        state.empty_hotkey_trigger = true;
    }
    let service = fake_portal(&observed).await?;
    let (tx, mut events) = mpsc::unbounded_channel();
    let worker = WorkerHandle::spawn(move |event| {
        let _ = tx.send(event);
    });
    let result: TestResult = async {
        event(
            &mut events,
            |e| matches!(e, WorkerEvent::Error(message) if message.contains("Kein Tastenkürzel")),
        )
        .await?;
        let (seen, release) = {
            let mut state = observed.lock().unwrap_or_else(|e| e.into_inner());
            state.empty_hotkey_trigger = false;
            state.delay_hotkey = true;
            (state.hotkey_seen.clone(), state.hotkey_release.clone())
        };
        worker.send(Command::ConfigureHotkey)?;
        event(&mut events, |e| {
            matches!(e, WorkerEvent::HotkeyPhase(HotkeyPhase::Registering))
        })
        .await?;
        timeout(Duration::from_secs(3), seen.notified()).await?;
        {
            let state = observed.lock().unwrap_or_else(|e| e.into_inner());
            ensure(
                state.hotkey_created == 2,
                "retry must create a fresh v1 session",
            )?;
            ensure(
                state.closed == 1,
                "retry must close the old unbound session",
            )?;
            ensure(
                state.configure_calls == 0,
                "v1 must never call ConfigureShortcuts",
            )?;
            // Issue #76: KDE only parses the XDG format, never a
            // localized description such as "Strg+F8".
            ensure(
                state.preferred_triggers == [Some("Pause".to_owned()), Some("Pause".to_owned())],
                "every BindShortcuts must suggest the XDG trigger Pause",
            )?;
        }
        worker.send(Command::Start(continuous()))?;
        event(&mut events, |e| {
            matches!(e, WorkerEvent::Error(message) if message.contains("Richte vor dem Start"))
        }).await?;
        release.notify_one();
        event(&mut events, |e| {
            matches!(e, WorkerEvent::HotkeyPhase(HotkeyPhase::Ready))
        })
        .await?;
        worker.send(Command::Start(ClickSettings {
            repeat: Some(1),
            ..continuous()
        }))?;
        event(&mut events, |e| {
            matches!(e, WorkerEvent::State(RunState::Stopped))
        })
        .await?;
        ensure(
            presses(&observed) == 1,
            "the recovered binding must allow a click run",
        )
    }
    .await;
    tokio::task::spawn_blocking(move || worker.shutdown()).await?;
    service.close().await?;
    result
}

/// When the portal disappears relative to a run.
#[derive(Clone, Copy, Debug)]
enum PortalLoss {
    Idle,
    Countdown,
    Running,
}

/// Issue #74: a crashed or restarted portal never sends `Closed` for its
/// sessions. Losing it must end them anyway and block starts until a new
/// hotkey has been registered with the new portal.
#[tokio::test]
#[ignore = "requires a private D-Bus session via dbus-run-session"]
async fn portal_restart_ends_its_sessions_until_the_hotkey_is_registered_again() -> TestResult {
    for loss in [PortalLoss::Idle, PortalLoss::Countdown, PortalLoss::Running] {
        portal_restart_once(loss)
            .await
            .map_err(|error| format!("{loss:?}: {error}"))?;
    }
    Ok(())
}

async fn portal_restart_once(loss: PortalLoss) -> TestResult {
    let _serial = BUS_TEST_LOCK.lock().await;
    let observed = Shared::default();
    let service = fake_portal(&observed).await?;
    // A fresh portal knows neither the app registration nor the old sessions.
    let fresh = Shared::default();
    fresh
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .require_registration = true;
    // The name passes on directly, without a moment in which it has no owner.
    let replacement = queued_fake_portal(&fresh).await?;
    let (tx, mut events) = mpsc::unbounded_channel();
    let worker = WorkerHandle::spawn(move |event| {
        let _ = tx.send(event);
    });
    let result: TestResult = async {
        event(&mut events, |e| {
            matches!(e, WorkerEvent::HotkeyPhase(HotkeyPhase::Ready))
        })
        .await?;
        match loss {
            PortalLoss::Idle => {}
            PortalLoss::Countdown => {
                worker.send(Command::StartFromButton(continuous()))?;
                event(&mut events, |e| matches!(e, WorkerEvent::Countdown(3))).await?;
            }
            PortalLoss::Running => {
                // The second click would be due long after the test.
                worker.send(Command::Start(ClickSettings {
                    interval_ms: 60_000,
                    ..continuous()
                }))?;
                event(&mut events, |e| {
                    matches!(e, WorkerEvent::State(RunState::Clicking))
                })
                .await?;
                timeout(Duration::from_secs(3), async {
                    while presses(&observed) == 0 {
                        sleep(Duration::from_millis(5)).await;
                    }
                })
                .await?;
            }
        }
        // The new portal rejects input for a session of the old one.
        let old_session = observed
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remote_session
            .clone();
        fresh
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .revoked_session = old_session;
        let presses_before = presses(&observed);

        // The portal leaves the bus without sending `Closed`.
        service.close().await?;
        let lost = events_until(&mut events, |e| {
            matches!(e, WorkerEvent::HotkeyPhase(HotkeyPhase::Unavailable))
        })
        .await?;
        let owner = zbus::fdo::DBusProxy::new(&replacement)
            .await?
            .get_name_owner("org.freedesktop.portal.Desktop".try_into()?)
            .await?;
        ensure(
            replacement.unique_name() == Some(&owner),
            "the replacement portal must own the name",
        )?;
        let states = run_states(&lost);
        match loss {
            PortalLoss::Idle => ensure(
                states.is_empty(),
                "an idle portal loss must not change the run state",
            )?,
            PortalLoss::Countdown | PortalLoss::Running => ensure(
                states.last() == Some(&RunState::Error) && !states.contains(&RunState::Clicking),
                &format!("the portal loss must end the countdown or run: {states:?}"),
            )?,
        }

        worker.send(Command::StartFromButton(ClickSettings {
            repeat: Some(1),
            ..continuous()
        }))?;
        let refused = events_until(&mut events, |e| {
            matches!(e,
                WorkerEvent::Error(message) if message.contains("Richte vor dem Start")
            )
        })
        .await?;
        ensure(
            !refused.iter().any(|e| {
                matches!(
                    e,
                    WorkerEvent::State(RunState::Starting | RunState::Clicking)
                        | WorkerEvent::Countdown(_)
                )
            }),
            "a start must wait for a hotkey registered with the new portal",
        )?;
        {
            let state = fresh.lock().unwrap_or_else(|e| e.into_inner());
            ensure(
                state.hotkey_created == 0
                    && state.remote_session.is_none()
                    && state.buttons.is_empty(),
                "nothing may use the new portal before the hotkey is registered again",
            )?;
        }
        ensure(
            presses(&observed) == presses_before,
            "no click may follow the portal loss",
        )?;

        // "Erneut versuchen" registers with the new portal and allows runs again.
        worker.send(Command::ConfigureHotkey)?;
        event(&mut events, |e| {
            matches!(e, WorkerEvent::HotkeyPhase(HotkeyPhase::Ready))
        })
        .await?;
        worker.send(Command::Start(ClickSettings {
            repeat: Some(1),
            ..continuous()
        }))?;
        event(&mut events, |e| {
            matches!(e, WorkerEvent::State(RunState::Stopped))
        })
        .await?;
        let recovered_presses = presses(&fresh);
        let state = fresh.lock().unwrap_or_else(|e| e.into_inner());
        ensure(
            state.hotkey_created == 1 && state.remote_session.is_some() && recovered_presses == 1,
            "recovery must use a new hotkey and mouse session of the new portal",
        )
    }
    .await;
    tokio::task::spawn_blocking(move || worker.shutdown()).await?;
    replacement.close().await?;
    result
}

/// A portal lost before it answered CreateSession must not leave the
/// registration pending, or "Erneut versuchen" would be ignored for good.
#[tokio::test]
#[ignore = "requires a private D-Bus session via dbus-run-session"]
async fn portal_lost_during_hotkey_session_creation_allows_retry() -> TestResult {
    let _serial = BUS_TEST_LOCK.lock().await;
    let observed = Shared::default();
    let seen = {
        let mut state = observed.lock().unwrap_or_else(|e| e.into_inner());
        state.hold_hotkey_session = true;
        state.hotkey_seen.clone()
    };
    let service = fake_portal(&observed).await?;
    let fresh = Shared::default();
    let replacement = queued_fake_portal(&fresh).await?;
    let (tx, mut events) = mpsc::unbounded_channel();
    let worker = WorkerHandle::spawn(move |event| {
        let _ = tx.send(event);
    });
    let result: TestResult = async {
        timeout(Duration::from_secs(3), seen.notified()).await?;
        // Let the method reply reach the worker, so only the Response is pending.
        sleep(Duration::from_millis(200)).await;
        service.close().await?;
        assert_hotkey_only(
            &events_until(&mut events, |e| matches!(e, WorkerEvent::Error(_))).await?,
        );
        worker.send(Command::ConfigureHotkey)?;
        event(&mut events, |e| {
            matches!(e, WorkerEvent::HotkeyPhase(HotkeyPhase::Ready))
        })
        .await?;
        ensure(
            fresh
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .hotkey_created
                == 1,
            "the retry must register with the new portal",
        )
    }
    .await;
    tokio::task::spawn_blocking(move || worker.shutdown()).await?;
    replacement.close().await?;
    result
}

async fn close_remote_session(service: &zbus::Connection, observed: &Shared) -> TestResult {
    let session = observed
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remote_session
        .clone()
        .ok_or("missing RemoteDesktop session")?;
    service
        .emit_signal(
            None::<&str>,
            &session,
            "org.freedesktop.portal.Session",
            "Closed",
            &(Options::new(),),
        )
        .await?;
    Ok(())
}

#[tokio::test]
#[ignore = "requires a private D-Bus session via dbus-run-session"]
async fn revocation_ends_start_while_response_is_pending() -> TestResult {
    let _bus_guard = BUS_TEST_LOCK.lock().await;
    let observed = Shared::default();
    let service = zbus::connection::Builder::session()?
        .name("org.freedesktop.portal.Desktop")?
        .serve_at(PATH, FakeHotkeys(observed.clone()))?
        .serve_at(PATH, FakeRemote(observed.clone()))?
        .build()
        .await?;
    let settings = ClickSettings {
        interval_ms: 100,
        button: MouseButton::Left,
        click_type: ClickType::Single,
        repeat: Some(1),
        position: None,
        monitor: None,
    };
    let (tx, mut events) = mpsc::unbounded_channel();
    let worker = WorkerHandle::spawn(move |event| {
        let _ = tx.send(event);
    });
    let (seen, release) = {
        let mut state = observed.lock().unwrap_or_else(|e| e.into_inner());
        state.delay_start = true;
        (state.start_seen.clone(), state.start_release.clone())
    };
    let result: TestResult = async {
        event(&mut events, |e| matches!(e, WorkerEvent::Hotkey(_))).await?;
        worker.send(Command::Start(settings))?;
        event(&mut events, |e| matches!(e, WorkerEvent::State(RunState::Starting))).await?;
        timeout(Duration::from_secs(3), seen.notified()).await?;
        close_remote_session(&service, &observed).await?;
        // Neither the pending Response nor the delayed request is released here.
        timeout(
            Duration::from_millis(700),
            event(&mut events, |e| matches!(e, WorkerEvent::State(RunState::Error))),
        )
        .await??;
        event(&mut events, |e| matches!(e, WorkerEvent::Error(message) if message.contains("Freigabe für die Maussteuerung"))).await?;
        assert!(observed.lock().unwrap_or_else(|e| e.into_inner()).buttons.is_empty());
        Ok(())
    }
    .await;
    release.notify_one();
    tokio::task::spawn_blocking(move || worker.shutdown()).await?;
    service.close().await?;
    result
}

/// Run with: dbus-run-session -- cargo test --test worker_portal -- --ignored
#[tokio::test]
#[ignore = "requires a private D-Bus session via dbus-run-session"]
async fn clicks_stop_hotkey_loss_and_shutdown() -> TestResult {
    let _bus_guard = BUS_TEST_LOCK.lock().await;
    let observed = Shared::default();
    let service = zbus::connection::Builder::session()?
        .name("org.freedesktop.portal.Desktop")?
        .serve_at(PATH, FakeHotkeys(observed.clone()))?
        .serve_at(PATH, FakeRemote(observed.clone()))?
        .serve_at(PATH, FakeScreencast(observed.clone()))?
        .build()
        .await?;
    let settings = ClickSettings {
        interval_ms: 10,
        button: MouseButton::Left,
        click_type: ClickType::Single,
        repeat: Some(3),
        position: None,
        monitor: None,
    };
    let (tx, mut events) = mpsc::unbounded_channel();
    let worker = WorkerHandle::spawn(move |event| {
        let _ = tx.send(event);
    });
    let result: TestResult = async {
        event(&mut events, |e| matches!(e, WorkerEvent::State(RunState::Ready))).await?;
        event(&mut events, |e| matches!(e, WorkerEvent::Hotkey(_))).await?;
        let session = OwnedObjectPath::try_from(
            observed
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .hotkey_session
                .clone(),
        )?;
        let activation = (&session, "toggle-clicking", 0_u64, Options::new());
        service
            .emit_signal(
                None::<&str>,
                PATH,
                "org.freedesktop.portal.GlobalShortcuts",
                "Activated",
                &activation,
            )
            .await?;
        let request = start_request(&mut events).await?;
        // A revoked long-running session must stop before the next scheduled click.
        let long_run = ClickSettings {
            interval_ms: 60_000,
            repeat: None,
            ..settings.clone()
        };
        request.start(long_run);
        event(&mut events, |e| matches!(e, WorkerEvent::State(RunState::Clicking))).await?;
        timeout(Duration::from_secs(3), async {
            loop {
                if observed.lock().unwrap_or_else(|e| e.into_inner()).buttons.len() >= 2 {
                    break;
                }
                sleep(Duration::from_millis(5)).await;
            }
        })
        .await?;
        let first_session = observed.lock().unwrap_or_else(|e| e.into_inner()).remote_session.clone();
        close_remote_session(&service, &observed).await?;
        timeout(Duration::from_millis(500), async {
            event(&mut events, |e| matches!(e, WorkerEvent::State(RunState::Stopped))).await?;
            event(&mut events, |e| matches!(e, WorkerEvent::State(RunState::Error))).await
        })
        .await??;
        event(&mut events, |e| matches!(e, WorkerEvent::Error(message) if message.contains("Freigabe für die Maussteuerung"))).await?;

        // A stopped session must also be discarded, so a later start asks again.
        worker.send(Command::Start(settings.clone()))?;
        event(&mut events, |e| matches!(e, WorkerEvent::State(RunState::Clicking))).await?;
        event(&mut events, |e| matches!(e, WorkerEvent::State(RunState::Stopped))).await?;
        assert_ne!(observed.lock().unwrap_or_else(|e| e.into_inner()).remote_session, first_session);
        let stopped_session = observed.lock().unwrap_or_else(|e| e.into_inner()).remote_session.clone();
        close_remote_session(&service, &observed).await?;
        event(&mut events, |e| matches!(e, WorkerEvent::Status(message) if message.contains("Freigabe erneut erforderlich"))).await?;
        worker.send(Command::Start(settings.clone()))?;
        event(&mut events, |e| matches!(e, WorkerEvent::State(RunState::Clicking))).await?;
        event(&mut events, |e| matches!(e, WorkerEvent::State(RunState::Stopped))).await?;
        assert_ne!(observed.lock().unwrap_or_else(|e| e.into_inner()).remote_session, stopped_session);

        // If permission is revoked during Start, no run may be confirmed.
        close_remote_session(&service, &observed).await?;
        event(&mut events, |e| matches!(e, WorkerEvent::Status(message) if message.contains("Freigabe erneut erforderlich"))).await?;
        let (seen, release) = {
            let mut state = observed.lock().unwrap_or_else(|e| e.into_inner());
            state.delay_start = true;
            (state.start_seen.clone(), state.start_release.clone())
        };
        worker.send(Command::Start(settings.clone()))?;
        timeout(Duration::from_secs(3), seen.notified()).await?;
        close_remote_session(&service, &observed).await?;
        release.notify_one();
        let mut incorrectly_started = false;
        let mut reported_error_state = false;
        timeout(Duration::from_secs(3), async {
            while let Some(event) = events.recv().await {
                match event {
                    WorkerEvent::State(RunState::Clicking) => incorrectly_started = true,
                    WorkerEvent::State(RunState::Error) => reported_error_state = true,
                    WorkerEvent::Error(_) => break,
                    _ => {}
                }
            }
        })
        .await?;
        assert!(!incorrectly_started, "revoked permission must not start a run");
        assert!(reported_error_state, "revoked permission must end the pending start");
        observed.lock().unwrap_or_else(|e| e.into_inner()).delay_start = false;
        worker.send(Command::Start(settings.clone()))?;
        event(&mut events, |e| matches!(e, WorkerEvent::State(RunState::Clicking))).await?;
        event(&mut events, |e| matches!(e, WorkerEvent::State(RunState::Stopped))).await?;

        for button in [MouseButton::Left, MouseButton::Right, MouseButton::Middle] {
            for click_type in [ClickType::Single, ClickType::Double] {
                observed
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .buttons
                    .clear();
                worker.send(Command::Start(ClickSettings {
                    button,
                    click_type,
                    ..settings.clone()
                }))?;
                event(&mut events, |e| matches!(e, WorkerEvent::State(RunState::Clicking))).await?;
                event(&mut events, |e| matches!(e, WorkerEvent::State(RunState::Stopped))).await?;
                let buttons = observed
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .buttons
                    .clone();
                let expected = [(button.evdev_code(), 1), (button.evdev_code(), 0)]
                    .repeat(3 * usize::from(click_type.clicks_per_tick()));
                assert_eq!(buttons, expected);
            }
        }
        {
            let mut state = observed.lock().unwrap_or_else(|e| e.into_inner());
            state.revoked_session = state.remote_session.clone();
        }
        worker.send(Command::Start(settings.clone()))?;
        event(&mut events, |e| matches!(e, WorkerEvent::State(RunState::Clicking))).await?;
        event(&mut events, |e| matches!(e, WorkerEvent::State(RunState::Error))).await?;
        event(&mut events, |e| matches!(e, WorkerEvent::Error(_))).await?;
        // Retry must request a fresh session instead of reusing the revoked one.
        worker.send(Command::Start(settings.clone()))?;
        event(&mut events, |e| matches!(e, WorkerEvent::State(RunState::Clicking))).await?;
        event(&mut events, |e| matches!(e, WorkerEvent::State(RunState::Stopped))).await?;
        {
            let state = observed.lock().unwrap_or_else(|e| e.into_inner());
            assert_ne!(state.remote_session, state.revoked_session);
        }

        let fixed = ClickSettings {
            position: Some((1919, 1079)),
            monitor: Some(MonitorGeometry { x: -1920, y: 0, width: 1920, height: 1080 }),
            ..settings.clone()
        };
        worker.send(Command::Start(fixed.clone()))?;
        event(&mut events, |e| matches!(e, WorkerEvent::State(RunState::Clicking))).await?;
        event(&mut events, |e| matches!(e, WorkerEvent::State(RunState::Stopped))).await?;
        assert_eq!(observed.lock().unwrap_or_else(|e| e.into_inner()).motions,
            vec![(42, 1919.0, 1079.0); 3]);

        // A different monitor must fail before sending motion or button events.
        let before = observed.lock().unwrap_or_else(|e| e.into_inner()).buttons.len();
        worker.send(Command::Start(ClickSettings {
            monitor: Some(MonitorGeometry { x: 0, y: 0, width: 1920, height: 1080 }),
            ..fixed.clone()
        }))?;
        event(&mut events, |e| matches!(e, WorkerEvent::State(RunState::Error))).await?;
        event(&mut events, |e| matches!(e, WorkerEvent::Error(message) if message.contains("Bildschirm stimmt nicht"))).await?;
        assert_eq!(observed.lock().unwrap_or_else(|e| e.into_inner()).buttons.len(), before);

        let (seen, release) = {
            let mut state = observed.lock().unwrap_or_else(|e| e.into_inner());
            state.delay_start = true;
            state.motions.clear();
            (state.start_seen.clone(), state.start_release.clone())
        };
        worker.send(Command::Start(fixed.clone()))?;
        event(&mut events, |e| matches!(e, WorkerEvent::State(RunState::Starting))).await?;
        timeout(Duration::from_secs(3), seen.notified()).await?;
        worker.send(Command::Start(ClickSettings { position: Some((100, 150)), ..fixed.clone() }))?;
        let closed_before_stop = observed.lock().unwrap_or_else(|e| e.into_inner()).closed;
        // Stop overtakes and outdates this queued duplicate. The next block
        // checks deterministically that a duplicate during Starting keeps the
        // pending settings: it is queued before the portal can answer.
        worker.send(Command::Stop)?;
        event(&mut events, |e| matches!(e, WorkerEvent::State(RunState::Stopped))).await?;
        assert!(observed.lock().unwrap_or_else(|e| e.into_inner()).closed > closed_before_stop,
            "Stop must close the pending RemoteDesktop session before confirming it");
        release.notify_one();
        sleep(Duration::from_millis(80)).await;
        assert_eq!(observed.lock().unwrap_or_else(|e| e.into_inner()).buttons.len(), before);
        assert!(observed.lock().unwrap_or_else(|e| e.into_inner()).motions.is_empty());

        // A new permission request owns its settings even if a duplicate arrives.
        worker.send(Command::Start(fixed.clone()))?;
        timeout(Duration::from_secs(3), seen.notified()).await?;
        worker.send(Command::Start(ClickSettings {
            position: Some((100, 150)),
            button: MouseButton::Right,
            click_type: ClickType::Double,
            repeat: Some(1),
            ..fixed
        }))?;
        release.notify_one();
        event(&mut events, |e| matches!(e, WorkerEvent::State(RunState::Clicking))).await?;
        event(&mut events, |e| matches!(e, WorkerEvent::State(RunState::Stopped))).await?;
        assert_eq!(observed.lock().unwrap_or_else(|e| e.into_inner()).motions,
            vec![(42, 1919.0, 1079.0); 3]);
        assert_eq!(&observed.lock().unwrap_or_else(|e| e.into_inner()).buttons[before..],
            [(MouseButton::Left.evdev_code(), 1), (MouseButton::Left.evdev_code(), 0)].repeat(3));
        observed.lock().unwrap_or_else(|e| e.into_inner()).delay_start = false;

        for failed_state in [1, 0] {
            {
                let mut observed = observed.lock().unwrap_or_else(|e| e.into_inner());
                observed.buttons.clear();
                observed.button_failures[failed_state] = 1;
            }
            worker.send(Command::Start(settings.clone()))?;
            event(&mut events, |e| matches!(e, WorkerEvent::State(RunState::Clicking))).await?;
            event(&mut events, |e| matches!(e, WorkerEvent::State(RunState::Error))).await?;
            event(&mut events, |e| matches!(e, WorkerEvent::Error(_))).await?;
            let expected = if failed_state == 1 {
                vec![(0x110, 1), (0x110, 0)]
            } else {
                vec![(0x110, 1), (0x110, 0), (0x110, 0)]
            };
            assert_eq!(observed.lock().unwrap_or_else(|e| e.into_inner()).buttons, expected);
        }

        worker.send(Command::Start(ClickSettings {
            repeat: None,
            ..settings.clone()
        }))?;
        event(&mut events, |e| matches!(e, WorkerEvent::State(RunState::Clicking))).await?;
        worker.send(Command::Start(ClickSettings {
            interval_ms: 0,
            ..settings.clone()
        }))?;
        worker.send(Command::Stop)?;
        event(&mut events, |e| matches!(e, WorkerEvent::State(RunState::Stopped))).await?;
        let count = observed
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .buttons
            .len();
        sleep(Duration::from_millis(40)).await;
        assert_eq!(
            observed
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .buttons
                .len(),
            count
        );

        worker.send(Command::Start(ClickSettings {
            repeat: None,
            ..settings.clone()
        }))?;
        event(&mut events, |e| matches!(e, WorkerEvent::State(RunState::Clicking))).await?;
        service
            .emit_signal(
                None::<&str>,
                PATH,
                "org.freedesktop.portal.GlobalShortcuts",
                "Activated",
                &activation,
            )
            .await?;
        event(&mut events, |e| matches!(e, WorkerEvent::State(RunState::Stopped))).await?;

        // Removing the hotkey fails a pending start and cancels its permission request.
        let hotkey_session = observed.lock().unwrap_or_else(|e| e.into_inner()).hotkey_session.clone();
        let (seen, release) = {
            let mut state = observed.lock().unwrap_or_else(|e| e.into_inner());
            state.delay_start = true;
            (state.start_seen.clone(), state.start_release.clone())
        };
        worker.send(Command::Start(ClickSettings {
            position: Some((1919, 1079)),
            monitor: Some(MonitorGeometry { x: -1920, y: 0, width: 1920, height: 1080 }),
            ..settings.clone()
        }))?;
        event(&mut events, |e| matches!(e, WorkerEvent::State(RunState::Starting))).await?;
        timeout(Duration::from_secs(3), seen.notified()).await?;
        shortcuts_changed(&service, &hotkey_session, "").await?;
        let removed = events_until(&mut events, |e| matches!(e, WorkerEvent::Error(message) if message.contains("entfernt"))).await?;
        assert_eq!(run_states(&removed), [RunState::Stopped, RunState::Error]);
        release.notify_one();
        observed.lock().unwrap_or_else(|e| e.into_inner()).delay_start = false;
        shortcuts_changed(&service, &hotkey_session, "Pause").await?;
        let rebound = events_until(&mut events, |e| matches!(e, WorkerEvent::HotkeyPhase(HotkeyPhase::Ready))).await?;
        assert!(run_states(&rebound).is_empty(), "a cancelled start must not resume: {rebound:?}");

        // Removing the hotkey also fails an active run.
        worker.send(Command::Start(ClickSettings {
            repeat: None,
            ..settings.clone()
        }))?;
        event(&mut events, |e| matches!(e, WorkerEvent::State(RunState::Clicking))).await?;
        shortcuts_changed(&service, &hotkey_session, "").await?;
        let removed = events_until(&mut events, |e| matches!(e, WorkerEvent::Error(message) if message.contains("entfernt"))).await?;
        assert_eq!(run_states(&removed), [RunState::Stopped, RunState::Error]);
        shortcuts_changed(&service, &hotkey_session, "Pause").await?;
        event(&mut events, |e| matches!(e, WorkerEvent::HotkeyPhase(HotkeyPhase::Ready))).await?;

        worker.send(Command::Start(ClickSettings {
            repeat: None,
            ..settings
        }))?;
        event(&mut events, |e| matches!(e, WorkerEvent::State(RunState::Clicking))).await?;
        let session = observed
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .hotkey_session
            .clone();
        service
            .emit_signal(
                None::<&str>,
                session.as_str(),
                "org.freedesktop.portal.Session",
                "Closed",
                &(Options::new(),),
            )
            .await?;
        event(&mut events, |e| matches!(e, WorkerEvent::State(RunState::Stopped))).await?;
        event(&mut events, |e| matches!(e, WorkerEvent::State(RunState::Error))).await?;
        event(
            &mut events,
            |e| matches!(e, WorkerEvent::Error(message) if message.contains("Freigabe für das Tastenkürzel")),
        )
        .await?;
        let count = observed
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .buttons
            .len();
        sleep(Duration::from_millis(40)).await;
        assert_eq!(
            observed
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .buttons
                .len(),
            count
        );
        // Cover both permission dialogs, with responsive and stalled Session.Close.
        for hotkey_pending in [true, false] {
            for stall_close in [false, true] {
                let (seen, release, closed_before) = {
                    let mut state = observed.lock().unwrap_or_else(|e| e.into_inner());
                    state.delay_hotkey = hotkey_pending;
                    state.delay_start = !hotkey_pending;
                    state.stall_close = stall_close;
                    let (seen, release) = if hotkey_pending {
                        (state.hotkey_seen.clone(), state.hotkey_release.clone())
                    } else {
                        (state.start_seen.clone(), state.start_release.clone())
                    };
                    (seen, release, state.closed)
                };
                let (tx, mut pending_events) = mpsc::unbounded_channel();
                let settings = ClickSettings {
                    interval_ms: 100, button: MouseButton::Left, click_type: ClickType::Single,
                    repeat: Some(1), position: None, monitor: None,
                };
                let pending_worker = WorkerHandle::spawn(move |event| {
                    let _ = tx.send(event);
                });
                if !hotkey_pending {
                    event(&mut pending_events, |e| matches!(e, WorkerEvent::Hotkey(_))).await?;
                    pending_worker.send(Command::Start(settings))?;
                }
                timeout(Duration::from_secs(3), seen.notified()).await?;
                let owner = {
                    let state = observed.lock().unwrap_or_else(|e| e.into_inner());
                    if hotkey_pending { state.hotkey_owner.clone() } else { state.remote_owner.clone() }
                };
                let before = std::time::Instant::now();
                timeout(Duration::from_secs(4), tokio::task::spawn_blocking(move || pending_worker.shutdown())).await??;
                eprintln!("Pending {} shutdown, stalled Close={stall_close}: {:?}",
                    if hotkey_pending { "hotkey" } else { "mouse" }, before.elapsed());
                assert!(observed.lock().unwrap_or_else(|e| e.into_inner()).closed > closed_before,
                    "Shutdown must close its pending permission session");
                let bus = zbus::fdo::DBusProxy::new(&service).await?;
                assert!(!bus.name_has_owner(owner.as_str().try_into()?).await?,
                    "Permission owner must disconnect even when Session.Close stalls");
                release.notify_one();
                // Mouse shutdown also closes the hotkey session. Give every
                // stalled close its own retained permit, including late waiters.
                let close_releases = std::mem::take(
                    &mut observed.lock().unwrap_or_else(|e| e.into_inner()).close_releases,
                );
                for close_release in close_releases {
                    close_release.notify_one();
                }
                sleep(Duration::from_millis(30)).await;
                while let Ok(event) = pending_events.try_recv() {
                    assert!(!matches!(event, WorkerEvent::State(RunState::Clicking)),
                        "A late permission response must not start clicking");
                }
                observed.lock().unwrap_or_else(|e| e.into_inner()).stall_close = false;
            }
        }
        Ok(())
    }
    .await;
    tokio::task::spawn_blocking(move || worker.shutdown()).await?;
    assert!(observed.lock().unwrap_or_else(|e| e.into_inner()).closed >= 1);
    result?;
    let mut saw_closing = false;
    while let Ok(event) = events.try_recv() {
        saw_closing |= matches!(event, WorkerEvent::State(RunState::Closing));
    }
    assert!(saw_closing, "shutdown must report the closing state");
    service.close().await?;
    Ok(())
}

/// Part of the worker's error when a held click is not answered in 250 ms.
const CLICK_TIMEOUT: &str = "nicht innerhalb von 250 ms bestätigt";

/// Retry a scenario only when its held click outlived the worker's 250 ms
/// event timeout, which a heavily loaded machine can cause. Every other
/// failure, including a real regression, fails in each attempt.
async fn retry_if_click_window_missed<F, Fut>(scenario: F) -> TestResult
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = TestResult>,
{
    let mut result = scenario().await;
    for _ in 1..3 {
        match &result {
            Err(error) if error.to_string().contains(CLICK_TIMEOUT) => {
                eprintln!("Held click timed out under load, retrying: {error}");
                result = scenario().await;
            }
            _ => break,
        }
    }
    result
}

/// Fail without panicking, so the test still shuts its worker down.
///
/// A panic skips that cleanup. The private bus would then start the real
/// desktop portal for the dropped worker's calls.
fn ensure(condition: bool, message: &str) -> TestResult {
    if condition {
        Ok(())
    } else {
        Err(message.into())
    }
}

async fn fake_portal(observed: &Shared) -> zbus::Result<zbus::Connection> {
    fake_portal_services(observed)?
        .name("org.freedesktop.portal.Desktop")?
        .build()
        .await
}

/// A fake portal that waits for the name behind its current owner and owns
/// it as soon as that connection leaves the bus, like a restarted portal.
async fn queued_fake_portal(
    observed: &Shared,
) -> Result<zbus::Connection, Box<dyn std::error::Error>> {
    let connection = fake_portal_services(observed)?.build().await?;
    // The builder's default flags would replace the owner instead of waiting.
    let reply = connection
        .request_name_with_flags(
            "org.freedesktop.portal.Desktop",
            zbus::fdo::RequestNameFlags::AllowReplacement.into(),
        )
        .await?;
    ensure(
        reply == zbus::fdo::RequestNameReply::InQueue,
        "the replacement portal must wait for the name",
    )?;
    Ok(connection)
}

fn fake_portal_services(observed: &Shared) -> zbus::Result<zbus::connection::Builder<'static>> {
    zbus::connection::Builder::session()?
        .serve_at(PATH, FakeRegistry(observed.clone()))?
        .serve_at(PATH, FakeHotkeys(observed.clone()))?
        .serve_at(PATH, FakeRemote(observed.clone()))?
        .serve_at(PATH, FakeScreencast(observed.clone()))
}

/// The real application, without a display and with its own settings. It is
/// killed on drop: after the fake portal ends, the private bus would start the
/// real desktop portal for its calls.
struct App(std::process::Child, std::path::PathBuf);

impl App {
    /// Log to a file: a full pipe would block a long-running instance.
    fn launch(
        config: &std::path::Path,
        name: &str,
        activation_token: Option<&str>,
    ) -> std::io::Result<Self> {
        let log = config.join(format!("{name}.log"));
        let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_klickmeister"));
        command
            .env("QT_QPA_PLATFORM", "offscreen")
            .env("XDG_CONFIG_HOME", config)
            .env_remove("XDG_ACTIVATION_TOKEN")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::fs::File::create(&log)?);
        if let Some(token) = activation_token {
            command.env("XDG_ACTIVATION_TOKEN", token);
        }
        command.spawn().map(|child| Self(child, log))
    }

    async fn wait_for_exit(
        &mut self,
    ) -> Result<std::process::ExitStatus, Box<dyn std::error::Error>> {
        timeout(Duration::from_secs(20), async {
            loop {
                if let Some(status) = self.0.try_wait()? {
                    return Ok(status);
                }
                sleep(Duration::from_millis(50)).await;
            }
        })
        .await?
    }

    fn stderr(&self) -> String {
        std::fs::read_to_string(&self.1).unwrap_or_default()
    }

    /// End the process abruptly, like a crash.
    fn kill(&mut self) -> std::io::Result<()> {
        self.0.kill()?;
        self.0.wait().map(drop)
    }
}

impl Drop for App {
    fn drop(&mut self) {
        let _ = self.kill();
    }
}

async fn hotkey_sessions_reach(observed: &Shared, count: usize, app: &mut App) -> TestResult {
    // Loading QML in a debug build takes a while on a busy machine.
    timeout(Duration::from_secs(60), async {
        loop {
            if observed
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .hotkey_created
                >= count
            {
                return Ok(());
            }
            if let Some(status) = app.0.try_wait()? {
                return Err(format!("application exited with {status}: {}", app.stderr()).into());
            }
            sleep(Duration::from_millis(50)).await;
        }
    })
    .await?
}

#[tokio::test]
#[ignore = "requires a private D-Bus session via dbus-run-session"]
async fn a_second_launch_shows_the_first_without_a_second_hotkey_session() -> TestResult {
    let _serial = BUS_TEST_LOCK.lock().await;
    let observed = Shared::default();
    let service = fake_portal(&observed).await?;
    let config = std::env::temp_dir().join(format!("klickmeister-instance-{}", std::process::id()));
    std::fs::create_dir_all(&config)?;
    let result: TestResult = async {
        let mut first = App::launch(&config, "first", None)?;
        hotkey_sessions_reach(&observed, 1, &mut first).await?;
        let registered = observed
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .registered_apps
            .len();

        let mut second = App::launch(&config, "second", Some("launcher-token"))?;
        let status = second.wait_for_exit().await?;
        ensure(
            status.success(),
            &format!(
                "second launch must show the first window: {}",
                second.stderr()
            ),
        )?;
        ensure(
            first.0.try_wait()?.is_none(),
            "first instance must keep running",
        )?;
        // Give a wrongly started second instance time to reach the portal.
        sleep(Duration::from_millis(500)).await;
        {
            let state = observed.lock().unwrap_or_else(|e| e.into_inner());
            ensure(
                state.hotkey_created == 1,
                "second launch must not create a hotkey session",
            )?;
            ensure(
                state.registered_apps.len() == registered,
                "second launch must not connect to the portal",
            )?;
        }

        // A crash must not leave the instance lock behind.
        first.kill()?;
        let mut third = App::launch(&config, "third", None)?;
        hotkey_sessions_reach(&observed, 2, &mut third).await?;
        third.kill()?;
        Ok(())
    }
    .await;
    let _ = std::fs::remove_dir_all(&config);
    service.close().await?;
    result
}

#[tokio::test]
#[ignore = "requires a private D-Bus session via dbus-run-session"]
async fn every_owned_portal_connection_registers_the_app_before_creating_sessions() -> TestResult {
    let _serial = BUS_TEST_LOCK.lock().await;
    let observed = Shared::default();
    observed
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .require_registration = true;
    let service = fake_portal(&observed).await?;
    let (tx, mut events) = mpsc::unbounded_channel();
    let worker = WorkerHandle::spawn(move |event| {
        let _ = tx.send(event);
    });
    let result: TestResult = async {
        event(&mut events, |e| {
            matches!(e, WorkerEvent::HotkeyPhase(HotkeyPhase::Ready))
        })
        .await?;
        worker.send(Command::Start(ClickSettings {
            position: Some((80, 400)),
            monitor: Some(MonitorGeometry {
                x: -1920,
                y: 0,
                width: 1920,
                height: 1080,
            }),
            repeat: Some(1),
            ..continuous()
        }))?;
        event(&mut events, |e| {
            matches!(e, WorkerEvent::State(RunState::Clicking))
        })
        .await?;
        event(&mut events, |e| {
            matches!(e, WorkerEvent::State(RunState::Stopped))
        })
        .await?;
        let state = observed.lock().unwrap_or_else(|e| e.into_inner());
        ensure(
            state.registered_apps.len() == 2,
            "each owned connection must register separately",
        )?;
        ensure(
            state.hotkey_owner != state.remote_owner,
            "portal connections must remain independent",
        )?;
        ensure(
            state.registered_apps.contains_key(&state.hotkey_owner),
            "hotkey owner was not registered",
        )?;
        ensure(
            state.registered_apps.contains_key(&state.remote_owner),
            "pointer owner was not registered",
        )?;
        ensure(
            state.motions == [(42, 80.0, 400.0)],
            "fixed position was not reached",
        )?;
        ensure(state.buttons.len() == 2, "expected one complete click")
    }
    .await;
    tokio::task::spawn_blocking(move || worker.shutdown()).await?;
    service.close().await?;
    result
}

#[tokio::test]
#[ignore = "requires a private D-Bus session via dbus-run-session"]
async fn denied_app_registration_does_not_create_portal_sessions() -> TestResult {
    let _serial = BUS_TEST_LOCK.lock().await;
    let observed = Shared::default();
    observed
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .reject_registration = true;
    let service = fake_portal(&observed).await?;
    let (tx, mut events) = mpsc::unbounded_channel();
    let worker = WorkerHandle::spawn(move |event| {
        let _ = tx.send(event);
    });
    let result: TestResult = async {
        event(
            &mut events,
            |e| matches!(e, WorkerEvent::Error(message) if message.contains("registration denied")),
        )
        .await?;
        let state = observed.lock().unwrap_or_else(|e| e.into_inner());
        ensure(
            state.hotkey_created == 0 && state.remote_session.is_none(),
            "denied registration must stop portal setup",
        )
    }
    .await;
    tokio::task::spawn_blocking(move || worker.shutdown()).await?;
    service.close().await?;
    result
}

#[tokio::test]
#[ignore = "requires a private D-Bus session via dbus-run-session"]
async fn fixed_position_selects_its_monitor_from_kde_output_streams() -> TestResult {
    let _serial = BUS_TEST_LOCK.lock().await;
    for reverse in [false, true] {
        let observed = Shared::default();
        {
            let mut state = observed.lock().unwrap_or_else(|e| e.into_inner());
            state.multi_monitor_layout = true;
            state.reverse_monitor_streams = reverse;
        }
        let service = fake_portal(&observed).await?;
        let (tx, mut events) = mpsc::unbounded_channel();
        let worker = WorkerHandle::spawn(move |event| {
            let _ = tx.send(event);
        });
        let result: TestResult = async {
            event(&mut events, |e| {
                matches!(e, WorkerEvent::HotkeyPhase(HotkeyPhase::Ready))
            })
            .await?;
            worker.send(Command::Start(ClickSettings {
                position: Some((80, 400)),
                monitor: Some(MonitorGeometry {
                    x: -1920,
                    y: 0,
                    width: 1920,
                    height: 1080,
                }),
                repeat: Some(1),
                ..continuous()
            }))?;
            event(&mut events, |e| {
                matches!(e, WorkerEvent::State(RunState::Clicking))
            })
            .await?;
            event(&mut events, |e| {
                matches!(e, WorkerEvent::State(RunState::Stopped))
            })
            .await?;
            let (closed_before, first_session) = {
                let state = observed.lock().unwrap_or_else(|e| e.into_inner());
                ensure(
                    state.multiple_sources,
                    "KDE needs individual monitor streams",
                )?;
                ensure(
                    state.motions == [(42, 80.0, 400.0)],
                    "wrong monitor stream selected",
                )?;
                ensure(state.buttons.len() == 2, "expected one complete click")?;
                (state.closed, state.remote_session.clone())
            };
            worker.send(Command::Start(ClickSettings {
                position: Some((120, 250)),
                monitor: Some(MonitorGeometry {
                    x: 0,
                    y: 0,
                    width: 1920,
                    height: 1080,
                }),
                repeat: Some(1),
                ..continuous()
            }))?;
            event(&mut events, |e| {
                matches!(e, WorkerEvent::State(RunState::Clicking))
            })
            .await?;
            event(&mut events, |e| {
                matches!(e, WorkerEvent::State(RunState::Stopped))
            })
            .await?;
            let state = observed.lock().unwrap_or_else(|e| e.into_inner());
            ensure(
                state.closed > closed_before && state.remote_session != first_session,
                "switching monitor must replace the granted session",
            )?;
            ensure(
                state.motions == [(42, 80.0, 400.0), (99, 120.0, 250.0)],
                "monitor switch used the old stream",
            )?;
            ensure(state.buttons.len() == 4, "expected two complete clicks")
        }
        .await;
        tokio::task::spawn_blocking(move || worker.shutdown()).await?;
        service.close().await?;
        result?;
    }
    Ok(())
}

#[tokio::test]
#[ignore = "requires a private D-Bus session via dbus-run-session"]
async fn invalid_monitor_streams_close_without_any_input() -> TestResult {
    let _serial = BUS_TEST_LOCK.lock().await;
    let matching = FakeMonitor {
        node_id: 42,
        position: Some((-1920, 0)),
        size: Some((1920, 1080)),
    };
    for streams in [
        vec![],
        vec![
            matching.clone(),
            FakeMonitor {
                node_id: 99,
                ..matching.clone()
            },
        ],
        vec![
            matching.clone(),
            FakeMonitor {
                node_id: 99,
                position: None,
                ..matching.clone()
            },
        ],
        vec![
            matching.clone(),
            FakeMonitor {
                node_id: 99,
                size: None,
                ..matching.clone()
            },
        ],
        vec![FakeMonitor {
            size: Some((1280, 720)),
            ..matching.clone()
        }],
    ] {
        let observed = Shared::default();
        observed
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .monitor_streams = Some(streams);
        let service = fake_portal(&observed).await?;
        let (tx, mut events) = mpsc::unbounded_channel();
        let worker = WorkerHandle::spawn(move |event| {
            let _ = tx.send(event);
        });
        let result: TestResult = async {
            event(&mut events, |e| {
                matches!(e, WorkerEvent::HotkeyPhase(HotkeyPhase::Ready))
            })
            .await?;
            worker.send(Command::Start(ClickSettings {
                position: Some((80, 400)),
                monitor: Some(MonitorGeometry {
                    x: -1920,
                    y: 0,
                    width: 1920,
                    height: 1080,
                }),
                repeat: Some(1),
                ..continuous()
            }))?;
            event(&mut events, |e| matches!(e, WorkerEvent::Error(_))).await?;
            let state = observed.lock().unwrap_or_else(|e| e.into_inner());
            ensure(
                state.motions.is_empty() && state.buttons.is_empty(),
                "invalid streams must emit no input",
            )?;
            ensure(
                state.closed >= 1,
                "invalid stream session must be closed before reporting the error",
            )
        }
        .await;
        tokio::task::spawn_blocking(move || worker.shutdown()).await?;
        service.close().await?;
        result?;
    }
    Ok(())
}

fn continuous() -> ClickSettings {
    ClickSettings {
        interval_ms: 10,
        button: MouseButton::Left,
        click_type: ClickType::Single,
        repeat: None,
        position: None,
        monitor: None,
    }
}

/// Start a continuous run and hold its first press, so the worker stays
/// inside that click until the returned notification releases it.
async fn hold_first_click(
    worker: &WorkerHandle,
    observed: &Shared,
    events: &mut mpsc::UnboundedReceiver<WorkerEvent>,
) -> Result<Arc<Notify>, Box<dyn std::error::Error>> {
    hold_first_click_of(worker, observed, events, continuous()).await
}

/// Like `hold_first_click`, for a run with the given settings.
async fn hold_first_click_of(
    worker: &WorkerHandle,
    observed: &Shared,
    events: &mut mpsc::UnboundedReceiver<WorkerEvent>,
    settings: ClickSettings,
) -> Result<Arc<Notify>, Box<dyn std::error::Error>> {
    let (seen, release) = {
        let mut state = observed.lock().unwrap_or_else(|e| e.into_inner());
        state.delay_button = true;
        (state.button_seen.clone(), state.button_release.clone())
    };
    worker.send(Command::Start(settings))?;
    event(events, |e| {
        matches!(e, WorkerEvent::State(RunState::Clicking))
    })
    .await?;
    timeout(Duration::from_secs(3), seen.notified()).await?;
    Ok(release)
}

/// Start a run, hold its first press and fill all 16 command slots meanwhile.
async fn fill_queue_during_held_click(
    worker: &WorkerHandle,
    observed: &Shared,
    events: &mut mpsc::UnboundedReceiver<WorkerEvent>,
) -> Result<Arc<Notify>, Box<dyn std::error::Error>> {
    let release = hold_first_click(worker, observed, events).await?;
    // The Start button sends StartFromButton; such starts must be outdated
    // by a later Stop just like hotkey starts.
    for _ in 0..16 {
        worker.send(Command::StartFromButton(continuous()))?;
    }
    ensure(
        worker.send(Command::StartFromButton(continuous()))
            == Err("Klickmeister ist ausgelastet. Versuche es erneut."),
        "the command queue must be full while the click is held",
    )?;
    // The controller keeps the shown run state unless the worker has ended.
    ensure(
        !worker.is_stopped(),
        "a full queue must not count as a stopped worker",
    )?;
    Ok(release)
}

fn release_held_click(observed: &Shared, release: &Notify) {
    observed
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .delay_button = false;
    release.notify_one();
}

/// Issue #16: Stop reaches a worker whose command queue is full, and the
/// starts queued before it cannot restart the run.
#[tokio::test]
#[ignore = "requires a private D-Bus session via dbus-run-session"]
async fn stop_overtakes_a_full_command_queue() -> TestResult {
    retry_if_click_window_missed(stop_overtakes_a_full_command_queue_once).await
}

async fn stop_overtakes_a_full_command_queue_once() -> TestResult {
    let _bus_guard = BUS_TEST_LOCK.lock().await;
    let observed = Shared::default();
    let service = fake_portal(&observed).await?;
    let (tx, mut events) = mpsc::unbounded_channel();
    let worker = WorkerHandle::spawn(move |event| {
        let _ = tx.send(event);
    });
    let result: TestResult = async {
        event(&mut events, |e| {
            matches!(e, WorkerEvent::HotkeyPhase(HotkeyPhase::Ready))
        })
        .await?;
        let release = fill_queue_during_held_click(&worker, &observed, &mut events).await?;
        worker.send(Command::Stop)?;
        release_held_click(&observed, &release);
        let stopped = events_until(&mut events, |e| {
            matches!(
                e,
                WorkerEvent::State(RunState::Stopped) | WorkerEvent::Error(_)
            )
        })
        .await?;
        if let Some(WorkerEvent::Error(error)) = stopped.last() {
            return Err(error.clone().into());
        }
        ensure(
            run_states(&stopped) == [RunState::Stopped],
            &format!("Stop must end the held run directly: {stopped:?}"),
        )?;

        let clicks = observed
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .buttons
            .len();
        sleep(Duration::from_millis(150)).await;
        ensure(
            observed
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .buttons
                .len()
                == clicks,
            "no click may follow a confirmed Stop",
        )?;
        let later: Vec<_> = std::iter::from_fn(|| events.try_recv().ok()).collect();
        ensure(
            run_states(&later).is_empty()
                && !later.iter().any(|e| matches!(e, WorkerEvent::Error(_))),
            &format!("starts queued before Stop must not restart the run: {later:?}"),
        )?;

        // The drained queue accepts new commands, and a later start runs.
        worker.send(Command::Start(continuous()))?;
        event(&mut events, |e| {
            matches!(e, WorkerEvent::State(RunState::Clicking))
        })
        .await?;
        worker.send(Command::Stop)?;
        event(&mut events, |e| {
            matches!(e, WorkerEvent::State(RunState::Stopped))
        })
        .await?;
        Ok(())
    }
    .await;
    tokio::task::spawn_blocking(move || worker.shutdown()).await?;
    service.close().await?;
    result
}

/// Issue #16: Shutdown ends a run whose command queue is full, without
/// handling the queued starts first.
#[tokio::test]
#[ignore = "requires a private D-Bus session via dbus-run-session"]
async fn shutdown_overtakes_a_full_command_queue() -> TestResult {
    retry_if_click_window_missed(shutdown_overtakes_a_full_command_queue_once).await
}

async fn shutdown_overtakes_a_full_command_queue_once() -> TestResult {
    let _bus_guard = BUS_TEST_LOCK.lock().await;
    let observed = Shared::default();
    let service = fake_portal(&observed).await?;
    let (tx, mut events) = mpsc::unbounded_channel();
    let worker = WorkerHandle::spawn(move |event| {
        let _ = tx.send(event);
    });
    let prepared = async {
        event(&mut events, |e| {
            matches!(e, WorkerEvent::HotkeyPhase(HotkeyPhase::Ready))
        })
        .await?;
        fill_queue_during_held_click(&worker, &observed, &mut events).await
    }
    .await;
    let closed_before = observed.lock().unwrap_or_else(|e| e.into_inner()).closed;
    let shutdown = tokio::task::spawn_blocking(move || worker.shutdown());
    if let Ok(release) = &prepared {
        release_held_click(&observed, release);
    }
    timeout(Duration::from_secs(4), shutdown).await??;
    prepared?;
    let rest: Vec<_> = std::iter::from_fn(|| events.try_recv().ok()).collect();
    if let Some(WorkerEvent::Error(error)) =
        rest.iter().find(|e| matches!(e, WorkerEvent::Error(_)))
    {
        return Err(error.clone().into());
    }
    ensure(
        run_states(&rest) == [RunState::Closing],
        &format!("Shutdown must end the run before any queued start: {rest:?}"),
    )?;
    ensure(
        observed.lock().unwrap_or_else(|e| e.into_inner()).closed > closed_before,
        "Shutdown must close the RemoteDesktop session",
    )?;
    service.close().await?;
    Ok(())
}

/// Count button presses the fake RemoteDesktop portal has received.
fn presses(observed: &Shared) -> usize {
    observed
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .buttons
        .iter()
        .filter(|(_, state)| *state == 1)
        .count()
}

fn countdowns(events: &[WorkerEvent]) -> Vec<u8> {
    events
        .iter()
        .filter_map(|e| match e {
            WorkerEvent::Countdown(remaining) => Some(*remaining),
            _ => None,
        })
        .collect()
}

fn waits_for_permission(events: &[WorkerEvent]) -> bool {
    events
        .iter()
        .any(|e| matches!(e, WorkerEvent::Status(status) if status.contains("Warte auf Freigabe für Maussteuerung")))
}

/// Drain queued events and fail if a cancelled countdown still ticked or started.
fn assert_no_late_start(events: &mut mpsc::UnboundedReceiver<WorkerEvent>) {
    while let Ok(event) = events.try_recv() {
        assert!(
            !matches!(
                event,
                WorkerEvent::State(RunState::Clicking) | WorkerEvent::Countdown(_)
            ),
            "a cancelled countdown must not continue: {event:?}"
        );
    }
}

#[tokio::test]
#[ignore = "requires a private D-Bus session via dbus-run-session"]
async fn button_countdown_delays_first_click_and_stop_cancels_it() -> TestResult {
    let _bus_guard = BUS_TEST_LOCK.lock().await;
    let observed = Shared::default();
    let service = zbus::connection::Builder::session()?
        .name("org.freedesktop.portal.Desktop")?
        .serve_at(PATH, FakeHotkeys(observed.clone()))?
        .serve_at(PATH, FakeRemote(observed.clone()))?
        .serve_at(PATH, FakeScreencast(observed.clone()))?
        .build()
        .await?;
    let settings = ClickSettings {
        interval_ms: 10,
        button: MouseButton::Left,
        click_type: ClickType::Single,
        repeat: Some(1),
        position: None,
        monitor: None,
    };
    let (tx, mut events) = mpsc::unbounded_channel();
    let worker = WorkerHandle::spawn(move |event| {
        let _ = tx.send(event);
    });
    let result: TestResult = async {
        event(&mut events, |e| matches!(e, WorkerEvent::Hotkey(_))).await?;
        let hotkey_session = observed
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .hotkey_session
            .clone();

        // The permission request comes first; the countdown starts only afterwards.
        worker.send(Command::StartFromButton(settings.clone()))?;
        let requested = events_until(&mut events, |e| matches!(e, WorkerEvent::Countdown(3))).await?;
        assert_eq!(run_states(&requested), [RunState::Starting]);
        assert!(waits_for_permission(&requested), "{requested:?}");
        assert_eq!(presses(&observed), 0);

        // The Stop button cancels the countdown without a late start.
        event(&mut events, |e| matches!(e, WorkerEvent::Countdown(2))).await?;
        worker.send(Command::Stop)?;
        let stopped = events_until(&mut events, |e| matches!(e, WorkerEvent::State(RunState::Stopped))).await?;
        assert_eq!(run_states(&stopped), [RunState::Stopped]);
        sleep(Duration::from_millis(2200)).await;
        assert_eq!(presses(&observed), 0);
        assert_no_late_start(&mut events);

        // The global shortcut also cancels a pending countdown. The retained
        // permission is reused, so the countdown follows the start directly.
        worker.send(Command::StartFromButton(settings.clone()))?;
        let reused = events_until(&mut events, |e| matches!(e, WorkerEvent::Countdown(3))).await?;
        assert_eq!(run_states(&reused), [RunState::Starting]);
        assert!(!waits_for_permission(&reused), "{reused:?}");
        let activation = (
            OwnedObjectPath::try_from(hotkey_session.as_str())?,
            "toggle-clicking",
            1_u64,
            Options::new(),
        );
        service
            .emit_signal(
                None::<&str>,
                PATH,
                "org.freedesktop.portal.GlobalShortcuts",
                "Activated",
                &activation,
            )
            .await?;
        let stopped = events_until(&mut events, |e| matches!(e, WorkerEvent::State(RunState::Stopped))).await?;
        assert_eq!(run_states(&stopped), [RunState::Stopped]);
        sleep(Duration::from_millis(3100)).await;
        assert_eq!(presses(&observed), 0);
        assert_no_late_start(&mut events);

        // The retained permission must not bypass the countdown: the first
        // click follows three seconds after the countdown began.
        worker.send(Command::StartFromButton(settings.clone()))?;
        event(&mut events, |e| matches!(e, WorkerEvent::Countdown(3))).await?;
        let began = std::time::Instant::now();
        sleep(Duration::from_millis(2700)).await;
        assert_eq!(presses(&observed), 0);
        timeout(Duration::from_secs(2), async {
            while presses(&observed) == 0 {
                sleep(Duration::from_millis(5)).await;
            }
        })
        .await?;
        let first_click = began.elapsed();
        assert!(
            (Duration::from_millis(2900)..Duration::from_millis(4000)).contains(&first_click),
            "first click after {first_click:?}"
        );
        let run = events_until(&mut events, |e| matches!(e, WorkerEvent::State(RunState::Stopped))).await?;
        assert_eq!(countdowns(&run), [2, 1]);
        assert_eq!(run_states(&run), [RunState::Clicking, RunState::Stopped]);
        assert_eq!(
            observed.lock().unwrap_or_else(|e| e.into_inner()).buttons,
            vec![(0x110, 1), (0x110, 0)]
        );

        // Hotkey-origin starts remain immediate when the permission is reused.
        worker.send(Command::Start(settings.clone()))?;
        let run = events_until(&mut events, |e| matches!(e, WorkerEvent::State(RunState::Stopped))).await?;
        assert!(countdowns(&run).is_empty(), "{run:?}");
        assert_eq!(run_states(&run), [RunState::Starting, RunState::Clicking, RunState::Stopped]);
        assert_eq!(presses(&observed), 2);

        // A fixed position moves the pointer itself, so a button start is immediate.
        let fixed = ClickSettings {
            position: Some((1919, 1079)),
            monitor: Some(MonitorGeometry { x: -1920, y: 0, width: 1920, height: 1080 }),
            ..settings.clone()
        };
        worker.send(Command::StartFromButton(fixed))?;
        let run = events_until(&mut events, |e| matches!(e, WorkerEvent::State(RunState::Stopped))).await?;
        assert!(countdowns(&run).is_empty(), "{run:?}");
        assert_eq!(run_states(&run), [RunState::Starting, RunState::Clicking, RunState::Stopped]);
        assert_eq!(presses(&observed), 3);
        assert_eq!(
            observed.lock().unwrap_or_else(|e| e.into_inner()).motions,
            vec![(42, 1919.0, 1079.0)]
        );

        // Revoking the permission during the countdown fails the start and clears its timer.
        worker.send(Command::StartFromButton(settings.clone()))?;
        event(&mut events, |e| matches!(e, WorkerEvent::Countdown(3))).await?;
        close_remote_session(&service, &observed).await?;
        let revoked = events_until(&mut events, |e| matches!(e, WorkerEvent::Error(message) if message.contains("Freigabe für die Maussteuerung"))).await?;
        assert_eq!(run_states(&revoked), [RunState::Stopped, RunState::Error]);
        sleep(Duration::from_millis(3200)).await;
        assert_eq!(presses(&observed), 3);
        assert_no_late_start(&mut events);

        // Losing the hotkey during the countdown also fails the start without a click.
        worker.send(Command::StartFromButton(settings.clone()))?;
        event(&mut events, |e| matches!(e, WorkerEvent::Countdown(3))).await?;
        shortcuts_changed(&service, &hotkey_session, "").await?;
        let removed = events_until(&mut events, |e| matches!(e, WorkerEvent::Error(message) if message.contains("entfernt"))).await?;
        assert_eq!(run_states(&removed), [RunState::Stopped, RunState::Error]);
        sleep(Duration::from_millis(3200)).await;
        assert_eq!(presses(&observed), 3);
        assert_no_late_start(&mut events);
        Ok(())
    }
    .await;
    tokio::task::spawn_blocking(move || worker.shutdown()).await?;
    result?;
    service.close().await?;
    Ok(())
}

fn hotkey_session(observed: &Shared) -> Result<OwnedObjectPath, Box<dyn std::error::Error>> {
    let session = observed
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .hotkey_session
        .clone();
    Ok(OwnedObjectPath::try_from(session)?)
}

/// Send `Activated` (key press) or `Deactivated` (release) for the toggle shortcut.
async fn shortcut_signal(
    service: &zbus::Connection,
    session: &OwnedObjectPath,
    signal: &str,
    timestamp: u64,
) -> TestResult {
    service
        .emit_signal(
            None::<&str>,
            PATH,
            "org.freedesktop.portal.GlobalShortcuts",
            signal,
            &(session, "toggle-clicking", timestamp, Options::new()),
        )
        .await?;
    Ok(())
}

/// Press and release the shortcut `count` times, numbering from `first`.
async fn key_presses(
    service: &zbus::Connection,
    session: &OwnedObjectPath,
    first: u64,
    count: u64,
) -> TestResult {
    for timestamp in first..first + count {
        shortcut_signal(service, session, "Activated", timestamp).await?;
        shortcut_signal(service, session, "Deactivated", timestamp).await?;
    }
    Ok(())
}

fn version_reads(observed: &Shared) -> usize {
    observed
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .version_reads
}

/// Give the worker's request after the end of a run time to be answered,
/// so a following key press is sent after that answer. Without such a
/// request this only waits, so the test also runs against older workers.
async fn wait_for_ordering_reply(observed: &Shared, reads_before: usize) {
    let _ = timeout(Duration::from_millis(500), async {
        while version_reads(observed) <= reads_before {
            sleep(Duration::from_millis(5)).await;
        }
    })
    .await;
    // The fake portal sends its reply right after the property getter.
    sleep(Duration::from_millis(50)).await;
}

fn start_requests(events: &[WorkerEvent]) -> usize {
    events
        .iter()
        .filter(|e| matches!(e, WorkerEvent::StartRequested(_)))
        .count()
}

/// Issue #18: key presses that the portal sent while the worker was held in
/// a click are still queued after Stop. None of them may start a new run,
/// whether the Stop button or the first of these presses stopped the run.
#[tokio::test]
#[ignore = "requires a private D-Bus session via dbus-run-session"]
async fn buffered_key_presses_before_stop_must_not_restart() -> TestResult {
    let mut failures = Vec::new();
    for stop_button in [true, false] {
        if let Err(error) =
            retry_if_click_window_missed(|| buffered_key_presses_once(stop_button)).await
        {
            let stop = if stop_button {
                "Stop button"
            } else {
                "key press"
            };
            failures.push(format!("Stop by {stop}: {error}"));
        }
    }
    ensure(failures.is_empty(), &failures.join("; "))
}

async fn buffered_key_presses_once(stop_button: bool) -> TestResult {
    let _bus_guard = BUS_TEST_LOCK.lock().await;
    let observed = Shared::default();
    let service = fake_portal(&observed).await?;
    let (tx, mut events) = mpsc::unbounded_channel();
    let worker = WorkerHandle::spawn(move |event| {
        let _ = tx.send(event);
    });
    let result: TestResult = async {
        event(&mut events, |e| {
            matches!(e, WorkerEvent::HotkeyPhase(HotkeyPhase::Ready))
        })
        .await?;
        let session = hotkey_session(&observed)?;
        let release = hold_first_click(&worker, &observed, &mut events).await?;
        // All 40 signals are sent before Stop, while the worker waits for the click.
        key_presses(&service, &session, 1, 20).await?;
        sleep(Duration::from_millis(20)).await;
        if stop_button {
            worker.send(Command::Stop)?;
        }
        release_held_click(&observed, &release);
        let stopped = events_until(&mut events, |e| {
            matches!(
                e,
                WorkerEvent::State(RunState::Stopped) | WorkerEvent::Error(_)
            )
        })
        .await?;
        if let Some(WorkerEvent::Error(error)) = stopped.last() {
            return Err(error.clone().into());
        }
        sleep(Duration::from_millis(200)).await;
        let later: Vec<_> = std::iter::from_fn(|| events.try_recv().ok()).collect();
        let stale_starts = start_requests(&later);
        if stale_starts > 0 {
            // Answer the request with the current controls, as Qt does.
            for event in later {
                if let WorkerEvent::StartRequested(request) = event {
                    request.start(continuous());
                }
            }
            event(&mut events, |e| {
                matches!(e, WorkerEvent::State(RunState::Clicking))
            })
            .await?;
            return Err(format!(
                "Restart confirmed after Stop; {stale_starts} start requests from old key presses"
            )
            .into());
        }
        ensure(
            run_states(&later).is_empty(),
            &format!("old key presses must not change the stopped run: {later:?}"),
        )
    }
    .await;
    tokio::task::spawn_blocking(move || worker.shutdown()).await?;
    service.close().await?;
    result
}

/// A new key press after Stop starts at once. Neither the first press after
/// Stop is swallowed nor is a release signal needed before it.
#[tokio::test]
#[ignore = "requires a private D-Bus session via dbus-run-session"]
async fn key_press_after_stop_starts_again() -> TestResult {
    let _bus_guard = BUS_TEST_LOCK.lock().await;
    let observed = Shared::default();
    let service = fake_portal(&observed).await?;
    let (tx, mut events) = mpsc::unbounded_channel();
    let worker = WorkerHandle::spawn(move |event| {
        let _ = tx.send(event);
    });
    let result: TestResult = async {
        event(&mut events, |e| {
            matches!(e, WorkerEvent::HotkeyPhase(HotkeyPhase::Ready))
        })
        .await?;
        let session = hotkey_session(&observed)?;
        // Start by key press and stop with the button while the key is held.
        shortcut_signal(&service, &session, "Activated", 1).await?;
        let request = start_request(&mut events).await?;
        request.start(continuous());
        event(&mut events, |e| {
            matches!(e, WorkerEvent::State(RunState::Clicking))
        })
        .await?;
        let reads = version_reads(&observed);
        worker.send(Command::Stop)?;
        event(&mut events, |e| {
            matches!(e, WorkerEvent::State(RunState::Stopped))
        })
        .await?;
        shortcut_signal(&service, &session, "Deactivated", 1).await?;
        wait_for_ordering_reply(&observed, reads).await;
        // The next press starts again.
        shortcut_signal(&service, &session, "Activated", 2).await?;
        let request = start_request(&mut events).await?;
        request.start(continuous());
        event(&mut events, |e| {
            matches!(e, WorkerEvent::State(RunState::Clicking))
        })
        .await?;
        // A press during the run stops it, and the very next press starts
        // again without a release signal in between.
        let reads = version_reads(&observed);
        shortcut_signal(&service, &session, "Activated", 3).await?;
        event(&mut events, |e| {
            matches!(e, WorkerEvent::State(RunState::Stopped))
        })
        .await?;
        wait_for_ordering_reply(&observed, reads).await;
        shortcut_signal(&service, &session, "Activated", 4).await?;
        let _request = start_request(&mut events).await?;
        Ok(())
    }
    .await;
    tokio::task::spawn_blocking(move || worker.shutdown()).await?;
    service.close().await?;
    result
}

/// The hotkey stays a reliable Stop: an old key press cannot start a run,
/// but it still stops one, here a start sent after the Stop that the
/// worker handles before the queued presses.
#[tokio::test]
#[ignore = "requires a private D-Bus session via dbus-run-session"]
async fn old_key_press_still_stops_a_newer_run() -> TestResult {
    retry_if_click_window_missed(old_key_press_stops_once).await
}

async fn old_key_press_stops_once() -> TestResult {
    let _bus_guard = BUS_TEST_LOCK.lock().await;
    let observed = Shared::default();
    let service = fake_portal(&observed).await?;
    let (tx, mut events) = mpsc::unbounded_channel();
    let worker = WorkerHandle::spawn(move |event| {
        let _ = tx.send(event);
    });
    let result: TestResult = async {
        event(&mut events, |e| {
            matches!(e, WorkerEvent::HotkeyPhase(HotkeyPhase::Ready))
        })
        .await?;
        let session = hotkey_session(&observed)?;
        let release = hold_first_click(&worker, &observed, &mut events).await?;
        key_presses(&service, &session, 1, 5).await?;
        sleep(Duration::from_millis(20)).await;
        worker.send(Command::Stop)?;
        worker.send(Command::Start(continuous()))?;
        release_held_click(&observed, &release);
        let restarted = events_until(&mut events, |e| {
            matches!(
                e,
                WorkerEvent::State(RunState::Clicking) | WorkerEvent::Error(_)
            )
        })
        .await?;
        if let Some(WorkerEvent::Error(error)) = restarted.last() {
            return Err(error.clone().into());
        }
        ensure(
            run_states(&restarted) == [RunState::Stopped, RunState::Starting, RunState::Clicking],
            &format!("the Stop and the later start must both apply: {restarted:?}"),
        )?;
        let stopped = timeout(
            Duration::from_secs(1),
            events_until(&mut events, |e| {
                matches!(e, WorkerEvent::State(RunState::Stopped))
            }),
        )
        .await
        .map_err(|_| "an old key press must still stop the newer run")??;
        sleep(Duration::from_millis(200)).await;
        let later: Vec<_> = std::iter::from_fn(|| events.try_recv().ok()).collect();
        ensure(
            run_states(&stopped) == [RunState::Stopped]
                && run_states(&later).is_empty()
                && start_requests(&stopped) + start_requests(&later) == 0,
            &format!("the remaining old presses must not restart: {stopped:?} {later:?}"),
        )
    }
    .await;
    tokio::task::spawn_blocking(move || worker.shutdown()).await?;
    service.close().await?;
    result
}

/// Hotkey changes and the end of the hotkey session that queue up behind
/// old key presses are still handled. 80 presses exceed the 64 signals the
/// D-Bus connection queues per subscription.
#[tokio::test]
#[ignore = "requires a private D-Bus session via dbus-run-session"]
async fn shortcut_change_and_closure_behind_old_key_presses_are_handled() -> TestResult {
    retry_if_click_window_missed(changes_behind_key_presses_once).await
}

async fn changes_behind_key_presses_once() -> TestResult {
    let _bus_guard = BUS_TEST_LOCK.lock().await;
    let observed = Shared::default();
    let service = fake_portal(&observed).await?;
    let (tx, mut events) = mpsc::unbounded_channel();
    let worker = WorkerHandle::spawn(move |event| {
        let _ = tx.send(event);
    });
    let result: TestResult = async {
        event(&mut events, |e| {
            matches!(e, WorkerEvent::HotkeyPhase(HotkeyPhase::Ready))
        })
        .await?;
        let session = hotkey_session(&observed)?;
        // A changed trigger between old presses, then the Stop button.
        let release = hold_first_click(&worker, &observed, &mut events).await?;
        key_presses(&service, &session, 1, 40).await?;
        shortcuts_changed(&service, session.as_str(), "F8").await?;
        key_presses(&service, &session, 41, 40).await?;
        worker.send(Command::Stop)?;
        release_held_click(&observed, &release);
        let changed = events_until(
            &mut events,
            |e| matches!(e, WorkerEvent::Hotkey(hotkey) if hotkey == "F8"),
        )
        .await?;
        sleep(Duration::from_millis(200)).await;
        let later: Vec<_> = std::iter::from_fn(|| events.try_recv().ok()).collect();
        ensure(
            run_states(&changed) == [RunState::Stopped]
                && run_states(&later).is_empty()
                && start_requests(&changed) + start_requests(&later) == 0,
            &format!("only the change may follow the old presses: {changed:?} {later:?}"),
        )?;

        // The end of the hotkey session behind old presses, without the button.
        let release = hold_first_click(&worker, &observed, &mut events).await?;
        key_presses(&service, &session, 81, 80).await?;
        service
            .emit_signal(
                None::<&str>,
                session.as_str(),
                "org.freedesktop.portal.Session",
                "Closed",
                &(Options::new(),),
            )
            .await?;
        release_held_click(&observed, &release);
        let closed = events_until(
            &mut events,
            |e| matches!(e, WorkerEvent::Error(message) if message.contains("Freigabe für das Tastenkürzel")),
        )
        .await?;
        sleep(Duration::from_millis(200)).await;
        let later: Vec<_> = std::iter::from_fn(|| events.try_recv().ok()).collect();
        ensure(
            closed
                .iter()
                .any(|e| matches!(e, WorkerEvent::HotkeyPhase(HotkeyPhase::Unavailable)))
                && run_states(&closed).first() == Some(&RunState::Stopped)
                && !run_states(&closed).contains(&RunState::Clicking)
                && run_states(&later).is_empty()
                && start_requests(&closed) + start_requests(&later) == 0,
            &format!("the session end must stop without a restart: {closed:?} {later:?}"),
        )
    }
    .await;
    tokio::task::spawn_blocking(move || worker.shutdown()).await?;
    service.close().await?;
    result
}

/// The barrier depends on D-Bus order on the hotkey connection itself:
/// presses that the portal sends before its reply stay old even when the
/// worker reads them only after that reply. Here the worker is held in
/// closing a revoked click session while the portal sends presses and only
/// then answers. A reply read on another connection has an unrelated
/// position and would let these presses start a run.
#[tokio::test]
#[ignore = "requires a private D-Bus session via dbus-run-session"]
async fn presses_sent_before_the_portal_reply_stay_old_when_read_later() -> TestResult {
    let _bus_guard = BUS_TEST_LOCK.lock().await;
    let observed = Shared::default();
    let service = fake_portal(&observed).await?;
    let (tx, mut events) = mpsc::unbounded_channel();
    let worker = WorkerHandle::spawn(move |event| {
        let _ = tx.send(event);
    });
    let result: TestResult = async {
        event(&mut events, |e| {
            matches!(e, WorkerEvent::HotkeyPhase(HotkeyPhase::Ready))
        })
        .await?;
        let session = hotkey_session(&observed)?;
        worker.send(Command::Start(continuous()))?;
        event(&mut events, |e| {
            matches!(e, WorkerEvent::State(RunState::Clicking))
        })
        .await?;
        let (version_seen, version_release) = {
            let mut state = observed.lock().unwrap_or_else(|e| e.into_inner());
            state.hold_version = true;
            (state.version_seen.clone(), state.version_release.clone())
        };
        worker.send(Command::Stop)?;
        event(&mut events, |e| {
            matches!(e, WorkerEvent::State(RunState::Stopped))
        })
        .await?;
        // The portal holds the worker's read after the Stop.
        timeout(Duration::from_secs(3), version_seen.notified()).await?;
        // Keep the worker busy closing the revoked click session.
        observed
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .stall_close = true;
        close_remote_session(&service, &observed).await?;
        timeout(Duration::from_secs(3), async {
            while observed
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .close_releases
                .is_empty()
            {
                sleep(Duration::from_millis(5)).await;
            }
        })
        .await?;
        // Presses first, then the reply, all while the worker is busy.
        key_presses(&service, &session, 1, 5).await?;
        observed
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .hold_version = false;
        version_release.notify_one();
        sleep(Duration::from_millis(50)).await;
        let close_releases = {
            let mut state = observed.lock().unwrap_or_else(|e| e.into_inner());
            state.stall_close = false;
            std::mem::take(&mut state.close_releases)
        };
        for close_release in close_releases {
            close_release.notify_one();
        }
        let closed = events_until(&mut events, |e| {
            matches!(e, WorkerEvent::Status(status) if status.contains("Freigabe erneut erforderlich"))
        })
        .await?;
        sleep(Duration::from_millis(200)).await;
        let later: Vec<_> = std::iter::from_fn(|| events.try_recv().ok()).collect();
        ensure(
            start_requests(&closed) + start_requests(&later) == 0
                && run_states(&closed).is_empty()
                && run_states(&later).is_empty(),
            &format!("presses sent before the portal reply must stay old: {closed:?} {later:?}"),
        )?;
        // The barrier has settled, so the next press starts.
        shortcut_signal(&service, &session, "Activated", 6).await?;
        start_request(&mut events).await.map(|_| ())
    }
    .await;
    tokio::task::spawn_blocking(move || worker.shutdown()).await?;
    service.close().await?;
    result
}

/// A run that ends by itself, by its last click or by a click error, also
/// sets the barrier: presses queued while that click was held stay old.
#[tokio::test]
#[ignore = "requires a private D-Bus session via dbus-run-session"]
async fn old_key_presses_after_last_click_or_error_do_not_restart() -> TestResult {
    let mut failures = Vec::new();
    for by_error in [false, true] {
        let mut result = presses_before_run_end_once(by_error).await;
        // Retry only if the presses could not be sent before the run ended.
        for _ in 1..3 {
            match &result {
                Err(error) if error.to_string().contains(RUN_ENDED_TOO_EARLY) => {
                    eprintln!("Run ended before the presses were sent, retrying: {error}");
                    result = presses_before_run_end_once(by_error).await;
                }
                _ => break,
            }
        }
        if let Err(error) = result {
            let end = if by_error {
                "click error"
            } else {
                "last click"
            };
            failures.push(format!("Run end by {end}: {error}"));
        }
    }
    ensure(failures.is_empty(), &failures.join("; "))
}

const RUN_ENDED_TOO_EARLY: &str = "the run ended before all presses were sent";

async fn presses_before_run_end_once(by_error: bool) -> TestResult {
    let _bus_guard = BUS_TEST_LOCK.lock().await;
    let observed = Shared::default();
    let service = fake_portal(&observed).await?;
    let (tx, mut events) = mpsc::unbounded_channel();
    let worker = WorkerHandle::spawn(move |event| {
        let _ = tx.send(event);
    });
    let result: TestResult = async {
        event(&mut events, |e| {
            matches!(e, WorkerEvent::HotkeyPhase(HotkeyPhase::Ready))
        })
        .await?;
        let session = hotkey_session(&observed)?;
        // One click finishes the run; a continuous run fails once the held
        // press outlives the worker's 250 ms event timeout.
        let settings = ClickSettings {
            repeat: (!by_error).then_some(1),
            ..continuous()
        };
        let release = hold_first_click_of(&worker, &observed, &mut events, settings).await?;
        key_presses(&service, &session, 1, 10).await?;
        let sent: Vec<_> = std::iter::from_fn(|| events.try_recv().ok()).collect();
        if !run_states(&sent).is_empty() {
            release_held_click(&observed, &release);
            return Err(format!("{RUN_ENDED_TOO_EARLY}: {sent:?}").into());
        }
        let end = if by_error {
            RunState::Error
        } else {
            release_held_click(&observed, &release);
            RunState::Stopped
        };
        let ended = timeout(
            Duration::from_secs(3),
            events_until(&mut events, |e| matches!(e, WorkerEvent::State(state) if *state == end)),
        )
        .await??;
        if by_error {
            release_held_click(&observed, &release);
        }
        sleep(Duration::from_millis(200)).await;
        let later: Vec<_> = std::iter::from_fn(|| events.try_recv().ok()).collect();
        let stale_starts = start_requests(&ended) + start_requests(&later);
        if stale_starts > 0 {
            // Answer the request with the current controls, as Qt does.
            for event in ended.into_iter().chain(later) {
                if let WorkerEvent::StartRequested(request) = event {
                    request.start(continuous());
                }
            }
            event(&mut events, |e| {
                matches!(e, WorkerEvent::State(RunState::Clicking))
            })
            .await?;
            return Err(format!(
                "Restart confirmed after the run ended; {stale_starts} start requests from old key presses"
            )
            .into());
        }
        ensure(
            run_states(&ended) == [end] && run_states(&later).is_empty(),
            &format!("only the end of the run may follow: {ended:?} {later:?}"),
        )
    }
    .await;
    tokio::task::spawn_blocking(move || worker.shutdown()).await?;
    service.close().await?;
    result
}

#[derive(Clone, Copy, Debug)]
enum GuiCancellation {
    Stop,
    SecondPress,
    Configure,
    Rebind,
    Remove,
    SessionClosed,
    Decline,
    InvalidSettings,
}

#[tokio::test]
#[ignore = "requires a private D-Bus session via dbus-run-session"]
async fn delayed_gui_reply_cannot_start_after_stop() -> TestResult {
    cancelled_gui_start_once(GuiCancellation::Stop).await
}

#[tokio::test]
#[ignore = "requires a private D-Bus session via dbus-run-session"]
async fn two_hotkey_presses_before_gui_reply_toggle_back_to_stopped() -> TestResult {
    cancelled_gui_start_once(GuiCancellation::SecondPress).await
}

#[tokio::test]
#[ignore = "requires a private D-Bus session via dbus-run-session"]
async fn hotkey_changes_revoke_delayed_gui_reply() -> TestResult {
    for cancellation in [
        GuiCancellation::Configure,
        GuiCancellation::Rebind,
        GuiCancellation::Remove,
        GuiCancellation::SessionClosed,
    ] {
        cancelled_gui_start_once(cancellation)
            .await
            .map_err(|error| format!("{cancellation:?}: {error}"))?;
    }
    Ok(())
}

#[tokio::test]
#[ignore = "requires a private D-Bus session via dbus-run-session"]
async fn declined_or_invalid_gui_reply_does_not_latch_starting() -> TestResult {
    for cancellation in [GuiCancellation::Decline, GuiCancellation::InvalidSettings] {
        cancelled_gui_start_once(cancellation)
            .await
            .map_err(|error| format!("{cancellation:?}: {error}"))?;
    }
    Ok(())
}

/// Keep the first Qt reply until after cancellation, then answer it while a
/// different activation is pending. It must neither click nor consume that start.
async fn cancelled_gui_start_once(cancellation: GuiCancellation) -> TestResult {
    let _bus_guard = BUS_TEST_LOCK.lock().await;
    let observed = Shared::default();
    let service = fake_portal(&observed).await?;
    let (tx, mut events) = mpsc::unbounded_channel();
    let worker = WorkerHandle::spawn(move |event| {
        let _ = tx.send(event);
    });
    let result: TestResult = async {
        event(&mut events, |e| {
            matches!(e, WorkerEvent::HotkeyPhase(HotkeyPhase::Ready))
        })
        .await?;
        let session = hotkey_session(&observed)?;
        shortcut_signal(&service, &session, "Activated", 1).await?;
        let mut emitted =
            events_until(&mut events, |e| matches!(e, WorkerEvent::StartRequested(_))).await?;
        ensure(
            run_states(&emitted) == [RunState::Starting],
            "the worker must reserve Starting before requesting Qt settings",
        )?;
        let mut delayed = match emitted.pop() {
            Some(WorkerEvent::StartRequested(request)) => Some(request),
            _ => return Err("missing GUI request".into()),
        };
        let reads = version_reads(&observed);
        match cancellation {
            GuiCancellation::Stop => worker.send(Command::Stop)?,
            GuiCancellation::SecondPress => {
                shortcut_signal(&service, &session, "Activated", 2).await?;
            }
            GuiCancellation::Configure => worker.send(Command::ConfigureHotkey)?,
            GuiCancellation::Rebind => shortcuts_changed(&service, session.as_str(), "F8").await?,
            GuiCancellation::Remove => shortcuts_changed(&service, session.as_str(), "").await?,
            GuiCancellation::SessionClosed => {
                service
                    .emit_signal(
                        None::<&str>,
                        &session,
                        "org.freedesktop.portal.Session",
                        "Closed",
                        &(Options::new(),),
                    )
                    .await?;
            }
            // The picker, invalid/unavailable Qt controls, or a failed Qt queue
            // all decline by dropping the single-use reply.
            GuiCancellation::Decline => drop(delayed.take()),
            GuiCancellation::InvalidSettings => {
                let request = delayed.take().ok_or("missing GUI request")?;
                request.start(ClickSettings {
                    interval_ms: 0,
                    ..continuous()
                });
            }
        }
        let ended = events_until(&mut events, |e| {
            matches!(e, WorkerEvent::State(RunState::Stopped | RunState::Error))
        })
        .await?;
        ensure(
            start_requests(&ended) == 0,
            "a second press must stop, not request another start",
        )?;
        match cancellation {
            GuiCancellation::Configure => {
                event(&mut events, |e| {
                    matches!(e, WorkerEvent::HotkeyPhase(HotkeyPhase::Configuring(true)))
                })
                .await?;
                let release = observed
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .configure_release
                    .clone();
                release.notify_one();
                event(&mut events, |e| {
                    matches!(e, WorkerEvent::HotkeyPhase(HotkeyPhase::Ready))
                })
                .await?;
            }
            GuiCancellation::Remove
            | GuiCancellation::SessionClosed
            | GuiCancellation::InvalidSettings => {
                events_until(&mut events, |e| matches!(e, WorkerEvent::Error(_))).await?;
            }
            _ => {}
        }
        wait_for_ordering_reply(&observed, reads).await;
        if let Some(request) = &delayed {
            ensure(
                request.is_cancelled(),
                "the pending GUI reply must be revoked",
            )?;
        }
        match cancellation {
            GuiCancellation::Remove => {
                shortcuts_changed(&service, session.as_str(), "Pause").await?;
                event(&mut events, |e| {
                    matches!(e, WorkerEvent::HotkeyPhase(HotkeyPhase::Ready))
                })
                .await?;
            }
            GuiCancellation::SessionClosed => {
                worker.send(Command::ConfigureHotkey)?;
                event(&mut events, |e| {
                    matches!(e, WorkerEvent::HotkeyPhase(HotkeyPhase::Ready))
                })
                .await?;
            }
            _ => {}
        }
        let session = hotkey_session(&observed)?;
        shortcut_signal(&service, &session, "Activated", 3).await?;
        let fresh = start_request(&mut events).await?;
        if let Some(delayed) = delayed {
            delayed.start(continuous());
        }
        // Even when delivered during the next reservation, an old reply does
        // not open a permission request or create pointer input.
        sleep(Duration::from_millis(100)).await;
        {
            let state = observed.lock().unwrap_or_else(|e| e.into_inner());
            ensure(
                state.remote_session.is_none() && state.buttons.is_empty(),
                "a cancelled GUI reply started mouse control",
            )?;
        }
        ensure(
            !fresh.is_cancelled(),
            "an old reply consumed the fresh reservation",
        )?;
        let skipped: Vec<_> = std::iter::from_fn(|| events.try_recv().ok()).collect();
        ensure(
            run_states(&skipped).is_empty() && start_requests(&skipped) == 0,
            &format!("unexpected events before fresh reply: {skipped:?}"),
        )?;
        // These current settings differ from the cancelled request. No cached
        // settings and no button countdown may replace the Qt reply.
        fresh.start(ClickSettings {
            button: MouseButton::Right,
            repeat: Some(2),
            ..continuous()
        });
        event(&mut events, |e| {
            matches!(e, WorkerEvent::State(RunState::Clicking))
        })
        .await?;
        event(&mut events, |e| {
            matches!(e, WorkerEvent::State(RunState::Stopped))
        })
        .await?;
        let state = observed.lock().unwrap_or_else(|e| e.into_inner());
        ensure(
            state.buttons == [(273, 1), (273, 0), (273, 1), (273, 0)],
            "fresh activation did not use its own current Qt settings",
        )
    }
    .await;
    tokio::task::spawn_blocking(move || worker.shutdown()).await?;
    service.close().await?;
    result
}

#[tokio::test]
#[ignore = "requires a private D-Bus session via dbus-run-session"]
async fn rejected_device_or_source_selection_does_not_start_mouse_control() -> TestResult {
    for (sources, fixed) in [(false, false), (false, true), (true, true)] {
        for status in [1, 2] {
            rejected_selection_once(sources, fixed, status).await?;
        }
    }
    Ok(())
}

async fn rejected_selection_once(sources: bool, fixed: bool, status: u32) -> TestResult {
    let _serial = BUS_TEST_LOCK.lock().await;
    let observed = Shared::default();
    {
        let mut state = observed.lock().unwrap_or_else(|e| e.into_inner());
        if sources {
            state.select_sources_response = status;
        } else {
            state.select_devices_response = status;
        }
    }
    let service = fake_portal(&observed).await?;
    let (tx, mut events) = mpsc::unbounded_channel();
    let worker = WorkerHandle::spawn(move |event| {
        let _ = tx.send(event);
    });
    let result: TestResult = async {
        event(&mut events, |e| matches!(e, WorkerEvent::HotkeyPhase(HotkeyPhase::Ready))).await?;
        let settings = ClickSettings {
            repeat: Some(1),
            position: fixed.then_some((80, 150)),
            monitor: fixed.then_some(MonitorGeometry { x: -1920, y: 0, width: 1920, height: 1080 }),
            ..continuous()
        };
        worker.send(Command::Start(settings.clone()))?;
        let ended = events_until(&mut events, |e| matches!(e, WorkerEvent::State(RunState::Error | RunState::Stopped))).await?;
        ensure(
            run_states(&ended) == [RunState::Starting, RunState::Error],
            &format!("rejected selection must fail before clicking: {ended:?}"),
        )?;
        let errors = events_until(&mut events, |e| matches!(e, WorkerEvent::Error(_))).await?;
        ensure(errors.iter().any(|e| matches!(e, WorkerEvent::Error(message) if message.contains("abgelehnt oder abgebrochen"))), "selection rejection must be reported")?;
        {
            let mut state = observed.lock().unwrap_or_else(|e| e.into_inner());
            ensure(state.remote_start_calls == 0, "Start must not follow a rejected selection")?;
            ensure(state.select_sources_calls == usize::from(sources), "device rejection must also skip source selection")?;
            ensure(state.buttons.is_empty() && state.motions.is_empty(), "rejected selection must not inject input")?;
            ensure(state.closed == 1, "the rejected session must be closed")?;
            state.select_devices_response = 0;
            state.select_sources_response = 0;
        }
        worker.send(Command::Start(settings))?;
        event(&mut events, |e| matches!(e, WorkerEvent::State(RunState::Stopped))).await?;
        let state = observed.lock().unwrap_or_else(|e| e.into_inner());
        ensure(state.remote_start_calls == 1 && state.buttons.len() == 2, "retry after rejection must create a new session and click once")
    }.await;
    tokio::task::spawn_blocking(move || worker.shutdown()).await?;
    service.close().await?;
    result
}
