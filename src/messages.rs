//! Every text that Klickmeister's Rust code shows.
//!
//! The worker, the click backend, the settings and the controller report
//! variants with their technical details; only this module turns them into
//! text, at the boundary to the interface. The texts are English and pass
//! through Qt's translation. `scripts/update-translations.sh` reads this file
//! with lupdate's C++ parser, which only finds literal
//! `translate("Klickmeister", "...")` calls, so every text is written out that
//! way, and the file avoids Rust lifetimes, which that parser mistakes for
//! character literals.
use std::{fmt, io, path::Path};

use crate::{
    config::ConfigError,
    eis::EisError,
    i18n::{fill, translate},
    instance::{InstanceError, ToggleError},
    model::{MAX_CLICKS_PER_SECOND, MAX_INTERVAL_HOURS, MAX_REPEAT_COUNT, ValidationError},
    portal::PortalError,
    worker::{HotkeyError, SendError, WorkerError, WorkerStatus},
};

/// Write a translated text with its placeholders filled in.
fn write(f: &mut fmt::Formatter, text: String, args: &[&dyn fmt::Display]) -> fmt::Result {
    f.write_str(&fill(&text, args))
}

/// Status shown when an error ends a run or a start.
pub fn error_status() -> String {
    translate("Klickmeister", "Error")
}

/// Name of the start/stop hotkey in the system shortcut settings.
pub fn hotkey_description() -> String {
    translate("Klickmeister", "Start or stop automatic clicking")
}

/// Status while a button start counts down.
pub fn countdown_status(remaining: u8) -> String {
    fill(
        &translate(
            "Klickmeister",
            "Start in %1 s · Move the pointer to the target.",
        ),
        &[&remaining],
    )
}

/// Error shown at startup when the settings file could not be loaded.
pub fn unloaded_config_notice(error: &ConfigError) -> String {
    fill(
        &translate(
            "Klickmeister",
            "%1\n\nKlickmeister uses default values for now. Before changed settings are saved, the file is backed up next to it as config.toml.invalid (with a number appended if needed).",
        ),
        &[error],
    )
}

pub fn interval_not_positive() -> String {
    translate("Klickmeister", "Choose a click interval greater than 0 ms.")
}

pub fn invalid_repeat_count() -> String {
    translate(
        "Klickmeister",
        "The number of click cycles is invalid. Check the value.",
    )
}

pub fn invalid_coordinates() -> String {
    translate(
        "Klickmeister",
        "The coordinates are invalid. Choose a new position.",
    )
}

pub fn no_repeat() -> String {
    translate("Klickmeister", "Choose at least one click cycle.")
}

pub fn negative_x() -> String {
    translate(
        "Klickmeister",
        "The X coordinate must be a whole number from 0.",
    )
}

pub fn negative_y() -> String {
    translate(
        "Klickmeister",
        "The Y coordinate must be a whole number from 0.",
    )
}

pub fn position_unconfirmed() -> String {
    translate(
        "Klickmeister",
        "Check the screen and coordinates and confirm the fixed position.",
    )
}

pub fn worker_unavailable() -> String {
    translate(
        "Klickmeister",
        "The click control is not available. Restart Klickmeister.",
    )
}

/// Detail of a failed backup of a settings file that was not loaded.
pub fn config_backup_failed(backup: &Path, error: &io::Error) -> String {
    fill(
        &translate(
            "Klickmeister",
            "The settings file that was not loaded could not be backed up to %1: %2",
        ),
        &[&backup.display(), error],
    )
}

/// Detail of a settings file link whose target cannot be resolved.
pub fn config_link_unresolved(path: &Path, error: &io::Error) -> String {
    fill(
        &translate(
            "Klickmeister",
            "The link target of the settings file %1 could not be resolved: %2",
        ),
        &[&path.display(), error],
    )
}

pub fn config_without_parent() -> String {
    translate(
        "Klickmeister",
        "The settings location has no parent directory.",
    )
}

/// Printed when a second start showed the running instance's window.
pub fn already_running() -> String {
    translate(
        "Klickmeister",
        "Klickmeister is already running; its window was shown.",
    )
}

