//! Every text the worker, the click backend and the settings validation show.
//!
//! They report variants with their technical details; only this module turns
//! them into text, at the boundary to the interface.
use std::fmt;

use crate::{
    model::{MAX_CLICKS_PER_SECOND, MAX_INTERVAL_HOURS, MAX_REPEAT_COUNT, ValidationError},
    portal::PortalError,
    worker::{HotkeyError, SendError, WorkerError, WorkerStatus},
};

/// Status shown when an error ends a run or a start.
pub const ERROR_STATUS: &str = "Fehler";

/// Name of the start/stop hotkey in the system shortcut settings.
pub const HOTKEY_DESCRIPTION: &str = "Automatisches Klicken starten oder stoppen";

/// Status while a button start counts down.
pub fn countdown_status(remaining: u8) -> String {
    format!("Start in {remaining} s · Bewege den Mauszeiger zum Ziel.")
}

impl fmt::Display for WorkerStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Ready => "Bereit",
            Self::RegisteringHotkey => "Tastenkürzel wird eingerichtet …",
            Self::AwaitingHotkey => "Warte auf Tastenkürzel für Start und Stopp …",
            Self::AwaitingClickPermission => "Warte auf Freigabe für Maussteuerung …",
            Self::PermissionRequired => "Freigabe erneut erforderlich",
            Self::Clicking => "Klicken aktiv",
            Self::Stopped => "Gestoppt",
        })
    }
}

impl fmt::Display for WorkerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::WorkerStartFailed(details) => {
                write!(f, "Die Klicksteuerung konnte nicht gestartet werden: {details}")
            }
            Self::HotkeyRequired => {
                f.write_str("Richte vor dem Start ein Tastenkürzel für Start und Stopp ein.")
            }
            Self::InvalidSettings(error) => error.fmt(f),
            Self::Click(error) => error.fmt(f),
            Self::ClickTaskFailed(details) => {
                write!(f, "Die Maussteuerung konnte nicht gestartet werden: {details}")
            }
            Self::ClickSessionLost => f.write_str(
                "Die Freigabe für die Maussteuerung wurde unerwartet beendet. Versuche den Start erneut.",
            ),
            Self::Hotkey(error) => error.fmt(f),
        }
    }
}

impl fmt::Display for HotkeyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SessionClosed => f.write_str(
                "Die Freigabe für das Tastenkürzel wurde beendet. Wähle „Erneut versuchen“.",
            ),
            Self::RequestCancelled => {
                f.write_str("Die Freigabe für das Tastenkürzel wurde abgebrochen.")
            }
            Self::SessionMissing => {
                f.write_str("Die Freigabe für das Tastenkürzel fehlt. Wähle „Erneut versuchen“.")
            }
            Self::Unbound => f.write_str(
                "Kein Tastenkürzel für Start und Stopp eingerichtet. Wähle unter „Tastenkürzel“ eine Taste.",
            ),
            Self::Removed => f.write_str(
                "Das Tastenkürzel für Start und Stopp wurde entfernt. Wähle „Erneut versuchen“.",
            ),
            Self::Setup(details) => {
                write!(f, "Das Tastenkürzel konnte nicht eingerichtet werden: {details}")
            }
            Self::Registration(details) => write!(
                f,
                "Klickmeister konnte nicht beim KDE-Portaldienst angemeldet werden: {details}"
            ),
            Self::Unavailable(details) => write!(
                f,
                "Tastenkürzel sind nicht verfügbar. Prüfe den KDE-Portaldienst. Details: {details}"
            ),
            Self::WatchUnavailable => f.write_str(
                "Die Freigabe für das Tastenkürzel kann nicht überwacht werden. Wähle „Erneut versuchen“.",
            ),
            Self::SubscribeTimeout => {
                f.write_str("Zeitüberschreitung beim Einrichten des Tastenkürzels.")
            }
            Self::Receive(details) => {
                write!(f, "Das Tastenkürzel kann nicht empfangen werden: {details}")
            }
            Self::Request(details) => write!(
                f,
                "Die Freigabe für das Tastenkürzel konnte nicht angefordert werden: {details}"
            ),
            Self::NotGranted(details) => {
                write!(f, "Das Tastenkürzel wurde nicht freigegeben: {details}")
            }
            Self::SettingsUnavailable(details) => write!(
                f,
                "Die Einstellungen für das Tastenkürzel konnten nicht geöffnet werden: {details}"
            ),
        }
    }
}

