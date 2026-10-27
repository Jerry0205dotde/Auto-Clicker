//! Single-instance claims on a private bus, without Qt.
use std::{
    sync::{Arc, Mutex, mpsc},
    thread,
    time::Duration,
};

use klickmeister::instance::{self, Activation, Claim};

type TestResult = Result<(), Box<dyn std::error::Error>>;
/// Every test competes for the same bus name.
static BUS_TEST_LOCK: Mutex<()> = Mutex::new(());

fn ensure(condition: bool, message: &str) -> TestResult {
    if condition {
        Ok(())
    } else {
        Err(message.into())
    }
}

fn primary(claim: Claim) -> Result<instance::InstanceGuard, Box<dyn std::error::Error>> {
    match claim {
        Claim::Primary(guard) => Ok(guard),
        Claim::Secondary => Err("expected to own the instance name".into()),
    }
}

/// Handler of a launch that is not expected to own the name.
fn refuse(activation: Activation) {
    activation.finish(false);
}

#[test]
#[ignore = "requires a private D-Bus session via dbus-run-session"]
fn later_launches_activate_the_running_instance_with_their_token() -> TestResult {
    let _serial = BUS_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tokens = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::clone(&tokens);
    let guard = primary(instance::claim(None, move |activation| {
        seen.lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(activation.token().to_owned());
        activation.finish(true);
    })?)?;

    let later = instance::claim(Some("launcher-token".into()), refuse)?;
    ensure(
        matches!(later, Claim::Secondary),
        "second launch must hand over",
    )?;
    let later = instance::claim(None, refuse)?;
    ensure(
        matches!(later, Claim::Secondary),
        "third launch must hand over",
    )?;
    ensure(
        *tokens.lock().unwrap_or_else(|e| e.into_inner()) == ["launcher-token", ""],
        "each launch must forward its own activation token",
    )?;

    // Dropping the guard releases the name for the next launch.
    drop(guard);
    let _next = primary(instance::claim(None, refuse)?)?;
    Ok(())
}

#[test]
#[ignore = "requires a private D-Bus session via dbus-run-session"]
fn a_closing_instance_hands_over_once_it_has_ended() -> TestResult {
    let _serial = BUS_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    // A refused request (window closing) and a dropped one (application quit
    // before the Qt thread answered) must both make the later launch wait.
    for answered in [true, false] {
        let (requested_tx, requested) = mpsc::channel();
        let guard = primary(instance::claim(None, move |activation| {
            let _ = requested_tx.send(());
            if answered {
                activation.finish(false);
            }
        })?)?;
        let closing = thread::spawn(move || {
            let requested = requested.recv_timeout(Duration::from_secs(5)).is_ok();
            thread::sleep(Duration::from_millis(300));
            drop(guard);
            requested
        });
        let later = instance::claim(None, refuse);
        let requested = closing.join().map_err(|_| "closing instance panicked")?;
        ensure(
            requested,
            "later launch must first ask the running instance",
        )?;
        let _later = primary(later?)?;
    }
    Ok(())
}
