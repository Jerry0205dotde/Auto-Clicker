//! The mouse permission: its request, revocation, idle end and dialogs.

use std::time::Duration;

use ashpd::zbus;
use klickmeister::{
    model::{ClickSettings, ClickType, MonitorGeometry, MouseButton},
    parent_window,
    portal::{PortalError, evdev_code},
    state::RunState,
    worker::{Command, HotkeyPhase, WorkerError, WorkerEvent, WorkerHandle, WorkerStatus},
};
use tokio::{
    sync::mpsc,
    time::{sleep, timeout},
};

use super::portal::{FakeHotkeys, FakeRemote, PATH, Shared, fake_portal, hotkey_portal};
use super::support::{
    BUS_TEST_LOCK, HotkeyWorker, TestResult, close_remote_session, continuous, ensure, event,
    events_until, fixed_on_left_monitor, hotkey_ready, hotkey_session, run_states, shortcut_signal,
    shut_down, start_request, three_clicks,
};

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
        event(&mut events, |e| {
            matches!(e, WorkerEvent::State(RunState::Starting))
        })
        .await?;
        timeout(Duration::from_secs(3), seen.notified()).await?;
        close_remote_session(&service, &observed).await?;
        // Neither the pending Response nor the delayed request is released here.
        timeout(
            Duration::from_millis(700),
            event(&mut events, |e| {
                matches!(e, WorkerEvent::State(RunState::Error))
            }),
        )
        .await??;
        event(&mut events, |e| {
            matches!(
                e,
                WorkerEvent::Error(WorkerError::Click(PortalError::SessionClosed))
            )
        })
        .await?;
        assert!(
            observed
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .buttons
                .is_empty()
        );
        Ok(())
    }
    .await;
    release.notify_one();
    tokio::task::spawn_blocking(move || worker.shutdown()).await?;
    service.close().await?;
    result
}

/// An unused mouse permission ends after the idle time, so KDE's remote
/// control indicator does not stay up between runs. A start within that time
/// reuses the session, a later one asks for a new one.
#[tokio::test]
#[ignore = "requires a private D-Bus session via dbus-run-session"]
async fn idle_mouse_permission_closes_until_the_next_start() -> TestResult {
    let _bus_guard = BUS_TEST_LOCK.lock().await;
    let observed = Shared::default();
    let service = fake_portal(&observed).await?;
    let idle = Duration::from_millis(500);
    let (tx, mut events) = mpsc::unbounded_channel();
    let worker = WorkerHandle::spawn_with_session_idle(
        move |event| {
            let _ = tx.send(event);
        },
        idle,
    );
    let remote = |observed: &Shared| {
        let state = observed.lock().unwrap_or_else(|e| e.into_inner());
        (state.remote_start_calls, state.closed)
    };
    let result: TestResult = async {
        event(&mut events, |e| {
            matches!(e, WorkerEvent::HotkeyPhase(HotkeyPhase::Ready))
        })
        .await?;
        // A run longer than the idle time keeps its session.
        worker.send(Command::Start(continuous()))?;
        event(&mut events, |e| {
            matches!(e, WorkerEvent::State(RunState::Clicking))
        })
        .await?;
        sleep(idle * 2).await;
        ensure(
            remote(&observed) == (1, 0),
            "a running session must stay open",
        )?;
        worker.send(Command::Stop)?;
        event(&mut events, |e| {
            matches!(e, WorkerEvent::State(RunState::Stopped))
        })
        .await?;
        let first_session = observed
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remote_session
            .clone();
        // A start within the idle time needs no new permission.
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
        ensure(
            remote(&observed) == (1, 0),
            "a quick restart must reuse the session",
        )?;
        timeout(Duration::from_secs(3), async {
            while remote(&observed).1 == 0 {
                sleep(Duration::from_millis(10)).await;
            }
        })
        .await?;
        ensure(
            remote(&observed) == (1, 1),
            "the idle session must be closed",
        )?;
        worker.send(Command::Start(continuous()))?;
        event(&mut events, |e| {
            matches!(e, WorkerEvent::State(RunState::Clicking))
        })
        .await?;
        let state = observed.lock().unwrap_or_else(|e| e.into_inner());
        ensure(
            state.remote_start_calls == 2 && state.remote_session != first_session,
            "the next start must request a new session",
        )
    }
    .await;
    tokio::task::spawn_blocking(move || worker.shutdown()).await?;
    service.close().await?;
    result
}