pub fn window_not_loaded() -> String {
    translate(
        "Klickmeister",
        "Klickmeister could not be opened: The application window could not be loaded.",
    )
}

/// Reason the running instance gives a second start that could not show it.
pub fn window_unavailable() -> String {
    translate(
        "Klickmeister",
        "The Klickmeister window is not available right now.",
    )
}

/// Reply to `klickmeister --toggle` before the worker has started.
pub fn toggle_not_ready() -> String {
    translate("Klickmeister", "Klickmeister is not ready yet.")
}

/// Reply to an `ActivateAction` other than `toggle-clicking`.
pub fn unknown_action(action: &str) -> String {
    fill(&translate("Klickmeister", "Unknown action: %1"), &[&action])
}

/// Reason a second start gives when the running instance does not answer.
pub fn instance_not_answering() -> String {
    translate("Klickmeister", "The running instance does not answer.")
}

/// Printed when closing the permissions outlasts the shutdown wait.
pub fn slow_shutdown(seconds: u64) -> String {
    fill(
        &translate(
            "Klickmeister",
            "Clicking ended. Closing the permissions takes longer than %1 s and continues in the background.",
        ),
        &[&seconds],
    )
}

pub fn instance_check_ended() -> String {
    translate("Klickmeister", "The check thread ended unexpectedly.")
}

impl fmt::Display for WorkerStatus {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str(&match self {
            Self::Ready => translate("Klickmeister", "Ready"),
            Self::RegisteringHotkey => translate("Klickmeister", "Setting up the shortcut…"),
            Self::AwaitingHotkey => {
                translate("Klickmeister", "Waiting for a start and stop shortcut…")
            }
            Self::AwaitingClickPermission => translate(
                "Klickmeister",
                "Waiting for permission to control the mouse…",
            ),
            Self::PermissionRequired => translate("Klickmeister", "Permission required again"),
            Self::Clicking => translate("Klickmeister", "Clicking"),
            Self::Stopped => translate("Klickmeister", "Stopped"),
        })
    }
}

impl fmt::Display for WorkerError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::WorkerStartFailed(details) => write(
                f,
                translate("Klickmeister", "The click control could not be started: %1"),
                &[details],
            ),
            Self::HotkeyRequired => f.write_str(&translate(
                "Klickmeister",
                "Set up a start and stop shortcut before starting.",
            )),
            Self::ExternalShortcutUnconfirmed => f.write_str(&translate(
                "Klickmeister",
                "Your shortcut for “klickmeister --toggle” is not confirmed yet. Choose “Check shortcut” and press it once.",
            )),
            Self::InvalidSettings(error) => error.fmt(f),
            Self::Click(error) => error.fmt(f),
            Self::ClickTaskFailed(details) => write(
                f,
                translate("Klickmeister", "Mouse control could not be started: %1"),
                &[details],
            ),
            Self::ClickSessionLost => f.write_str(&translate(
                "Klickmeister",
                "The permission to control the mouse ended unexpectedly. Try starting again.",
            )),
            Self::Hotkey(error) => error.fmt(f),
        }
    }
}

