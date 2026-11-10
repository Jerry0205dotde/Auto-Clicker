//! Helpers shared by the test modules: worker events, key presses and held clicks.

use std::{collections::HashMap, sync::Arc, time::Duration};

use ashpd::zbus::{
    self,
    zvariant::{OwnedObjectPath, Value},
};
use klickmeister::{
    model::{ClickSettings, ClickType, MonitorGeometry, MouseButton},
    portal::PortalError,
    state::RunState,
    worker::{Command, HotkeyPhase, StartRequest, WorkerEvent, WorkerHandle},
};
use tokio::{
    sync::{Notify, mpsc},
    time::{sleep, timeout},
};

use super::portal::{Options, PATH, Shared, hotkey_portal};

pub(crate) type TestResult = Result<(), Box<dyn std::error::Error>>;

pub(crate) static BUS_TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

pub(crate) async fn event(
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
pub(crate) async fn start_request(
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
pub(crate) async fn events_until(
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

pub(crate) fn run_states(events: &[WorkerEvent]) -> Vec<RunState> {
    events
        .iter()
        .filter_map(|e| match e {
            WorkerEvent::State(state) => Some(*state),
            _ => None,
        })
        .collect()
}

/// Hotkey problems without an active run must only change the hotkey phase.
pub(crate) fn assert_hotkey_only(events: &[WorkerEvent]) {
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

pub(crate) async fn shortcuts_changed(
    service: &zbus::Connection,
    session: &str,
    trigger: &str,
) -> TestResult {
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

pub(crate) async fn close_remote_session(
    service: &zbus::Connection,
    observed: &Shared,
) -> TestResult {
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

/// Retry a scenario only when its held click outlived the worker's 250 ms
/// event timeout, which a heavily loaded machine can cause. Every other
/// failure, including a real regression, fails in each attempt.
pub(crate) async fn retry_if_click_window_missed<F, Fut>(scenario: F) -> TestResult
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = TestResult>,
{
    let mut result = scenario().await;
    for _ in 1..3 {
        match &result {
            // Match the variant's own text, so rewording it keeps the retry.
            Err(error)
                if error
                    .to_string()
                    .contains(&PortalError::EventTimeout.to_string()) =>
            {
                eprintln!("Held click timed out under load, retrying: {error}");
                result = scenario().await;
            }
            _ => break,
        }
    }
    result
}

/// Issue #98: the retry must recognise the worker's real timeout message, so
/// rewording `PortalError::EventTimeout` cannot silently disable it.
#[tokio::test]
async fn click_timeout_retries_the_scenario() -> TestResult {
    let attempts = std::sync::atomic::AtomicUsize::new(0);
    retry_if_click_window_missed(|| async {
        if attempts.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
            // The worker reports the portal error through its Display text.
            Err(PortalError::EventTimeout.to_string().into())
        } else {
            Ok(())
        }
    })
    .await?;
    ensure(
        attempts.load(std::sync::atomic::Ordering::SeqCst) == 2,
        "a click timeout must run the scenario again",
    )
}

/// Fail without panicking, so the test still shuts its worker down.
///
/// A panic skips that cleanup. The private bus would then start the real
/// desktop portal for the dropped worker's calls.
pub(crate) fn ensure(condition: bool, message: &str) -> TestResult {
    if condition {
        Ok(())
    } else {
        Err(message.into())
    }
}

pub(crate) fn continuous() -> ClickSettings {
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
pub(crate) async fn hold_first_click(
    worker: &WorkerHandle,
    observed: &Shared,
    events: &mut mpsc::UnboundedReceiver<WorkerEvent>,
) -> Result<Arc<Notify>, Box<dyn std::error::Error>> {
    hold_first_click_of(worker, observed, events, continuous()).await
}

/// Like `hold_first_click`, for a run with the given settings.
pub(crate) async fn hold_first_click_of(
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

pub(crate) fn release_held_click(observed: &Shared, release: &Notify) {
    observed
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .delay_button = false;
    release.notify_one();
}

/// Count button presses the fake RemoteDesktop portal has received.
pub(crate) fn presses(observed: &Shared) -> usize {
    observed
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .buttons
        .iter()
        .filter(|(_, state)| *state == 1)
        .count()
}

pub(crate) fn hotkey_session(
    observed: &Shared,
) -> Result<OwnedObjectPath, Box<dyn std::error::Error>> {
    let session = observed
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .hotkey_session
        .clone();
    Ok(OwnedObjectPath::try_from(session)?)
}

/// Send `Activated` (key press) or `Deactivated` (release) for the toggle shortcut.
pub(crate) async fn shortcut_signal(
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
pub(crate) async fn key_presses(
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

pub(crate) fn version_reads(observed: &Shared) -> usize {
    observed
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .version_reads
}

/// Give the worker's request after the end of a run time to be answered,
/// so a following key press is sent after that answer. Without such a
/// request this only waits, so the test also runs against older workers.
pub(crate) async fn wait_for_ordering_reply(observed: &Shared, reads_before: usize) {
    let _ = timeout(Duration::from_millis(500), async {
        while version_reads(observed) <= reads_before {
            sleep(Duration::from_millis(5)).await;
        }
    })
    .await;
    // The fake portal sends its reply right after the property getter.
    sleep(Duration::from_millis(50)).await;
}

pub(crate) fn start_requests(events: &[WorkerEvent]) -> usize {
    events
        .iter()
        .filter(|e| matches!(e, WorkerEvent::StartRequested(_)))
        .count()
}

pub(crate) async fn until(condition: impl Fn() -> bool, message: &str) -> TestResult {
    timeout(Duration::from_secs(3), async {
        while !condition() {
            sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .map_err(|_| Box::<dyn std::error::Error>::from(message))
}

pub(crate) fn notify_calls(observed: &Shared) -> usize {
    observed
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .notify_calls
}

/// Three quick left clicks, the base settings of most click flow tests.
pub(crate) fn three_clicks() -> ClickSettings {
    ClickSettings {
        interval_ms: 10,
        button: MouseButton::Left,
        click_type: ClickType::Single,
        repeat: Some(3),
        position: None,
        monitor: None,
    }
}

/// A worker on a fake portal without the app registry. Wait for
/// `hotkey_ready` first and end every test with `shut_down`.
pub(crate) struct HotkeyWorker {
    pub(crate) observed: Shared,
    pub(crate) service: zbus::Connection,
    pub(crate) worker: WorkerHandle,
    pub(crate) events: mpsc::UnboundedReceiver<WorkerEvent>,
}

impl HotkeyWorker {
    pub(crate) async fn spawn() -> Result<Self, Box<dyn std::error::Error>> {
        let observed = Shared::default();
        let service = hotkey_portal(&observed).await?;
        let (tx, events) = mpsc::unbounded_channel();
        let worker = WorkerHandle::spawn(move |event| {
            let _ = tx.send(event);
        });
        Ok(Self {
            observed,
            service,
            worker,
            events,
        })
    }
}

/// Wait until the worker is ready and has bound its hotkey.
pub(crate) async fn hotkey_ready(events: &mut mpsc::UnboundedReceiver<WorkerEvent>) -> TestResult {
    event(events, |e| matches!(e, WorkerEvent::State(RunState::Ready))).await?;
    event(events, |e| matches!(e, WorkerEvent::Hotkey(_))).await
}

/// Shut the worker down before its fake portal ends, then return `result`.
pub(crate) async fn shut_down(
    worker: WorkerHandle,
    service: zbus::Connection,
    result: TestResult,
) -> TestResult {
    tokio::task::spawn_blocking(move || worker.shutdown()).await?;
    service.close().await?;
    result
}

/// A fixed position at the bottom right of the monitor left of the primary
/// one, which the fake portal streams as node 42.
pub(crate) fn fixed_on_left_monitor() -> ClickSettings {
    ClickSettings {
        position: Some((1919, 1079)),
        monitor: Some(MonitorGeometry {
            x: -1920,
            y: 0,
            width: 1920,
            height: 1080,
        }),
        ..three_clicks()
    }
}
