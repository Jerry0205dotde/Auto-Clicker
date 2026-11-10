//! `klickmeister --toggle` in the worker: start barrier and the check of
//! an external shortcut.

use std::time::Duration;

use klickmeister::{
    state::RunState,
    worker::{Command, ExternalShortcutPhase, HotkeyPhase, WorkerError, WorkerEvent, WorkerHandle},
};
use tokio::{sync::mpsc, time::sleep};

use super::portal::{Shared, fake_portal};
use super::support::{
    BUS_TEST_LOCK, TestResult, continuous, ensure, event, events_until, hold_first_click, presses,
    release_held_click, retry_if_click_window_missed, run_states, start_request, start_requests,
};

/// Issue #143: like `buffered_key_presses_before_stop_must_not_restart` for
/// `klickmeister --toggle`. Toggles that arrived while the worker was held in
/// a click are still queued after Stop; none of them may start a new run.
#[tokio::test]
#[ignore = "requires a private D-Bus session via dbus-run-session"]
async fn buffered_toggles_before_stop_must_not_restart() -> TestResult {
    let mut failures = Vec::new();
    for stop_button in [true, false] {
        if let Err(error) =
            retry_if_click_window_missed(|| buffered_toggles_once(stop_button)).await
        {
            let stop = if stop_button { "Stop button" } else { "toggle" };
            failures.push(format!("Stop by {stop}: {error}"));
        }
    }
    ensure(failures.is_empty(), &failures.join("; "))
}

async fn buffered_toggles_once(stop_button: bool) -> TestResult {
    let _bus_guard = BUS_TEST_LOCK.lock().await;
    let observed = Shared::default();
    let service = fake_portal(&observed).await?;
    let (tx, mut events) = mpsc::unbounded_channel();
    let worker = WorkerHandle::spawn(move |event| {
        let _ = tx.send(event);
    });
    let toggler = worker.toggler();
    let result: TestResult = async {
        event(&mut events, |e| {
            matches!(e, WorkerEvent::HotkeyPhase(HotkeyPhase::Ready))
        })
        .await?;
        let release = hold_first_click(&worker, &observed, &mut events).await?;
        // All toggles arrive before Stop, while the worker waits for the click.
        for _ in 0..10 {
            toggler.toggle()?;
        }
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
        ensure(
            start_requests(&later) == 0 && run_states(&later).is_empty(),
            &format!("old toggles must not change the stopped run: {later:?}"),
        )?;
        // A toggle after the run end starts as usual.
        toggler.toggle()?;
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

/// Issue #143: without a portal hotkey, only a desktop shortcut that the user
/// has pressed during an explicit check opens the start lock. The check
/// itself never clicks, and ordinary toggles before it never start.
#[tokio::test]
#[ignore = "requires a private D-Bus session via dbus-run-session"]
async fn external_shortcut_opens_the_start_lock_only_after_its_check() -> TestResult {
    let _bus_guard = BUS_TEST_LOCK.lock().await;
    let observed = Shared::default();
    observed
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .reject_hotkey = true;
    let service = fake_portal(&observed).await?;
    let (tx, mut events) = mpsc::unbounded_channel();
    let worker = WorkerHandle::spawn(move |event| {
        let _ = tx.send(event);
    });
    let toggler = worker.toggler();
    let result: TestResult = async {
        // The rejected binding reports Unavailable and then its error.
        let seen = events_until(&mut events, |e| matches!(e, WorkerEvent::Error(_))).await?;
        ensure(
            seen.iter()
                .any(|e| matches!(e, WorkerEvent::HotkeyPhase(HotkeyPhase::Unavailable))),
            &format!("the portal hotkey must be unavailable: {seen:?}"),
        )?;
        let unconfirmed = |events: &[WorkerEvent]| {
            events.iter().any(|e| {
                matches!(
                    e,
                    WorkerEvent::Error(WorkerError::ExternalShortcutUnconfirmed)
                )
            })
        };
        let phase = |phase| move |e: &WorkerEvent| matches!(e, WorkerEvent::ExternalShortcut(p) if *p == phase);

        // Neither a toggle nor the Start button can start yet.
        toggler.toggle()?;
        let seen = events_until(&mut events, |e| matches!(e, WorkerEvent::Error(_))).await?;
        ensure(unconfirmed(&seen), &format!("unconfirmed toggle: {seen:?}"))?;
        worker.send(Command::StartFromButton(continuous()))?;
        let seen = events_until(&mut events, |e| matches!(e, WorkerEvent::Error(_))).await?;
        ensure(
            start_requests(&seen) == 0 && run_states(&seen).is_empty(),
            &format!("no start without a stop shortcut: {seen:?}"),
        )?;

        // A cancelled check confirms nothing.
        worker.send(Command::CheckExternalShortcut)?;
        events_until(&mut events, phase(ExternalShortcutPhase::Checking)).await?;
        worker.send(Command::CancelExternalShortcutCheck)?;
        events_until(&mut events, phase(ExternalShortcutPhase::Unconfirmed)).await?;
        toggler.toggle()?;
        let seen = events_until(&mut events, |e| matches!(e, WorkerEvent::Error(_))).await?;
        ensure(unconfirmed(&seen), &format!("toggle after cancel: {seen:?}"))?;

        // The toggle during the check only confirms the shortcut.
        worker.send(Command::CheckExternalShortcut)?;
        events_until(&mut events, phase(ExternalShortcutPhase::Checking)).await?;
        toggler.toggle()?;
        events_until(&mut events, phase(ExternalShortcutPhase::Confirmed)).await?;
        sleep(Duration::from_millis(200)).await;
        let later: Vec<_> = std::iter::from_fn(|| events.try_recv().ok()).collect();
        ensure(
            start_requests(&later) == 0 && run_states(&later).is_empty() && presses(&observed) == 0,
            &format!("the check must not click: {later:?}"),
        )?;

        // Now a toggle starts and the next one stops.
        toggler.toggle()?;
        start_request(&mut events).await?.start(continuous());
        event(&mut events, |e| {
            matches!(e, WorkerEvent::State(RunState::Clicking))
        })
        .await?;
        toggler.toggle()?;
        event(&mut events, |e| {
            matches!(e, WorkerEvent::State(RunState::Stopped))
        })
        .await?;
        // The Start button works as well, with its countdown.
        worker.send(Command::StartFromButton(continuous()))?;
        event(&mut events, |e| matches!(e, WorkerEvent::Countdown(_))).await?;
        toggler.toggle()?;
        event(&mut events, |e| {
            matches!(e, WorkerEvent::State(RunState::Stopped))
        })
        .await
    }
    .await;
    tokio::task::spawn_blocking(move || worker.shutdown()).await?;
    service.close().await?;
    result
}
