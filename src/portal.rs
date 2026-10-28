use std::{future::Future, pin::Pin, sync::Arc};

use ashpd::{
    desktop::{
        PersistMode, Session,
        remote_desktop::{
            DeviceType, KeyState, NotifyPointerMotionAbsoluteOptions, RemoteDesktop,
            SelectDevicesOptions,
        },
        screencast::{CursorMode, Screencast, SelectSourcesOptions, SourceType},
    },
    zbus::{
        fdo::DBusProxy,
        names::{OwnedUniqueName, UniqueName},
    },
};
use futures_util::{FutureExt, StreamExt, future::FusedFuture};
use thiserror::Error;
use tokio::{
    sync::oneshot,
    time::{Duration, timeout},
};

use crate::{
    model::{ClickSettings, MonitorGeometry},
    parent_window,
};

const EVENT_TIMEOUT: Duration = Duration::from_millis(250);
const CLOSE_TIMEOUT: Duration = Duration::from_secs(1);
const PORTAL_NAME: &str = "org.freedesktop.portal.Desktop";

/// Return the portal's current unique name and a future that resolves when
/// that connection leaves the bus. Sessions created afterwards belong to it.
/// A crashed or restarted portal never sends `Closed` for them, and zbus
/// follows a new owner of the well-known name without ending any stream.
pub(crate) async fn watch_portal_owner(
    connection: &ashpd::zbus::Connection,
) -> ashpd::zbus::Result<(OwnedUniqueName, impl Future<Output = ()> + Send + 'static)> {
    let bus = DBusProxy::new(connection).await?;
    // Subscribe first, so a change right after reading the owner is not missed.
    let mut changes = bus
        .receive_name_owner_changed_with_args(&[(0, PORTAL_NAME)])
        .await?;
    let owner = bus.get_name_owner(PORTAL_NAME.try_into()?).await?;
    let watched = owner.clone();
    let lost = async move {
        while let Some(change) = changes.next().await {
            if change.args().is_ok_and(|args| {
                args.old_owner().as_ref().map(UniqueName::as_str) == Some(watched.as_str())
            }) {
                break;
            }
        }
    };
    Ok((owner, lost))
}

/// Register each owned bus connection before its first portal request. Qt's
/// desktop file name does not identify the worker's separate D-Bus peers.
pub(crate) async fn register_connection(
    connection: &ashpd::zbus::Connection,
) -> Result<(), ashpd::Error> {
    let app_id = ashpd::AppID::try_from(crate::APP_ID)?;
    match ashpd::register_host_app_with_connection(connection.clone(), app_id).await {
        // Older portal frontends derive the identity from the launch context.
        // Only the absence of Registry permits that legacy path; a denied or
        // failed registration must still abort setup.
        Err(ashpd::Error::PortalNotFound(interface))
            if interface.as_str() == "org.freedesktop.host.portal.Registry" =>
        {
            Ok(())
        }
        Err(ashpd::Error::Zbus(ashpd::zbus::Error::MethodError(name, _, _)))
            if matches!(
                name.as_str(),
                "org.freedesktop.DBus.Error.UnknownMethod"
                    | "org.freedesktop.DBus.Error.UnknownInterface"
            ) =>
        {
            Ok(())
        }
        result => result,
    }
}

