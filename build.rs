use cxx_qt_build::{CxxQtBuilder, QmlModule};
use std::process::Command;

/// Query pkg-config, which `makedepends` already provides.
fn pkg_config(argument: &str, library: &str) -> String {
    let output = Command::new("pkg-config")
        .args([argument, library])
        .output()
        .unwrap_or_else(|error| panic!("pkg-config could not be run: {error}"));
    if !output.status.success() {
        panic!(
            "pkg-config did not find {library}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

fn main() {
    // KWindowSystem exports the main window for the portal dialogs.
    let window_system = "KF6WindowSystem";
    let mut builder = CxxQtBuilder::new_qml_module(
        QmlModule::new("io.github.jerry0205.klickmeister")
            .qml_file("qml/Main.qml")
            .qml_file("qml/PositionPicker.qml")
            .qml_file("qml/MonitorSelection.qml")
            .qml_file("qml/ChoiceButtons.qml"),
    )
    .files(["src/controller.rs", "src/qml_runtime.rs"])
    .cpp_file("src/qml_runtime.cpp");
    let includes = pkg_config("--cflags-only-I", window_system);
    // SAFETY: The callback only adds include directories; cxx-qt marks any
    // access to its internal `cc::Build` as unsafe.
    builder = unsafe {
        builder.cc_builder(|cc| {
            for include in includes.split_whitespace() {
                if let Some(dir) = include.strip_prefix("-I") {
                    cc.include(dir);
                }
            }
        })
    };
    for flag in pkg_config("--libs", window_system).split_whitespace() {
        if let Some(dir) = flag.strip_prefix("-L") {
            println!("cargo::rustc-link-search=native={dir}");
        } else if let Some(library) = flag.strip_prefix("-l") {
            println!("cargo::rustc-link-lib={library}");
        }
    }
    println!("cargo::rerun-if-env-changed=PKG_CONFIG_PATH");
    builder.build();
}
