use std::{path::Path, pin::Pin};

use crate::{
    config::{self, AppConfig},
    model::{
        ClickSettings, ClickType, MAX_REPEAT_COUNT, MonitorGeometry, MouseButton, PositionMode,
        RepeatMode, validate_interval,
    },
    state::RunState,
    worker::{Command, HotkeyPhase, WorkerEvent, WorkerHandle},
};
use cxx_qt::{CxxQtType, Threading};
use cxx_qt_lib::QString;

#[cxx_qt::bridge]
pub mod qobject {
    unsafe extern "C++" {
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;
    }

    extern "RustQt" {
        #[qobject]
        #[qml_element]
        #[qproperty(QString, status)]
        #[qproperty(QString, error_message)]
        #[qproperty(QString, hotkey)]
        #[qproperty(bool, hotkey_ready)]
        #[qproperty(bool, hotkey_pending)]
        #[qproperty(bool, hotkey_configuring)]
        #[qproperty(bool, running)]
        #[qproperty(bool, busy)]
        #[qproperty(i32, countdown_remaining)]
        #[qproperty(i64, interval_ms)]
        #[qproperty(i32, mouse_button)]
        #[qproperty(i32, click_type)]
        #[qproperty(bool, repeat_until_stopped)]
        #[qproperty(i64, repeat_count)]
        #[qproperty(bool, current_position)]
        #[qproperty(i64, fixed_x)]
        #[qproperty(i64, fixed_y)]
        #[qproperty(QString, monitor_identity)]
        #[qproperty(bool, fixed_position_confirmed)]
        #[qproperty(i32, monitor_x)]
        #[qproperty(i32, monitor_y)]
        #[qproperty(i32, monitor_width)]
        #[qproperty(i32, monitor_height)]
        #[qproperty(bool, selecting_position)]
        type AppController = super::AppControllerRust;

        #[qinvokable]
        fn initialize(self: Pin<&mut AppController>);

        #[qinvokable]
        fn start(self: Pin<&mut AppController>);

        #[qinvokable]
        fn start_from_button(self: Pin<&mut AppController>);

        #[qinvokable]
        fn stop(self: Pin<&mut AppController>);

        #[qinvokable]
        fn configure_hotkey(self: Pin<&mut AppController>);

        #[qinvokable]
        fn clear_error(self: Pin<&mut AppController>);

        #[qinvokable]
        fn save_config(self: Pin<&mut AppController>) -> bool;

        #[qinvokable]
        fn mark_settings_changed(self: Pin<&mut AppController>);

        #[qinvokable]
        fn mark_position_changed(self: Pin<&mut AppController>);

        #[qinvokable]
        fn restore_saved_position(self: Pin<&mut AppController>) -> bool;

        #[qinvokable]
        fn shutdown(self: Pin<&mut AppController>);
    }

    impl cxx_qt::Threading for AppController {}
}

pub struct AppControllerRust {
    status: QString,
    error_message: QString,
    hotkey: QString,
    hotkey_ready: bool,
    hotkey_pending: bool,
    hotkey_configuring: bool,
    running: bool,
    busy: bool,
    countdown_remaining: i32,
    interval_ms: i64,
    mouse_button: i32,
    click_type: i32,
    repeat_until_stopped: bool,
    repeat_count: i64,
    current_position: bool,
    fixed_x: i64,
    fixed_y: i64,
    monitor_identity: QString,
    fixed_position_confirmed: bool,
    monitor_x: i32,
    monitor_y: i32,
    monitor_width: i32,
    monitor_height: i32,
    selecting_position: bool,
    worker: Option<WorkerHandle>,
    worker_epoch: u64,
    startup_error: Option<String>,
    initial_config: AppConfig,
    saved_position: SavedPosition,
    config_dirty: bool,
}

/// Fixed position that is saved while the displayed position is unconfirmed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct SavedPosition {
    fixed_x: u32,
    fixed_y: u32,
    monitor_identity: String,
}

impl SavedPosition {
    fn new(fixed_x: i64, fixed_y: i64, monitor_identity: String) -> Option<Self> {
        let coordinate = |value| u32::try_from(value).ok().filter(|value| *value <= 100_000);
        Some(Self {
            fixed_x: coordinate(fixed_x)?,
            fixed_y: coordinate(fixed_y)?,
            monitor_identity,
        })
    }

    /// Out-of-range coordinates can never be restored, so they are dropped.
    fn from_config(config: &AppConfig) -> Self {
        Self::new(
            config.fixed_x.into(),
            config.fixed_y.into(),
            config.monitor_identity.clone(),
        )
        .unwrap_or_default()
    }
}

