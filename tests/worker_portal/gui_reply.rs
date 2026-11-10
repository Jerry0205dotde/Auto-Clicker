//! Qt's delayed reply to a hotkey start request.

use std::time::Duration;

use klickmeister::{
    model::{ClickSettings, MouseButton},
    state::RunState,
    worker::{Command, HotkeyPhase, WorkerEvent, WorkerHandle},
};
use tokio::{sync::mpsc, time::sleep};

use super::portal::{Options, Shared, fake_portal};
use super::support::{
    BUS_TEST_LOCK, TestResult, continuous, ensure, event, events_until, hotkey_session, run_states,
    shortcut_signal, shortcuts_changed, start_request, start_requests, version_reads,
    wait_for_ordering_reply,
};

#[derive(Clone, Copy, Debug)]
enum GuiCancellation {
    Stop,
    SecondPress,
    Configure,
    Rebind,
    Remove,
    SessionClosed,
    Decline,
    InvalidSettings,
}

#[tokio::test]
#[ignore = "requires a private D-Bus session via dbus-run-session"]
async fn delayed_gui_reply_cannot_start_after_stop() -> TestResult {
    cancelled_gui_start_once(GuiCancellation::Stop).await
}

#[tokio::test]
#[ignore = "requires a private D-Bus session via dbus-run-session"]
async fn two_hotkey_presses_before_gui_reply_toggle_back_to_stopped() -> TestResult {
    cancelled_gui_start_once(GuiCancellation::SecondPress).await
}

#[tokio::test]
#[ignore = "requires a private D-Bus session via dbus-run-session"]
async fn hotkey_changes_revoke_delayed_gui_reply() -> TestResult {
    for cancellation in [
        GuiCancellation::Configure,
        GuiCancellation::Rebind,
        GuiCancellation::Remove,
        GuiCancellation::SessionClosed,
    ] {
        cancelled_gui_start_once(cancellation)
            .await
            .map_err(|error| format!("{cancellation:?}: {error}"))?;
    }
    Ok(())
}

#[tokio::test]
#[ignore = "requires a private D-Bus session via dbus-run-session"]
async fn declined_or_invalid_gui_reply_does_not_latch_starting() -> TestResult {
    for cancellation in [GuiCancellation::Decline, GuiCancellation::InvalidSettings] {
        cancelled_gui_start_once(cancellation)
            .await
            .map_err(|error| format!("{cancellation:?}: {error}"))?;
    }
    Ok(())
}

/// Keep the first Qt reply until after cancellation, then answer it while a
/// different activation is pending. It must neither click nor consume that start.
async fn cancelled_gui_start_once(cancellation: GuiCancellation) -> TestResult {
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
        shortcut_signal(&service, &session, "Activated", 1).await?;
        let mut emitted =
            events_until(&mut events, |e| matches!(e, WorkerEvent::StartRequested(_))).await?;
        ensure(
            run_states(&emitted) == [RunState::Starting],
            "the worker must reserve Starting before requesting Qt settings",
        )?;
        let mut delayed = match emitted.pop() {
            Some(WorkerEvent::StartRequested(request)) => Some(request),
            _ => return Err("missing GUI request".into()),
        };
        let reads = version_reads(&observed);
        match cancellation {
            GuiCancellation::Stop => worker.send(Command::Stop)?,
            GuiCancellation::SecondPress => {
                shortcut_signal(&service, &session, "Activated", 2).await?;
            }
            GuiCancellation::Configure => worker.send(Command::ConfigureHotkey)?,
            GuiCancellation::Rebind => shortcuts_changed(&service, session.as_str(), "F8").await?,
            GuiCancellation::Remove => shortcuts_changed(&service, session.as_str(), "").await?,
            GuiCancellation::SessionClosed => {
                service
                    .emit_signal(
                        None::<&str>,
                        &session,
                        "org.freedesktop.portal.Session",
                        "Closed",
                        &(Options::new(),),
                    )
                    .await?;
            }
            // The picker, invalid/unavailable Qt controls, or a failed Qt queue
            // all decline by dropping the single-use reply.
            GuiCancellation::Decline => drop(delayed.take()),
            GuiCancellation::InvalidSettings => {
                let request = delayed.take().ok_or("missing GUI request")?;
                request.start(ClickSettings {
                    interval_ms: 0,
                    ..continuous()
                });
            }
        }
        let ended = events_until(&mut events, |e| {
            matches!(e, WorkerEvent::State(RunState::Stopped | RunState::Error))
        })
        .await?;
        ensure(
            start_requests(&ended) == 0,
            "a second press must stop, not request another start",
        )?;
        match cancellation {
            GuiCancellation::Configure => {
                event(&mut events, |e| {
                    matches!(e, WorkerEvent::HotkeyPhase(HotkeyPhase::Configuring(true)))
                })
                .await?;
                let release = observed
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .configure_release
                    .clone();
                release.notify_one();
                event(&mut events, |e| {
                    matches!(e, WorkerEvent::HotkeyPhase(HotkeyPhase::Ready))
                })
                .await?;
            }
            GuiCancellation::Remove
            | GuiCancellation::SessionClosed
            | GuiCancellation::InvalidSettings => {
                events_until(&mut events, |e| matches!(e, WorkerEvent::Error(_))).await?;
            }
            _ => {}
        }
        wait_for_ordering_reply(&observed, reads).await;
        if let Some(request) = &delayed {
            ensure(
                request.is_cancelled(),
                "the pending GUI reply must be revoked",
            )?;
        }
        match cancellation {
            GuiCancellation::Remove => {
                shortcuts_changed(&service, session.as_str(), "Pause").await?;
                event(&mut events, |e| {
                    matches!(e, WorkerEvent::HotkeyPhase(HotkeyPhase::Ready))
                })
                .await?;
            }
            GuiCancellation::SessionClosed => {
                worker.send(Command::ConfigureHotkey)?;
                event(&mut events, |e| {
                    matches!(e, WorkerEvent::HotkeyPhase(HotkeyPhase::Ready))
                })
                .await?;
            }
            _ => {}
        }
        let session = hotkey_session(&observed)?;
        shortcut_signal(&service, &session, "Activated", 3).await?;
        let fresh = start_request(&mut events).await?;
        if let Some(delayed) = delayed {
            delayed.start(continuous());
        }
        // Even when delivered during the next reservation, an old reply does
        // not open a permission request or create pointer input.
        sleep(Duration::from_millis(100)).await;
        {
            let state = observed.lock().unwrap_or_else(|e| e.into_inner());
            ensure(
                state.remote_session.is_none() && state.buttons.is_empty(),
                "a cancelled GUI reply started mouse control",
            )?;
        }
        ensure(
            !fresh.is_cancelled(),
            "an old reply consumed the fresh reservation",
        )?;
        let skipped: Vec<_> = std::iter::from_fn(|| events.try_recv().ok()).collect();
        ensure(
            run_states(&skipped).is_empty() && start_requests(&skipped) == 0,
            &format!("unexpected events before fresh reply: {skipped:?}"),
        )?;
        // These current settings differ from the cancelled request. No cached
        // settings and no button countdown may replace the Qt reply.
        fresh.start(ClickSettings {
            button: MouseButton::Right,
            repeat: Some(2),
            ..continuous()
        });
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
            state.buttons == [(273, 1), (273, 0), (273, 1), (273, 0)],
            "fresh activation did not use its own current Qt settings",
        )
    }
    .await;
    tokio::task::spawn_blocking(move || worker.shutdown()).await?;
    service.close().await?;
    result
}