#[derive(Debug, Error)]
pub enum PortalError {
    #[error("Die Freigabe wurde abgebrochen.")]
    Cancelled,
    #[error("Die Maussteuerung ist nicht verfügbar. Prüfe den KDE-Portaldienst. Details: {0}")]
    Unavailable(#[source] ashpd::Error),
    #[error("Die Freigabe wurde abgelehnt oder abgebrochen: {0}")]
    Denied(#[source] ashpd::Error),
    #[error(
        "Die Maussteuerung wurde nicht freigegeben. Erlaube sie beim nächsten Start im KDE-Dialog."
    )]
    PointerNotGranted,
    #[error(
        "Die Freigabe für die Maussteuerung kann nicht überwacht werden. Versuche den Start erneut."
    )]
    SessionWatchUnavailable,
    #[error("Die Freigabe für die Maussteuerung wurde beendet. Starte erneut, um sie anzufordern.")]
    SessionClosed,
    #[error(
        "KDE hat keine Bildschirmdaten für die feste Position geliefert. Prüfe die Bildschirmfreigabe und starte erneut."
    )]
    MissingMonitorStream,
    #[error(
        "Der ausgewählte Bildschirm stimmt nicht mit den freigegebenen Bildschirmdaten überein. Prüfe die Bildschirmanordnung und wähle die feste Position erneut."
    )]
    MonitorMismatch,
    #[error(
        "Die feste Position kann keinem Bildschirm eindeutig zugeordnet werden. Wähle die aktuelle Mauszeigerposition oder versuche es erneut."
    )]
    MonitorUnverifiable,
    #[error(
        "Die Position ({x}, {y}) liegt außerhalb des freigegebenen Bildschirms ({width} × {height}). Wähle eine neue Position."
    )]
    PositionOutsideStream {
        x: u32,
        y: u32,
        width: i32,
        height: i32,
    },
    #[error("Der Klick konnte nicht ausgeführt werden: {0}")]
    Send(#[source] ashpd::Error),
    #[error("KDE hat die Maussteuerung nicht rechtzeitig bestätigt. Versuche den Start erneut.")]
    EventTimeout,
}

pub struct PortalClickSession {
    connection: ashpd::zbus::Connection,
    portal: RemoteDesktop,
    session: Arc<Session<RemoteDesktop>>,
    closed: ClosedWatcher,
    stream: Option<MonitorStream>,
}

type ClosedWatcher = Pin<Box<dyn FusedFuture<Output = ()> + Send>>;

#[derive(Debug, Clone, Copy)]
struct MonitorStream {
    node_id: u32,
    width: i32,
    height: i32,
    position: Option<(i32, i32)>,
}

#[derive(Clone, Copy)]
struct StreamCandidate {
    node_id: u32,
    position: Option<(i32, i32)>,
    size: Option<(i32, i32)>,
    source_type: Option<SourceType>,
}

/// Select by logical geometry, never by stream order or an inferred origin.
fn select_monitor_stream(
    expected: MonitorGeometry,
    candidates: impl IntoIterator<Item = StreamCandidate>,
) -> Result<MonitorStream, PortalError> {
    let mut selected = None;
    let mut has_monitor = false;
    for candidate in candidates {
        if candidate
            .source_type
            .is_some_and(|source| source != SourceType::Monitor)
        {
            continue;
        }
        has_monitor = true;
        let position = candidate.position.ok_or(PortalError::MonitorUnverifiable)?;
        let (width, height) = candidate.size.ok_or(PortalError::MonitorUnverifiable)?;
        if validate_monitor(expected, Some(position), (width, height)).is_ok() {
            if selected.is_some() {
                return Err(PortalError::MonitorUnverifiable);
            }
            selected = Some(MonitorStream {
                node_id: candidate.node_id,
                position: Some(position),
                width,
                height,
            });
        }
    }
    selected.ok_or(if has_monitor {
        PortalError::MonitorMismatch
    } else {
        PortalError::MissingMonitorStream
    })
}

impl PortalClickSession {
    /// Request pointer permission and verify any fixed-position monitor geometry.
    pub async fn create(
        monitor: Option<MonitorGeometry>,
        mut cancel: oneshot::Receiver<()>,
    ) -> Result<Self, PortalError> {
        // Own the connection so cancellation also cleans up a CreateSession
        // request whose session handle has not arrived yet.
        let connection = tokio::select! {
            biased;
            _ = &mut cancel => return Err(PortalError::Cancelled),
            result = ashpd::zbus::Connection::session() =>
                result.map_err(|error| PortalError::Unavailable(error.into()))?,
        };
        let mut session = None;
        let mut closed = None;
        let result = tokio::select! {
            biased;
            _ = &mut cancel => Err(PortalError::Cancelled),
            result = Self::create_on(&connection, &mut session, &mut closed, monitor) => result,
        };
        match result {
            Ok((portal, stream)) => {
                let mut closed = closed.ok_or(PortalError::SessionWatchUnavailable)?;
                if closed.is_terminated() || closed.as_mut().now_or_never().is_some() {
                    if let Some(session) = session {
                        let _ = timeout(CLOSE_TIMEOUT, session.close()).await;
                    }
                    let _ = timeout(CLOSE_TIMEOUT, connection.close()).await;
                    return Err(PortalError::SessionClosed);
                }
                Ok(Self {
                    connection,
                    portal,
                    session: session.ok_or(PortalError::Cancelled)?,
                    closed,
                    stream,
                })
            }
            Err(error) => {
                if let Some(session) = session {
                    let _ = timeout(CLOSE_TIMEOUT, session.close()).await;
                }
                let _ = timeout(CLOSE_TIMEOUT, connection.close()).await;
                Err(error)
            }
        }
    }

