//! One Klickmeister per session bus.
//!
//! KDE forwards a global shortcut to every portal session of the same app ID,
//! so a second instance would make the start/stop hotkey ambiguous. The lock is
//! the well-known D-Bus name `APP_ID`: the bus releases it with the connection,
//! also after a crash, so no stale lock can remain.
//!
//! The same name carries `klickmeister --toggle` to the running instance, so
//! that a shortcut set up in the desktop's settings can start and stop runs.

use std::{collections::HashMap, io, sync::Arc, sync::mpsc as std_mpsc, thread};

use ashpd::zbus::{
    self,
    fdo::{RequestNameFlags, RequestNameReply},
    proxy::{CacheProperties, MethodFlags},
    zvariant::{OwnedValue, Value},
};
use tokio::{
    runtime::Builder,
    sync::oneshot,
    time::{Duration, Instant, sleep, timeout},
};

use crate::{APP_ID, messages};

/// Path that `org.freedesktop.Application` derives from the app ID.
pub const OBJECT_PATH: &str = "/io/github/jerry0205/klickmeister";
const APPLICATION_INTERFACE: &str = "org.freedesktop.Application";
const ACTIVATION_TOKEN: &str = "activation-token";
/// Action of `org.freedesktop.Application.ActivateAction` behind `--toggle`.
pub const TOGGLE_ACTION: &str = "toggle-clicking";
/// The Qt thread must answer within this time; it is blocked while the window closes.
const ACTIVATION_TIMEOUT: Duration = Duration::from_secs(2);
const CALL_TIMEOUT: Duration = Duration::from_secs(3);
/// Covers the bounded worker shutdown (5 s) of an instance that is closing.
const CLAIM_TIMEOUT: Duration = Duration::from_secs(10);
const RETRY_INTERVAL: Duration = Duration::from_millis(200);
const RELEASE_TIMEOUT: Duration = Duration::from_secs(1);

type Handler = Arc<dyn Fn(Activation) + Send + Sync>;
type ToggleHandler = Arc<dyn Fn() -> Result<(), String> + Send + Sync>;

/// A later launch asks the running instance to show its window.
pub struct Activation {
    token: String,
    reply: oneshot::Sender<bool>,
}

impl Activation {
    /// Wayland activation token of the later launch; empty if it had none.
    pub fn token(&self) -> &str {
        &self.token
    }

    /// Report whether a window was activated. Dropping the request reports failure.
    pub fn finish(self, activated: bool) {
        let _ = self.reply.send(activated);
    }
}

pub enum Claim {
    /// This process is the instance. Keep the guard until the application ends.
    Primary(InstanceGuard),
    /// The running instance has shown its window; this process must exit.
    Secondary,
}

/// Why the instance check failed. `messages` holds the shown texts.
#[derive(Debug)]
pub enum InstanceError {
    /// Without a session bus the portals are unreachable as well.
    NoSessionBus(zbus::Error),
    Activation(String),
    Bus(zbus::Error),
    Thread(io::Error),
}

impl std::error::Error for InstanceError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::NoSessionBus(error) | Self::Bus(error) => Some(error),
            Self::Activation(_) => None,
            Self::Thread(error) => Some(error),
        }
    }
}

/// Why `klickmeister --toggle` failed. `messages` holds the shown texts.
#[derive(Debug)]
pub enum ToggleError {
    NoSessionBus(zbus::Error),
    NotRunning,
    Refused(String),
    Timeout,
    Thread(io::Error),
}

impl std::error::Error for ToggleError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::NoSessionBus(error) => Some(error),
            Self::Thread(error) => Some(error),
            Self::NotRunning | Self::Refused(_) | Self::Timeout => None,
        }
    }
}

/// Owns the instance name and answers activation requests until dropped.
pub struct InstanceGuard {
    release: Option<oneshot::Sender<()>>,
    thread: Option<thread::JoinHandle<()>>,
}

