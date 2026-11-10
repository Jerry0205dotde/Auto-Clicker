//! Registering the global shortcut, and losing or changing it.

use std::time::Duration;

use ashpd::zbus;
use klickmeister::{
    model::{ClickSettings, ClickType, MouseButton},
    state::RunState,
    worker::{
        Command, HotkeyError, HotkeyPhase, WorkerError, WorkerEvent, WorkerHandle, WorkerStatus,
    },
};
use tokio::{
    sync::mpsc,
    time::{sleep, timeout},
};

use super::portal::{FakeHotkeys, FakeRemote, FakeScreencast, Options, PATH, Shared, fake_portal};
use super::support::{
    BUS_TEST_LOCK, HotkeyWorker, TestResult, assert_hotkey_only, continuous, ensure, event,
    events_until, fixed_on_left_monitor, hotkey_ready, hotkey_session, key_presses, presses,
    run_states, shortcut_signal, shortcuts_changed, shut_down, start_request, three_clicks,
};

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
            service
                .emit_signal(
                    None::<&str>,
                    hotkey_session(&observed)?,
                    "org.freedesktop.portal.Session",
                    "Closed",
                    &(Options::new(),),
                )
                .await?;
        }
        let phases = events_until(&mut events, |e| {
            matches!(
                e,
                WorkerEvent::HotkeyPhase(HotkeyPhase::Ready | HotkeyPhase::Unavailable)
            )
        })
        .await?;
        let incorrectly_ready = phases
            .iter()
            .any(|e| matches!(e, WorkerEvent::HotkeyPhase(HotkeyPhase::Ready)));
        if incorrectly_ready {
            worker.send(Command::Start(continuous()))?;
            event(&mut events, |e| {
                matches!(e, WorkerEvent::State(RunState::Clicking))
            })
            .await?;
            timeout(Duration::from_secs(1), async {
                while presses(&observed) == 0 {
                    sleep(Duration::from_millis(5)).await;
                }
            })
            .await?;
        }
        ensure(
            !incorrectly_ready && presses(&observed) == 0,
            &format!(
                "revoked hotkey was reported Ready and allowed {} pointer presses",
                presses(&observed)
            ),
        )?;
        assert_hotkey_only(&phases);
        let failed = events_until(&mut events, |e| {
            matches!(
                e,
                WorkerEvent::Error(WorkerError::Hotkey(HotkeyError::SessionClosed))
            )
        })
        .await?;
        ensure(
            run_states(&failed).is_empty(),
            "idle revocation changed the run state",
        )?;
        let (old_session, owner, closed) = {
            let state = observed.lock().unwrap_or_else(|e| e.into_inner());
            (
                state.hotkey_session.clone(),
                state.hotkey_owner.clone(),
                state.closed,
            )
        };
        ensure(
            closed == 1,
            "revocation must close the failed registration session",
        )?;
        let bus = zbus::fdo::DBusProxy::new(&service).await?;
        ensure(
            !bus.name_has_owner(owner.as_str().try_into()?).await?,
            "revocation must disconnect the registration owner, even if Close stalls",
        )?;
        worker.send(Command::Start(continuous()))?;
        event(&mut events, |e| {
            matches!(e, WorkerEvent::Error(WorkerError::HotkeyRequired))
        })
        .await?;
        ensure(
            observed
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remote_session
                .is_none(),
            "a revoked registration must not request pointer permission",
        )?;

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
        ensure(
            events.try_recv().is_err(),
            "a late bind response published a worker event",
        )?;
        worker.send(Command::ConfigureHotkey)?;
        event(&mut events, |e| {
            matches!(e, WorkerEvent::HotkeyPhase(HotkeyPhase::Registering))
        })
        .await?;
        event(&mut events, |e| {
            matches!(e, WorkerEvent::HotkeyPhase(HotkeyPhase::Ready))
        })
        .await?;
        {
            let state = observed.lock().unwrap_or_else(|e| e.into_inner());
            ensure(
                state.hotkey_created == 2 && state.hotkey_session != old_session,
                "retry must create a fresh hotkey session",
            )?;
        }
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
            "healthy retry must allow exactly its requested click",
        )?;

        // The watcher that protected binding must remain active at runtime.
        service
            .emit_signal(
                None::<&str>,
                hotkey_session(&observed)?,
                "org.freedesktop.portal.Session",
                "Closed",
                &(Options::new(),),
            )
            .await?;
        assert_hotkey_only(
            &events_until(&mut events, |e| {
                matches!(
                    e,
                    WorkerEvent::Error(WorkerError::Hotkey(HotkeyError::SessionClosed))
                )
            })
            .await?,
        );
        Ok(())
    }
    .await;
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