impl Default for AppControllerRust {
    /// Load saved controls while requiring fixed-position monitor restoration.
    fn default() -> Self {
        let (config, startup_error) = match config::load() {
            Ok(config) => (config, None),
            Err(error) => (AppConfig::default(), Some(error.to_string())),
        };
        Self::from_config(config, startup_error)
    }
}

impl AppControllerRust {
    fn from_config(config: AppConfig, startup_error: Option<String>) -> Self {
        Self {
            status: QString::from("Bereit"),
            error_message: QString::default(),
            hotkey: QString::from(&config.hotkey),
            hotkey_ready: false,
            hotkey_pending: true,
            hotkey_configuring: false,
            running: false,
            busy: false,
            countdown_remaining: 0,
            interval_ms: i64::try_from(config.interval_ms).unwrap_or(100),
            mouse_button: match config.mouse_button {
                MouseButton::Left => 0,
                MouseButton::Right => 1,
                MouseButton::Middle => 2,
            },
            click_type: match config.click_type {
                ClickType::Single => 0,
                ClickType::Double => 1,
            },
            repeat_until_stopped: config.repeat_mode == RepeatMode::UntilStopped,
            repeat_count: i64::try_from(config.repeat_count).unwrap_or(100),
            current_position: config.position_mode == PositionMode::CurrentCursor,
            fixed_x: i64::from(config.fixed_x),
            fixed_y: i64::from(config.fixed_y),
            monitor_identity: QString::from(&config.monitor_identity),
            fixed_position_confirmed: false,
            monitor_x: 0,
            monitor_y: 0,
            monitor_width: 0,
            monitor_height: 0,
            selecting_position: false,
            worker: None,
            worker_epoch: 0,
            startup_error,
            saved_position: SavedPosition::from_config(&config),
            initial_config: config,
            config_dirty: false,
        }
    }

    /// A draft needs valid values but does not need an approved monitor yet.
    fn config_snapshot(&self) -> Result<AppConfig, String> {
        let interval_ms = u64::try_from(self.interval_ms)
            .map_err(|_| "Wähle ein Klickintervall größer als 0 ms.".to_owned())?;
        validate_interval(interval_ms).map_err(|error| error.to_string())?;
        let repeat_count = match u64::try_from(self.repeat_count) {
            Ok(count) if (1..=MAX_REPEAT_COUNT).contains(&count) => count,
            _ if self.repeat_until_stopped => 100,
            _ => return Err("Die Anzahl der Klickzyklen ist ungültig. Prüfe den Wert.".to_owned()),
        };
        let position = if !self.fixed_position_confirmed {
            // A missing saved monitor makes the UI clamp coordinates to a
            // substitute monitor. Keep the chosen position until it is replaced.
            self.saved_position.clone()
        } else if let Some(position) = SavedPosition::new(
            self.fixed_x,
            self.fixed_y,
            self.monitor_identity.to_string(),
        ) {
            position
        } else if self.current_position {
            SavedPosition::default()
        } else {
            return Err("Die Koordinaten sind ungültig. Wähle eine neue Position.".to_owned());
        };
        let mouse_button = match self.mouse_button {
            0 => MouseButton::Left,
            1 => MouseButton::Right,
            2 => MouseButton::Middle,
            _ => return Err("Wähle eine gültige Maustaste.".to_owned()),
        };
        let click_type = match self.click_type {
            0 => ClickType::Single,
            1 => ClickType::Double,
            _ => return Err("Wähle eine gültige Klickart.".to_owned()),
        };
        Ok(AppConfig {
            interval_ms,
            mouse_button,
            click_type,
            repeat_mode: if self.repeat_until_stopped {
                RepeatMode::UntilStopped
            } else {
                RepeatMode::Count
            },
            repeat_count,
            position_mode: if self.current_position {
                PositionMode::CurrentCursor
            } else {
                PositionMode::Fixed
            },
            fixed_x: position.fixed_x,
            fixed_y: position.fixed_y,
            hotkey: self.hotkey.to_string(),
            monitor_identity: position.monitor_identity,
        })
    }

    /// Adopt a deliberately edited position; unconfirmed edits lose the monitor.
    fn record_position_change(&mut self) {
        let monitor_identity = if self.fixed_position_confirmed {
            self.monitor_identity.to_string()
        } else {
            String::new()
        };
        self.saved_position =
            SavedPosition::new(self.fixed_x, self.fixed_y, monitor_identity).unwrap_or_default();
        self.config_dirty = true;
    }

    /// Saved coordinates for the live monitor while they await confirmation.
    fn saved_coordinates_for_live_monitor(&self) -> Option<(i64, i64)> {
        let saved = &self.saved_position;
        (!self.fixed_position_confirmed
            && !saved.monitor_identity.is_empty()
            && saved.monitor_identity == self.monitor_identity.to_string())
        .then(|| (saved.fixed_x.into(), saved.fixed_y.into()))
    }