impl fmt::Display for HotkeyError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::SessionClosed => f.write_str(&translate(
                "Klickmeister",
                "The shortcut permission has ended. Choose “Try again”.",
            )),
            Self::RequestCancelled => f.write_str(&translate(
                "Klickmeister",
                "The shortcut permission was cancelled.",
            )),
            Self::SessionMissing => f.write_str(&translate(
                "Klickmeister",
                "The shortcut permission is missing. Choose “Try again”.",
            )),
            Self::Unbound => f.write_str(&translate(
                "Klickmeister",
                "No start and stop shortcut is set up. Choose a key under “Shortcut”.",
            )),
            Self::Removed => f.write_str(&translate(
                "Klickmeister",
                "The start and stop shortcut was removed. Choose “Try again”.",
            )),
            Self::Setup(details) => write(
                f,
                translate("Klickmeister", "The shortcut could not be set up: %1"),
                &[details],
            ),
            Self::Registration(details) => write(
                f,
                translate(
                    "Klickmeister",
                    "Klickmeister could not register with the KDE portal service: %1",
                ),
                &[details],
            ),
            Self::Unavailable(details) => write(
                f,
                translate(
                    "Klickmeister",
                    "Shortcuts are not available. Check the KDE portal service. Details: %1",
                ),
                &[details],
            ),
            Self::WatchUnavailable => f.write_str(&translate(
                "Klickmeister",
                "The shortcut permission cannot be monitored. Choose “Try again”.",
            )),
            Self::SubscribeTimeout => f.write_str(&translate(
                "Klickmeister",
                "Setting up the shortcut timed out.",
            )),
            Self::Receive(details) => write(
                f,
                translate("Klickmeister", "The shortcut cannot be received: %1"),
                &[details],
            ),
            Self::Request(details) => write(
                f,
                translate(
                    "Klickmeister",
                    "The shortcut permission could not be requested: %1",
                ),
                &[details],
            ),
            Self::NotGranted(details) => write(
                f,
                translate("Klickmeister", "The shortcut was not permitted: %1"),
                &[details],
            ),
            Self::SettingsUnavailable(details) => write(
                f,
                translate(
                    "Klickmeister",
                    "The shortcut settings could not be opened: %1",
                ),
                &[details],
            ),
        }
    }
}

impl fmt::Display for SendError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str(&match self {
            Self::Closing => translate("Klickmeister", "Klickmeister is closing."),
            Self::Busy => translate("Klickmeister", "Klickmeister is busy. Try again."),
            Self::Stopped => translate(
                "Klickmeister",
                "The click control has ended. Restart Klickmeister.",
            ),
        })
    }
}

impl fmt::Display for PortalError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let unavailable = || {
            translate(
                "Klickmeister",
                "Mouse control is not available. Check the KDE portal service. Details: %1",
            )
        };
        let not_sent = || translate("Klickmeister", "The click could not be performed: %1");
        match self {
            Self::Cancelled => {
                f.write_str(&translate("Klickmeister", "The permission was cancelled."))
            }
            Self::Unavailable(details) => write(f, unavailable(), &[details]),
            Self::EisUnavailable(details) => write(f, unavailable(), &[details]),
            Self::Denied(details) => write(
                f,
                translate("Klickmeister", "The permission was denied or cancelled: %1"),
                &[details],
            ),
            Self::PointerNotGranted => f.write_str(&translate(
                "Klickmeister",
                "Mouse control was not permitted. Allow it in the KDE dialog on the next start.",
            )),
            Self::SessionWatchUnavailable => f.write_str(&translate(
                "Klickmeister",
                "The permission to control the mouse cannot be monitored. Try starting again.",
            )),
            Self::SessionClosed => f.write_str(&translate(
                "Klickmeister",
                "The permission to control the mouse has ended. Start again to request it.",
            )),
            Self::MissingMonitorStream => f.write_str(&translate(
                "Klickmeister",
                "KDE did not provide screen data for the fixed position. Check the screen sharing permission and start again.",
            )),
            Self::MonitorMismatch => f.write_str(&translate(
                "Klickmeister",
                "The selected screen does not match the shared screen data. Check the screen arrangement and choose the fixed position again.",
            )),
            Self::MonitorUnverifiable => f.write_str(&translate(
                "Klickmeister",
                "The fixed position cannot be matched to exactly one screen. Choose the current pointer position or try again.",
            )),
            Self::PositionOutsideStream {
                x,
                y,
                width,
                height,
            } => write(
                f,
                translate(
                    "Klickmeister",
                    "The position (%1, %2) lies outside the shared screen (%3 × %4). Choose a new position.",
                ),
                &[x, y, width, height],
            ),
            Self::Send(details) => write(f, not_sent(), &[details]),
            Self::EisSend(details) => write(f, not_sent(), &[details]),
            Self::EventTimeout => f.write_str(&translate(
                "Klickmeister",
                "KDE did not confirm mouse control in time. Try starting again.",
            )),
        }
    }
}

