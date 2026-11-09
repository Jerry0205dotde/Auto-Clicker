//! The platform boundary for clicking. The worker only sees `ClickSession`;
//! each platform provides one implementation, selected for the build below.
use std::future::Future;

use tokio::sync::oneshot;

use crate::model::{ClickSettings, MonitorGeometry};

/// A granted permission to click, for the cursor or one verified monitor.
///
/// The worker keeps a session across runs, reuses it while it still matches the
/// settings and closes it after the idle timeout, on shutdown or after an error.
pub trait ClickSession: Sized + Send + 'static {
    type Error: std::error::Error + Send + 'static;

    /// Ask for permission and verify the monitor of a fixed position. Fails
    /// with an error, and leaves nothing open, once `cancel` fires.
    fn create(
        monitor: Option<MonitorGeometry>,
        cancel: oneshot::Receiver<()>,
    ) -> impl Future<Output = Result<Self, Self::Error>> + Send;

    /// Whether the granted monitor still matches the settings of a new run.
    fn matches_monitor(&self, monitor: Option<MonitorGeometry>) -> bool;

    /// Resolve when the platform revokes the session, also between runs.
    fn wait_closed(&mut self) -> impl Future<Output = ()>;

    /// Whether a revocation is already pending.
    fn is_closed(&mut self) -> bool;

    /// Emit one click cycle: a single or double click at the configured target.
    fn click(&self, settings: &ClickSettings) -> impl Future<Output = Result<(), Self::Error>>;

    /// End the session within a bounded time.
    fn close(self) -> impl Future<Output = ()>;

    /// The error reported when the platform ended the session.
    fn closed_error() -> Self::Error;
}

/// The click backend of this build.
pub type PlatformClickSession = crate::portal::PortalClickSession;