#[derive(Clone, Copy)]
enum WindowExport {
    Missing,
    Ready,
    /// The handle arrives after the worker has asked for the first dialog.
    Late,
}

/// Issue #78: KDE attaches its dialogs to the exported main window.
#[tokio::test]
#[ignore = "requires a private D-Bus session via dbus-run-session"]
async fn portal_dialogs_use_the_exported_window_as_parent() -> TestResult {
    let _bus_guard = BUS_TEST_LOCK.lock().await;
    let mut result = Ok(());
    for export in [
        WindowExport::Missing,
        WindowExport::Ready,
        WindowExport::Late,
    ] {
        result = dialog_parents_once(export).await;
        if result.is_err() {
            break;
        }
    }
    // Later tests in this process expect no window.
    parent_window::finish_export("");
    result
}

async fn dialog_parents_once(export: WindowExport) -> TestResult {
    let observed = Shared::default();
    let service = fake_portal(&observed).await?;
    let expected = match export {
        WindowExport::Missing => {
            parent_window::finish_export("");
            ""
        }
        WindowExport::Ready => {
            parent_window::finish_export("ready-handle");
            "wayland:ready-handle"
        }
        WindowExport::Late => {
            parent_window::begin_export();
            "wayland:late-handle"
        }
    };
    let (tx, mut events) = mpsc::unbounded_channel();
    let worker = WorkerHandle::spawn(move |event| {
        let _ = tx.send(event);
    });
    let result: TestResult = async {
        if let WindowExport::Late = export {
            sleep(Duration::from_millis(300)).await;
            ensure(
                observed
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .parents
                    .is_empty(),
                "BindShortcuts must wait for a pending export",
            )?;
            parent_window::finish_export("late-handle");
        }
        event(&mut events, |e| {
            matches!(e, WorkerEvent::HotkeyPhase(HotkeyPhase::Ready))
        })
        .await?;
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
        let seen = observed
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .configure_seen
            .clone();
        worker.send(Command::ConfigureHotkey)?;
        timeout(Duration::from_secs(3), seen.notified()).await?;
        let parents = observed
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .parents
            .clone();
        ensure(
            parents
                == [
                    ("BindShortcuts", expected.to_owned()),
                    ("Start", expected.to_owned()),
                    ("ConfigureShortcuts", expected.to_owned()),
                ],
            &format!("every dialog must use the parent {expected:?}, got {parents:?}"),
        )
    }
    .await;
    observed
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .configure_release
        .notify_one();
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
        event(&mut events, |e| {
            matches!(e, WorkerEvent::HotkeyPhase(HotkeyPhase::Ready))
        })
        .await?;
        let settings = ClickSettings {
            repeat: Some(1),
            position: fixed.then_some((80, 150)),
            monitor: fixed.then_some(MonitorGeometry {
                x: -1920,
                y: 0,
                width: 1920,
                height: 1080,
            }),
            ..continuous()
        };
        worker.send(Command::Start(settings.clone()))?;
        let ended = events_until(&mut events, |e| {
            matches!(e, WorkerEvent::State(RunState::Error | RunState::Stopped))
        })
        .await?;
        ensure(
            run_states(&ended) == [RunState::Starting, RunState::Error],
            &format!("rejected selection must fail before clicking: {ended:?}"),
        )?;
        let errors = events_until(&mut events, |e| matches!(e, WorkerEvent::Error(_))).await?;
        ensure(
            errors.iter().any(|e| {
                matches!(
                    e,
                    WorkerEvent::Error(WorkerError::Click(PortalError::Denied(_)))
                )
            }),
            "selection rejection must be reported",
        )?;
        {
            let mut state = observed.lock().unwrap_or_else(|e| e.into_inner());
            ensure(
                state.remote_start_calls == 0,
                "Start must not follow a rejected selection",
            )?;
            ensure(
                state.select_sources_calls == usize::from(sources),
                "device rejection must also skip source selection",
            )?;
            ensure(
                state.buttons.is_empty() && state.motions.is_empty(),
                "rejected selection must not inject input",
            )?;
            ensure(state.closed == 1, "the rejected session must be closed")?;
            state.select_devices_response = 0;
            state.select_sources_response = 0;
        }
        worker.send(Command::Start(settings))?;
        event(&mut events, |e| {
            matches!(e, WorkerEvent::State(RunState::Stopped))
        })
        .await?;
        let state = observed.lock().unwrap_or_else(|e| e.into_inner());
        ensure(
            state.remote_start_calls == 1 && state.buttons.len() == 2,
            "retry after rejection must create a new session and click once",
        )
    }
    .await;
    tokio::task::spawn_blocking(move || worker.shutdown()).await?;
    service.close().await?;
    result
}

