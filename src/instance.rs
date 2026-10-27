//! One Klickmeister per session bus.
//!
//! KDE forwards a global shortcut to every portal session of the same app ID,
//! so a second instance would make the start/stop hotkey ambiguous. The lock is
//! the well-known D-Bus name `APP_ID`: the bus releases it with the connection,
//! also after a crash, so no stale lock can remain.

use std::{collections::HashMap, io, sync::Arc, sync::mpsc as std_mpsc, thread};

use ashpd::zbus::{
    self,
    fdo::{RequestNameFlags, RequestNameReply},
    zvariant::{OwnedValue, Value},
};
use thiserror::Error;
use tokio::{
    runtime::Builder,
    sync::oneshot,
    time::{Duration, Instant, sleep, timeout},
};

use crate::APP_ID;

/// Path that `org.freedesktop.Application` derives from the app ID.
pub const OBJECT_PATH: &str = "/io/github/jerry0205/klickmeister";
const APPLICATION_INTERFACE: &str = "org.freedesktop.Application";
const ACTIVATION_TOKEN: &str = "activation-token";
/// The Qt thread must answer within this time; it is blocked while the window closes.
const ACTIVATION_TIMEOUT: Duration = Duration::from_secs(2);
const CALL_TIMEOUT: Duration = Duration::from_secs(3);
/// Covers the bounded worker shutdown (5 s) of an instance that is closing.
const CLAIM_TIMEOUT: Duration = Duration::from_secs(10);
const RETRY_INTERVAL: Duration = Duration::from_millis(200);
const RELEASE_TIMEOUT: Duration = Duration::from_secs(1);

type Handler = Arc<dyn Fn(Activation) + Send + Sync>;

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

#[derive(Debug, Error)]
pub enum InstanceError {
    /// Without a session bus the portals are unreachable as well.
    #[error("Die D-Bus-Sitzung ist nicht erreichbar: {0}")]
    NoSessionBus(#[source] zbus::Error),
    #[error(
        "Klickmeister läuft bereits, das vorhandene Fenster ließ sich aber nicht anzeigen: {0}"
    )]
    Activation(String),
    #[error("Es lässt sich nicht prüfen, ob Klickmeister bereits läuft: {0}")]
    Bus(#[source] zbus::Error),
    #[error("Es lässt sich nicht prüfen, ob Klickmeister bereits läuft: {0}")]
    Thread(#[source] io::Error),
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
pub fn claim<F>(activation_token: Option<String>, activate: F) -> Result<Claim, InstanceError>
where
    F: Fn(Activation) + Send + Sync + 'static,
{
    let handler: Handler = Arc::new(activate);
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
                let connection = match claim_on_bus(activation_token.as_deref(), handler).await {
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
            "Der Prüfthread wurde unerwartet beendet.",
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
) -> Result<Option<zbus::Connection>, InstanceError> {
    // Serve before requesting the name, so that no activation request is lost.
    let connection = zbus::connection::Builder::session()
        .and_then(|builder| builder.serve_at(OBJECT_PATH, Application { activate }))
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
        Err(_) => Err("Die laufende Instanz antwortet nicht.".to_owned()),
    }
}

struct Application {
    activate: Handler,
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
            _ => Err(zbus::fdo::Error::Failed(
                "Das Klickmeister-Fenster ist gerade nicht verfügbar.".to_owned(),
            )),
        }
    }
}
