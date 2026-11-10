//! Fake desktop portals on the private bus, and what they observed.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Duration,
};

use ashpd::zbus::{
    self,
    zvariant::{OwnedObjectPath, OwnedValue, Value},
};
use tokio::{sync::Notify, time::sleep};

use super::fake_eis::FakeEis;
use super::support::{ensure, key_presses, shortcuts_changed};

pub(crate) type Options = HashMap<String, OwnedValue>;

pub(crate) const PATH: &str = "/org/freedesktop/portal/desktop";

#[derive(Clone)]
pub(crate) struct FakeMonitor {
    pub(crate) node_id: u32,
    pub(crate) position: Option<(i32, i32)>,
    pub(crate) size: Option<(i32, i32)>,
}

#[derive(Default)]
pub(crate) struct Observed {
    pub(crate) registered_apps: HashMap<String, String>,
    pub(crate) require_registration: bool,
    pub(crate) reject_registration: bool,
    pub(crate) hotkey_session: String,
    pub(crate) hotkey_created: usize,
    /// `preferred_trigger` of every BindShortcuts call.
    pub(crate) preferred_triggers: Vec<Option<String>>,
    /// Method and `parent_window` of every request that can show a dialog.
    pub(crate) parents: Vec<(&'static str, String)>,
    pub(crate) reject_hotkey: bool,
    pub(crate) empty_hotkey_trigger: bool,
    pub(crate) hotkey_version: Option<u32>,
    pub(crate) remote_session: Option<OwnedObjectPath>,
    pub(crate) revoked_session: Option<OwnedObjectPath>,
    pub(crate) buttons: Vec<(i32, u32)>,
    pub(crate) button_failures: [usize; 2],
    pub(crate) closed: usize,
    pub(crate) stall_close: bool,
    pub(crate) close_releases: Vec<Arc<Notify>>,
    pub(crate) remote_owner: String,
    pub(crate) hotkey_owner: String,
    pub(crate) fixed_stream: bool,
    pub(crate) multiple_sources: bool,
    pub(crate) multi_monitor_layout: bool,
    pub(crate) reverse_monitor_streams: bool,
    pub(crate) monitor_streams: Option<Vec<FakeMonitor>>,
    pub(crate) motions: Vec<(u32, f64, f64)>,
    pub(crate) select_devices_response: u32,
    pub(crate) select_sources_response: u32,
    pub(crate) select_sources_calls: usize,
    pub(crate) remote_start_calls: usize,
    pub(crate) delay_hotkey: bool,
    /// Never send the Response to a hotkey CreateSession request.
    pub(crate) hold_hotkey_session: bool,
    pub(crate) close_after_hotkey_response: bool,
    /// Remove the shortcut with ShortcutsChanged right before or after the
    /// BindShortcuts response (`Some(true)` = after).
    pub(crate) remove_around_hotkey_response: Option<bool>,
    /// Press the shortcut right before the BindShortcuts response.
    pub(crate) press_before_hotkey_response: bool,
    pub(crate) hotkey_seen: Arc<Notify>,
    pub(crate) hotkey_release: Arc<Notify>,
    pub(crate) hotkey_responded: Arc<Notify>,
    pub(crate) configure_calls: usize,
    pub(crate) configure_seen: Arc<Notify>,
    pub(crate) configure_release: Arc<Notify>,
    pub(crate) delay_start: bool,
    pub(crate) start_seen: Arc<Notify>,
    pub(crate) start_release: Arc<Notify>,
    pub(crate) delay_button: bool,
    /// Answer this many next presses only after `press_delay`.
    pub(crate) slow_presses: usize,
    pub(crate) press_delay: Duration,
    pub(crate) press_times: Vec<std::time::Instant>,
    /// When each slow press reply was sent.
    pub(crate) slow_press_ends: Vec<std::time::Instant>,
    pub(crate) button_seen: Arc<Notify>,
    pub(crate) button_release: Arc<Notify>,
    pub(crate) version_reads: usize,
    pub(crate) hold_version: bool,
    pub(crate) version_seen: Arc<Notify>,
    pub(crate) version_release: Arc<Notify>,
    /// Answer ConnectToEIS with this server; without one the call fails.
    pub(crate) eis: Option<Arc<FakeEis>>,
    pub(crate) notify_calls: usize,
}

pub(crate) type Shared = Arc<Mutex<Observed>>;

/// How the fake portal holds a press reply.
enum Hold {
    /// Until the test releases it.
    Until(Arc<Notify>, Arc<Notify>),
    /// For a fixed time, like a slow portal.
    For(Duration),
}

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

pub(crate) struct FakeHotkeys(pub(crate) Shared);

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
        parent: String,
        options: Options,
        #[zbus(connection)] connection: &zbus::Connection,
        #[zbus(header)] header: zbus::message::Header<'_>,
    ) -> zbus::fdo::Result<OwnedObjectPath> {
        {
            let mut state = self.0.lock().unwrap_or_else(|e| e.into_inner());
            state.parents.push(("BindShortcuts", parent));
            state
                .preferred_triggers
                .extend(shortcuts.iter().map(|(_, shortcut)| {
                    shortcut
                        .get("preferred_trigger")
                        .and_then(|value| <&str>::try_from(value).ok())
                        .map(str::to_owned)
                }));
        }
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
        let remove = self
            .0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove_around_hotkey_response;
        if remove == Some(false) {
            shortcuts_changed(connection, _session.as_str(), "")
                .await
                .map_err(failed)?;
        }
        let press = self
            .0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .press_before_hotkey_response;
        if press {
            key_presses(connection, &_session, 1, 1)
                .await
                .map_err(failed)?;
        }
        let response = respond(connection, &header, &options, results).await;
        if remove == Some(true) {
            shortcuts_changed(connection, _session.as_str(), "")
                .await
                .map_err(failed)?;
        }
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
        parent: String,
        _options: Options,
    ) {
        let (seen, release) = {
            let mut state = self.0.lock().unwrap_or_else(|e| e.into_inner());
            state.configure_calls += 1;
            state.parents.push(("ConfigureShortcuts", parent));
            (
                state.configure_seen.clone(),
                state.configure_release.clone(),
            )
        };
        seen.notify_one();
        release.notified().await;
    }
}