impl fmt::Display for SendError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Closing => "Klickmeister wird geschlossen.",
            Self::Busy => "Klickmeister ist ausgelastet. Versuche es erneut.",
            Self::Stopped => "Die Klicksteuerung wurde beendet. Starte Klickmeister neu.",
        })
    }
}

impl fmt::Display for PortalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Cancelled => f.write_str("Die Freigabe wurde abgebrochen."),
            Self::Unavailable(details) => write!(
                f,
                "Die Maussteuerung ist nicht verfügbar. Prüfe den KDE-Portaldienst. Details: {details}"
            ),
            Self::EisUnavailable(details) => write!(
                f,
                "Die Maussteuerung ist nicht verfügbar. Prüfe den KDE-Portaldienst. Details: {details}"
            ),
            Self::Denied(details) => {
                write!(f, "Die Freigabe wurde abgelehnt oder abgebrochen: {details}")
            }
            Self::PointerNotGranted => f.write_str(
                "Die Maussteuerung wurde nicht freigegeben. Erlaube sie beim nächsten Start im KDE-Dialog.",
            ),
            Self::SessionWatchUnavailable => f.write_str(
                "Die Freigabe für die Maussteuerung kann nicht überwacht werden. Versuche den Start erneut.",
            ),
            Self::SessionClosed => f.write_str(
                "Die Freigabe für die Maussteuerung wurde beendet. Starte erneut, um sie anzufordern.",
            ),
            Self::MissingMonitorStream => f.write_str(
                "KDE hat keine Bildschirmdaten für die feste Position geliefert. Prüfe die Bildschirmfreigabe und starte erneut.",
            ),
            Self::MonitorMismatch => f.write_str(
                "Der ausgewählte Bildschirm stimmt nicht mit den freigegebenen Bildschirmdaten überein. Prüfe die Bildschirmanordnung und wähle die feste Position erneut.",
            ),
            Self::MonitorUnverifiable => f.write_str(
                "Die feste Position kann keinem Bildschirm eindeutig zugeordnet werden. Wähle die aktuelle Mauszeigerposition oder versuche es erneut.",
            ),
            Self::PositionOutsideStream {
                x,
                y,
                width,
                height,
            } => write!(
                f,
                "Die Position ({x}, {y}) liegt außerhalb des freigegebenen Bildschirms ({width} × {height}). Wähle eine neue Position."
            ),
            Self::Send(details) => write!(f, "Der Klick konnte nicht ausgeführt werden: {details}"),
            Self::EisSend(details) => {
                write!(f, "Der Klick konnte nicht ausgeführt werden: {details}")
            }
            Self::EventTimeout => f.write_str(
                "KDE hat die Maussteuerung nicht rechtzeitig bestätigt. Versuche den Start erneut.",
            ),
        }
    }
}

