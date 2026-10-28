use crate::instance::Activation;

#[cxx::bridge]
mod ffi {
    extern "Rust" {
        type Activation;

        #[cxx_name = "klickmeisterActivationToken"]
        fn activation_token(activation: &Activation) -> &str;

        #[cxx_name = "klickmeisterFinishActivation"]
        fn finish_activation(activation: Box<Activation>, activated: bool);

        #[cxx_name = "klickmeisterFinishParentWindowExport"]
        fn finish_parent_window_export(handle: &str);
    }

    unsafe extern "C++" {
        include!("klickmeister/src/qml_runtime.h");

        type QQmlApplicationEngine = cxx_qt_lib::QQmlApplicationEngine;

        #[rust_name = "qml_engine_has_root_object"]
        fn klickmeisterQmlEngineHasRootObject(engine: &QQmlApplicationEngine) -> bool;

        #[rust_name = "set_activation_window"]
        fn klickmeisterSetActivationWindow(engine: &QQmlApplicationEngine);

        #[rust_name = "export_parent_window"]
        fn klickmeisterExportParentWindow(engine: &QQmlApplicationEngine);

        #[rust_name = "queue_activation"]
        fn klickmeisterQueueActivation(activation: Box<Activation>);
    }
}

pub fn has_root_object(engine: &cxx_qt_lib::QQmlApplicationEngine) -> bool {
    ffi::qml_engine_has_root_object(engine)
}

/// Make the loaded main window the target of later activation requests.
/// Call on the Qt thread.
pub fn set_activation_window(engine: &cxx_qt_lib::QQmlApplicationEngine) {
    ffi::set_activation_window(engine);
}

/// Export the loaded main window as parent of KDE's portal dialogs. Call on
/// the Qt thread after `parent_window::begin_export`.
pub fn export_parent_window(engine: &cxx_qt_lib::QQmlApplicationEngine) {
    ffi::export_parent_window(engine);
}

/// Show the main window on the Qt thread. Callable from any thread while the
/// `QGuiApplication` exists; the request fails if the application quits first.
pub fn queue_activation(activation: Activation) {
    ffi::queue_activation(Box::new(activation));
}

fn finish_parent_window_export(handle: &str) {
    crate::parent_window::finish_export(handle);
}

fn activation_token(activation: &Activation) -> &str {
    activation.token()
}

// C++ hands the request back by value, which CXX requires to be a Box.
#[allow(clippy::boxed_local)]
fn finish_activation(activation: Box<Activation>, activated: bool) {
    activation.finish(activated);
}
