use cxx_qt_build::{CxxQtBuilder, QmlModule};
use std::{
    env, fs,
    path::{Path, PathBuf},
    process::Command,
};

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

/// Compile each `translations/*.ts` with lrelease and list the results in a
/// resource file below `:/i18n`, where the application loads them from.
fn compile_translations() -> PathBuf {
    println!("cargo::rerun-if-changed=translations");
    println!("cargo::rerun-if-env-changed=QMAKE");
    // cxx-qt-build finds Qt the same way.
    let qmake = env::var("QMAKE").unwrap_or_else(|_| "qmake6".to_owned());
    let output = Command::new(&qmake)
        .args(["-query", "QT_HOST_BINS"])
        .output()
        .unwrap_or_else(|error| panic!("{qmake} could not be run: {error}"));
    let lrelease = Path::new(String::from_utf8_lossy(&output.stdout).trim()).join("lrelease");
    let out_dir = PathBuf::from(env::var_os("OUT_DIR").unwrap_or_default());
    let entries = fs::read_dir("translations")
        .unwrap_or_else(|error| panic!("translations/ could not be read: {error}"));
    let mut sources: Vec<PathBuf> = entries
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.extension().is_some_and(|extension| extension == "ts"))
        .collect();
    sources.sort();
    let mut files = String::new();
    for source in sources {
        // The resource file lies next to the .qm files and names them relatively.
        let name = source.with_extension("qm");
        let name = name.file_name().unwrap_or_default().to_string_lossy();
        let qm = out_dir.join(name.as_ref());
        let status = Command::new(&lrelease)
            .arg("-silent")
            .arg(&source)
            .arg("-qm")
            .arg(&qm)
            .status()
            .unwrap_or_else(|error| panic!("{} could not be run: {error}", lrelease.display()));
        if !status.success() {
            panic!("lrelease failed for {}", source.display());
        }
        files += &format!("<file>{name}</file>");
    }
    let qrc = out_dir.join("translations.qrc");
    let resources = format!("<RCC><qresource prefix=\"/i18n\">{files}</qresource></RCC>\n");
    fs::write(&qrc, resources)
        .unwrap_or_else(|error| panic!("{} could not be written: {error}", qrc.display()));
    qrc
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
    .files(["src/controller.rs", "src/i18n.rs", "src/qml_runtime.rs"])
    .cpp_file("src/i18n.cpp")
    .cpp_file("src/qml_runtime.cpp")
    .qrc(compile_translations());
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
