//! Clicks over libei after ConnectToEIS.

use std::{sync::Arc, time::Duration};

use klickmeister::{
    model::{ClickSettings, ClickType, MonitorGeometry, MouseButton},
    portal::{PortalError, evdev_code},
    state::RunState,
    worker::{Command, HotkeyPhase, WorkerError, WorkerEvent, WorkerHandle, WorkerStatus},
};
use tokio::{
    sync::mpsc,
    time::{sleep, timeout},
};

use super::fake_eis::{FakeEis, Input, Region};
use super::portal::{Shared, fake_portal};
use super::support::{
    BUS_TEST_LOCK, TestResult, continuous, ensure, event, events_until, notify_calls, run_states,
    until,
};

/// Regions with KWin's mapping IDs. The one for the -1920 stream sits at 0
/// and is not the first, so only its ID can select it.
fn kwin_like_eis() -> Arc<FakeEis> {
    Arc::new(FakeEis::new(vec![
        Region {
            x: 1920,
            width: 1920,
            mapping_id: "HDMI-A-1",
        },
        Region {
            x: 0,
            width: 1920,
            mapping_id: "DP-1",
        },
    ]))
}

fn closed_sessions(observed: &Shared) -> usize {
    observed.lock().unwrap_or_else(|e| e.into_inner()).closed
}

