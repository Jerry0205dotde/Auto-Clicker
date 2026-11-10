//! Click sequences, the command queue and the start countdown.

use std::{sync::Arc, time::Duration};

use ashpd::zbus::{self, zvariant::OwnedObjectPath};
use klickmeister::{
    model::{ClickSettings, ClickType, MonitorGeometry, MouseButton},
    portal::{PortalError, evdev_code},
    state::RunState,
    worker::{
        Command, HotkeyError, HotkeyPhase, SendError, WorkerError, WorkerEvent, WorkerHandle,
        WorkerStatus,
    },
};
use tokio::{
    sync::{Notify, mpsc},
    time::{sleep, timeout},
};

use super::portal::{FakeHotkeys, FakeRemote, FakeScreencast, Options, PATH, Shared, fake_portal};
use super::support::{
    BUS_TEST_LOCK, HotkeyWorker, TestResult, close_remote_session, continuous, ensure, event,
    events_until, hold_first_click, hotkey_ready, presses, release_held_click,
    retry_if_click_window_missed, run_states, shortcuts_changed, shut_down, three_clicks,
};

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
        worker.send(Command::StartFromButton(continuous())) == Err(SendError::Busy),
        "the command queue must be full while the click is held",
    )?;
    // The controller keeps the shown run state unless the worker has ended.
    ensure(
        !worker.is_stopped(),
        "a full queue must not count as a stopped worker",
    )?;
    Ok(release)
}