    /// Begin a new worker generation so that events of the previous one are dropped.
    fn next_worker_epoch(&mut self) -> u64 {
        self.worker_epoch = self.worker_epoch.wrapping_add(1);
        self.worker_epoch
    }

    /// Pass on only events of the current worker. Late events of a shut-down
    /// worker, including its final State(Closing), are dropped. A changed
    /// hotkey description is a setting that must be saved on close.
    fn accept_worker_event(
        &mut self,
        worker_epoch: u64,
        event: WorkerEvent,
    ) -> Option<WorkerEvent> {
        if worker_epoch != self.worker_epoch {
            return None;
        }
        if let WorkerEvent::Hotkey(hotkey) = &event {
            if self.hotkey.to_string() == *hotkey {
                return None;
            }
            self.config_dirty = true;
        }
        Some(event)
    }

    fn persist_config(&mut self, force: bool) -> Result<(), String> {
        if !force && !self.config_dirty {
            return Ok(());
        }
        let path = config::config_path().map_err(|error| error.to_string())?;
        self.persist_config_to(&path, force)
    }

    fn persist_config_to(&mut self, path: &Path, force: bool) -> Result<(), String> {
        if !force && !self.config_dirty {
            return Ok(());
        }
        let config = self.config_snapshot()?;
        if config != self.initial_config {
            config::save_to(path, &config).map_err(|error| error.to_string())?;
        }
        self.saved_position = SavedPosition::from_config(&config);
        self.initial_config = config;
        self.config_dirty = false;
        Ok(())
    }
}

impl Drop for AppControllerRust {
    /// Shut down the worker before destroying the Qt controller.
    fn drop(&mut self) {
        if let Some(worker) = self.worker.take() {
            worker.shutdown();
        }
    }
}

impl qobject::AppController {
    /// Start the worker and hotkey even if saved coordinates need confirmation.
    pub fn initialize(mut self: Pin<&mut Self>) {
        if self.rust().worker.is_some() {
            return;
        }
        self.as_mut().set_hotkey_ready(false);
        self.as_mut().set_hotkey_pending(true);
        self.as_mut().set_hotkey_configuring(false);
        if let Some(error) = self.as_mut().rust_mut().get_mut().startup_error.take() {
            self.as_mut().set_error_message(QString::from(&error));
        }
        let worker_epoch = self.as_mut().rust_mut().get_mut().next_worker_epoch();
        let qt_thread = self.qt_thread();
        let worker = WorkerHandle::spawn(move |event| {
            let _ = qt_thread.queue(move |mut controller| {
                controller.as_mut().handle_worker_event(worker_epoch, event);
            });
        });
        self.as_mut().rust_mut().get_mut().worker = Some(worker);
    }

    /// Validate and save current controls before requesting a click run.
    pub fn start(mut self: Pin<&mut Self>) {
        self.as_mut().start_with_origin(false);
    }

    /// Give the user time to move the pointer after using the Start button.
    pub fn start_from_button(mut self: Pin<&mut Self>) {
        self.as_mut().start_with_origin(true);
    }

    fn start_with_origin(mut self: Pin<&mut Self>, from_button: bool) {
        let Some(settings) = self.as_mut().collect_start_settings() else {
            return;
        };
        let command = if from_button {
            Command::StartFromButton(settings)
        } else {
            Command::Start(settings)
        };
        self.as_mut().send_command(command);
    }

    /// Both start paths read and validate the live controls only on the Qt thread.
    fn collect_start_settings(mut self: Pin<&mut Self>) -> Option<ClickSettings> {
        if *self.selecting_position() {
            return None;
        }
        self.as_mut().clear_error();
        let settings = match self.as_ref().settings() {
            Ok(settings) => settings,
            Err(error) => {
                self.as_mut().show_error(&error);
                return None;
            }
        };
        self.as_mut().persist_config(true);
        Some(settings)
    }

    /// Stop a run or an outstanding start request.
    pub fn stop(mut self: Pin<&mut Self>) {
        self.as_mut().send_command(Command::Stop);
    }

    /// Open the desktop portal configuration for the global shortcut.
    pub fn configure_hotkey(mut self: Pin<&mut Self>) {
        if *self.hotkey_pending() {
            return;
        }
        if !*self.hotkey_ready() {
            self.as_mut().clear_error();
        }
        self.as_mut().send_command(Command::ConfigureHotkey);
    }

    /// Dismiss the current user-visible error message.
    pub fn clear_error(mut self: Pin<&mut Self>) {
        self.as_mut().set_error_message(QString::default());
    }