/// Issue #142: with ConnectToEIS, clicks go over libei and the session never
/// sends Notify calls. Stop, a lost libei connection, the idle timeout and
/// shutdown end it like the D-Bus path ends a session.
#[tokio::test]
#[ignore = "requires a private D-Bus session via dbus-run-session"]
async fn eis_clicks_without_notify_calls_and_ends_with_the_session() -> TestResult {
    let _bus_guard = BUS_TEST_LOCK.lock().await;
    let observed = Shared::default();
    let eis = kwin_like_eis();
    observed.lock().unwrap_or_else(|e| e.into_inner()).eis = Some(eis.clone());
    let service = fake_portal(&observed).await?;
    let idle = Duration::from_millis(500);
    let (tx, mut events) = mpsc::unbounded_channel();
    let worker = WorkerHandle::spawn_with_session_idle(
        move |event| {
            let _ = tx.send(event);
        },
        idle,
    );
    let settings = ClickSettings {
        repeat: Some(3),
        ..continuous()
    };
    let one_click = ClickSettings {
        repeat: Some(1),
        ..continuous()
    };
    let result: TestResult = async {
        event(&mut events, |e| {
            matches!(e, WorkerEvent::HotkeyPhase(HotkeyPhase::Ready))
        })
        .await?;
        for button in [MouseButton::Left, MouseButton::Right, MouseButton::Middle] {
            for click_type in [ClickType::Single, ClickType::Double] {
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
                let code = evdev_code(button).cast_unsigned();
                let expected = [Input::Button(code, true), Input::Button(code, false)]
                    .repeat(3 * usize::from(click_type.clicks_per_tick()));
                ensure(
                    eis.take_inputs() == expected,
                    "every click must arrive as a press/release pair over libei",
                )?;
            }
        }
        ensure(
            eis.connections() == (1, 0),
            "runs within the idle time must reuse the libei connection",
        )?;

        // A fixed position moves within its monitor's region, found by mapping ID.
        worker.send(Command::Start(ClickSettings {
            position: Some((1919, 1079)),
            monitor: Some(MonitorGeometry {
                x: -1920,
                y: 0,
                width: 1920,
                height: 1080,
            }),
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
        let cycle = [
            Input::Motion(1919.0, 1079.0),
            Input::Button(0x110, true),
            Input::Button(0x110, false),
        ];
        ensure(
            eis.take_inputs() == cycle.repeat(3),
            "a fixed position must move within the matching region before each click",
        )?;
        until(
            || eis.connections() == (2, 1),
            "a session for another monitor must replace the old libei connection",
        )
        .await?;

        // Stop ends a continuous run; nothing arrives afterwards.
        worker.send(Command::Start(continuous()))?;
        event(&mut events, |e| {
            matches!(e, WorkerEvent::State(RunState::Clicking))
        })
        .await?;
        until(|| eis.input_count() >= 4, "a continuous run must click").await?;
        worker.send(Command::Stop)?;
        event(&mut events, |e| {
            matches!(e, WorkerEvent::State(RunState::Stopped))
        })
        .await?;
        let count = eis.input_count();
        sleep(Duration::from_millis(40)).await;
        ensure(eis.input_count() == count, "no click may follow Stop")?;

        // Losing the libei connection ends a run like a revoked session.
        let closed = closed_sessions(&observed);
        worker.send(Command::Start(continuous()))?;
        event(&mut events, |e| {
            matches!(e, WorkerEvent::State(RunState::Clicking))
        })
        .await?;
        until(|| eis.input_count() >= count + 4, "the run must click").await?;
        eis.disconnect();
        let ended = events_until(&mut events, |e| {
            matches!(
                e,
                WorkerEvent::Error(WorkerError::Click(PortalError::SessionClosed))
            )
        })
        .await?;
        ensure(
            run_states(&ended).last() == Some(&RunState::Error),
            "a lost libei connection must end the run with an error",
        )?;
        until(
            || closed_sessions(&observed) > closed,
            "the portal session must close with its libei connection",
        )
        .await?;

        // Losing it between runs asks for permission again.
        worker.send(Command::Start(one_click.clone()))?;
        event(&mut events, |e| {
            matches!(e, WorkerEvent::State(RunState::Clicking))
        })
        .await?;
        event(&mut events, |e| {
            matches!(e, WorkerEvent::State(RunState::Stopped))
        })
        .await?;
        eis.disconnect();
        event(&mut events, |e| {
            matches!(e, WorkerEvent::Status(WorkerStatus::PermissionRequired))
        })
        .await?;

        // The idle timeout closes the libei connection with the session.
        worker.send(Command::Start(one_click.clone()))?;
        event(&mut events, |e| {
            matches!(e, WorkerEvent::State(RunState::Clicking))
        })
        .await?;
        event(&mut events, |e| {
            matches!(e, WorkerEvent::State(RunState::Stopped))
        })
        .await?;
        let (opened, _) = eis.connections();
        until(
            || eis.connections() == (opened, opened),
            "the idle timeout must close the libei connection",
        )
        .await?;

        worker.send(Command::Start(one_click))?;
        event(&mut events, |e| {
            matches!(e, WorkerEvent::State(RunState::Clicking))
        })
        .await?;
        event(&mut events, |e| {
            matches!(e, WorkerEvent::State(RunState::Stopped))
        })
        .await?;
        ensure(
            notify_calls(&observed) == 0,
            "a libei session must never send Notify calls",
        )
    }
    .await;
    tokio::task::spawn_blocking(move || worker.shutdown()).await?;
    result?;
    until(
        || {
            let (opened, closed) = eis.connections();
            opened == closed
        },
        "shutdown must close the libei connection",
    )
    .await?;
    service.close().await?;
    Ok(())
}

/// Issue #142: once KWin handed out a libei socket, the session must not fall
/// back to Notify calls, also when libei fails or a click is not confirmed.
#[tokio::test]
#[ignore = "requires a private D-Bus session via dbus-run-session"]
async fn eis_failures_never_fall_back_to_notify_calls() -> TestResult {
    let _bus_guard = BUS_TEST_LOCK.lock().await;
    let observed = Shared::default();
    let eis = kwin_like_eis();
    eis.refuse.store(true, std::sync::atomic::Ordering::SeqCst);
    observed.lock().unwrap_or_else(|e| e.into_inner()).eis = Some(eis.clone());
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
        // A failed handshake ends the start.
        worker.send(Command::Start(continuous()))?;
        event(&mut events, |e| {
            matches!(
                e,
                WorkerEvent::Error(WorkerError::Click(PortalError::EisUnavailable(_)))
            )
        })
        .await?;
        until(
            || closed_sessions(&observed) >= 1,
            "the failed session must close",
        )
        .await?;
        eis.refuse.store(false, std::sync::atomic::Ordering::SeqCst);

        // A fixed position without a region for its monitor fails before any input.
        let other_monitor = Arc::new(FakeEis::new(vec![Region {
            x: 1920,
            width: 1920,
            mapping_id: "HDMI-A-1",
        }]));
        observed.lock().unwrap_or_else(|e| e.into_inner()).eis = Some(other_monitor.clone());
        worker.send(Command::Start(ClickSettings {
            position: Some((80, 400)),
            monitor: Some(MonitorGeometry {
                x: -1920,
                y: 0,
                width: 1920,
                height: 1080,
            }),
            ..continuous()
        }))?;
        event(&mut events, |e| {
            matches!(
                e,
                WorkerEvent::Error(WorkerError::Click(PortalError::MonitorUnverifiable))
            )
        })
        .await?;
        ensure(
            other_monitor.input_count() == 0,
            "a fixed position without its region must send no input",
        )?;

        // An unconfirmed click times out instead of retrying over D-Bus.
        observed.lock().unwrap_or_else(|e| e.into_inner()).eis = Some(eis.clone());
        eis.hold_sync
            .store(true, std::sync::atomic::Ordering::SeqCst);
        worker.send(Command::Start(continuous()))?;
        event(&mut events, |e| {
            matches!(
                e,
                WorkerEvent::Error(WorkerError::Click(PortalError::EventTimeout))
            )
        })
        .await?;
        ensure(
            notify_calls(&observed) == 0,
            "a libei session must never send Notify calls",
        )?;

        // Shutdown during a held click still closes the libei connection.
        worker.send(Command::Start(continuous()))?;
        event(&mut events, |e| {
            matches!(e, WorkerEvent::State(RunState::Clicking))
        })
        .await?;
        until(
            || eis.input_count() >= 4,
            "the held click must have been sent",
        )
        .await
    }
    .await;
    timeout(
        Duration::from_secs(4),
        tokio::task::spawn_blocking(move || worker.shutdown()),
    )
    .await??;
    eis.hold_sync
        .store(false, std::sync::atomic::Ordering::SeqCst);
    result?;
    until(
        || {
            let (opened, closed) = eis.connections();
            opened == closed
        },
        "shutdown must close the libei connection",
    )
    .await?;
    ensure(
        notify_calls(&observed) == 0,
        "a libei session must never send Notify calls",
    )?;
    service.close().await?;
    Ok(())
}