    /// Negotiate permission while exposing the session to cancellation cleanup.
    async fn create_on(
        connection: &ashpd::zbus::Connection,
        owned_session: &mut Option<Arc<Session<RemoteDesktop>>>,
        owned_closed: &mut Option<ClosedWatcher>,
        monitor: Option<MonitorGeometry>,
    ) -> Result<(RemoteDesktop, Option<MonitorStream>), PortalError> {
        register_connection(connection)
            .await
            .map_err(PortalError::Unavailable)?;
        let fixed_position = monitor.is_some();
        let portal = RemoteDesktop::with_connection(connection.clone())
            .await
            .map_err(PortalError::Unavailable)?;
        let available = portal
            .available_device_types()
            .await
            .map_err(PortalError::Unavailable)?;
        if !available.contains(DeviceType::Pointer) {
            return Err(PortalError::PointerNotGranted);
        }

        let (_, owner_lost) = watch_portal_owner(connection)
            .await
            .map_err(|error| PortalError::Unavailable(error.into()))?;
        *owned_session = Some(Arc::new(
            portal
                .create_session(Default::default())
                .await
                .map_err(PortalError::Unavailable)?,
        ));
        let session = owned_session.as_ref().ok_or(PortalError::Cancelled)?;
        let watched_session = Arc::clone(session);
        let (ready_tx, ready_rx) = oneshot::channel();
        let mut closed: ClosedWatcher = Box::pin(
            async move {
                if let Ok(mut events) = watched_session.receive_closed().await {
                    let _ = ready_tx.send(());
                    tokio::select! {
                        _ = events.next() => {}
                        () = owner_lost => {}
                    }
                }
            }
            .fuse(),
        );
        let watching = timeout(CLOSE_TIMEOUT, async {
            tokio::select! {
                result = ready_rx => result.is_ok(),
                _ = &mut closed => false,
            }
        })
        .await
        .unwrap_or(false);
        if !watching {
            return Err(PortalError::SessionWatchUnavailable);
        }
        *owned_closed = Some(closed);
        let closed = owned_closed
            .as_mut()
            .ok_or(PortalError::SessionWatchUnavailable)?;
        let permission = async {
            portal
                .select_devices(
                    session,
                    SelectDevicesOptions::default()
                        .set_devices(Some(DeviceType::Pointer.into()))
                        .set_persist_mode(PersistMode::Application),
                )
                .await
                .map_err(PortalError::Unavailable)?
                .response()
                .map_err(PortalError::Denied)?;

            if fixed_position {
                let screencast = Screencast::with_connection(connection.clone())
                    .await
                    .map_err(PortalError::Unavailable)?;
                screencast
                    .select_sources(
                        session,
                        SelectSourcesOptions::default()
                            .set_sources(Some(SourceType::Monitor.into()))
                            // KDE otherwise combines multiple outputs into a
                            // workspace stream without a monitor position.
                            .set_multiple(true)
                            .set_cursor_mode(CursorMode::Hidden),
                    )
                    .await
                    .map_err(PortalError::Unavailable)?
                    .response()
                    .map_err(PortalError::Denied)?;
            }

            let parent = parent_window::identifier().await;
            let selected = portal
                .start(session, parent.as_ref(), Default::default())
                .await
                .map_err(PortalError::Unavailable)?
                .response()
                .map_err(PortalError::Denied)?;
            if !selected.devices().contains(DeviceType::Pointer) {
                return Err(PortalError::PointerNotGranted);
            }

            let stream = if let Some(expected) = monitor {
                Some(select_monitor_stream(
                    expected,
                    selected.streams().iter().map(|stream| StreamCandidate {
                        node_id: stream.pipe_wire_node_id(),
                        position: stream.position(),
                        size: stream.size(),
                        source_type: stream.source_type(),
                    }),
                )?)
            } else {
                None
            };

            Ok((portal, stream))
        };
        tokio::select! {
            biased;
            _ = closed.as_mut() => Err(PortalError::SessionClosed),
            result = permission => result,
        }
    }

