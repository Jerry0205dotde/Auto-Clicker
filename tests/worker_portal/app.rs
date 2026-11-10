//! The real application: single instance and app registration.

use std::time::Duration;

use klickmeister::{
    model::{ClickSettings, MonitorGeometry},
    state::RunState,
    worker::{Command, HotkeyError, HotkeyPhase, WorkerError, WorkerEvent, WorkerHandle},
};
use tokio::{
    sync::mpsc,
    time::{sleep, timeout},
};

use super::portal::{Shared, fake_portal};
use super::support::{BUS_TEST_LOCK, TestResult, continuous, ensure, event, presses};

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
            // Offscreen there is no Wayland window to export.
            ensure(
                state.parents == [("BindShortcuts", String::new())],
                &format!("the hotkey dialog must have no parent: {:?}", state.parents),
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

/// Run `klickmeister --toggle` and return whether it succeeded and its stderr.
async fn run_toggle(
    config: &std::path::Path,
) -> Result<(bool, String), Box<dyn std::error::Error>> {
    let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_klickmeister"));
    command
        .arg("--toggle")
        .env("QT_QPA_PLATFORM", "offscreen")
        // English texts, whatever the developer's language.
        .env("LC_ALL", "C")
        .env_remove("LANGUAGE")
        .env("XDG_CONFIG_HOME", config)
        .stdin(std::process::Stdio::null());
    // The call itself gives up after a few seconds.
    let output = tokio::task::spawn_blocking(move || command.output()).await??;
    Ok((
        output.status.success(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    ))
}

async fn presses_reach(observed: &Shared, count: usize) -> TestResult {
    timeout(Duration::from_secs(5), async {
        while presses(observed) < count {
            sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .map_err(|_| format!("expected {count} clicks, saw {}", presses(observed)).into())
}

/// Issue #143: `klickmeister --toggle` starts and stops clicking in the
/// running application through the same path as the hotkey, and without a
/// running instance it fails instead of opening one.
#[tokio::test]
#[ignore = "requires a private D-Bus session via dbus-run-session"]
async fn toggle_command_starts_and_stops_the_running_application() -> TestResult {
    let _serial = BUS_TEST_LOCK.lock().await;
    let observed = Shared::default();
    let service = fake_portal(&observed).await?;
    let config = std::env::temp_dir().join(format!("klickmeister-toggle-{}", std::process::id()));
    std::fs::create_dir_all(&config)?;
    let result: TestResult = async {
        let (ok, stderr) = run_toggle(&config).await?;
        ensure(
            !ok && stderr.contains("not running"),
            &format!("a toggle without an instance must fail: {stderr}"),
        )?;
        ensure(
            observed
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .hotkey_created
                == 0,
            "a toggle must not start Klickmeister",
        )?;

        let mut app = App::launch(&config, "toggled", None)?;
        hotkey_sessions_reach(&observed, 1, &mut app).await?;
        // The hotkey binding completes shortly after its session; until then
        // the worker drops a toggle. Count only a toggle that led to clicks.
        let mut started = false;
        for _ in 0..20 {
            let (ok, _) = run_toggle(&config).await?;
            if ok && presses_reach(&observed, 2).await.is_ok() {
                started = true;
                break;
            }
            sleep(Duration::from_millis(100)).await;
        }
        ensure(
            started,
            &format!("a toggle must start clicking: {}", app.stderr()),
        )?;

        let (ok, stderr) = run_toggle(&config).await?;
        ensure(ok, &format!("the second toggle must be accepted: {stderr}"))?;
        sleep(Duration::from_millis(500)).await;
        let stopped_at = presses(&observed);
        sleep(Duration::from_millis(500)).await;
        ensure(
            presses(&observed) == stopped_at,
            "the second toggle must stop clicking",
        )?;
        ensure(
            app.0.try_wait()?.is_none(),
            "the application must keep running",
        )?;

        // A new toggle after the stop starts again.
        let (ok, stderr) = run_toggle(&config).await?;
        ensure(ok, &format!("the third toggle must be accepted: {stderr}"))?;
        presses_reach(&observed, stopped_at + 2).await?;
        app.kill()?;
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
            |e| matches!(e, WorkerEvent::Error(WorkerError::Hotkey(HotkeyError::Registration(details))) if details.contains("registration denied")),
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
