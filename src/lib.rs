pub mod config;
pub mod controller;
pub mod instance;
pub mod model;
pub mod portal;
pub mod qml_runtime;
pub mod scheduler;
pub mod state;
pub mod worker;

/// Desktop entry, portal identity for every application connection and the
/// D-Bus name that keeps the application to one instance.
pub const APP_ID: &str = "io.github.jerry0205.klickmeister";
