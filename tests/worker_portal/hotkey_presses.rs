//! Key presses that are already old when the worker reads them.

use std::time::Duration;

use klickmeister::{
    model::ClickSettings,
    state::RunState,
    worker::{
        Command, HotkeyError, HotkeyPhase, WorkerError, WorkerEvent, WorkerHandle, WorkerStatus,
    },
};
use tokio::{
    sync::mpsc,
    time::{sleep, timeout},
};

use super::portal::{Options, Shared, fake_portal};
use super::support::{
    BUS_TEST_LOCK, TestResult, close_remote_session, continuous, ensure, event, events_until,
    hold_first_click, hold_first_click_of, hotkey_session, key_presses, presses,
    release_held_click, retry_if_click_window_missed, run_states, shortcut_signal,
    shortcuts_changed, start_request, start_requests, version_reads, wait_for_ordering_reply,
};

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
            return Err(error.to_string().into());
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

/// Issue #97: a close attempt stops the run before saving and keeps the
/// worker when saving fails. This follows the controller's protocol: Stop on
/// `begin_close`, decline every start request while the save error dialog is
/// open, and Stop again on `cancel_close` ("Zurück zur Anwendung"). No key
/// press from that time may start a run, also not after returning.
#[tokio::test]
#[ignore = "requires a private D-Bus session via dbus-run-session"]
async fn key_presses_during_a_failed_close_never_start() -> TestResult {
    retry_if_click_window_missed(key_presses_during_failed_close_once).await
}

async fn key_presses_during_failed_close_once() -> TestResult {
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
        // The held click delays the worker's confirmation of the Stop.
        let release = hold_first_click(&worker, &observed, &mut events).await?;
        let reads = version_reads(&observed);
        worker.send(Command::Stop)?;
        // A press while the Stop is still outstanding must not restart.
        shortcut_signal(&service, &session, "Activated", 1).await?;
        sleep(Duration::from_millis(20)).await;
        release_held_click(&observed, &release);
        let stopped = events_until(&mut events, |e| {
            matches!(
                e,
                WorkerEvent::State(RunState::Stopped) | WorkerEvent::Error(_)
            )
        })
        .await?;
        if let Some(WorkerEvent::Error(error)) = stopped.last() {
            return Err(error.to_string().into());
        }
        wait_for_ordering_reply(&observed, reads).await;
        let clicked = presses(&observed);

        // The save error dialog is open: the controller declines the request.
        shortcut_signal(&service, &session, "Activated", 2).await?;
        let declined = start_request(&mut events).await?;
        let reads = version_reads(&observed);
        drop(declined);
        event(&mut events, |e| {
            matches!(e, WorkerEvent::State(RunState::Stopped))
        })
        .await?;
        wait_for_ordering_reply(&observed, reads).await;

        // A request still queued on Qt when the user returns to the
        // application is answered only after the Stop of `cancel_close`.
        shortcut_signal(&service, &session, "Activated", 3).await?;
        let late = start_request(&mut events).await?;
        let reads = version_reads(&observed);
        worker.send(Command::Stop)?;
        late.start(continuous());
        sleep(Duration::from_millis(200)).await;
        let later: Vec<_> = std::iter::from_fn(|| events.try_recv().ok()).collect();
        ensure(
            !run_states(&later).contains(&RunState::Clicking) && start_requests(&later) == 0,
            &format!("a start from the close attempt must not run: {later:?}"),
        )?;
        ensure(
            presses(&observed) == clicked,
            "no click may follow the close attempt",
        )?;

        wait_for_ordering_reply(&observed, reads).await;

        // After returning, a new press starts as usual.
        shortcut_signal(&service, &session, "Activated", 4).await?;
        start_request(&mut events).await?.start(continuous());
        event(&mut events, |e| {
            matches!(e, WorkerEvent::State(RunState::Clicking))
        })
        .await
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
            return Err(error.to_string().into());
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
        let closed = events_until(&mut events, |e| {
            matches!(
                e,
                WorkerEvent::Error(WorkerError::Hotkey(HotkeyError::SessionClosed))
            )
        })
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
            matches!(e, WorkerEvent::Status(WorkerStatus::PermissionRequired))
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

/// Issue #95: a Stop after a run that already ended by its count also sets
/// the barrier. A press sent before that Stop, while the worker was busy,
/// must not start a new run once the worker reads it.
#[tokio::test]
#[ignore = "requires a private D-Bus session via dbus-run-session"]
async fn key_press_before_stop_after_finished_run_does_not_restart() -> TestResult {
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
        let reads = version_reads(&observed);
        worker.send(Command::Start(ClickSettings {
            repeat: Some(1),
            ..continuous()
        }))?;
        event(&mut events, |e| {
            matches!(e, WorkerEvent::State(RunState::Stopped))
        })
        .await?;
        // The barrier of the finished run has settled before the press.
        wait_for_ordering_reply(&observed, reads).await;
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
        // Press first, then Stop, both while the worker is busy.
        key_presses(&service, &session, 1, 1).await?;
        sleep(Duration::from_millis(20)).await;
        let reads = version_reads(&observed);
        worker.send(Command::Stop)?;
        let close_releases = {
            let mut state = observed.lock().unwrap_or_else(|e| e.into_inner());
            state.stall_close = false;
            std::mem::take(&mut state.close_releases)
        };
        for close_release in close_releases {
            close_release.notify_one();
        }
        let closed = events_until(&mut events, |e| {
            matches!(e, WorkerEvent::Status(WorkerStatus::PermissionRequired))
        })
        .await?;
        sleep(Duration::from_millis(200)).await;
        let later: Vec<_> = std::iter::from_fn(|| events.try_recv().ok()).collect();
        ensure(
            start_requests(&closed) + start_requests(&later) == 0
                && run_states(&closed).is_empty()
                && run_states(&later).is_empty(),
            &format!("a press sent before Stop must not start: {closed:?} {later:?}"),
        )?;
        // A press after the Stop's barrier starts again.
        wait_for_ordering_reply(&observed, reads).await;
        shortcut_signal(&service, &session, "Activated", 2).await?;
        start_request(&mut events).await.map(|_| ())
    }
    .await;
    tokio::task::spawn_blocking(move || worker.shutdown()).await?;
    service.close().await?;
    result
}