/// A revoked long-running session must stop before the next scheduled click,
/// and the next start asks for a new session.
#[tokio::test]
#[ignore = "requires a private D-Bus session via dbus-run-session"]
async fn revoked_mouse_permission_stops_a_run_before_its_next_click() -> TestResult {
    let _bus_guard = BUS_TEST_LOCK.lock().await;
    let HotkeyWorker {
        observed,
        service,
        worker,
        mut events,
    } = HotkeyWorker::spawn().await?;
    let result: TestResult = async {
        hotkey_ready(&mut events).await?;
        shortcut_signal(&service, &hotkey_session(&observed)?, "Activated", 0).await?;
        let request = start_request(&mut events).await?;
        let long_run = ClickSettings {
            interval_ms: 60_000,
            repeat: None,
            ..three_clicks()
        };
        request.start(long_run);
        event(&mut events, |e| {
            matches!(e, WorkerEvent::State(RunState::Clicking))
        })
        .await?;
        timeout(Duration::from_secs(3), async {
            loop {
                if observed
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .buttons
                    .len()
                    >= 2
                {
                    break;
                }
                sleep(Duration::from_millis(5)).await;
            }
        })
        .await?;
        let first_session = observed
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remote_session
            .clone();
        close_remote_session(&service, &observed).await?;
        timeout(Duration::from_millis(500), async {
            event(&mut events, |e| {
                matches!(e, WorkerEvent::State(RunState::Stopped))
            })
            .await?;
            event(&mut events, |e| {
                matches!(e, WorkerEvent::State(RunState::Error))
            })
            .await
        })
        .await??;
        event(&mut events, |e| {
            matches!(
                e,
                WorkerEvent::Error(WorkerError::Click(PortalError::SessionClosed))
            )
        })
        .await?;
        worker.send(Command::Start(three_clicks()))?;
        event(&mut events, |e| {
            matches!(e, WorkerEvent::State(RunState::Clicking))
        })
        .await?;
        event(&mut events, |e| {
            matches!(e, WorkerEvent::State(RunState::Stopped))
        })
        .await?;
        assert_ne!(
            observed
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remote_session,
            first_session
        );
        Ok(())
    }
    .await;
    shut_down(worker, service, result).await
}

/// A stopped session must also be discarded once it is closed, so a later
/// start asks again.
#[tokio::test]
#[ignore = "requires a private D-Bus session via dbus-run-session"]
async fn closed_idle_mouse_permission_is_requested_again() -> TestResult {
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
        .await?;
        let stopped_session = observed
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remote_session
            .clone();
        close_remote_session(&service, &observed).await?;
        event(&mut events, |e| {
            matches!(e, WorkerEvent::Status(WorkerStatus::PermissionRequired))
        })
        .await?;
        worker.send(Command::Start(three_clicks()))?;
        event(&mut events, |e| {
            matches!(e, WorkerEvent::State(RunState::Clicking))
        })
        .await?;
        event(&mut events, |e| {
            matches!(e, WorkerEvent::State(RunState::Stopped))
        })
        .await?;
        assert_ne!(
            observed
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remote_session,
            stopped_session
        );
        Ok(())
    }
    .await;
    shut_down(worker, service, result).await
}

