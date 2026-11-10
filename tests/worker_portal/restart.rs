//! A crashed or restarted portal.

use std::time::Duration;

use ashpd::zbus;
use klickmeister::{
    model::ClickSettings,
    state::RunState,
    worker::{Command, HotkeyPhase, WorkerError, WorkerEvent, WorkerHandle},
};
use tokio::{
    sync::mpsc,
    time::{sleep, timeout},
};

use super::portal::{Shared, fake_portal, queued_fake_portal};
use super::support::{
    BUS_TEST_LOCK, TestResult, assert_hotkey_only, continuous, ensure, event, events_until,
    presses, run_states,
};

/// When the portal disappears relative to a run.
#[derive(Clone, Copy, Debug)]
enum PortalLoss {
    Idle,
    Countdown,
    Running,
}

/// Issue #74: a crashed or restarted portal never sends `Closed` for its
/// sessions. Losing it must end them anyway and block starts until a new
/// hotkey has been registered with the new portal.
#[tokio::test]
#[ignore = "requires a private D-Bus session via dbus-run-session"]
async fn portal_restart_ends_its_sessions_until_the_hotkey_is_registered_again() -> TestResult {
    for loss in [PortalLoss::Idle, PortalLoss::Countdown, PortalLoss::Running] {
        portal_restart_once(loss)
            .await
            .map_err(|error| format!("{loss:?}: {error}"))?;
    }
    Ok(())
}

async fn portal_restart_once(loss: PortalLoss) -> TestResult {
    let _serial = BUS_TEST_LOCK.lock().await;
    let observed = Shared::default();
    let service = fake_portal(&observed).await?;
    // A fresh portal knows neither the app registration nor the old sessions.
    let fresh = Shared::default();
    fresh
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .require_registration = true;
    // The name passes on directly, without a moment in which it has no owner.
    let replacement = queued_fake_portal(&fresh).await?;
    let (tx, mut events) = mpsc::unbounded_channel();
    let worker = WorkerHandle::spawn(move |event| {
        let _ = tx.send(event);
    });
    let result: TestResult = async {
        event(&mut events, |e| {
            matches!(e, WorkerEvent::HotkeyPhase(HotkeyPhase::Ready))
        })
        .await?;
        match loss {
            PortalLoss::Idle => {}
            PortalLoss::Countdown => {
                worker.send(Command::StartFromButton(continuous()))?;
                event(&mut events, |e| matches!(e, WorkerEvent::Countdown(3))).await?;
            }
            PortalLoss::Running => {
                // The second click would be due long after the test.
                worker.send(Command::Start(ClickSettings {
                    interval_ms: 60_000,
                    ..continuous()
                }))?;
                event(&mut events, |e| {
                    matches!(e, WorkerEvent::State(RunState::Clicking))
                })
                .await?;
                timeout(Duration::from_secs(3), async {
                    while presses(&observed) == 0 {
                        sleep(Duration::from_millis(5)).await;
                    }
                })
                .await?;
            }
        }
        // The new portal rejects input for a session of the old one.
        let old_session = observed
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remote_session
            .clone();
        fresh
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .revoked_session = old_session;
        let presses_before = presses(&observed);

        // The portal leaves the bus without sending `Closed`.
        service.close().await?;
        let lost = events_until(&mut events, |e| {
            matches!(e, WorkerEvent::HotkeyPhase(HotkeyPhase::Unavailable))
        })
        .await?;
        let owner = zbus::fdo::DBusProxy::new(&replacement)
            .await?
            .get_name_owner("org.freedesktop.portal.Desktop".try_into()?)
            .await?;
        ensure(
            replacement.unique_name() == Some(&owner),
            "the replacement portal must own the name",
        )?;
        let states = run_states(&lost);
        match loss {
            PortalLoss::Idle => ensure(
                states.is_empty(),
                "an idle portal loss must not change the run state",
            )?,
            PortalLoss::Countdown | PortalLoss::Running => ensure(
                states.last() == Some(&RunState::Error) && !states.contains(&RunState::Clicking),
                &format!("the portal loss must end the countdown or run: {states:?}"),
            )?,
        }

        worker.send(Command::StartFromButton(ClickSettings {
            repeat: Some(1),
            ..continuous()
        }))?;
        let refused = events_until(&mut events, |e| {
            matches!(e, WorkerEvent::Error(WorkerError::HotkeyRequired))
        })
        .await?;
        ensure(
            !refused.iter().any(|e| {
                matches!(
                    e,
                    WorkerEvent::State(RunState::Starting | RunState::Clicking)
                        | WorkerEvent::Countdown(_)
                )
            }),
            "a start must wait for a hotkey registered with the new portal",
        )?;
        {
            let state = fresh.lock().unwrap_or_else(|e| e.into_inner());
            ensure(
                state.hotkey_created == 0
                    && state.remote_session.is_none()
                    && state.buttons.is_empty(),
                "nothing may use the new portal before the hotkey is registered again",
            )?;
        }
        ensure(
            presses(&observed) == presses_before,
            "no click may follow the portal loss",
        )?;

        // "Erneut versuchen" registers with the new portal and allows runs again.
        worker.send(Command::ConfigureHotkey)?;
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
        let recovered_presses = presses(&fresh);
        let state = fresh.lock().unwrap_or_else(|e| e.into_inner());
        ensure(
            state.hotkey_created == 1 && state.remote_session.is_some() && recovered_presses == 1,
            "recovery must use a new hotkey and mouse session of the new portal",
        )
    }
    .await;
    tokio::task::spawn_blocking(move || worker.shutdown()).await?;
    replacement.close().await?;
    result
}

/// A portal lost before it answered CreateSession must not leave the
/// registration pending, or "Erneut versuchen" would be ignored for good.
#[tokio::test]
#[ignore = "requires a private D-Bus session via dbus-run-session"]
async fn portal_lost_during_hotkey_session_creation_allows_retry() -> TestResult {
    let _serial = BUS_TEST_LOCK.lock().await;
    let observed = Shared::default();
    let seen = {
        let mut state = observed.lock().unwrap_or_else(|e| e.into_inner());
        state.hold_hotkey_session = true;
        state.hotkey_seen.clone()
    };
    let service = fake_portal(&observed).await?;
    let fresh = Shared::default();
    let replacement = queued_fake_portal(&fresh).await?;
    let (tx, mut events) = mpsc::unbounded_channel();
    let worker = WorkerHandle::spawn(move |event| {
        let _ = tx.send(event);
    });
    let result: TestResult = async {
        timeout(Duration::from_secs(3), seen.notified()).await?;
        // Let the method reply reach the worker, so only the Response is pending.
        sleep(Duration::from_millis(200)).await;
        service.close().await?;
        assert_hotkey_only(
            &events_until(&mut events, |e| matches!(e, WorkerEvent::Error(_))).await?,
        );
        worker.send(Command::ConfigureHotkey)?;
        event(&mut events, |e| {
            matches!(e, WorkerEvent::HotkeyPhase(HotkeyPhase::Ready))
        })
        .await?;
        ensure(
            fresh
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .hotkey_created
                == 1,
            "the retry must register with the new portal",
        )
    }
    .await;
    tokio::task::spawn_blocking(move || worker.shutdown()).await?;
    replacement.close().await?;
    result
}