impl fmt::Display for ValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::IntervalTooShort { min_interval_ms } => write!(
                f,
                "Wähle ein Klickintervall von mindestens {min_interval_ms} ms (höchstens {MAX_CLICKS_PER_SECOND} Klicks/s)."
            ),
            Self::IntervalTooLong => write!(
                f,
                "Wähle ein Klickintervall von höchstens {MAX_INTERVAL_HOURS} Stunden."
            ),
            Self::InvalidRepeat => {
                write!(f, "Wähle zwischen 1 und {MAX_REPEAT_COUNT} Klickzyklen.")
            }
            Self::InvalidCoordinates => f.write_str(
                "Die Koordinaten liegen außerhalb des unterstützten Bereichs. Wähle eine neue Position.",
            ),
            Self::InvalidMonitorPosition => f.write_str(
                "Die feste Position liegt außerhalb des Bildschirms oder kein Bildschirm ist ausgewählt. Prüfe Bildschirm und Koordinaten.",
            ),
            Self::InvalidMouseButton => f.write_str("Wähle eine gültige Maustaste."),
            Self::InvalidClickType => f.write_str("Wähle eine gültige Klickart."),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::eis::EisError;

    /// Issue #144: the texts are the same as when the worker sent them itself.
    #[test]
    fn shows_every_message_unchanged() {
        let details = || "Testfehler".to_owned();
        let shown: Vec<(String, &str)> = vec![
            (WorkerStatus::Ready.to_string(), "Bereit"),
            (
                WorkerStatus::RegisteringHotkey.to_string(),
                "Tastenkürzel wird eingerichtet …",
            ),
            (
                WorkerStatus::AwaitingHotkey.to_string(),
                "Warte auf Tastenkürzel für Start und Stopp …",
            ),
            (
                WorkerStatus::AwaitingClickPermission.to_string(),
                "Warte auf Freigabe für Maussteuerung …",
            ),
            (
                WorkerStatus::PermissionRequired.to_string(),
                "Freigabe erneut erforderlich",
            ),
            (WorkerStatus::Clicking.to_string(), "Klicken aktiv"),
            (WorkerStatus::Stopped.to_string(), "Gestoppt"),
            (ERROR_STATUS.to_owned(), "Fehler"),
            (
                countdown_status(3),
                "Start in 3 s · Bewege den Mauszeiger zum Ziel.",
            ),
            (
                WorkerError::WorkerStartFailed(details()).to_string(),
                "Die Klicksteuerung konnte nicht gestartet werden: Testfehler",
            ),
            (
                WorkerError::HotkeyRequired.to_string(),
                "Richte vor dem Start ein Tastenkürzel für Start und Stopp ein.",
            ),
            (
                WorkerError::InvalidSettings(ValidationError::InvalidMouseButton).to_string(),
                "Wähle eine gültige Maustaste.",
            ),
            (
                WorkerError::Click(PortalError::Cancelled).to_string(),
                "Die Freigabe wurde abgebrochen.",
            ),
            (
                WorkerError::ClickTaskFailed(details()).to_string(),
                "Die Maussteuerung konnte nicht gestartet werden: Testfehler",
            ),
            (
                WorkerError::ClickSessionLost.to_string(),
                "Die Freigabe für die Maussteuerung wurde unerwartet beendet. Versuche den Start erneut.",
            ),
            (
                WorkerError::Hotkey(HotkeyError::SessionClosed).to_string(),
                "Die Freigabe für das Tastenkürzel wurde beendet. Wähle „Erneut versuchen“.",
            ),
            (
                HotkeyError::RequestCancelled.to_string(),
                "Die Freigabe für das Tastenkürzel wurde abgebrochen.",
            ),
            (
                HotkeyError::SessionMissing.to_string(),
                "Die Freigabe für das Tastenkürzel fehlt. Wähle „Erneut versuchen“.",
            ),
            (
                HotkeyError::Unbound.to_string(),
                "Kein Tastenkürzel für Start und Stopp eingerichtet. Wähle unter „Tastenkürzel“ eine Taste.",
            ),
            (
                HotkeyError::Removed.to_string(),
                "Das Tastenkürzel für Start und Stopp wurde entfernt. Wähle „Erneut versuchen“.",
            ),
            (
                HotkeyError::Setup(details()).to_string(),
                "Das Tastenkürzel konnte nicht eingerichtet werden: Testfehler",
            ),
            (
                HotkeyError::Registration(details()).to_string(),
                "Klickmeister konnte nicht beim KDE-Portaldienst angemeldet werden: Testfehler",
            ),
            (
                HotkeyError::Unavailable(details()).to_string(),
                "Tastenkürzel sind nicht verfügbar. Prüfe den KDE-Portaldienst. Details: Testfehler",
            ),
            (
                HotkeyError::WatchUnavailable.to_string(),
                "Die Freigabe für das Tastenkürzel kann nicht überwacht werden. Wähle „Erneut versuchen“.",
            ),
            (
                HotkeyError::SubscribeTimeout.to_string(),
                "Zeitüberschreitung beim Einrichten des Tastenkürzels.",
            ),
            (
                HotkeyError::Receive(details()).to_string(),
                "Das Tastenkürzel kann nicht empfangen werden: Testfehler",
            ),
            (
                HotkeyError::Request(details()).to_string(),
                "Die Freigabe für das Tastenkürzel konnte nicht angefordert werden: Testfehler",
            ),
            (
                HotkeyError::NotGranted(details()).to_string(),
                "Das Tastenkürzel wurde nicht freigegeben: Testfehler",
            ),
            (
                HotkeyError::SettingsUnavailable(details()).to_string(),
                "Die Einstellungen für das Tastenkürzel konnten nicht geöffnet werden: Testfehler",
            ),
            (
                SendError::Closing.to_string(),
                "Klickmeister wird geschlossen.",
            ),
            (
                SendError::Busy.to_string(),
                "Klickmeister ist ausgelastet. Versuche es erneut.",
            ),
            (
                SendError::Stopped.to_string(),
                "Die Klicksteuerung wurde beendet. Starte Klickmeister neu.",
            ),
            (
                PortalError::EisUnavailable(EisError::NoDevice).to_string(),
                "Die Maussteuerung ist nicht verfügbar. Prüfe den KDE-Portaldienst. Details: kein passendes libei-Zeigegerät",
            ),
            (
                PortalError::PointerNotGranted.to_string(),
                "Die Maussteuerung wurde nicht freigegeben. Erlaube sie beim nächsten Start im KDE-Dialog.",
            ),
            (
                PortalError::SessionWatchUnavailable.to_string(),
                "Die Freigabe für die Maussteuerung kann nicht überwacht werden. Versuche den Start erneut.",
            ),
            (
                PortalError::SessionClosed.to_string(),
                "Die Freigabe für die Maussteuerung wurde beendet. Starte erneut, um sie anzufordern.",
            ),
            (
                PortalError::MissingMonitorStream.to_string(),
                "KDE hat keine Bildschirmdaten für die feste Position geliefert. Prüfe die Bildschirmfreigabe und starte erneut.",
            ),
            (
                PortalError::MonitorMismatch.to_string(),
                "Der ausgewählte Bildschirm stimmt nicht mit den freigegebenen Bildschirmdaten überein. Prüfe die Bildschirmanordnung und wähle die feste Position erneut.",
            ),
            (
                PortalError::MonitorUnverifiable.to_string(),
                "Die feste Position kann keinem Bildschirm eindeutig zugeordnet werden. Wähle die aktuelle Mauszeigerposition oder versuche es erneut.",
            ),
            (
                PortalError::PositionOutsideStream {
                    x: 10,
                    y: 20,
                    width: 1920,
                    height: 1080,
                }
                .to_string(),
                "Die Position (10, 20) liegt außerhalb des freigegebenen Bildschirms (1920 × 1080). Wähle eine neue Position.",
            ),
            (
                PortalError::EisSend(EisError::Disconnected).to_string(),
                "Der Klick konnte nicht ausgeführt werden: libei-Verbindung beendet",
            ),
            (
                PortalError::EventTimeout.to_string(),
                "KDE hat die Maussteuerung nicht rechtzeitig bestätigt. Versuche den Start erneut.",
            ),
            (
                ValidationError::IntervalTooShort { min_interval_ms: 4 }.to_string(),
                "Wähle ein Klickintervall von mindestens 4 ms (höchstens 500 Klicks/s).",
            ),
            (
                ValidationError::IntervalTooLong.to_string(),
                "Wähle ein Klickintervall von höchstens 24 Stunden.",
            ),
            (
                ValidationError::InvalidRepeat.to_string(),
                "Wähle zwischen 1 und 10000000 Klickzyklen.",
            ),
            (
                ValidationError::InvalidCoordinates.to_string(),
                "Die Koordinaten liegen außerhalb des unterstützten Bereichs. Wähle eine neue Position.",
            ),
            (
                ValidationError::InvalidMonitorPosition.to_string(),
                "Die feste Position liegt außerhalb des Bildschirms oder kein Bildschirm ist ausgewählt. Prüfe Bildschirm und Koordinaten.",
            ),
            (
                ValidationError::InvalidClickType.to_string(),
                "Wähle eine gültige Klickart.",
            ),
        ];
        for (shown, expected) in shown {
            assert_eq!(shown, expected);
        }
    }
}