impl Drop for InstanceGuard {
    /// Release the name before the process ends so that a new launch need not
    /// wait for the exit.
    fn drop(&mut self) {
        drop(self.release.take());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Become the instance or hand `activation_token` to the running one.
///
/// `activate` runs on the bus thread for each later launch. It must answer
/// each request, and fail it while no window can be shown, so that the later
/// launch takes over once a closing instance has ended.
///
/// `toggle` runs on the bus thread for each `klickmeister --toggle`; its
/// error is returned to that caller.
pub fn claim<F, T>(
    activation_token: Option<String>,
    activate: F,
    toggle: T,
) -> Result<Claim, InstanceError>
where
    F: Fn(Activation) + Send + Sync + 'static,
    T: Fn() -> Result<(), String> + Send + Sync + 'static,
{
    let handler: Handler = Arc::new(activate);
    let toggle: ToggleHandler = Arc::new(toggle);
    // Reports whether this process owns the name.
    let (primary_tx, primary) = std_mpsc::channel();
    let (release, released) = oneshot::channel::<()>();
    // The bus connection needs a running executor while Qt owns the main thread.
    let thread = thread::Builder::new()
        .name("klickmeister-instance".to_owned())
        .spawn(move || {
            let runtime = match Builder::new_current_thread().enable_all().build() {
                Ok(runtime) => runtime,
                Err(error) => {
                    let _ = primary_tx.send(Err(InstanceError::Thread(error)));
                    return;
                }
            };
            runtime.block_on(async move {
                let connection =
                    match claim_on_bus(activation_token.as_deref(), handler, toggle).await {
                        Ok(Some(connection)) => connection,
                        Ok(None) => {
                            let _ = primary_tx.send(Ok(false));
                            return;
                        }
                        Err(error) => {
                            let _ = primary_tx.send(Err(error));
                            return;
                        }
                    };
                if primary_tx.send(Ok(true)).is_ok() {
                    let _ = released.await;
                }
                let _ = timeout(RELEASE_TIMEOUT, connection.close()).await;
            });
        })
        .map_err(InstanceError::Thread)?;
    let primary = primary.recv().unwrap_or_else(|_| {
        Err(InstanceError::Thread(io::Error::other(
            messages::instance_check_ended(),
        )))
    });
    if let Ok(true) = primary {
        return Ok(Claim::Primary(InstanceGuard {
            release: Some(release),
            thread: Some(thread),
        }));
    }
    let _ = thread.join();
    primary.map(|_| Claim::Secondary)
}

/// Return the connection that owns the name, or `None` after a successful handover.
async fn claim_on_bus(
    activation_token: Option<&str>,
    activate: Handler,
    toggle: ToggleHandler,
) -> Result<Option<zbus::Connection>, InstanceError> {
    // Serve before requesting the name, so that no activation request is lost.
    let connection = zbus::connection::Builder::session()
        .and_then(|builder| builder.serve_at(OBJECT_PATH, Application { activate, toggle }))
        .map_err(InstanceError::NoSessionBus)?
        .build()
        .await
        .map_err(InstanceError::NoSessionBus)?;
    let deadline = Instant::now() + CLAIM_TIMEOUT;
    loop {
        match connection
            .request_name_with_flags(APP_ID, RequestNameFlags::DoNotQueue.into())
            .await
        {
            Ok(RequestNameReply::PrimaryOwner | RequestNameReply::AlreadyOwner) => {
                return Ok(Some(connection));
            }
            // zbus reports an existing owner as NameTaken; DoNotQueue never queues.
            Ok(RequestNameReply::Exists | RequestNameReply::InQueue)
            | Err(zbus::Error::NameTaken) => {}
            Err(error) => return Err(InstanceError::Bus(error)),
        }
        match activate_running(&connection, activation_token).await {
            Ok(()) => return Ok(None),
            Err(error) if Instant::now() >= deadline => {
                return Err(InstanceError::Activation(error));
            }
            // A closing instance refuses activation until its name is free.
            Err(_) => sleep(RETRY_INTERVAL).await,
        }
    }
}

async fn activate_running(
    connection: &zbus::Connection,
    activation_token: Option<&str>,
) -> Result<(), String> {
    let platform_data: HashMap<&str, Value<'_>> = activation_token
        .map(|token| (ACTIVATION_TOKEN, Value::from(token)))
        .into_iter()
        .collect();
    let body = (platform_data,);
    let call = connection.call_method(
        Some(APP_ID),
        OBJECT_PATH,
        Some(APPLICATION_INTERFACE),
        "Activate",
        &body,
    );
    match timeout(CALL_TIMEOUT, call).await {
        Ok(Ok(_)) => Ok(()),
        Ok(Err(error)) => Err(error.to_string()),
        Err(_) => Err(messages::instance_not_answering()),
    }
}

/// Ask the running instance to start or stop clicking, exactly once.
///
/// Unlike a second launch, this never retries and never opens a window:
/// without a running instance it fails. The bus must not start one either.
pub fn toggle_running() -> Result<(), ToggleError> {
    let runtime = Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(ToggleError::Thread)?;
    runtime.block_on(async {
        let connection = zbus::Connection::session()
            .await
            .map_err(ToggleError::NoSessionBus)?;
        let result = timeout(CALL_TIMEOUT, send_toggle(&connection)).await;
        let _ = timeout(RELEASE_TIMEOUT, connection.close()).await;
        result.map_err(|_| ToggleError::Timeout)?
    })
}

async fn send_toggle(connection: &zbus::Connection) -> Result<(), ToggleError> {
    let proxy = zbus::proxy::Builder::<zbus::Proxy<'_>>::new(connection)
        .destination(APP_ID)
        .and_then(|builder| builder.path(OBJECT_PATH))
        .and_then(|builder| builder.interface(APPLICATION_INTERFACE))
        .map(|builder| builder.cache_properties(CacheProperties::No))
        .map_err(|error| ToggleError::Refused(error.to_string()))?
        .build()
        .await
        .map_err(|error| ToggleError::Refused(error.to_string()))?;
    let body = (
        TOGGLE_ACTION,
        Vec::<Value<'_>>::new(),
        HashMap::<&str, Value<'_>>::new(),
    );
    match proxy
        .call_with_flags::<_, _, ()>("ActivateAction", MethodFlags::NoAutoStart.into(), &body)
        .await
    {
        Ok(_) => Ok(()),
        Err(zbus::Error::MethodError(name, _, _))
            if matches!(
                name.as_str(),
                "org.freedesktop.DBus.Error.NameHasNoOwner"
                    | "org.freedesktop.DBus.Error.ServiceUnknown"
            ) =>
        {
            Err(ToggleError::NotRunning)
        }
        Err(zbus::Error::MethodError(_, Some(message), _)) => Err(ToggleError::Refused(message)),
        Err(error) => Err(ToggleError::Refused(error.to_string())),
    }
}

struct Application {
    activate: Handler,
    toggle: ToggleHandler,
}

#[zbus::interface(name = "org.freedesktop.Application", crate = "ashpd::zbus")]
impl Application {
    /// Show the running window; fail while none can be shown.
    async fn activate(&self, platform_data: HashMap<String, OwnedValue>) -> zbus::fdo::Result<()> {
        let token = platform_data
            .get(ACTIVATION_TOKEN)
            .and_then(|value| <&str>::try_from(value).ok())
            .unwrap_or_default()
            .to_owned();
        let (reply, activated) = oneshot::channel();
        (self.activate)(Activation { token, reply });
        match timeout(ACTIVATION_TIMEOUT, activated).await {
            Ok(Ok(true)) => Ok(()),
            _ => Err(zbus::fdo::Error::Failed(messages::window_unavailable())),
        }
    }

    /// Start or stop clicking for `klickmeister --toggle`. Neither shows a
    /// window nor takes an activation token.
    async fn activate_action(
        &self,
        action_name: String,
        _parameter: Vec<OwnedValue>,
        _platform_data: HashMap<String, OwnedValue>,
    ) -> zbus::fdo::Result<()> {
        if action_name != TOGGLE_ACTION {
            return Err(zbus::fdo::Error::NotSupported(messages::unknown_action(
                &action_name,
            )));
        }
        (self.toggle)().map_err(zbus::fdo::Error::Failed)
    }
}