    /// Record deliberate edits; display initialization must not rewrite a config.
    pub fn mark_settings_changed(mut self: Pin<&mut Self>) {
        self.as_mut().rust_mut().get_mut().config_dirty = true;
    }

    /// Record a position chosen by the user rather than restored or clamped.
    pub fn mark_position_changed(mut self: Pin<&mut Self>) {
        self.as_mut().rust_mut().get_mut().record_position_change();
    }

    /// Undo substitute-monitor clamping once the saved monitor is selected again.
    /// Returns whether the saved coordinates replaced the displayed ones.
    pub fn restore_saved_position(mut self: Pin<&mut Self>) -> bool {
        let Some((x, y)) = self.rust().saved_coordinates_for_live_monitor() else {
            return false;
        };
        self.as_mut().set_fixed_x(x);
        self.as_mut().set_fixed_y(y);
        true
    }

    /// Save a changed draft, including settings that do not require a click run.
    pub fn save_config(mut self: Pin<&mut Self>) -> bool {
        self.as_mut().persist_config(false)
    }

    fn persist_config(mut self: Pin<&mut Self>, force: bool) -> bool {
        match self.as_mut().rust_mut().get_mut().persist_config(force) {
            Ok(()) => true,
            Err(error) => {
                self.as_mut().set_error_message(QString::from(&error));
                false
            }
        }
    }

    /// End the worker with a bounded wait and clear running indicators during window closure.
    pub fn shutdown(mut self: Pin<&mut Self>) {
        let worker = {
            let rust = self.as_mut().rust_mut().get_mut();
            rust.next_worker_epoch();
            rust.worker.take()
        };
        if let Some(worker) = worker {
            worker.shutdown();
        }
        // The new epoch drops the old worker's final State(Closing) as well.
        self.as_mut().set_running(false);
        self.as_mut().set_busy(false);
        self.as_mut().set_countdown_remaining(0);
        self.as_mut().set_hotkey_ready(false);
        self.as_mut().set_hotkey_pending(false);
        self.as_mut().set_hotkey_configuring(false);
        self.as_mut().set_status(QString::from("Bereit"));
    }

    /// Hand a command to the worker and surface delivery failures.
    ///
    /// Stop and Shutdown bypass the bounded queue. A failed send leaves the
    /// shown run state alone unless the worker has ended.
    fn send_command(mut self: Pin<&mut Self>, command: Command) {
        let worker = self.rust().worker.as_ref();
        let result = worker
            .ok_or("Die Klicksteuerung ist nicht verfügbar. Starte Klickmeister neu.")
            .and_then(|worker| worker.send(command));
        if let Err(error) = result {
            if send_failure_ends_run(worker) {
                self.as_mut().set_running(false);
                self.as_mut().set_busy(false);
                self.as_mut().set_countdown_remaining(0);
            }
            self.as_mut().show_error(error);
        }
    }

    /// Build validated settings; unconfirmed fixed positions cannot start.
    fn settings(self: Pin<&Self>) -> Result<ClickSettings, String> {
        if !*self.current_position() && !*self.fixed_position_confirmed() {
            return Err(
                "Prüfe Bildschirm und Koordinaten und bestätige die feste Position.".to_owned(),
            );
        }
        let button = match *self.mouse_button() {
            0 => MouseButton::Left,
            1 => MouseButton::Right,
            2 => MouseButton::Middle,
            _ => return Err("Wähle eine gültige Maustaste.".to_owned()),
        };
        let click_type = match *self.click_type() {
            0 => ClickType::Single,
            1 => ClickType::Double,
            _ => return Err("Wähle eine gültige Klickart.".to_owned()),
        };
        let interval_ms = u64::try_from(*self.interval_ms())
            .map_err(|_| "Wähle ein Klickintervall größer als 0 ms.".to_owned())?;
        let repeat = if *self.repeat_until_stopped() {
            None
        } else {
            Some(
                u64::try_from(*self.repeat_count())
                    .map_err(|_| "Wähle mindestens einen Klickzyklus.".to_owned())?,
            )
        };
        let position = if *self.current_position() {
            None
        } else {
            Some((
                u32::try_from(*self.fixed_x())
                    .map_err(|_| "Die X-Koordinate muss eine ganze Zahl ab 0 sein.".to_owned())?,
                u32::try_from(*self.fixed_y())
                    .map_err(|_| "Die Y-Koordinate muss eine ganze Zahl ab 0 sein.".to_owned())?,
            ))
        };
        let settings = ClickSettings {
            interval_ms,
            button,
            click_type,
            repeat,
            position,
            monitor: position.map(|_| MonitorGeometry {
                x: *self.monitor_x(),
                y: *self.monitor_y(),
                width: *self.monitor_width(),
                height: *self.monitor_height(),
            }),
        };
        settings.validate().map_err(|error| error.to_string())?;
        Ok(settings)
    }

