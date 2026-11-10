//! Fixed positions and the monitor they are clicked on.

use klickmeister::{
    model::{ClickSettings, MonitorGeometry},
    portal::PortalError,
    state::RunState,
    worker::{Command, HotkeyPhase, WorkerError, WorkerEvent, WorkerHandle},
};
use tokio::sync::mpsc;

use super::portal::{FakeMonitor, Shared, fake_portal};
use super::support::{
    BUS_TEST_LOCK, HotkeyWorker, TestResult, continuous, ensure, event, fixed_on_left_monitor,
    hotkey_ready, shut_down,
};

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

/// A fixed position moves to its point on the selected monitor before each
/// click. A different monitor must fail before sending motion or button events.
#[tokio::test]
#[ignore = "requires a private D-Bus session via dbus-run-session"]
async fn fixed_position_clicks_on_its_monitor_and_fails_on_another() -> TestResult {
    let _bus_guard = BUS_TEST_LOCK.lock().await;
    let HotkeyWorker {
        observed,
        service,
        worker,
        mut events,
    } = HotkeyWorker::spawn().await?;
    let result: TestResult = async {
        hotkey_ready(&mut events).await?;
        let fixed = fixed_on_left_monitor();
        worker.send(Command::Start(fixed.clone()))?;
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

        let before = observed
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .buttons
            .len();
        worker.send(Command::Start(ClickSettings {
            monitor: Some(MonitorGeometry {
                x: 0,
                y: 0,
                width: 1920,
                height: 1080,
            }),
            ..fixed
        }))?;
        event(&mut events, |e| {
            matches!(e, WorkerEvent::State(RunState::Error))
        })
        .await?;
        event(&mut events, |e| {
            matches!(
                e,
                WorkerEvent::Error(WorkerError::Click(PortalError::MonitorMismatch))
            )
        })
        .await?;
        assert_eq!(
            observed
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .buttons
                .len(),
            before
        );
        Ok(())
    }
    .await;
    shut_down(worker, service, result).await
}