/// A shortcut removed around the BindShortcuts response, before the worker
/// handed the registration over, must not be lost (issue #96).
async fn hotkey_removed_around_bind_response(after_response: bool) -> TestResult {
    let _serial = BUS_TEST_LOCK.lock().await;
    let observed = Shared::default();
    observed
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove_around_hotkey_response = Some(after_response);
    let service = fake_portal(&observed).await?;
    let (tx, mut events) = mpsc::unbounded_channel();
    let worker = WorkerHandle::spawn(move |event| {
        let _ = tx.send(event);
    });
    let result: TestResult = async {
        let phases = events_until(&mut events, |e| {
            matches!(e, WorkerEvent::HotkeyPhase(HotkeyPhase::Unavailable))
        })
        .await?;
        // A change queued before the response is applied before the handoff.
        // One sent after it may still briefly show Ready.
        ensure(
            after_response
                || !phases
                    .iter()
                    .any(|e| matches!(e, WorkerEvent::HotkeyPhase(HotkeyPhase::Ready))),
            "a shortcut removed before the bind response was reported Ready",
        )?;
        assert_hotkey_only(&phases);
        events_until(&mut events, |e| matches!(e, WorkerEvent::Error(_))).await?;
        worker.send(Command::Start(continuous()))?;
        event(&mut events, |e| {
            matches!(e, WorkerEvent::Error(WorkerError::HotkeyRequired))
        })
        .await?;
        let state = observed.lock().unwrap_or_else(|e| e.into_inner());
        ensure(
            state.remote_session.is_none() && state.buttons.is_empty(),
            "a removed shortcut must not allow a run",
        )
    }
    .await;
    tokio::task::spawn_blocking(move || worker.shutdown()).await?;
    service.close().await?;
    result
}

#[tokio::test]
#[ignore = "requires a private D-Bus session via dbus-run-session"]
async fn hotkey_removed_right_before_bind_response_is_never_ready() -> TestResult {
    hotkey_removed_around_bind_response(false).await
}

#[tokio::test]
#[ignore = "requires a private D-Bus session via dbus-run-session"]
async fn hotkey_removed_right_after_bind_response_blocks_start() -> TestResult {
    hotkey_removed_around_bind_response(true).await
}