    /// Reuse a session only when its granted monitor matches current settings.
    pub fn matches_monitor(&self, monitor: Option<MonitorGeometry>) -> bool {
        match (self.stream, monitor) {
            (None, None) => true,
            (Some(stream), Some(expected)) => {
                validate_monitor(expected, stream.position, (stream.width, stream.height)).is_ok()
            }
            _ => false,
        }
    }

    /// Resolve when the portal revokes this session, including while no run is active.
    pub async fn wait_closed(&mut self) {
        self.closed.as_mut().await;
    }

    /// Poll a queued revocation before reusing a session for a new run.
    pub fn is_closed(&mut self) -> bool {
        self.closed.is_terminated() || self.closed.as_mut().now_or_never().is_some()
    }

    /// Move within the verified monitor if needed and emit a bounded click cycle.
    pub async fn click(&self, settings: &ClickSettings) -> Result<(), PortalError> {
        if let Some((x, y)) = settings.position {
            let stream = self.stream.ok_or(PortalError::MissingMonitorStream)?;
            if x >= stream.width.max(0) as u32 || y >= stream.height.max(0) as u32 {
                return Err(PortalError::PositionOutsideStream {
                    x,
                    y,
                    width: stream.width,
                    height: stream.height,
                });
            }
            timeout(
                EVENT_TIMEOUT,
                self.portal.notify_pointer_motion_absolute(
                    &self.session,
                    stream.node_id,
                    f64::from(x),
                    f64::from(y),
                    NotifyPointerMotionAbsoluteOptions::default(),
                ),
            )
            .await
            .map_err(|_| PortalError::EventTimeout)?
            .map_err(PortalError::Send)?;
        }

        let button = settings.button.evdev_code();
        for _ in 0..settings.click_type.clicks_per_tick() {
            for state in [KeyState::Pressed, KeyState::Released] {
                if let Err(error) = self.send_button(button, state).await {
                    // A failed or timed-out press may still have reached KWin.
                    let _ = self.send_button(button, KeyState::Released).await;
                    return Err(error);
                }
            }
        }
        Ok(())
    }

    /// Send each button transition with the same timeout and error handling.
    async fn send_button(&self, button: i32, state: KeyState) -> Result<(), PortalError> {
        timeout(
            EVENT_TIMEOUT,
            self.portal
                .notify_pointer_button(&self.session, button, state, Default::default()),
        )
        .await
        .map_err(|_| PortalError::EventTimeout)?
        .map_err(PortalError::Send)
    }

    /// Close the portal session after clicking stops or permissions change.
    pub async fn close(self) {
        let _ = timeout(CLOSE_TIMEOUT, self.session.close()).await;
        let _ = timeout(CLOSE_TIMEOUT, self.connection.close()).await;
    }
}