/// Issue #114: a click that overruns its interval is followed right away,
/// without another interval of idle time and without replaying missed ticks.
#[tokio::test]
#[ignore = "requires a private D-Bus session via dbus-run-session"]
async fn overdue_clicks_follow_immediately_without_a_burst() -> TestResult {
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
        let interval = Duration::from_millis(60);
        let slow = Duration::from_millis(150);
        {
            let mut state = observed.lock().unwrap_or_else(|e| e.into_inner());
            // Each of the first two clicks misses more than two deadlines.
            state.slow_presses = 2;
            state.press_delay = slow;
        }
        worker.send(Command::Start(ClickSettings {
            interval_ms: 60,
            repeat: Some(6),
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
        let (buttons, presses, slow_ends) = {
            let state = observed.lock().unwrap_or_else(|e| e.into_inner());
            (
                state.buttons.clone(),
                state.press_times.clone(),
                state.slow_press_ends.clone(),
            )
        };
        ensure(
            buttons == [(0x110, 1), (0x110, 0)].repeat(6),
            &format!("overdue ticks must keep the exact press/release pairs: {buttons:?}"),
        )?;
        // Measured from the slow reply, so a late sleep cannot fail the test.
        // Waiting another interval after an overrun would idle at least 60 ms.
        let idle: Vec<_> = slow_ends
            .iter()
            .zip(&presses[1..])
            .map(|(end, next)| next.saturating_duration_since(*end))
            .collect();
        ensure(
            idle.len() == 2 && idle.iter().all(|idle| *idle < interval / 2),
            &format!("an overdue click must follow the slow one directly: {idle:?}"),
        )?;
        let gaps: Vec<_> = presses.windows(2).map(|pair| pair[1] - pair[0]).collect();
        // Replaying the missed deadlines would follow the fast click at once.
        ensure(
            gaps[2..].iter().all(|gap| *gap >= interval / 2),
            &format!("missed deadlines must not be replayed as a burst: {gaps:?}"),
        )
    }
    .await;
    tokio::task::spawn_blocking(move || worker.shutdown()).await?;
    service.close().await?;
    result
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
            return Err(error.to_string().into());
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
        return Err(error.to_string().into());
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
    events.iter().any(|e| {
        matches!(
            e,
            WorkerEvent::Status(WorkerStatus::AwaitingClickPermission)
        )
    })
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
        let requested =
            events_until(&mut events, |e| matches!(e, WorkerEvent::Countdown(3))).await?;
        assert_eq!(run_states(&requested), [RunState::Starting]);
        assert!(waits_for_permission(&requested), "{requested:?}");
        assert_eq!(presses(&observed), 0);

        // The Stop button cancels the countdown without a late start.
        event(&mut events, |e| matches!(e, WorkerEvent::Countdown(2))).await?;
        worker.send(Command::Stop)?;
        let stopped = events_until(&mut events, |e| {
            matches!(e, WorkerEvent::State(RunState::Stopped))
        })
        .await?;
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
        let stopped = events_until(&mut events, |e| {
            matches!(e, WorkerEvent::State(RunState::Stopped))
        })
        .await?;
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
        let run = events_until(&mut events, |e| {
            matches!(e, WorkerEvent::State(RunState::Stopped))
        })
        .await?;
        assert_eq!(countdowns(&run), [2, 1]);
        assert_eq!(run_states(&run), [RunState::Clicking, RunState::Stopped]);
        assert_eq!(
            observed.lock().unwrap_or_else(|e| e.into_inner()).buttons,
            vec![(0x110, 1), (0x110, 0)]
        );

        // Hotkey-origin starts remain immediate when the permission is reused.
        worker.send(Command::Start(settings.clone()))?;
        let run = events_until(&mut events, |e| {
            matches!(e, WorkerEvent::State(RunState::Stopped))
        })
        .await?;
        assert!(countdowns(&run).is_empty(), "{run:?}");
        assert_eq!(
            run_states(&run),
            [RunState::Starting, RunState::Clicking, RunState::Stopped]
        );
        assert_eq!(presses(&observed), 2);

        // A fixed position moves the pointer itself, so a button start is immediate.
        let fixed = ClickSettings {
            position: Some((1919, 1079)),
            monitor: Some(MonitorGeometry {
                x: -1920,
                y: 0,
                width: 1920,
                height: 1080,
            }),
            ..settings.clone()
        };
        worker.send(Command::StartFromButton(fixed))?;
        let run = events_until(&mut events, |e| {
            matches!(e, WorkerEvent::State(RunState::Stopped))
        })
        .await?;
        assert!(countdowns(&run).is_empty(), "{run:?}");
        assert_eq!(
            run_states(&run),
            [RunState::Starting, RunState::Clicking, RunState::Stopped]
        );
        assert_eq!(presses(&observed), 3);
        assert_eq!(
            observed.lock().unwrap_or_else(|e| e.into_inner()).motions,
            vec![(42, 1919.0, 1079.0)]
        );

        // Revoking the permission during the countdown fails the start and clears its timer.
        worker.send(Command::StartFromButton(settings.clone()))?;
        event(&mut events, |e| matches!(e, WorkerEvent::Countdown(3))).await?;
        close_remote_session(&service, &observed).await?;
        let revoked = events_until(&mut events, |e| {
            matches!(
                e,
                WorkerEvent::Error(WorkerError::Click(PortalError::SessionClosed))
            )
        })
        .await?;
        assert_eq!(run_states(&revoked), [RunState::Stopped, RunState::Error]);
        sleep(Duration::from_millis(3200)).await;
        assert_eq!(presses(&observed), 3);
        assert_no_late_start(&mut events);

        // Losing the hotkey during the countdown also fails the start without a click.
        worker.send(Command::StartFromButton(settings.clone()))?;
        event(&mut events, |e| matches!(e, WorkerEvent::Countdown(3))).await?;
        shortcuts_changed(&service, &hotkey_session, "").await?;
        let removed = events_until(&mut events, |e| {
            matches!(
                e,
                WorkerEvent::Error(WorkerError::Hotkey(HotkeyError::Removed))
            )
        })
        .await?;
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

/// Every button sends its press and release, twice per tick for a double click.
#[tokio::test]
#[ignore = "requires a private D-Bus session via dbus-run-session"]
async fn every_button_and_click_type_sends_its_presses_and_releases() -> TestResult {
    let _bus_guard = BUS_TEST_LOCK.lock().await;
    let HotkeyWorker {
        observed,
        service,
        worker,
        mut events,
    } = HotkeyWorker::spawn().await?;
    let result: TestResult = async {
        hotkey_ready(&mut events).await?;
        let settings = three_clicks();
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
                event(&mut events, |e| {
                    matches!(e, WorkerEvent::State(RunState::Clicking))
                })
                .await?;
                event(&mut events, |e| {
                    matches!(e, WorkerEvent::State(RunState::Stopped))
                })
                .await?;
                let buttons = observed
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .buttons
                    .clone();
                let expected = [(evdev_code(button), 1), (evdev_code(button), 0)]
                    .repeat(3 * usize::from(click_type.clicks_per_tick()));
                assert_eq!(buttons, expected);
            }
        }
        Ok(())
    }
    .await;
    shut_down(worker, service, result).await
}

/// A failed press or release ends the run with an error. A failed press is
/// still released.
#[tokio::test]
#[ignore = "requires a private D-Bus session via dbus-run-session"]
async fn failed_press_or_release_ends_the_run_with_an_error() -> TestResult {
    let _bus_guard = BUS_TEST_LOCK.lock().await;
    let HotkeyWorker {
        observed,
        service,
        worker,
        mut events,
    } = HotkeyWorker::spawn().await?;
    let result: TestResult = async {
        hotkey_ready(&mut events).await?;
        for failed_state in [1, 0] {
            {
                let mut observed = observed.lock().unwrap_or_else(|e| e.into_inner());
                observed.buttons.clear();
                observed.button_failures[failed_state] = 1;
            }
            worker.send(Command::Start(three_clicks()))?;
            event(&mut events, |e| {
                matches!(e, WorkerEvent::State(RunState::Clicking))
            })
            .await?;
            event(&mut events, |e| {
                matches!(e, WorkerEvent::State(RunState::Error))
            })
            .await?;
            event(&mut events, |e| matches!(e, WorkerEvent::Error(_))).await?;
            let expected = if failed_state == 1 {
                vec![(0x110, 1), (0x110, 0)]
            } else {
                vec![(0x110, 1), (0x110, 0), (0x110, 0)]
            };
            assert_eq!(
                observed.lock().unwrap_or_else(|e| e.into_inner()).buttons,
                expected
            );
        }
        Ok(())
    }
    .await;
    shut_down(worker, service, result).await
}

/// Stop ends an endless run together with a start queued before it, and no
/// click follows.
#[tokio::test]
#[ignore = "requires a private D-Bus session via dbus-run-session"]
async fn stop_ends_a_run_and_the_start_queued_before_it() -> TestResult {
    let _bus_guard = BUS_TEST_LOCK.lock().await;
    let HotkeyWorker {
        observed,
        service,
        worker,
        mut events,
    } = HotkeyWorker::spawn().await?;
    let result: TestResult = async {
        hotkey_ready(&mut events).await?;
        worker.send(Command::Start(ClickSettings {
            repeat: None,
            ..three_clicks()
        }))?;
        event(&mut events, |e| {
            matches!(e, WorkerEvent::State(RunState::Clicking))
        })
        .await?;
        worker.send(Command::Start(ClickSettings {
            interval_ms: 0,
            ..three_clicks()
        }))?;
        worker.send(Command::Stop)?;
        event(&mut events, |e| {
            matches!(e, WorkerEvent::State(RunState::Stopped))
        })
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
        Ok(())
    }
    .await;
    shut_down(worker, service, result).await
}

/// Shutdown closes the worker's portal sessions and reports the closing state.
#[tokio::test]
#[ignore = "requires a private D-Bus session via dbus-run-session"]
async fn shutdown_closes_its_sessions_and_reports_closing() -> TestResult {
    let _bus_guard = BUS_TEST_LOCK.lock().await;
    let HotkeyWorker {
        observed,
        service,
        worker,
        mut events,
    } = HotkeyWorker::spawn().await?;
    let result: TestResult = async {
        hotkey_ready(&mut events).await?;
        worker.send(Command::Start(three_clicks()))?;
        event(&mut events, |e| {
            matches!(e, WorkerEvent::State(RunState::Clicking))
        })
        .await?;
        event(&mut events, |e| {
            matches!(e, WorkerEvent::State(RunState::Stopped))
        })
        .await
    }
    .await;
    let closed_before = observed.lock().unwrap_or_else(|e| e.into_inner()).closed;
    tokio::task::spawn_blocking(move || worker.shutdown()).await?;
    assert!(observed.lock().unwrap_or_else(|e| e.into_inner()).closed > closed_before);
    result?;
    let mut saw_closing = false;
    while let Ok(event) = events.try_recv() {
        saw_closing |= matches!(event, WorkerEvent::State(RunState::Closing));
    }
    assert!(saw_closing, "shutdown must report the closing state");
    service.close().await?;
    Ok(())
}