pub(crate) struct FakeRemote(pub(crate) Shared);

#[zbus::interface(name = "org.freedesktop.portal.RemoteDesktop", crate = "ashpd::zbus")]
impl FakeRemote {
    // Version 2 offers ConnectToEIS.
    #[zbus(property, name = "version")]
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
        parent: String,
        options: Options,
        #[zbus(connection)] connection: &zbus::Connection,
        #[zbus(header)] header: zbus::message::Header<'_>,
    ) -> zbus::fdo::Result<OwnedObjectPath> {
        let (fixed, multiple, multi_layout, reverse, override_streams, delay, seen, release) = {
            let mut state = self.0.lock().unwrap_or_else(|e| e.into_inner());
            state.remote_start_calls += 1;
            state.parents.push(("Start", parent));
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
                    ("mapping_id", Value::new("DP-1")),
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
    #[zbus(name = "ConnectToEIS")]
    async fn connect_to_eis(
        &self,
        _session: OwnedObjectPath,
        _options: Options,
    ) -> zbus::fdo::Result<zbus::zvariant::OwnedFd> {
        let eis = self.0.lock().unwrap_or_else(|e| e.into_inner()).eis.clone();
        let eis = eis.ok_or_else(|| zbus::fdo::Error::NotSupported("no EIS".into()))?;
        Ok(eis.connect().map_err(failed)?.into())
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
            observed.notify_calls += 1;
            if observed.revoked_session.as_ref() == Some(&session) {
                return Err(failed("session revoked"));
            }
            observed.buttons.push((button, state));
            if observed.button_failures[state as usize] > 0 {
                observed.button_failures[state as usize] -= 1;
                return Err(failed("simulated button failure"));
            }
            if state == 1 {
                observed.press_times.push(std::time::Instant::now());
            }
            if state == 1 && observed.slow_presses > 0 {
                observed.slow_presses -= 1;
                Some(Hold::For(observed.press_delay))
            } else {
                (state == 1 && observed.delay_button).then(|| {
                    Hold::Until(
                        observed.button_seen.clone(),
                        observed.button_release.clone(),
                    )
                })
            }
        };
        // Hold the press reply, so the worker stays inside this click.
        match delay {
            Some(Hold::Until(seen, release)) => {
                seen.notify_one();
                release.notified().await;
            }
            Some(Hold::For(duration)) => {
                sleep(duration).await;
                self.0
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .slow_press_ends
                    .push(std::time::Instant::now());
            }
            None => {}
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
        let mut observed = self.0.lock().unwrap_or_else(|e| e.into_inner());
        observed.notify_calls += 1;
        observed.motions.push((stream, x, y));
    }
}

pub(crate) struct FakeScreencast(pub(crate) Shared);

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

/// A fake portal with hotkeys, mouse control and screen casts but without
/// the app registry.
pub(crate) async fn hotkey_portal(observed: &Shared) -> zbus::Result<zbus::Connection> {
    zbus::connection::Builder::session()?
        .name("org.freedesktop.portal.Desktop")?
        .serve_at(PATH, FakeHotkeys(observed.clone()))?
        .serve_at(PATH, FakeRemote(observed.clone()))?
        .serve_at(PATH, FakeScreencast(observed.clone()))?
        .build()
        .await
}

pub(crate) async fn fake_portal(observed: &Shared) -> zbus::Result<zbus::Connection> {
    fake_portal_services(observed)?
        .name("org.freedesktop.portal.Desktop")?
        .build()
        .await
}

/// A fake portal that waits for the name behind its current owner and owns
/// it as soon as that connection leaves the bus, like a restarted portal.
pub(crate) async fn queued_fake_portal(
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