    /// Display an error without replacing an active worker status.
    fn show_error(mut self: Pin<&mut Self>, message: &str) {
        if should_mark_status_as_error(*self.running(), *self.busy()) {
            self.as_mut().set_status(QString::from("Fehler"));
        }
        self.as_mut().set_error_message(QString::from(message));
    }

    /// Apply results only from the current worker on the Qt thread.
    fn handle_worker_event(mut self: Pin<&mut Self>, worker_epoch: u64, event: WorkerEvent) {
        let Some(event) = self
            .as_mut()
            .rust_mut()
            .get_mut()
            .accept_worker_event(worker_epoch, event)
        else {
            return;
        };
        match event {
            WorkerEvent::Countdown(remaining) => {
                // The run is already `Starting`; the worker ends the countdown via `State`.
                self.as_mut().set_countdown_remaining(i32::from(remaining));
                self.as_mut().set_status(QString::from(&format!(
                    "Start in {remaining} s · Bewege den Mauszeiger zum Ziel."
                )));
            }
            WorkerEvent::Status(status) => {
                self.as_mut().set_status(QString::from(&status));
            }
            WorkerEvent::State(state) => {
                let (running, busy) = run_indicators(state);
                self.as_mut().set_running(running);
                self.as_mut().set_busy(busy);
                if !keeps_countdown(state) {
                    self.as_mut().set_countdown_remaining(0);
                }
            }
            WorkerEvent::Hotkey(hotkey) => self.as_mut().set_hotkey(QString::from(&hotkey)),
            WorkerEvent::HotkeyPhase(phase) => {
                self.as_mut().set_hotkey_ready(matches!(
                    phase,
                    HotkeyPhase::Ready | HotkeyPhase::Configuring(true)
                ));
                self.as_mut().set_hotkey_pending(matches!(
                    phase,
                    HotkeyPhase::Registering | HotkeyPhase::Configuring(_)
                ));
                self.as_mut()
                    .set_hotkey_configuring(matches!(phase, HotkeyPhase::Configuring(_)));
            }
            WorkerEvent::StartRequested(request) => {
                // The worker owns the pending run; GUI flags can lag behind it.
                // Dropping the reply declines it on picker/validation failures.
                if !request.is_cancelled()
                    && let Some(settings) = self.as_mut().collect_start_settings()
                {
                    request.start(settings);
                }
            }
            WorkerEvent::Error(error) => self.as_mut().show_error(&error),
        }
    }
}

fn run_indicators(state: RunState) -> (bool, bool) {
    match state {
        RunState::Starting => (false, true),
        RunState::Clicking => (true, false),
        RunState::Ready | RunState::Stopped | RunState::Error | RunState::Closing => (false, false),
    }
}

/// A countdown belongs to a pending start; any other state has ended it.
fn keeps_countdown(state: RunState) -> bool {
    state == RunState::Starting
}

/// A failed send ends the shown run only if no worker can report its state
/// anymore. A full queue during a run keeps it shown; Stop stays possible.
fn send_failure_ends_run(worker: Option<&WorkerHandle>) -> bool {
    worker.is_none_or(WorkerHandle::is_stopped)
}

