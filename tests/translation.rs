//! The German interface keeps the texts it had before the texts became English.
//! One test per process: Qt reads the language once, when the application starts.
use std::{io, path::Path};

use ashpd::zbus;
use cxx_qt_lib::QCoreApplication;
use klickmeister::{
    config::ConfigError,
    eis::EisError,
    i18n::{install_translator, translate},
    instance::InstanceError,
    messages::*,
    model::ValidationError,
    portal::PortalError,
    worker::{HotkeyError, SendError, WorkerError, WorkerStatus},
};

#[test]
fn german_translation_keeps_every_text() {
    // SAFETY: No other thread of this test process reads the environment yet.
    unsafe {
        std::env::set_var("LANGUAGE", "de");
        std::env::set_var("LC_ALL", "de_DE.UTF-8");
    }
    let _application = QCoreApplication::new();
    install_translator();
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
        (error_status(), "Fehler"),
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
        (
            hotkey_description(),
            "Automatisches Klicken starten oder stoppen",
        ),
        (
            ValidationError::InvalidMouseButton.to_string(),
            "Wähle eine gültige Maustaste.",
        ),
        (
            EisError::Io(io::Error::other("Testfehler")).to_string(),
            "libei-Verbindung gestört: Testfehler",
        ),
        (
            EisError::Protocol("Testfehler".to_owned()).to_string(),
            "libei-Protokollfehler: Testfehler",
        ),
        (
            EisError::NoRegion.to_string(),
            "kein libei-Bereich für den Bildschirm",
        ),
        (
            ConfigError::MissingHome.to_string(),
            "Der Speicherort für die Einstellungen fehlt. HOME oder XDG_CONFIG_HOME muss ein absoluter Pfad sein.",
        ),
        (
            ConfigError::Read(io::Error::other("Testfehler")).to_string(),
            "Einstellungen konnten nicht geladen werden: Testfehler",
        ),
        (
            ConfigError::Write(io::Error::other("Testfehler")).to_string(),
            "Einstellungen konnten nicht gespeichert werden: Testfehler",
        ),
        (
            unloaded_config_notice(&ConfigError::Read(io::Error::other("Testfehler"))),
            "Einstellungen konnten nicht geladen werden: Testfehler\n\nKlickmeister nutzt vorerst Standardwerte. Bevor geänderte Einstellungen gespeichert werden, wird die Datei daneben als config.toml.invalid gesichert (bei Bedarf mit angehängter Nummer).",
        ),
        (
            config_backup_failed(
                Path::new("/a/config.toml.invalid"),
                &io::Error::other("Testfehler"),
            ),
            "Die nicht geladene Einstellungsdatei konnte nicht nach /a/config.toml.invalid gesichert werden: Testfehler",
        ),
        (
            config_link_unresolved(Path::new("/a/config.toml"), &io::Error::other("Testfehler")),
            "Das Linkziel der Einstellungsdatei /a/config.toml konnte nicht aufgelöst werden: Testfehler",
        ),
        (
            config_without_parent(),
            "Der Speicherort hat kein übergeordnetes Verzeichnis.",
        ),
        (
            interval_not_positive(),
            "Wähle ein Klickintervall größer als 0 ms.",
        ),
        (
            invalid_repeat_count(),
            "Die Anzahl der Klickzyklen ist ungültig. Prüfe den Wert.",
        ),
        (
            invalid_coordinates(),
            "Die Koordinaten sind ungültig. Wähle eine neue Position.",
        ),
        (no_repeat(), "Wähle mindestens einen Klickzyklus."),
        (
            negative_x(),
            "Die X-Koordinate muss eine ganze Zahl ab 0 sein.",
        ),
        (
            negative_y(),
            "Die Y-Koordinate muss eine ganze Zahl ab 0 sein.",
        ),
        (
            position_unconfirmed(),
            "Prüfe Bildschirm und Koordinaten und bestätige die feste Position.",
        ),
        (
            worker_unavailable(),
            "Die Klicksteuerung ist nicht verfügbar. Starte Klickmeister neu.",
        ),
        (
            already_running(),
            "Klickmeister läuft bereits; das vorhandene Fenster wurde angezeigt.",
        ),
        (
            window_not_loaded(),
            "Klickmeister konnte nicht geöffnet werden: Das Anwendungsfenster konnte nicht geladen werden.",
        ),
        (
            window_unavailable(),
            "Das Klickmeister-Fenster ist gerade nicht verfügbar.",
        ),
        (
            instance_not_answering(),
            "Die laufende Instanz antwortet nicht.",
        ),
        (
            slow_shutdown(5),
            "Klicken beendet. Das Schließen der Freigaben dauert länger als 5 s und wird im Hintergrund fortgesetzt.",
        ),
        (
            instance_check_ended(),
            "Der Prüfthread wurde unerwartet beendet.",
        ),
        (
            InstanceError::Activation("Testfehler".to_owned()).to_string(),
            "Klickmeister läuft bereits, das vorhandene Fenster ließ sich aber nicht anzeigen: Testfehler",
        ),
        (
            InstanceError::Thread(io::Error::other("Testfehler")).to_string(),
            "Es lässt sich nicht prüfen, ob Klickmeister bereits läuft: Testfehler",
        ),
        (
            InstanceError::Bus(zbus::Error::Failure("Testfehler".to_owned())).to_string(),
            "Es lässt sich nicht prüfen, ob Klickmeister bereits läuft: Testfehler",
        ),
        (
            InstanceError::NoSessionBus(zbus::Error::Failure("Testfehler".to_owned())).to_string(),
            "Die D-Bus-Sitzung ist nicht erreichbar: Testfehler",
        ),
        (
            translate("Main", "Settings not saved"),
            "Einstellungen nicht gespeichert",
        ),
    ];
    for (shown, expected) in shown {
        assert_eq!(shown, expected);
    }
    let denied = ashpd::Error::Zbus(zbus::Error::Failure("Testfehler".to_owned()));
    assert_eq!(
        PortalError::Denied(denied).to_string(),
        format!(
            "Die Freigabe wurde abgelehnt oder abgebrochen: {}",
            ashpd::Error::Zbus(zbus::Error::Failure("Testfehler".to_owned()))
        )
    );
}