impl fmt::Display for ValidationError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::IntervalTooShort { min_interval_ms } => write(
                f,
                translate(
                    "Klickmeister",
                    "Choose a click interval of at least %1 ms (at most %2 clicks/s).",
                ),
                &[min_interval_ms, &MAX_CLICKS_PER_SECOND],
            ),
            Self::IntervalTooLong => write(
                f,
                translate(
                    "Klickmeister",
                    "Choose a click interval of at most %1 hours.",
                ),
                &[&MAX_INTERVAL_HOURS],
            ),
            Self::InvalidRepeat => write(
                f,
                translate("Klickmeister", "Choose between 1 and %1 click cycles."),
                &[&MAX_REPEAT_COUNT],
            ),
            Self::InvalidCoordinates => f.write_str(&translate(
                "Klickmeister",
                "The coordinates are outside the supported range. Choose a new position.",
            )),
            Self::InvalidMonitorPosition => f.write_str(&translate(
                "Klickmeister",
                "The fixed position is outside the screen, or no screen is selected. Check the screen and coordinates.",
            )),
            Self::InvalidMouseButton => {
                f.write_str(&translate("Klickmeister", "Choose a valid mouse button."))
            }
            Self::InvalidClickType => {
                f.write_str(&translate("Klickmeister", "Choose a valid click type."))
            }
        }
    }
}

impl fmt::Display for EisError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::Io(details) => write(
                f,
                translate("Klickmeister", "libei connection failed: %1"),
                &[details],
            ),
            Self::Protocol(details) => write(
                f,
                translate("Klickmeister", "libei protocol error: %1"),
                &[details],
            ),
            Self::Disconnected => f.write_str(&translate("Klickmeister", "libei connection ended")),
            Self::NoDevice => f.write_str(&translate(
                "Klickmeister",
                "no matching libei pointer device",
            )),
            Self::NoRegion => {
                f.write_str(&translate("Klickmeister", "no libei region for the screen"))
            }
        }
    }
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::MissingHome => f.write_str(&translate(
                "Klickmeister",
                "The settings location is missing. HOME or XDG_CONFIG_HOME must be an absolute path.",
            )),
            Self::Read(details) => write(
                f,
                translate("Klickmeister", "Settings could not be loaded: %1"),
                &[details],
            ),
            Self::Parse(details) => write(
                f,
                translate("Klickmeister", "The settings file is invalid: %1"),
                &[details],
            ),
            Self::Write(details) => write(
                f,
                translate("Klickmeister", "Settings could not be saved: %1"),
                &[details],
            ),
            Self::Serialize(details) => write(
                f,
                translate("Klickmeister", "Settings could not be prepared for saving: %1"),
                &[details],
            ),
        }
    }
}

impl fmt::Display for ToggleError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::NoSessionBus(details) => write(
                f,
                translate("Klickmeister", "The D-Bus session is not reachable: %1"),
                &[details],
            ),
            Self::NotRunning => f.write_str(&translate(
                "Klickmeister",
                "Klickmeister is not running. Open Klickmeister first.",
            )),
            Self::Refused(details) => write(
                f,
                translate(
                    "Klickmeister",
                    "Klickmeister did not accept the command: %1",
                ),
                &[details],
            ),
            Self::Timeout => f.write_str(&translate(
                "Klickmeister",
                "Klickmeister does not answer. The command may not have been carried out.",
            )),
            Self::Thread(details) => write(
                f,
                translate("Klickmeister", "Klickmeister could not be reached: %1"),
                &[details],
            ),
        }
    }
}

impl fmt::Display for InstanceError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let unchecked = || {
            translate(
                "Klickmeister",
                "Cannot check whether Klickmeister is already running: %1",
            )
        };
        match self {
            Self::NoSessionBus(details) => write(
                f,
                translate("Klickmeister", "The D-Bus session is not reachable: %1"),
                &[details],
            ),
            Self::Activation(details) => write(
                f,
                translate(
                    "Klickmeister",
                    "Klickmeister is already running, but its window could not be shown: %1",
                ),
                &[details],
            ),
            Self::Bus(details) => write(f, unchecked(), &[details]),
            Self::Thread(details) => write(f, unchecked(), &[details]),
        }
    }
}