#[tokio::test]
#[ignore = "requires a private D-Bus session via dbus-run-session"]
async fn hotkey_press_before_bind_response_does_not_start_after_ready() -> TestResult {
    let _serial = BUS_TEST_LOCK.lock().await;
    let observed = Shared::default();
    observed
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .press_before_hotkey_response = true;
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
        sleep(Duration::from_millis(300)).await;
        while let Ok(event) = events.try_recv() {
            ensure(
                !matches!(
                    event,
                    WorkerEvent::StartRequested(_) | WorkerEvent::State(_)
                ),
                &format!("a press from before Ready started a run: {event:?}"),
            )?;
        }
        // A press after Ready still starts.
        key_presses(&service, &hotkey_session(&observed)?, 2, 1).await?;
        start_request(&mut events).await?;
        Ok(())
    }
    .await;
    tokio::task::spawn_blocking(move || worker.shutdown()).await?;
    service.close().await?;
    result
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
        assert_hotkey_only(
            &events_until(&mut events, |e| {
                matches!(
                    e,
                    WorkerEvent::Error(WorkerError::Hotkey(HotkeyError::NotGranted(_)))
                )
            })
            .await?,
        );
        worker.send(Command::Start(settings.clone()))?;
        event(&mut events, |e| {
            matches!(e, WorkerEvent::Error(WorkerError::HotkeyRequired))
        })
        .await?;

        let (seen, release) = {
            let mut state = observed.lock().unwrap_or_else(|e| e.into_inner());
            state.reject_hotkey = false;
            state.delay_hotkey = true;
            (state.hotkey_seen.clone(), state.hotkey_release.clone())
        };
        worker.send(Command::ConfigureHotkey)?;
        event(&mut events, |e| {
            matches!(e, WorkerEvent::HotkeyPhase(HotkeyPhase::Registering))
        })
        .await?;
        timeout(Duration::from_secs(3), seen.notified()).await?;
        worker.send(Command::ConfigureHotkey)?;
        worker.send(Command::Start(settings.clone()))?;
        event(&mut events, |e| {
            matches!(e, WorkerEvent::Error(WorkerError::HotkeyRequired))
        })
        .await?;
        assert_eq!(
            observed
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .hotkey_created,
            2
        );
        release.notify_one();
        event(&mut events, |e| {
            matches!(e, WorkerEvent::HotkeyPhase(HotkeyPhase::Ready))
        })
        .await?;
        let old_session = observed
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .hotkey_session
            .clone();

        service
            .emit_signal(
                None::<&str>,
                old_session.as_str(),
                "org.freedesktop.portal.Session",
                "Closed",
                &(Options::new(),),
            )
            .await?;
        assert_hotkey_only(
            &events_until(&mut events, |e| {
                matches!(
                    e,
                    WorkerEvent::Error(WorkerError::Hotkey(HotkeyError::SessionClosed))
                )
            })
            .await?,
        );
        worker.send(Command::Start(settings.clone()))?;
        event(&mut events, |e| {
            matches!(e, WorkerEvent::Error(WorkerError::HotkeyRequired))
        })
        .await?;

        {
            let mut state = observed.lock().unwrap_or_else(|e| e.into_inner());
            state.delay_hotkey = false;
            state.empty_hotkey_trigger = true;
        }
        worker.send(Command::ConfigureHotkey)?;
        event(&mut events, |e| {
            matches!(e, WorkerEvent::HotkeyPhase(HotkeyPhase::Unavailable))
        })
        .await?;
        event(&mut events, |e| {
            matches!(
                e,
                WorkerEvent::Error(WorkerError::Hotkey(HotkeyError::Unbound))
            )
        })
        .await?;
        {
            let state = observed.lock().unwrap_or_else(|e| e.into_inner());
            assert_eq!(state.hotkey_created, 3);
            assert_ne!(state.hotkey_session, old_session);
        }
        let session = observed
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .hotkey_session
            .clone();
        let (seen, release) = {
            let mut state = observed.lock().unwrap_or_else(|e| e.into_inner());
            state.empty_hotkey_trigger = false;
            (
                state.configure_seen.clone(),
                state.configure_release.clone(),
            )
        };
        worker.send(Command::ConfigureHotkey)?;
        event(&mut events, |e| {
            matches!(e, WorkerEvent::HotkeyPhase(HotkeyPhase::Configuring(false)))
        })
        .await?;
        timeout(Duration::from_secs(3), seen.notified()).await?;
        shortcuts_changed(&service, &session, "Pause").await?;
        event(&mut events, |e| {
            matches!(e, WorkerEvent::HotkeyPhase(HotkeyPhase::Configuring(true)))
        })
        .await?;
        release.notify_one();
        event(&mut events, |e| {
            matches!(e, WorkerEvent::HotkeyPhase(HotkeyPhase::Ready))
        })
        .await?;
        event(&mut events, |e| {
            matches!(e, WorkerEvent::Status(WorkerStatus::Ready))
        })
        .await?;
        let (seen, release) = {
            let state = observed.lock().unwrap_or_else(|e| e.into_inner());
            (
                state.configure_seen.clone(),
                state.configure_release.clone(),
            )
        };
        worker.send(Command::ConfigureHotkey)?;
        event(&mut events, |e| {
            matches!(e, WorkerEvent::HotkeyPhase(HotkeyPhase::Configuring(true)))
        })
        .await?;
        timeout(Duration::from_secs(3), seen.notified()).await?;
        worker.send(Command::ConfigureHotkey)?;
        worker.send(Command::Start(settings.clone()))?;
        event(&mut events, |e| {
            matches!(e, WorkerEvent::Error(WorkerError::HotkeyRequired))
        })
        .await?;
        release.notify_one();
        event(&mut events, |e| {
            matches!(e, WorkerEvent::HotkeyPhase(HotkeyPhase::Ready))
        })
        .await?;
        assert_eq!(
            observed
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .configure_calls,
            2
        );
        worker.send(Command::Start(settings.clone()))?;
        event(&mut events, |e| {
            matches!(e, WorkerEvent::State(RunState::Starting))
        })
        .await?;
        event(&mut events, |e| {
            matches!(e, WorkerEvent::State(RunState::Clicking))
        })
        .await?;
        event(&mut events, |e| {
            matches!(e, WorkerEvent::State(RunState::Stopped))
        })
        .await?;
        let (session, created_before, calls_before) = {
            let state = observed.lock().unwrap_or_else(|e| e.into_inner());
            (
                state.hotkey_session.clone(),
                state.hotkey_created,
                state.configure_calls,
            )
        };
        shortcuts_changed(&service, &session, "").await?;
        assert_hotkey_only(
            &events_until(&mut events, |e| {
                matches!(
                    e,
                    WorkerEvent::Error(WorkerError::Hotkey(HotkeyError::Removed))
                )
            })
            .await?,
        );
        worker.send(Command::Start(settings))?;
        event(&mut events, |e| {
            matches!(e, WorkerEvent::Error(WorkerError::HotkeyRequired))
        })
        .await?;
        worker.send(Command::ConfigureHotkey)?;
        event(&mut events, |e| {
            matches!(e, WorkerEvent::HotkeyPhase(HotkeyPhase::Configuring(false)))
        })
        .await?;
        let (seen, release) = {
            let state = observed.lock().unwrap_or_else(|e| e.into_inner());
            (
                state.configure_seen.clone(),
                state.configure_release.clone(),
            )
        };
        timeout(Duration::from_secs(3), seen.notified()).await?;
        shortcuts_changed(&service, &session, "Pause").await?;
        event(&mut events, |e| {
            matches!(e, WorkerEvent::HotkeyPhase(HotkeyPhase::Configuring(true)))
        })
        .await?;
        release.notify_one();
        event(&mut events, |e| {
            matches!(e, WorkerEvent::HotkeyPhase(HotkeyPhase::Ready))
        })
        .await?;
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
        event(&mut events, |e| {
            matches!(
                e,
                WorkerEvent::Error(WorkerError::Hotkey(HotkeyError::Unbound))
            )
        })
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
            matches!(e, WorkerEvent::Error(WorkerError::HotkeyRequired))
        })
        .await?;
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