/// If permission is revoked during Start, no run may be confirmed, and the
/// next start works again.
#[tokio::test]
#[ignore = "requires a private D-Bus session via dbus-run-session"]
async fn mouse_permission_revoked_during_start_confirms_no_run() -> TestResult {
    let _bus_guard = BUS_TEST_LOCK.lock().await;
    let HotkeyWorker {
        observed,
        service,
        worker,
        mut events,
    } = HotkeyWorker::spawn().await?;
    let result: TestResult = async {
        hotkey_ready(&mut events).await?;
        let (seen, release) = {
            let mut state = observed.lock().unwrap_or_else(|e| e.into_inner());
            state.delay_start = true;
            (state.start_seen.clone(), state.start_release.clone())
        };
        worker.send(Command::Start(three_clicks()))?;
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
        assert!(
            !incorrectly_started,
            "revoked permission must not start a run"
        );
        assert!(
            reported_error_state,
            "revoked permission must end the pending start"
        );
        observed
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .delay_start = false;
        worker.send(Command::Start(three_clicks()))?;
        event(&mut events, |e| {
            matches!(e, WorkerEvent::State(RunState::Clicking))
        })
        .await?;
        event(&mut events, |e| {
            matches!(e, WorkerEvent::State(RunState::Stopped))
        })
        .await?;
        Ok(())
    }
    .await;
    shut_down(worker, service, result).await
}

/// A session the portal revoked fails its run. The retry must request a
/// fresh session instead of reusing the revoked one.
#[tokio::test]
#[ignore = "requires a private D-Bus session via dbus-run-session"]
async fn revoked_session_fails_its_run_and_is_replaced_on_retry() -> TestResult {
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
        .await?;
        {
            let mut state = observed.lock().unwrap_or_else(|e| e.into_inner());
            state.revoked_session = state.remote_session.clone();
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
        worker.send(Command::Start(three_clicks()))?;
        event(&mut events, |e| {
            matches!(e, WorkerEvent::State(RunState::Clicking))
        })
        .await?;
        event(&mut events, |e| {
            matches!(e, WorkerEvent::State(RunState::Stopped))
        })
        .await?;
        {
            let state = observed.lock().unwrap_or_else(|e| e.into_inner());
            assert_ne!(state.remote_session, state.revoked_session);
        }
        Ok(())
    }
    .await;
    shut_down(worker, service, result).await
}

/// Stop closes a pending RemoteDesktop session before it confirms the stop,
/// and the late portal response sends no input.
#[tokio::test]
#[ignore = "requires a private D-Bus session via dbus-run-session"]
async fn stop_during_permission_request_closes_its_session_without_input() -> TestResult {
    let _bus_guard = BUS_TEST_LOCK.lock().await;
    let HotkeyWorker {
        observed,
        service,
        worker,
        mut events,
    } = HotkeyWorker::spawn().await?;
    let fixed = fixed_on_left_monitor();
    let (seen, release) = {
        let mut state = observed.lock().unwrap_or_else(|e| e.into_inner());
        state.delay_start = true;
        (state.start_seen.clone(), state.start_release.clone())
    };
    let result: TestResult = async {
        hotkey_ready(&mut events).await?;
        worker.send(Command::Start(fixed.clone()))?;
        event(&mut events, |e| {
            matches!(e, WorkerEvent::State(RunState::Starting))
        })
        .await?;
        timeout(Duration::from_secs(3), seen.notified()).await?;
        worker.send(Command::Start(ClickSettings {
            position: Some((100, 150)),
            ..fixed.clone()
        }))?;
        let closed_before_stop = observed.lock().unwrap_or_else(|e| e.into_inner()).closed;
        // Stop overtakes and outdates this queued duplicate.
        // `duplicate_start_during_permission_request_keeps_its_settings` checks
        // deterministically that a duplicate during Starting keeps the pending
        // settings: it is queued before the portal can answer.
        worker.send(Command::Stop)?;
        event(&mut events, |e| {
            matches!(e, WorkerEvent::State(RunState::Stopped))
        })
        .await?;
        assert!(
            observed.lock().unwrap_or_else(|e| e.into_inner()).closed > closed_before_stop,
            "Stop must close the pending RemoteDesktop session before confirming it"
        );
        release.notify_one();
        sleep(Duration::from_millis(80)).await;
        assert!(
            observed
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .buttons
                .is_empty()
        );
        assert!(
            observed
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .motions
                .is_empty()
        );
        Ok(())
    }
    .await;
    release.notify_one();
    shut_down(worker, service, result).await
}

