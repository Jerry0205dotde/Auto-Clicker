//! The exported main window that KDE attaches its portal dialogs to.
use std::sync::LazyLock;

use ashpd::WindowIdentifier;
use tokio::{
    sync::watch,
    time::{Duration, timeout},
};

/// How long a portal dialog waits for a pending window export.
const EXPORT_WAIT: Duration = Duration::from_secs(1);

#[derive(Debug, Clone, PartialEq, Eq)]
enum Export {
    Pending,
    /// The xdg-foreign handle, or `None` without a usable export.
    Finished(Option<String>),
}

static EXPORT: LazyLock<watch::Sender<Export>> =
    LazyLock::new(|| watch::Sender::new(Export::Finished(None)));

/// Announce an export, so the first dialogs wait briefly for its handle.
pub fn begin_export() {
    EXPORT.send_replace(Export::Pending);
}

/// Publish the exported handle. An empty handle means the export failed.
pub fn finish_export(handle: &str) {
    let handle = (!handle.is_empty()).then(|| handle.to_owned());
    EXPORT.send_replace(Export::Finished(handle));
}

/// The parent for a portal dialog. Without a handle the dialog stays a
/// separate window, as before.
pub(crate) async fn identifier() -> Option<WindowIdentifier> {
    let mut export = EXPORT.subscribe();
    let finished = timeout(
        EXPORT_WAIT,
        export.wait_for(|export| *export != Export::Pending),
    )
    .await;
    let handle = match finished {
        Ok(Ok(export)) => match &*export {
            Export::Finished(handle) => handle.clone(),
            Export::Pending => None,
        },
        // The export did not answer in time; open the dialog without a parent.
        _ => None,
    };
    handle.map(WindowIdentifier::from_xdg_foreign_exported)
}