fn should_mark_status_as_error(running: bool, busy: bool) -> bool {
    !running && !busy
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_ID: AtomicU64 = AtomicU64::new(0);

    fn test_directory() -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "klickmeister-controller-test-{}-{}",
            std::process::id(),
            NEXT_ID.fetch_add(1, Ordering::Relaxed)
        ))
    }

    /// Mirror Main.qml when the saved monitor is replaced by a smaller one.
    fn substitute_monitor(controller: &mut AppControllerRust) {
        controller.fixed_position_confirmed = false;
        controller.monitor_identity = QString::from("laptop");
        controller.fixed_x = controller.fixed_x.min(1919);
        controller.fixed_y = controller.fixed_y.min(1079);
    }

    #[test]
    fn draft_survives_close_and_reload_without_confirming_position_or_starting() {
        let mut controller = AppControllerRust::from_config(AppConfig::default(), None);
        controller.interval_ms = 250;
        controller.mouse_button = 1;
        controller.click_type = 1;
        controller.repeat_until_stopped = false;
        controller.repeat_count = 42;
        controller.current_position = false;
        controller.fixed_x = 640;
        controller.fixed_y = 480;
        controller.monitor_identity = QString::from("monitor-a");
        controller.hotkey = QString::from("F8");
        controller.record_position_change();
        // The monitor has not been confirmed, so this draft cannot start.
        assert!(!controller.fixed_position_confirmed);

        let directory = test_directory();
        let path = directory.join("config.toml");
        assert!(controller.persist_config_to(&path, false).is_ok());
        let loaded = config::load_from(&path).unwrap_or_else(|error| panic!("{error}"));
        assert!(loaded.monitor_identity.is_empty());
        let reopened = AppControllerRust::from_config(loaded, None);
        assert_eq!(reopened.interval_ms, 250);
        assert_eq!(reopened.mouse_button, 1);
        assert_eq!(reopened.click_type, 1);
        assert_eq!(reopened.repeat_count, 42);
        assert_eq!(reopened.fixed_x, 640);
        assert_eq!(reopened.fixed_y, 480);
        assert_eq!(reopened.hotkey.to_string(), "F8");
        assert!(!reopened.current_position);
        assert!(!reopened.fixed_position_confirmed);
        assert!(!reopened.running);
        assert!(reopened.worker.is_none());
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn closing_unchanged_does_not_replace_another_write_or_a_malformed_file() {
        let directory = test_directory();
        let path = directory.join("config.toml");
        let initial = AppConfig::default();
        assert!(config::save_to(&path, &initial).is_ok());
        let mut first = AppControllerRust::from_config(initial.clone(), None);
        let mut second = AppControllerRust::from_config(initial.clone(), None);
        first.interval_ms = 250;
        first.config_dirty = true;
        assert!(first.persist_config_to(&path, false).is_ok());
        assert!(second.persist_config_to(&path, false).is_ok());
        assert_eq!(
            config::load_from(&path).ok().map(|c| c.interval_ms),
            Some(250)
        );

        let malformed = "interval_ms = [\n";
        assert!(std::fs::write(&path, malformed).is_ok());
        assert!(config::load_from(&path).is_err());
        let mut failed_load = AppControllerRust::from_config(initial, Some("parse error".into()));
        assert!(failed_load.persist_config_to(&path, false).is_ok());
        assert_eq!(
            std::fs::read_to_string(&path).ok().as_deref(),
            Some(malformed)
        );
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn failed_write_keeps_previous_file_and_unsaved_draft() {
        let directory = test_directory();
        let path = directory.join("config.toml");
        let initial = AppConfig::default();
        assert!(config::save_to(&path, &initial).is_ok());
        let previous = std::fs::read_to_string(&path).ok();
        let mut controller = AppControllerRust::from_config(initial, None);
        controller.interval_ms = 250;
        controller.config_dirty = true;
        // Occupy the temporary file that config::save_to renames over the
        // config, so the write fails before the old file could be replaced.
        let blocker = directory.join(format!(".config.toml.tmp-{}", std::process::id()));
        assert!(std::fs::create_dir(&blocker).is_ok());
        let error = controller
            .persist_config_to(&path, false)
            .err()
            .unwrap_or_default();
        assert!(
            error.starts_with("Einstellungen konnten nicht gespeichert werden"),
            "{error}"
        );
        assert_eq!(std::fs::read_to_string(&path).ok(), previous);
        assert!(controller.config_dirty);

        // Closing again after the cause is fixed saves the same draft.
        assert!(std::fs::remove_dir(&blocker).is_ok());
        assert!(controller.persist_config_to(&path, false).is_ok());
        assert_eq!(
            config::load_from(&path).ok().map(|c| c.interval_ms),
            Some(250)
        );
        assert!(!controller.config_dirty);
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn each_unchanged_guard_keeps_another_instances_write() {
        let directory = test_directory();
        let path = directory.join("config.toml");
        let other = AppConfig {
            interval_ms: 300,
            ..AppConfig::default()
        };
        assert!(config::save_to(&path, &other).is_ok());

        // Without a user edit, even a differing displayed value is not written.
        let mut untouched = AppControllerRust::from_config(AppConfig::default(), None);
        untouched.interval_ms = 250;
        assert!(untouched.persist_config_to(&path, false).is_ok());
        assert_eq!(
            config::load_from(&path).ok().map(|c| c.interval_ms),
            Some(300)
        );

        // An edit that leaves the loaded values unchanged is not written either.
        let mut reverted = AppControllerRust::from_config(AppConfig::default(), None);
        reverted.config_dirty = true;
        assert!(reverted.persist_config_to(&path, false).is_ok());
        assert_eq!(
            config::load_from(&path).ok().map(|c| c.interval_ms),
            Some(300)
        );
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn late_events_of_a_shut_down_worker_are_dropped() {
        let mut controller = AppControllerRust::from_config(AppConfig::default(), None);
        let first = controller.next_worker_epoch(); // initialize()
        assert!(matches!(
            controller.accept_worker_event(first, WorkerEvent::State(RunState::Clicking)),
            Some(WorkerEvent::State(RunState::Clicking))
        ));

        // shutdown() starts a new epoch, so it clears the run indicators itself.
        controller.next_worker_epoch();
        let (request, mut response) = crate::worker::StartRequest::channel();
        for event in [
            WorkerEvent::State(RunState::Closing),
            WorkerEvent::Countdown(2),
            WorkerEvent::Hotkey("F8".into()),
            WorkerEvent::StartRequested(request),
        ] {
            assert!(controller.accept_worker_event(first, event).is_none());
        }
        assert!(!controller.config_dirty);
        assert_eq!(
            response.try_recv(),
            Err(tokio::sync::oneshot::error::TryRecvError::Closed)
        );

        // "Zurück zur Anwendung" starts a worker whose events apply again.
        let second = controller.next_worker_epoch();
        assert!(matches!(
            controller.accept_worker_event(second, WorkerEvent::State(RunState::Ready)),
            Some(WorkerEvent::State(RunState::Ready))
        ));
        assert!(
            controller
                .accept_worker_event(first, WorkerEvent::State(RunState::Closing))
                .is_none()
        );
    }

    #[test]
    fn changed_worker_hotkey_is_saved_on_close() {
        let directory = test_directory();
        let path = directory.join("config.toml");
        let mut controller = AppControllerRust::from_config(AppConfig::default(), None);
        let epoch = controller.next_worker_epoch();
        // KWin confirming the saved trigger is no change.
        assert!(
            controller
                .accept_worker_event(epoch, WorkerEvent::Hotkey("Pause".into()))
                .is_none()
        );
        assert!(!controller.config_dirty);

        let Some(WorkerEvent::Hotkey(hotkey)) =
            controller.accept_worker_event(epoch, WorkerEvent::Hotkey("F8".into()))
        else {
            panic!("a changed hotkey must reach the UI");
        };
        assert!(controller.config_dirty);
        // As set_hotkey() in handle_worker_event.
        controller.hotkey = QString::from(&hotkey);
        assert!(controller.persist_config_to(&path, false).is_ok());
        assert_eq!(
            config::load_from(&path).ok().map(|c| c.hotkey).as_deref(),
            Some("F8")
        );
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn inactive_invalid_fields_do_not_block_valid_changes() {
        let directory = test_directory();
        let path = directory.join("config.toml");
        let initial = AppConfig {
            repeat_count: 0,
            fixed_x: 100_001,
            fixed_y: 100_001,
            ..AppConfig::default()
        };
        assert!(config::save_to(&path, &initial).is_ok());
        let mut controller = AppControllerRust::from_config(initial, None);
        controller.interval_ms = 250;
        controller.config_dirty = true;
        assert!(controller.persist_config_to(&path, false).is_ok());
        let saved = config::load_from(&path).unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(saved.interval_ms, 250);
        assert_eq!(saved.repeat_count, 100);
        assert_eq!((saved.fixed_x, saved.fixed_y), (0, 0));

        let fixed_draft = AppConfig {
            position_mode: PositionMode::Fixed,
            fixed_x: 100_001,
            monitor_identity: "stale-monitor".into(),
            ..AppConfig::default()
        };
        assert!(config::save_to(&path, &fixed_draft).is_ok());
        let mut controller = AppControllerRust::from_config(fixed_draft, None);
        controller.interval_ms = 300;
        controller.config_dirty = true;
        assert!(controller.persist_config_to(&path, false).is_ok());
        let saved = config::load_from(&path).unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(saved.interval_ms, 300);
        assert_eq!(saved.fixed_x, 0);
        assert!(saved.monitor_identity.is_empty());
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn unconfirmed_saved_position_survives_unrelated_edits() {
        let directory = test_directory();
        let path = directory.join("config.toml");
        let initial = AppConfig {
            position_mode: PositionMode::Fixed,
            fixed_x: 2400,
            fixed_y: 1300,
            monitor_identity: "external".into(),
            ..AppConfig::default()
        };
        assert!(config::save_to(&path, &initial).is_ok());
        let mut controller = AppControllerRust::from_config(initial, None);
        // The external monitor is absent at startup.
        substitute_monitor(&mut controller);
        controller.interval_ms = 250;
        controller.config_dirty = true;
        assert!(controller.persist_config_to(&path, false).is_ok());
        let saved = config::load_from(&path).unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(saved.interval_ms, 250);
        assert_eq!((saved.fixed_x, saved.fixed_y), (2400, 1300));
        assert_eq!(saved.monitor_identity, "external");
        assert_eq!(saved.position_mode, PositionMode::Fixed);

        // Switching to the cursor mode keeps the fixed position for later.
        controller.current_position = true;
        controller.config_dirty = true;
        assert!(controller.persist_config_to(&path, false).is_ok());
        let saved = config::load_from(&path).unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(saved.position_mode, PositionMode::CurrentCursor);
        assert_eq!((saved.fixed_x, saved.fixed_y), (2400, 1300));
        assert_eq!(saved.monitor_identity, "external");
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn saved_position_returns_when_its_monitor_is_selected_again() {
        let initial = AppConfig {
            position_mode: PositionMode::Fixed,
            fixed_x: 2400,
            fixed_y: 1300,
            monitor_identity: "external".into(),
            ..AppConfig::default()
        };
        let mut controller = AppControllerRust::from_config(initial, None);
        substitute_monitor(&mut controller);
        assert_eq!(controller.saved_coordinates_for_live_monitor(), None);
        controller.monitor_identity = QString::from("external");
        assert_eq!(
            controller.saved_coordinates_for_live_monitor(),
            Some((2400, 1300))
        );
        controller.fixed_position_confirmed = true;
        assert_eq!(controller.saved_coordinates_for_live_monitor(), None);

        // An unconfirmed deliberate edit has no monitor to return to.
        controller.fixed_position_confirmed = false;
        controller.record_position_change();
        assert_eq!(controller.saved_coordinates_for_live_monitor(), None);
    }

    #[test]
    fn chosen_position_survives_losing_its_monitor_before_close() {
        let directory = test_directory();
        let path = directory.join("config.toml");
        let mut controller = AppControllerRust::from_config(AppConfig::default(), None);
        controller.current_position = false;
        controller.fixed_x = 2400;
        controller.fixed_y = 1300;
        controller.monitor_identity = QString::from("external");
        controller.fixed_position_confirmed = true;
        controller.record_position_change();
        substitute_monitor(&mut controller);
        assert!(controller.persist_config_to(&path, false).is_ok());
        let saved = config::load_from(&path).unwrap_or_else(|error| panic!("{error}"));
        assert_eq!((saved.fixed_x, saved.fixed_y), (2400, 1300));
        assert_eq!(saved.monitor_identity, "external");

        // A deliberate edit on the substitute monitor replaces the position.
        controller.fixed_x = 100;
        controller.record_position_change();
        assert!(controller.persist_config_to(&path, false).is_ok());
        let saved = config::load_from(&path).unwrap_or_else(|error| panic!("{error}"));
        assert_eq!((saved.fixed_x, saved.fixed_y), (100, 1079));
        assert!(saved.monitor_identity.is_empty());

        // Confirming the substitute monitor saves it with the shown position.
        controller.fixed_position_confirmed = true;
        controller.record_position_change();
        assert!(controller.persist_config_to(&path, false).is_ok());
        let saved = config::load_from(&path).unwrap_or_else(|error| panic!("{error}"));
        assert_eq!((saved.fixed_x, saved.fixed_y), (100, 1079));
        assert_eq!(saved.monitor_identity, "laptop");
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn controls_follow_worker_state() {
        assert_eq!(run_indicators(RunState::Ready), (false, false));
        assert_eq!(run_indicators(RunState::Starting), (false, true));
        assert_eq!(run_indicators(RunState::Clicking), (true, false));
        for state in [RunState::Stopped, RunState::Error, RunState::Closing] {
            assert_eq!(run_indicators(state), (false, false));
        }
    }

    #[test]
    fn only_a_pending_start_keeps_its_countdown() {
        assert!(keeps_countdown(RunState::Starting));
        for state in [
            RunState::Ready,
            RunState::Clicking,
            RunState::Stopped,
            RunState::Error,
            RunState::Closing,
        ] {
            assert!(!keeps_countdown(state));
        }
    }

    #[test]
    fn send_failures_keep_the_run_state_unless_the_worker_has_ended() {
        let settings = ClickSettings {
            interval_ms: 100,
            button: MouseButton::Left,
            click_type: ClickType::Single,
            repeat: None,
            position: None,
            monitor: None,
        };
        let (worker, side) = WorkerHandle::stalled(1);
        assert!(worker.send(Command::Start(settings.clone())).is_ok());
        // A full queue while running: the worker still reports its state.
        assert!(worker.send(Command::StartFromButton(settings)).is_err());
        assert!(!send_failure_ends_run(Some(&worker)));
        // A stopped worker sends no further state, so its last one is cleared.
        drop(side);
        assert!(worker.send(Command::Stop).is_err());
        assert!(send_failure_ends_run(Some(&worker)));
        assert!(send_failure_ends_run(None));
    }

    #[test]
    fn local_errors_preserve_active_and_pending_run_status() {
        assert!(should_mark_status_as_error(false, false));
        assert!(!should_mark_status_as_error(true, false));
        assert!(!should_mark_status_as_error(false, true));
    }
}