/// Issue #132: GlobalShortcuts v1 has no ConfigureShortcuts. A bound v1
/// hotkey is reported as not configurable, and a configure request keeps
/// the binding and a pending hotkey start.
#[tokio::test]
#[ignore = "requires a private D-Bus session via dbus-run-session"]
async fn bound_v1_hotkey_is_not_configured_and_keeps_its_start() -> TestResult {
    let _serial = BUS_TEST_LOCK.lock().await;
    let observed = Shared::default();
    observed
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .hotkey_version = Some(1);
    let service = fake_portal(&observed).await?;
    let (tx, mut events) = mpsc::unbounded_channel();
    let worker = WorkerHandle::spawn(move |event| {
        let _ = tx.send(event);
    });
    let once = ClickSettings {
        repeat: Some(1),
        ..continuous()
    };
    let result: TestResult = async {
        event(&mut events, |e| {
            matches!(e, WorkerEvent::HotkeyConfigurable(false))
        })
        .await?;
        event(&mut events, |e| {
            matches!(e, WorkerEvent::HotkeyPhase(HotkeyPhase::Ready))
        })
        .await?;
        let session = hotkey_session(&observed)?;
        shortcut_signal(&service, &session, "Activated", 1).await?;
        let request = start_request(&mut events).await?;
        worker.send(Command::ConfigureHotkey)?;
        request.start(once.clone());
        // `event` fails on any error, such as a failed ConfigureShortcuts.
        event(&mut events, |e| {
            matches!(e, WorkerEvent::State(RunState::Stopped))
        })
        .await?;
        // Queued behind ConfigureHotkey, so it has been handled by now.
        worker.send(Command::Start(once))?;
        event(&mut events, |e| {
            matches!(e, WorkerEvent::State(RunState::Clicking))
        })
        .await?;
        event(&mut events, |e| {
            matches!(e, WorkerEvent::State(RunState::Stopped))
        })
        .await?;
        ensure(presses(&observed) == 2, "both runs must click")?;
        let state = observed.lock().unwrap_or_else(|e| e.into_inner());
        ensure(
            state.configure_calls == 0,
            "v1 must never call ConfigureShortcuts",
        )?;
        ensure(
            state.hotkey_created == 1,
            "a bound v1 hotkey must keep its session",
        )
    }
    .await;
    tokio::task::spawn_blocking(move || worker.shutdown()).await?;
    service.close().await?;
    result
}