/// Reject missing, mismatched or invalid monitor metadata before clicking.
fn validate_monitor(
    expected: MonitorGeometry,
    position: Option<(i32, i32)>,
    size: (i32, i32),
) -> Result<(), PortalError> {
    let position = position.ok_or(PortalError::MonitorUnverifiable)?;
    if position != (expected.x, expected.y)
        || size != (expected.width, expected.height)
        || expected.width <= 0
        || expected.height <= 0
    {
        return Err(PortalError::MonitorMismatch);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const LEFT: MonitorGeometry = MonitorGeometry {
        x: -1920,
        y: 0,
        width: 1920,
        height: 1080,
    };

    fn candidate(node_id: u32, x: i32) -> StreamCandidate {
        StreamCandidate {
            node_id,
            position: Some((x, 0)),
            size: Some((1920, 1080)),
            source_type: Some(SourceType::Monitor),
        }
    }

    #[test]
    fn selects_the_matching_output_independently_of_stream_order() -> Result<(), PortalError> {
        let left = candidate(42, -1920);
        let right = candidate(99, 0);
        for streams in [[left, right], [right, left]] {
            assert_eq!(select_monitor_stream(LEFT, streams)?.node_id, 42);
            assert_eq!(
                select_monitor_stream(MonitorGeometry { x: 0, ..LEFT }, streams)?.node_id,
                99
            );
        }
        Ok(())
    }

    #[test]
    fn accepts_a_single_output_and_legacy_missing_source_type() -> Result<(), PortalError> {
        let stream = StreamCandidate {
            source_type: None,
            ..candidate(42, -1920)
        };
        assert_eq!(select_monitor_stream(LEFT, [stream])?.node_id, 42);
        Ok(())
    }

    #[test]
    fn rejects_missing_metadata_even_next_to_a_matching_output() {
        let matching = candidate(42, -1920);
        for unknown in [
            StreamCandidate {
                position: None,
                ..candidate(99, 0)
            },
            StreamCandidate {
                size: None,
                ..candidate(99, 0)
            },
        ] {
            for streams in [[matching, unknown], [unknown, matching]] {
                assert!(matches!(
                    select_monitor_stream(LEFT, streams),
                    Err(PortalError::MonitorUnverifiable)
                ));
            }
        }
    }

    #[test]
    fn rejects_mirrored_outputs_with_indistinguishable_geometry() {
        assert!(matches!(
            select_monitor_stream(LEFT, [candidate(42, -1920), candidate(99, -1920)]),
            Err(PortalError::MonitorUnverifiable)
        ));
    }

    #[test]
    fn rejects_empty_nonmatching_or_invalid_output_geometry() {
        assert!(matches!(
            select_monitor_stream(LEFT, []),
            Err(PortalError::MissingMonitorStream)
        ));
        for stream in [
            candidate(99, 0),
            StreamCandidate {
                size: Some((1280, 720)),
                ..candidate(42, -1920)
            },
        ] {
            assert!(matches!(
                select_monitor_stream(LEFT, [stream]),
                Err(PortalError::MonitorMismatch)
            ));
        }
        assert!(matches!(
            select_monitor_stream(MonitorGeometry { width: 0, ..LEFT }, [candidate(42, -1920)]),
            Err(PortalError::MonitorMismatch)
        ));
    }

    #[test]
    fn ignores_explicit_non_monitor_sources() -> Result<(), PortalError> {
        let window = StreamCandidate {
            source_type: Some(SourceType::Window),
            ..candidate(99, -1920)
        };
        assert_eq!(
            select_monitor_stream(LEFT, [window, candidate(42, -1920)])?.node_id,
            42
        );
        assert!(matches!(
            select_monitor_stream(LEFT, [window]),
            Err(PortalError::MissingMonitorStream)
        ));
        Ok(())
    }

    #[test]
    fn monitor_matching_checks_origin_size_and_missing_metadata() {
        let monitor = MonitorGeometry {
            x: -1920,
            y: 0,
            width: 1920,
            height: 1080,
        };
        assert!(validate_monitor(monitor, Some((-1920, 0)), (1920, 1080)).is_ok());
        assert!(matches!(
            validate_monitor(monitor, Some((0, 0)), (1920, 1080)),
            Err(PortalError::MonitorMismatch)
        ));
        assert!(matches!(
            validate_monitor(monitor, Some((-1920, 0)), (1280, 720)),
            Err(PortalError::MonitorMismatch)
        ));
        assert!(matches!(
            validate_monitor(monitor, None, (1920, 1080)),
            Err(PortalError::MonitorUnverifiable)
        ));
    }
}
