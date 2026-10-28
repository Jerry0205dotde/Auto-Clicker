use cxx_qt_lib::{
    QGuiApplication, QMap, QMapPair_QString_QVariant, QQmlApplicationEngine, QQuickStyle, QString,
    QUrl, QVariant,
};
use klickmeister::{
    APP_ID,
    instance::{self, Claim, InstanceError},
    parent_window, qml_runtime,
};
use std::process::ExitCode;

fn main() -> ExitCode {
    let smoke_test = std::env::args_os().any(|argument| argument == "--smoke-test");
    // Read before Qt starts: Qt Wayland consumes the launcher's token.
    let activation_token = std::env::var("XDG_ACTIVATION_TOKEN")
        .ok()
        .filter(|token| !token.is_empty());
    QQuickStyle::set_style(&QString::from("org.kde.desktop"));

    let mut app = QGuiApplication::new();
    QGuiApplication::set_desktop_file_name(&QString::from(APP_ID));
    let mut app_pin = app.pin_mut();
    app_pin
        .as_mut()
        .set_application_name(&QString::from("klickmeister"));
    app_pin
        .as_mut()
        .set_application_display_name(&QString::from("Klickmeister"));
    app_pin
        .as_mut()
        .set_application_version(&QString::from(env!("CARGO_PKG_VERSION")));
    app_pin
        .as_mut()
        .set_organization_name(&QString::from("Klickmeister"));

    // Claim before QML creates the controller and its portal sessions. Declared
    // after the application, so the guard stops before Qt is destroyed.
    let _instance = if smoke_test {
        None
    } else {
        match instance::claim(activation_token, qml_runtime::queue_activation) {
            Ok(Claim::Primary(guard)) => Some(guard),
            Ok(Claim::Secondary) => {
                println!("Klickmeister läuft bereits; das vorhandene Fenster wurde angezeigt.");
                return ExitCode::SUCCESS;
            }
            // The portals need the same bus, so no second hotkey session can arise.
            Err(error @ InstanceError::NoSessionBus(_)) => {
                eprintln!("{error}");
                None
            }
            Err(error) => {
                eprintln!("{error}");
                return ExitCode::FAILURE;
            }
        }
    };

    // The worker starts with QML and binds the hotkey right away. Its dialog
    // waits briefly for the window handle, which needs the loaded window.
    if !smoke_test {
        parent_window::begin_export();
    }
    let mut engine = QQmlApplicationEngine::new();
    let mut engine_pin = engine.pin_mut();
    if smoke_test {
        let mut properties: QMap<QMapPair_QString_QVariant> = QMap::default();
        properties.insert_clone(&QString::from("smokeTest"), &QVariant::from(&smoke_test));
        engine_pin.as_mut().set_initial_properties(&properties);
    }
    engine_pin.as_mut().load(&QUrl::from(
        "qrc:/qt/qml/io/github/jerry0205/klickmeister/qml/Main.qml",
    ));
    if !qml_runtime::has_root_object(&engine) {
        eprintln!(
            "Klickmeister konnte nicht geöffnet werden: Das Anwendungsfenster konnte nicht geladen werden."
        );
        return ExitCode::FAILURE;
    }
    if smoke_test {
        println!(
            "Qt desktop file name: {}",
            QGuiApplication::desktop_file_name()
        );
        return ExitCode::SUCCESS;
    }
    qml_runtime::set_activation_window(&engine);
    qml_runtime::export_parent_window(&engine);

    if app_pin.exec() == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