/// A new permission request owns its settings even if a duplicate arrives.
#[tokio::test]
#[ignore = "requires a private D-Bus session via dbus-run-session"]
async fn duplicate_start_during_permission_request_keeps_its_settings() -> TestResult {
    let _bus_guard = BUS_TEST_LOCK.lock().await;
    let HotkeyWorker {
        observed,
        service,
        worker,
        mut events,
    } = HotkeyWorker::spawn().await?;
    let fixed = fixed_on_left_monitor();
    let (seen, release) = {
        let mut state = observed.lock().unwrap_or_else(|e| e.into_inner());
        state.delay_start = true;
        (state.start_seen.clone(), state.start_release.clone())
    };
    let result: TestResult = async {
        hotkey_ready(&mut events).await?;
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
        event(&mut events, |e| {
            matches!(e, WorkerEvent::State(RunState::Clicking))
        })
        .await?;
        event(&mut events, |e| {
            matches!(e, WorkerEvent::State(RunState::Stopped))
        })
        .await?;
        assert_eq!(
            observed.lock().unwrap_or_else(|e| e.into_inner()).motions,
            vec![(42, 1919.0, 1079.0); 3]
        );
        assert_eq!(
            observed.lock().unwrap_or_else(|e| e.into_inner()).buttons,
            [
                (evdev_code(MouseButton::Left), 1),
                (evdev_code(MouseButton::Left), 0)
            ]
            .repeat(3)
        );
        Ok(())
    }
    .await;
    shut_down(worker, service, result).await
}

/// Shutdown closes a pending permission request of either dialog, also when
/// Session.Close stalls, and a late response cannot start clicking.
#[tokio::test]
#[ignore = "requires a private D-Bus session via dbus-run-session"]
async fn shutdown_closes_pending_permission_requests() -> TestResult {
    let _bus_guard = BUS_TEST_LOCK.lock().await;
    let observed = Shared::default();
    let service = hotkey_portal(&observed).await?;
    let result: TestResult = async {
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
                    interval_ms: 100,
                    button: MouseButton::Left,
                    click_type: ClickType::Single,
                    repeat: Some(1),
                    position: None,
                    monitor: None,
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
                    if hotkey_pending {
                        state.hotkey_owner.clone()
                    } else {
                        state.remote_owner.clone()
                    }
                };
                let before = std::time::Instant::now();
                timeout(
                    Duration::from_secs(4),
                    tokio::task::spawn_blocking(move || pending_worker.shutdown()),
                )
                .await??;
                eprintln!(
                    "Pending {} shutdown, stalled Close={stall_close}: {:?}",
                    if hotkey_pending { "hotkey" } else { "mouse" },
                    before.elapsed()
                );
                assert!(
                    observed.lock().unwrap_or_else(|e| e.into_inner()).closed > closed_before,
                    "Shutdown must close its pending permission session"
                );
                let bus = zbus::fdo::DBusProxy::new(&service).await?;
                assert!(
                    !bus.name_has_owner(owner.as_str().try_into()?).await?,
                    "Permission owner must disconnect even when Session.Close stalls"
                );
                release.notify_one();
                // Mouse shutdown also closes the hotkey session. Give every
                // stalled close its own retained permit, including late waiters.
                let close_releases = std::mem::take(
                    &mut observed
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .close_releases,
                );
                for close_release in close_releases {
                    close_release.notify_one();
                }
                sleep(Duration::from_millis(30)).await;
                while let Ok(event) = pending_events.try_recv() {
                    assert!(
                        !matches!(event, WorkerEvent::State(RunState::Clicking)),
                        "A late permission response must not start clicking"
                    );
                }
                observed
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .stall_close = false;
            }
        }
        Ok(())
    }
    .await;
    service.close().await?;
    result
}