/// The hotkey stops a running run.
#[tokio::test]
#[ignore = "requires a private D-Bus session via dbus-run-session"]
async fn hotkey_press_stops_a_running_run() -> TestResult {
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
        shortcut_signal(&service, &hotkey_session(&observed)?, "Activated", 0).await?;
        event(&mut events, |e| {
            matches!(e, WorkerEvent::State(RunState::Stopped))
        })
        .await
    }
    .await;
    shut_down(worker, service, result).await
}

/// Removing the hotkey fails a pending start and cancels its permission
/// request. Binding it again does not resume that start.
#[tokio::test]
#[ignore = "requires a private D-Bus session via dbus-run-session"]
async fn removed_hotkey_fails_a_pending_start_and_cancels_its_permission_request() -> TestResult {
    let _bus_guard = BUS_TEST_LOCK.lock().await;
    let HotkeyWorker {
        observed,
        service,
        worker,
        mut events,
    } = HotkeyWorker::spawn().await?;
    let (seen, release) = {
        let mut state = observed.lock().unwrap_or_else(|e| e.into_inner());
        state.delay_start = true;
        (state.start_seen.clone(), state.start_release.clone())
    };
    let result: TestResult = async {
        hotkey_ready(&mut events).await?;
        let hotkey_session = observed
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .hotkey_session
            .clone();
        worker.send(Command::Start(fixed_on_left_monitor()))?;
        event(&mut events, |e| {
            matches!(e, WorkerEvent::State(RunState::Starting))
        })
        .await?;
        timeout(Duration::from_secs(3), seen.notified()).await?;
        shortcuts_changed(&service, &hotkey_session, "").await?;
        let removed = events_until(&mut events, |e| {
            matches!(
                e,
                WorkerEvent::Error(WorkerError::Hotkey(HotkeyError::Removed))
            )
        })
        .await?;
        assert_eq!(run_states(&removed), [RunState::Stopped, RunState::Error]);
        release.notify_one();
        observed
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .delay_start = false;
        shortcuts_changed(&service, &hotkey_session, "Pause").await?;
        let rebound = events_until(&mut events, |e| {
            matches!(e, WorkerEvent::HotkeyPhase(HotkeyPhase::Ready))
        })
        .await?;
        assert!(
            run_states(&rebound).is_empty(),
            "a cancelled start must not resume: {rebound:?}"
        );
        Ok(())
    }
    .await;
    release.notify_one();
    shut_down(worker, service, result).await
}

/// Removing the hotkey also fails an active run.
#[tokio::test]
#[ignore = "requires a private D-Bus session via dbus-run-session"]
async fn removed_hotkey_fails_an_active_run() -> TestResult {
    let _bus_guard = BUS_TEST_LOCK.lock().await;
    let HotkeyWorker {
        observed,
        service,
        worker,
        mut events,
    } = HotkeyWorker::spawn().await?;
    let result: TestResult = async {
        hotkey_ready(&mut events).await?;
        let hotkey_session = observed
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .hotkey_session
            .clone();
        worker.send(Command::Start(ClickSettings {
            repeat: None,
            ..three_clicks()
        }))?;
        event(&mut events, |e| {
            matches!(e, WorkerEvent::State(RunState::Clicking))
        })
        .await?;
        shortcuts_changed(&service, &hotkey_session, "").await?;
        let removed = events_until(&mut events, |e| {
            matches!(
                e,
                WorkerEvent::Error(WorkerError::Hotkey(HotkeyError::Removed))
            )
        })
        .await?;
        assert_eq!(run_states(&removed), [RunState::Stopped, RunState::Error]);
        shortcuts_changed(&service, &hotkey_session, "Pause").await?;
        event(&mut events, |e| {
            matches!(e, WorkerEvent::HotkeyPhase(HotkeyPhase::Ready))
        })
        .await
    }
    .await;
    shut_down(worker, service, result).await
}

/// A closed hotkey session ends an active run with an error, and no click
/// follows.
#[tokio::test]
#[ignore = "requires a private D-Bus session via dbus-run-session"]
async fn closed_hotkey_session_ends_an_active_run() -> TestResult {
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
        event(&mut events, |e| {
            matches!(e, WorkerEvent::State(RunState::Stopped))
        })
        .await?;
        event(&mut events, |e| {
            matches!(e, WorkerEvent::State(RunState::Error))
        })
        .await?;
        event(&mut events, |e| {
            matches!(
                e,
                WorkerEvent::Error(WorkerError::Hotkey(HotkeyError::SessionClosed))
            )
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
