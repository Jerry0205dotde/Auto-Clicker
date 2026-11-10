use std::{
    fmt, io,
    ops::ControlFlow,
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self as std_mpsc, RecvTimeoutError},
    },
    thread,
    time::Duration,
};

use ashpd::{
    desktop::{
        Session,
        global_shortcuts::{
            Activated, BindShortcutsOptions, ConfigureShortcutsOptions, GlobalShortcuts,
            NewShortcut, Shortcut, ShortcutsChanged,
        },
    },
    zbus::{
        self, Message,
        message::Sequence,
        names::{OwnedUniqueName, UniqueName},
    },
};
use futures_util::{
    FutureExt, Stream, StreamExt,
    future::{self, FusedFuture},
};
use tokio::{
    runtime::{Builder, Runtime},
    sync::{
        mpsc::{self, error::TrySendError},
        oneshot, watch,
    },
    task::JoinHandle,
    time::{Instant, sleep, sleep_until, timeout},
};

use crate::{
    input::{ClickSession, PlatformClickSession},
    messages,
    model::{ClickSettings, ValidationError},
    parent_window,
    portal::{register_connection, watch_portal_owner},
    scheduler::Schedule,
    state::{RunState, StateMachine},
};

const COMMAND_CAPACITY: usize = 16;
const PORTAL_CLOSE_TIMEOUT: Duration = Duration::from_secs(1);
/// Longest time the window closing handler waits for the worker's cleanup.
/// A worker that is still busy afterwards no longer clicks, closes its own
/// sessions, and the controller drops its events via the worker epoch.
const SHUTDOWN_WAIT: Duration = Duration::from_secs(5);
const HOTKEY_ID: &str = "toggle-clicking";
const BUTTON_START_DELAY_SECS: u8 = 3;
/// How long the mouse permission outlives a run. A start within this time
/// needs no new KDE dialog; afterwards KDE's remote control indicator and any
/// monitor streams end.
const CLICK_SESSION_IDLE: Duration = Duration::from_secs(60);
/// Suggested key in the XDG shortcut format. The saved hotkey is KDE's
/// localized description ("Strg+F8") and only fit for display.
const PREFERRED_TRIGGER: &str = "Pause";

#[derive(Debug)]
pub enum Command {
    Start(ClickSettings),
    StartFromButton(ClickSettings),
    Stop,
    ConfigureHotkey,
    Shutdown,
}

#[derive(Debug)]
pub enum WorkerEvent {
    Status(WorkerStatus),
    State(RunState),
    /// Seconds left before a button-started run clicks; the state stays `Starting`.
    Countdown(u8),
    Hotkey(String),
    HotkeyPhase(HotkeyPhase),
    /// Whether the portal can change a bound hotkey (GlobalShortcuts v2).
    HotkeyConfigurable(bool),
    /// Reserved hotkey start: return current Qt settings, or drop to decline.
    StartRequested(StartRequest),
    Error(WorkerError),
}

/// What the worker is doing. `messages` turns it into the shown status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkerStatus {
    Ready,
    RegisteringHotkey,
    /// The hotkey settings closed without a bound hotkey.
    AwaitingHotkey,
    AwaitingClickPermission,
    /// The mouse permission ended between runs; the next start asks again.
    PermissionRequired,
    Clicking,
    Stopped,
}

/// Why a start or the worker failed. `messages` turns it into the shown text.
#[derive(Debug)]
pub enum WorkerError {
    /// The worker thread or its runtime could not start.
    WorkerStartFailed(String),
    /// A start arrived without a bound hotkey or while its settings are open.
    HotkeyRequired,
    InvalidSettings(ValidationError),
    Click(ClickError),
    /// The task that requests the mouse permission failed.
    ClickTaskFailed(String),
    /// A run had no mouse permission when its next click was due.
    ClickSessionLost,
    Hotkey(HotkeyError),
}

/// The error of this build's click backend.
pub type ClickError = <PlatformClickSession as ClickSession>::Error;

/// Why the hotkey cannot be used. Fields hold the technical details.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HotkeyError {
    SessionClosed,
    RequestCancelled,
    SessionMissing,
    Unbound,
    /// The portal reported that the hotkey has no trigger any more.
    Removed,
    Setup(String),
    Registration(String),
    Unavailable(String),
    WatchUnavailable,
    SubscribeTimeout,
    Receive(String),
    Request(String),
    NotGranted(String),
    SettingsUnavailable(String),
}

/// Why `WorkerHandle::send` could not deliver a command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendError {
    Closing,
    Busy,
    Stopped,
}

impl std::error::Error for WorkerError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::InvalidSettings(error) => Some(error),
            Self::Click(error) => Some(error),
            Self::Hotkey(error) => Some(error),
            _ => None,
        }
    }
}

impl std::error::Error for HotkeyError {}

impl std::error::Error for SendError {}

/// A single-use reply to one hotkey activation, with its own cancellation identity.
/// Dropping it declines the start, including when Qt cannot queue the event.
#[derive(Debug)]
pub struct StartRequest {
    reply: oneshot::Sender<ClickSettings>,
}

impl StartRequest {
    pub(crate) fn channel() -> (Self, oneshot::Receiver<ClickSettings>) {
        let (reply, response) = oneshot::channel();
        (Self { reply }, response)
    }

    pub fn is_cancelled(&self) -> bool {
        self.reply.is_closed()
    }

    /// Return the controls collected on the Qt thread. A cancelled reply is ignored.
    pub fn start(self, settings: ClickSettings) {
        let _ = self.reply.send(settings);
    }
}

struct PendingStartRequest {
    reply: oneshot::Receiver<ClickSettings>,
    /// Keep the Stop generation from the activation, never from the GUI reply.
    stops: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotkeyPhase {
    Unavailable,
    Registering,
    Ready,
    Configuring(bool),
}

pub struct WorkerHandle {
    tx: mpsc::Sender<QueuedCommand>,
    control: watch::Sender<Control>,
    closing: Arc<AtomicBool>,
    join: Option<thread::JoinHandle<()>>,
    /// Disconnects when the worker thread ends, also after a panic.
    finished: std_mpsc::Receiver<()>,
}

/// Stop and Shutdown bypass the bounded queue, so a full queue cannot delay them.
#[derive(Debug, Clone, Copy, Default)]
struct Control {
    /// Counts Stop requests. Starts queued before the latest Stop are dropped.
    stops: u64,
    shutdown: bool,
}

/// Commands that wait in the bounded queue.
enum QueuedCommand {
    Start {
        settings: ClickSettings,
        stops: u64,
        from_button: bool,
        reserved: bool,
    },
    ConfigureHotkey,
}

/// A Stop or Shutdown that the worker has not handled yet.
#[derive(Debug, PartialEq, Eq)]
enum ControlChange {
    None,
    Stop,
    Shutdown,
}

impl WorkerHandle {
    pub fn spawn<F>(emit: F) -> Self
    where
        F: Fn(WorkerEvent) + Send + Sync + 'static,
    {
        Self::spawn_with_session_idle(emit, CLICK_SESSION_IDLE)
    }

    /// Like `spawn`, but close an unused mouse permission after `session_idle`.
    pub fn spawn_with_session_idle<F>(emit: F, session_idle: Duration) -> Self
    where
        F: Fn(WorkerEvent) + Send + Sync + 'static,
    {
        let builder = thread::Builder::new().name("klickmeister-worker".to_owned());
        Self::spawn_with(builder, Arc::new(emit), session_idle)
    }

    /// Spawn with an injectable thread builder and report start failures as errors.
    fn spawn_with(builder: thread::Builder, emit: Emitter, session_idle: Duration) -> Self {
        let (tx, rx) = mpsc::channel(COMMAND_CAPACITY);
        let (control, control_rx) = watch::channel(Control::default());
        let (finished_tx, finished) = std_mpsc::channel();
        let closing = Arc::new(AtomicBool::new(false));
        let worker_closing = Arc::clone(&closing);
        let worker_emit = Arc::clone(&emit);
        let join = builder
            .spawn(move || {
                let _finished = finished_tx;
                // Runtime construction only fails for OS resource exhaustion.
                let runtime = Builder::new_current_thread().enable_all().build();
                let worker = run_worker(
                    rx,
                    control_rx,
                    Arc::clone(&worker_emit),
                    worker_closing,
                    session_idle,
                );
                block_on_runtime(runtime, &worker_emit, worker);
            })
            .inspect_err(|error| (emit)(worker_start_failed(error)))
            .ok();
        Self {
            tx,
            control,
            closing,
            join,
            finished,
        }
    }

    /// Queue Start and ConfigureHotkey; Stop and Shutdown never wait for queue space.
    pub fn send(&self, command: Command) -> Result<(), SendError> {
        if self.closing.load(Ordering::Acquire) {
            return Err(SendError::Closing);
        }
        let command = match command {
            Command::Stop => {
                let mut control = *self.control.borrow();
                control.stops = control.stops.wrapping_add(1);
                // Fails only when the worker loop has ended.
                return self.control.send(control).map_err(|_| SendError::Stopped);
            }
            Command::Shutdown => {
                self.request_shutdown();
                return Ok(());
            }
            // Every start, also from the button, carries the Stop count.
            Command::Start(settings) => QueuedCommand::Start {
                settings,
                stops: self.control.borrow().stops,
                from_button: false,
                reserved: false,
            },
            Command::StartFromButton(settings) => QueuedCommand::Start {
                settings,
                stops: self.control.borrow().stops,
                from_button: true,
                reserved: false,
            },
            Command::ConfigureHotkey => QueuedCommand::ConfigureHotkey,
        };
        self.tx.try_send(command).map_err(|error| match error {
            TrySendError::Full(_) => SendError::Busy,
            TrySendError::Closed(_) => SendError::Stopped,
        })
    }

    /// Whether the worker loop has ended and can no longer report state changes.
    pub fn is_stopped(&self) -> bool {
        self.tx.is_closed()
    }

    /// End the worker and wait a bounded time for its portal cleanup.
    pub fn shutdown(self) {
        if !self.shutdown_within(SHUTDOWN_WAIT) {
            eprintln!(
                "Klicken beendet. Das Schließen der Freigaben dauert länger als {} s \
                 und wird im Hintergrund fortgesetzt.",
                SHUTDOWN_WAIT.as_secs()
            );
        }
    }

    /// Report whether the worker thread ended within `wait`.
    fn shutdown_within(mut self, wait: Duration) -> bool {
        self.request_shutdown();
        let finished = !matches!(
            self.finished.recv_timeout(wait),
            Err(RecvTimeoutError::Timeout)
        );
        // A worker that is still running is detached instead of joined.
        if let Some(join) = self.join.take()
            && finished
        {
            let _ = join.join();
        }
        finished
    }

    fn request_shutdown(&self) {
        if !self.closing.swap(true, Ordering::AcqRel) {
            self.control.send_modify(|control| control.shutdown = true);
        }
    }
}

impl Drop for WorkerHandle {
    fn drop(&mut self) {
        // Rust does not wait for detached threads at process exit. Explicit
        // shutdown waits in the window closing handler.
        self.request_shutdown();
    }
}

fn worker_start_failed(error: &dyn fmt::Display) -> WorkerEvent {
    WorkerEvent::Error(WorkerError::WorkerStartFailed(error.to_string()))
}

fn hotkey_settings_error(error: impl fmt::Display) -> HotkeyError {
    HotkeyError::SettingsUnavailable(error.to_string())
}

fn block_on_runtime(
    runtime: io::Result<Runtime>,
    emit: &Emitter,
    worker: impl std::future::Future<Output = ()>,
) {
    match runtime {
        Ok(runtime) => runtime.block_on(worker),
        Err(error) => {
            // Close the command channel before the UI can react to the error.
            drop(worker);
            (emit)(worker_start_failed(&error));
        }
    }
}

struct HotkeyState {
    connection: ashpd::zbus::Connection,
    portal: Arc<GlobalShortcuts>,
    session: Arc<Session<GlobalShortcuts>>,
    events: HotkeySignals,
    /// Unique bus name of the portal connection that sends the hotkey signals.
    portal_owner: OwnedUniqueName,
    start_barrier: StartBarrier<Sequence>,
}

enum HotkeySignal {
    /// A key press and where the hotkey connection received it.
    Activated(Activated, Sequence),
    Changed(ShortcutsChanged),
    Closed,
}

/// A portal reply that resolves to the position at which it was received.
type PortalReply<P> = Pin<Box<dyn std::future::Future<Output = P> + Send>>;

/// Tells key presses from before the end of the latest run apart from new ones.
///
/// After a run ends, the worker reads a portal property over the hotkey
/// connection. D-Bus delivers the messages of one sender in order, so every
/// `Activated` signal the portal sent before its reply was received before
/// it. Such an old press may still stop a run, but it never starts one.
/// Key releases (`Deactivated`) are not subscribed and change nothing.
struct StartBarrier<P> {
    reply: Option<PortalReply<P>>,
    /// Presses received before this position are older than the latest run end.
    boundary: Option<P>,
}

impl<P: Copy + Ord> StartBarrier<P> {
    const fn new() -> Self {
        Self {
            reply: None,
            boundary: None,
        }
    }

    /// Start over after a run ended. Until the reply arrives, every press is old.
    fn arm(&mut self, reply: PortalReply<P>) {
        self.reply = Some(reply);
    }

    /// Drive the outstanding request and record where its reply arrived.
    async fn settle(&mut self) {
        match self.reply.as_mut() {
            Some(reply) => {
                let boundary = reply.await;
                self.reply = None;
                self.boundary = Some(boundary);
            }
            None => future::pending().await,
        }
    }

    /// Whether a press received at `position` is older than the latest run end.
    fn is_old(&mut self, position: P) -> bool {
        if let Some(reply) = self.reply.as_mut() {
            // zbus completes a call before it queues anything received later.
            // A reply that arrived before this press is therefore ready now.
            match reply.now_or_never() {
                Some(boundary) => {
                    self.reply = None;
                    self.boundary = Some(boundary);
                }
                None => return true,
            }
        }
        self.boundary.is_some_and(|boundary| position < boundary)
    }
}

/// Read a portal property over the hotkey connection and return where its
/// reply arrived. The call goes to `owner`, the portal connection that sends
/// the `Activated` signals, so the reply is ordered behind them. Anything
/// else is retried; the hotkey signals report the end of the session.
async fn portal_reply_position(portal: zbus::Proxy<'static>, owner: OwnedUniqueName) -> Sequence {
    loop {
        let reply = portal
            .connection()
            .call_method(
                Some(owner.inner().clone()),
                portal.path().clone(),
                Some("org.freedesktop.DBus.Properties"),
                "Get",
                &(portal.interface().as_str(), "version"),
            )
            .await;
        if let Some(position) = ordered_reply_position(reply, &owner) {
            return position;
        }
        sleep(PORTAL_CLOSE_TIMEOUT).await;
    }
}

/// The receive position of a reply, also an error reply, that `owner` sent
/// itself. Errors from the bus daemon, such as an unknown or vanished
/// portal, are not ordered behind the portal's signals and do not count.
fn ordered_reply_position(
    reply: zbus::Result<Message>,
    owner: &UniqueName<'_>,
) -> Option<Sequence> {
    let (Ok(reply) | Err(zbus::Error::MethodError(_, _, reply))) = reply else {
        return None;
    };
    (reply.header().sender() == Some(owner)).then(|| reply.recv_position())
}

struct ActiveRun {
    settings: ClickSettings,
    schedule: Schedule,
    next_tick: Instant,
}

struct PendingCountdown {
    settings: ClickSettings,
    remaining: u8,
    next_tick: Instant,
}

struct StartedSession {
    session: PlatformClickSession,
    settings: ClickSettings,
    /// A button start at the cursor still needs its countdown after the permission.
    delay: bool,
}

type Emitter = Arc<dyn Fn(WorkerEvent) + Send + Sync>;

type HotkeyClosedWatcher = Pin<Box<dyn FusedFuture<Output = HotkeySignal> + Send>>;

type HotkeySignals = Pin<Box<dyn Stream<Item = HotkeySignal> + Send>>;

struct HotkeyRegistration {
    connection: ashpd::zbus::Connection,
    portal: GlobalShortcuts,
    session: Arc<Session<GlobalShortcuts>>,
    /// Unique bus name of the portal connection that created the session.
    portal_owner: OwnedUniqueName,
    closed: HotkeyClosedWatcher,
    /// `Activated` and `ShortcutsChanged`, subscribed before BindShortcuts.
    signals: HotkeySignals,
    actual: Option<String>,
}

/// The trigger of the start/stop shortcut, if it still has one.
fn bound_trigger(shortcuts: &[Shortcut]) -> Option<String> {
    shortcuts
        .iter()
        .find(|shortcut| {
            shortcut.id() == HOTKEY_ID && !shortcut.trigger_description().trim().is_empty()
        })
        .map(|shortcut| shortcut.trigger_description().to_owned())
}

/// Keep the same revocation subscription through binding and the worker handoff.
/// Check once more after completion: the operation can queue Closed in the same
/// poll that returns its successful result.
async fn while_hotkey_open<T>(
    closed: &mut HotkeyClosedWatcher,
    operation: impl std::future::Future<Output = Result<T, HotkeyError>>,
) -> Result<T, HotkeyError> {
    if closed.is_terminated() {
        return Err(HotkeyError::SessionClosed);
    }
    let result = tokio::select! {
        biased;
        _ = closed.as_mut() => return Err(HotkeyError::SessionClosed),
        result = operation => result,
    };
    if closed.as_mut().now_or_never().is_some() {
        return Err(HotkeyError::SessionClosed);
    }
    result
}

/// Bind the stop hotkey on an owned connection and clean up failed or cancelled requests.
async fn setup_hotkey(
    mut cancel: oneshot::Receiver<()>,
) -> Result<HotkeyRegistration, HotkeyError> {
    let connection = tokio::select! {
        biased;
        _ = &mut cancel => return Err(HotkeyError::RequestCancelled),
        result = ashpd::zbus::Connection::session() => result.map_err(|error| HotkeyError::Setup(error.to_string()))?,
    };
    let mut session = None;
    let operation = async {
        register_connection(&connection)
            .await
            .map_err(|error| HotkeyError::Registration(error.to_string()))?;
        let portal = GlobalShortcuts::with_connection(connection.clone())
            .await
            .map_err(|error| HotkeyError::Unavailable(error.to_string()))?;
        let (portal_owner, owner_lost) = watch_portal_owner(&connection)
            .await
            .map_err(|error| HotkeyError::Setup(error.to_string()))?;
        let mut owner_lost = Box::pin(owner_lost);
        // A vanished portal never sends the Response to its pending request.
        let created = tokio::select! {
            biased;
            () = &mut owner_lost => return Err(HotkeyError::SessionClosed),
            result = portal.create_session(Default::default()) => result,
        };
        session = Some(Arc::new(
            created.map_err(|error| HotkeyError::Setup(error.to_string()))?,
        ));
        let owned_session = session.as_ref().ok_or(HotkeyError::SessionMissing)?;
        let watched_session = Arc::clone(owned_session);
        let (ready_tx, ready_rx) = oneshot::channel();
        let mut closed: HotkeyClosedWatcher = Box::pin(
            async move {
                if let Ok(events) = watched_session.receive_closed().await {
                    let _ = ready_tx.send(());
                    futures_util::pin_mut!(events);
                    tokio::select! {
                        _ = events.next() => {}
                        () = owner_lost => {}
                    }
                }
                HotkeySignal::Closed
            }
            .fuse(),
        );
        if !register_hotkey_watcher(ready_rx, &mut closed).await {
            return Err(HotkeyError::WatchUnavailable);
        }
        // ashpd adds the match rules only when subscribing. Subscribe before
        // binding, so no change the portal sends after its response is lost.
        // Bounded, so a stalled bus cannot delay a later Stop or Shutdown.
        let (activations, changes) = while_hotkey_open(&mut closed, async {
            timeout(PORTAL_CLOSE_TIMEOUT, async {
                // Raw messages keep their receive position for the start barrier.
                let activations = portal.receive_signal("Activated").await?;
                let changes = portal.receive_shortcuts_changed().await?;
                Ok::<_, ashpd::Error>((activations, changes))
            })
            .await
            .map_err(|_| HotkeyError::SubscribeTimeout)?
            .map_err(|error| HotkeyError::Receive(error.to_string()))
        })
        .await?;
        let binding = async {
            let shortcut = NewShortcut::new(HOTKEY_ID, messages::hotkey_description())
                .preferred_trigger(Some(PREFERRED_TRIGGER));
            let parent = parent_window::identifier().await;
            let response = portal
                .bind_shortcuts(
                    owned_session,
                    &[shortcut],
                    parent.as_ref(),
                    BindShortcutsOptions::default(),
                )
                .await
                .map_err(|error| HotkeyError::Request(error.to_string()))?
                .response()
                .map_err(|error| HotkeyError::NotGranted(error.to_string()))?;
            let shortcut = response
                .shortcuts()
                .iter()
                .find(|shortcut| shortcut.id() == HOTKEY_ID)
                .ok_or(HotkeyError::Unbound)?;
            let actual = (!shortcut.trigger_description().trim().is_empty())
                .then(|| shortcut.trigger_description().to_owned());
            Ok(actual)
        };
        let actual = while_hotkey_open(&mut closed, binding).await?;
        let activations = activations.filter_map(|message| {
            let activation = message.body().deserialize::<Activated>().ok();
            future::ready(
                activation
                    .map(|activation| HotkeySignal::Activated(activation, message.recv_position())),
            )
        });
        let signals: HotkeySignals = Box::pin(futures_util::stream::select(
            activations,
            changes.map(HotkeySignal::Changed),
        ));
        Ok((portal, portal_owner, actual, closed, signals))
    };
    let result = tokio::select! {
        biased;
        _ = &mut cancel => Err(HotkeyError::RequestCancelled),
        result = operation => result,
    };
    match result {
        Ok((portal, portal_owner, actual, closed, signals)) => Ok(HotkeyRegistration {
            connection,
            portal,
            portal_owner,
            closed,
            signals,
            actual,
            session: session.ok_or(HotkeyError::SessionMissing)?,
        }),
        Err(error) => {
            if let Some(session) = session {
                let _ = timeout(PORTAL_CLOSE_TIMEOUT, session.close()).await;
            }
            let _ = timeout(PORTAL_CLOSE_TIMEOUT, connection.close()).await;
            Err(error)
        }
    }
}

/// Create a portal session for the selected monitor or current cursor.
async fn setup_click_session(
    settings: ClickSettings,
    delay: bool,
    cancel: oneshot::Receiver<()>,
) -> Result<StartedSession, Box<ClickError>> {
    let session = PlatformClickSession::create(settings.monitor, cancel)
        .await
        .map_err(Box::new)?;
    Ok(StartedSession {
        session,
        settings,
        delay,
    })
}

async fn wait_for_tick(deadline: Option<Instant>) {
    match deadline {
        // The timer rounds every deadline up to its next millisecond, even
        // one that has passed. A due tick must not wait for that.
        Some(deadline) if deadline <= Instant::now() => {}
        Some(deadline) => sleep_until(deadline).await,
        None => future::pending().await,
    }
}

/// Wait for the next hotkey signal while driving the start barrier's request.
async fn next_hotkey(stream: &mut Option<HotkeyState>) -> Option<HotkeySignal> {
    let Some(state) = stream else {
        return future::pending().await;
    };
    loop {
        tokio::select! {
            biased;
            () = state.start_barrier.settle() => {}
            signal = state.events.next() => return signal,
        }
    }
}

async fn wait_click_session_closed(session: &mut Option<PlatformClickSession>) {
    match session {
        Some(session) => session.wait_closed().await,
        None => future::pending().await,
    }
}

async fn wait_task<T>(task: &mut Option<JoinHandle<T>>) -> Result<T, tokio::task::JoinError> {
    match task {
        Some(task) => task.await,
        None => future::pending().await,
    }
}

/// Bound signal registration without limiting the subsequent session lifetime.
async fn register_hotkey_watcher(
    ready: oneshot::Receiver<()>,
    closed: impl std::future::Future<Output = HotkeySignal>,
) -> bool {
    timeout(PORTAL_CLOSE_TIMEOUT, async {
        tokio::select! {
            result = ready => result.is_ok(),
            _ = closed => false,
        }
    })
    .await
    .unwrap_or(false)
}

/// What woke the worker loop, in the order `next_wake` checks it.
enum Wake {
    Control(Result<(), watch::error::RecvError>),
    ClickSessionClosed,
    ClickSessionIdle,
    Command(Option<QueuedCommand>),
    StartReply(Result<ClickSettings, oneshot::error::RecvError>),
    StartTask(Result<Result<StartedSession, Box<ClickError>>, tokio::task::JoinError>),
    HotkeyTask(Result<Result<HotkeyRegistration, HotkeyError>, tokio::task::JoinError>),
    ConfigureTask(Result<Result<(), HotkeyError>, tokio::task::JoinError>),
    Hotkey(Option<HotkeySignal>),
    CountdownTick,
    ClickTick,
}

/// Everything the worker loop waits on; `None` deadlines are not armed.
struct WakeSources<'a> {
    control: &'a mut watch::Receiver<Control>,
    click_session: &'a mut Option<PlatformClickSession>,
    commands: &'a mut mpsc::Receiver<QueuedCommand>,
    start_request: &'a mut Option<PendingStartRequest>,
    start_task: &'a mut Option<JoinHandle<Result<StartedSession, Box<ClickError>>>>,
    hotkey_task: &'a mut Option<JoinHandle<Result<HotkeyRegistration, HotkeyError>>>,
    configure_task: &'a mut Option<JoinHandle<Result<(), HotkeyError>>>,
    hotkey: &'a mut Option<HotkeyState>,
    countdown: Option<Instant>,
    tick: Option<Instant>,
    session_idle: Option<Instant>,
}

/// Wait for the next event. Stop and Shutdown are checked first, so they
/// overtake every queued command and a countdown or click tick that is due.
async fn next_wake(sources: WakeSources<'_>) -> Wake {
    let WakeSources {
        control,
        click_session,
        commands,
        start_request,
        start_task,
        hotkey_task,
        configure_task,
        hotkey,
        countdown,
        tick,
        session_idle,
    } = sources;
    tokio::select! {
        biased;
        changed = control.changed() => Wake::Control(changed),
        () = wait_click_session_closed(click_session), if click_session.is_some() => Wake::ClickSessionClosed,
        command = commands.recv() => Wake::Command(command),
        result = async {
            match start_request {
                Some(request) => (&mut request.reply).await,
                None => future::pending().await,
            }
        } => Wake::StartReply(result),
        result = wait_task(start_task), if start_task.is_some() => Wake::StartTask(result),
        result = wait_task(hotkey_task), if hotkey_task.is_some() => Wake::HotkeyTask(result),
        result = wait_task(configure_task), if configure_task.is_some() => Wake::ConfigureTask(result),
        signal = next_hotkey(hotkey), if hotkey.is_some() => Wake::Hotkey(signal),
        () = wait_for_tick(countdown), if countdown.is_some() => Wake::CountdownTick,
        () = wait_for_tick(tick), if tick.is_some() => Wake::ClickTick,
        () = wait_for_tick(session_idle), if session_idle.is_some() => Wake::ClickSessionIdle,
    }
}

/// Serialize clicks, permissions and shortcut events.
async fn run_worker(
    commands: mpsc::Receiver<QueuedCommand>,
    control: watch::Receiver<Control>,
    emit: Emitter,
    closing: Arc<AtomicBool>,
    session_idle: Duration,
) {
    let mut worker = Worker::new(commands, control, emit, session_idle);
    loop {
        let wake = worker.wait().await;
        if worker.handle(wake).await.is_break() {
            break;
        }
    }
    worker.close(&closing).await;
}

/// The worker loop's state. Each `Wake` has one handler; `Break` ends the loop.
struct Worker {
    commands: mpsc::Receiver<QueuedCommand>,
    control: watch::Receiver<Control>,
    emit: Emitter,
    session_idle: Duration,
    machine: StateMachine,
    click_session: Option<PlatformClickSession>,
    click_session_idle: Option<Instant>,
    active: Option<ActiveRun>,
    countdown: Option<PendingCountdown>,
    start_task: Option<JoinHandle<Result<StartedSession, Box<ClickError>>>>,
    start_cancel: Option<oneshot::Sender<()>>,
    start_request: Option<PendingStartRequest>,
    hotkey_cancel: Option<oneshot::Sender<()>>,
    hotkey_task: Option<JoinHandle<Result<HotkeyRegistration, HotkeyError>>>,
    configure_task: Option<JoinHandle<Result<(), HotkeyError>>>,
    hotkey: Option<HotkeyState>,
    hotkey_bound: bool,
    /// Run ends and Stops the start barrier has seen.
    ended_runs: u64,
    barrier_stops: u64,
}

impl Worker {
    /// Begin the hotkey registration and report the initial state.
    fn new(
        commands: mpsc::Receiver<QueuedCommand>,
        control: watch::Receiver<Control>,
        emit: Emitter,
        session_idle: Duration,
    ) -> Self {
        let machine = StateMachine::default();
        let (tx, rx) = oneshot::channel();
        let hotkey_task = tokio::spawn(setup_hotkey(rx));
        (emit)(WorkerEvent::State(machine.state()));
        (emit)(WorkerEvent::Status(WorkerStatus::RegisteringHotkey));
        (emit)(WorkerEvent::HotkeyPhase(HotkeyPhase::Registering));
        let ended_runs = machine.ended_runs();
        let barrier_stops = control.borrow().stops;
        Self {
            commands,
            control,
            emit,
            session_idle,
            machine,
            click_session: None,
            click_session_idle: None,
            active: None,
            countdown: None,
            start_task: None,
            start_cancel: None,
            start_request: None,
            hotkey_cancel: Some(tx),
            hotkey_task: Some(hotkey_task),
            configure_task: None,
            hotkey: None,
            hotkey_bound: false,
            ended_runs,
            barrier_stops,
        }
    }

    fn emit(&self, event: WorkerEvent) {
        (self.emit)(event);
    }

    /// Update the start barrier and idle deadline, then wait for the next event.
    async fn wait(&mut self) -> Wake {
        if self.machine.state() != RunState::Starting {
            // Closing this receiver revokes even a reply already queued on Qt.
            self.start_request = None;
        }
        // A run ended, by Stop, key press, error or its last click, or a
        // Stop arrived without a run. Key presses the portal has sent up to
        // now may still be queued behind the busy worker. Ask for a new
        // boundary before reading the next one.
        let stops = self.control.borrow().stops;
        if self.machine.ended_runs() != self.ended_runs || stops != self.barrier_stops {
            self.ended_runs = self.machine.ended_runs();
            self.barrier_stops = stops;
            if let Some(state) = self.hotkey.as_mut() {
                let portal = zbus::Proxy::clone(&state.portal);
                let owner = state.portal_owner.clone();
                state
                    .start_barrier
                    .arm(Box::pin(portal_reply_position(portal, owner)));
            }
        }
        // The idle time of the mouse permission starts when a run ends.
        self.click_session_idle = if self.click_session.is_none() || self.is_running() {
            None
        } else {
            self.click_session_idle
                .or_else(|| Some(Instant::now() + self.session_idle))
        };
        next_wake(WakeSources {
            control: &mut self.control,
            click_session: &mut self.click_session,
            commands: &mut self.commands,
            start_request: &mut self.start_request,
            start_task: &mut self.start_task,
            hotkey_task: &mut self.hotkey_task,
            configure_task: &mut self.configure_task,
            hotkey: &mut self.hotkey,
            countdown: self.countdown.as_ref().map(|pending| pending.next_tick),
            tick: self.active.as_ref().map(|run| run.next_tick),
            session_idle: self.click_session_idle,
        })
        .await
    }

    async fn handle(&mut self, wake: Wake) -> ControlFlow<()> {
        match wake {
            Wake::Control(changed) => return self.on_control(changed).await,
            Wake::ClickSessionClosed => {
                self.discard_closed_click_session().await;
            }
            Wake::ClickSessionIdle => self.on_click_session_idle().await,
            Wake::Command(None) => return ControlFlow::Break(()),
            Wake::Command(Some(QueuedCommand::Start {
                settings,
                stops,
                from_button,
                reserved,
            })) => return self.on_start(settings, stops, from_button, reserved).await,
            Wake::Command(Some(QueuedCommand::ConfigureHotkey)) => {
                self.on_configure_hotkey().await;
            }
            Wake::StartReply(result) => return self.on_start_reply(result).await,
            Wake::StartTask(result) => self.on_start_task(result).await,
            Wake::HotkeyTask(result) => self.on_hotkey_task(result),
            Wake::ConfigureTask(result) => self.on_configure_task(result).await,
            Wake::Hotkey(signal) => self.on_hotkey(signal).await,
            Wake::CountdownTick => self.on_countdown_tick(),
            Wake::ClickTick => self.on_click_tick().await,
        }
        ControlFlow::Continue(())
    }

    fn is_running(&self) -> bool {
        matches!(
            self.machine.state(),
            RunState::Starting | RunState::Clicking
        )
    }

    /// Stop or Shutdown, which overtake every queued command.
    async fn on_control(
        &mut self,
        changed: Result<(), watch::error::RecvError>,
    ) -> ControlFlow<()> {
        if changed.is_err() || self.control.borrow().shutdown {
            return ControlFlow::Break(());
        }
        self.cancel_start().await;
        self.stop_run();
        ControlFlow::Continue(())
    }

    async fn on_click_session_idle(&mut self) {
        self.click_session_idle = None;
        if let Some(session) = self.click_session.take() {
            session.close().await;
        }
    }

    /// Qt's reply to a reserved hotkey start, or its refusal.
    async fn on_start_reply(
        &mut self,
        result: Result<ClickSettings, oneshot::error::RecvError>,
    ) -> ControlFlow<()> {
        let Some(request) = self.start_request.take() else {
            return ControlFlow::Continue(());
        };
        match result {
            Ok(settings) => self.on_start(settings, request.stops, false, true).await,
            Err(_) => {
                self.stop_run();
                ControlFlow::Continue(())
            }
        }
    }

    async fn on_start(
        &mut self,
        settings: ClickSettings,
        stops: u64,
        from_button: bool,
        reserved: bool,
    ) -> ControlFlow<()> {
        // A Stop may have arrived after this turn checked for one.
        // Handle it first, so it cannot cancel a start sent after it.
        match take_control_change(&mut self.control) {
            ControlChange::None => {}
            ControlChange::Stop => {
                self.cancel_start().await;
                self.stop_run();
            }
            ControlChange::Shutdown => return ControlFlow::Break(()),
        }
        // A later Stop overtook this start in the queue.
        if is_outdated(stops, &self.control) {
            return ControlFlow::Continue(());
        }
        let delay = from_button && settings.position.is_none();
        if !self.hotkey_bound || self.configure_task.is_some() {
            self.emit(WorkerEvent::Error(WorkerError::HotkeyRequired));
            return ControlFlow::Continue(());
        }
        if self
            .click_session
            .as_mut()
            .is_some_and(PlatformClickSession::is_closed)
            && self.discard_closed_click_session().await
        {
            return ControlFlow::Continue(());
        }
        match request_validated_start(&mut self.machine, &settings, reserved) {
            Ok(true) => {
                self.emit(WorkerEvent::State(self.machine.state()));
            }
            Ok(false) => return ControlFlow::Continue(()),
            Err(error) => {
                self.emit(WorkerEvent::State(self.machine.state()));
                self.emit(WorkerEvent::Error(WorkerError::InvalidSettings(error)));
                return ControlFlow::Continue(());
            }
        }
        if self
            .click_session
            .as_ref()
            .is_some_and(|session| session.matches_monitor(settings.monitor))
        {
            self.begin_run(settings, delay);
        } else {
            if let Some(old_session) = self.click_session.take() {
                old_session.close().await;
            }
            self.emit(WorkerEvent::Status(WorkerStatus::AwaitingClickPermission));
            let (tx, rx) = oneshot::channel();
            self.start_cancel = Some(tx);
            self.start_task = Some(tokio::spawn(setup_click_session(settings, delay, rx)));
        }
        ControlFlow::Continue(())
    }

    async fn on_configure_hotkey(&mut self) {
        if self.configure_task.is_some() || self.hotkey_task.is_some() {
            return;
        }
        // ConfigureShortcuts requires v2. On v1 only the system settings
        // can change a bound hotkey, and the GUI points there, so keep
        // the binding and a pending start. An unbound v1 session can
        // only recover through a fresh BindShortcuts.
        let v1 = self
            .hotkey
            .as_ref()
            .is_some_and(|state| state.portal.version() < 2);
        if self.hotkey_bound && v1 {
            return;
        }
        if self.start_request.take().is_some() {
            self.stop_run();
        }
        if v1 && let Some(state) = self.hotkey.take() {
            let _ = timeout(PORTAL_CLOSE_TIMEOUT, state.session.close()).await;
            let _ = timeout(PORTAL_CLOSE_TIMEOUT, state.connection.close()).await;
        }
        if let Some(state) = &self.hotkey {
            let portal = Arc::clone(&state.portal);
            let session = Arc::clone(&state.session);
            self.emit(WorkerEvent::HotkeyPhase(HotkeyPhase::Configuring(
                self.hotkey_bound,
            )));
            self.configure_task = Some(tokio::spawn(async move {
                let parent = parent_window::identifier().await;
                portal
                    .configure_shortcuts(
                        &session,
                        parent.as_ref(),
                        ConfigureShortcutsOptions::default(),
                    )
                    .await
                    .map_err(hotkey_settings_error)
            }));
        } else {
            let (tx, rx) = oneshot::channel();
            self.hotkey_cancel = Some(tx);
            self.hotkey_task = Some(tokio::spawn(setup_hotkey(rx)));
            self.emit(WorkerEvent::HotkeyPhase(HotkeyPhase::Registering));
            self.emit(WorkerEvent::Status(WorkerStatus::RegisteringHotkey));
        }
    }

    /// The mouse permission for a start was granted, refused or cancelled.
    async fn on_start_task(
        &mut self,
        result: Result<Result<StartedSession, Box<ClickError>>, tokio::task::JoinError>,
    ) {
        self.start_task = None;
        self.start_cancel = None;
        match result {
            Ok(Ok(StartedSession {
                mut session,
                settings,
                delay,
            })) => {
                if session.is_closed() {
                    session.close().await;
                    self.fail_run();
                    self.emit(WorkerEvent::Error(WorkerError::Click(
                        PlatformClickSession::closed_error(),
                    )));
                } else {
                    self.click_session = Some(session);
                    self.begin_run(settings, delay);
                }
            }
            Ok(Err(error)) => {
                self.fail_run();
                self.emit(WorkerEvent::Error(WorkerError::Click(*error)));
            }
            Err(error) if error.is_cancelled() => {
                self.stop_run();
            }
            Err(error) => {
                self.fail_run();
                self.emit(WorkerEvent::Error(WorkerError::ClickTaskFailed(
                    error.to_string(),
                )));
            }
        }
    }

    fn on_hotkey_task(
        &mut self,
        result: Result<Result<HotkeyRegistration, HotkeyError>, tokio::task::JoinError>,
    ) {
        self.hotkey_task = None;
        self.hotkey_cancel = None;
        match result {
            Ok(Ok(HotkeyRegistration {
                connection,
                portal,
                session,
                portal_owner,
                closed,
                mut signals,
                mut actual,
            })) => {
                // Apply changes received so far right before Ready allows a start.
                // A press before Ready never starts a run, as before the early subscription.
                while let Some(Some(signal)) = signals.next().now_or_never() {
                    if let HotkeySignal::Changed(changed) = signal {
                        actual = bound_trigger(changed.shortcuts());
                    }
                }
                let events =
                    futures_util::stream::select(signals, futures_util::stream::once(closed));
                self.emit(WorkerEvent::HotkeyConfigurable(portal.version() >= 2));
                self.hotkey = Some(HotkeyState {
                    connection,
                    portal: Arc::new(portal),
                    session,
                    events: Box::pin(events),
                    portal_owner,
                    start_barrier: StartBarrier::new(),
                });
                if let Some(actual) = actual {
                    self.hotkey_bound = true;
                    self.emit(WorkerEvent::Hotkey(actual));
                    self.emit(WorkerEvent::HotkeyPhase(HotkeyPhase::Ready));
                    if matches!(
                        self.machine.state(),
                        RunState::Ready | RunState::Stopped | RunState::Error
                    ) {
                        self.emit(WorkerEvent::Status(WorkerStatus::Ready));
                    }
                } else {
                    self.hotkey_bound = false;
                    self.emit(WorkerEvent::HotkeyPhase(HotkeyPhase::Unavailable));
                    self.emit(WorkerEvent::Error(WorkerError::Hotkey(
                        HotkeyError::Unbound,
                    )));
                }
            }
            Ok(Err(error)) => {
                self.emit(WorkerEvent::HotkeyPhase(HotkeyPhase::Unavailable));
                self.emit(WorkerEvent::Error(WorkerError::Hotkey(error)));
            }
            Err(error) if !error.is_cancelled() => {
                self.emit(WorkerEvent::HotkeyPhase(HotkeyPhase::Unavailable));
                self.emit(WorkerEvent::Error(WorkerError::Hotkey(HotkeyError::Setup(
                    error.to_string(),
                ))));
            }
            Err(_) => self.emit(WorkerEvent::HotkeyPhase(HotkeyPhase::Unavailable)),
        }
    }

    async fn on_configure_task(
        &mut self,
        result: Result<Result<(), HotkeyError>, tokio::task::JoinError>,
    ) {
        self.configure_task = None;
        // ConfigureShortcuts only acknowledges the method call. The portal
        // does not report when its settings window is closed.
        self.emit(WorkerEvent::HotkeyPhase(if self.hotkey_bound {
            HotkeyPhase::Ready
        } else {
            HotkeyPhase::Unavailable
        }));
        match result {
            Ok(Ok(())) if self.hotkey_bound && !self.is_running() => {
                self.emit(WorkerEvent::Status(WorkerStatus::Ready));
            }
            Ok(Ok(())) if !self.hotkey_bound => {
                self.emit(WorkerEvent::Status(WorkerStatus::AwaitingHotkey));
            }
            Ok(Ok(())) => {}
            Ok(Err(error)) => {
                self.cancel_start().await;
                self.abort_run();
                self.emit(WorkerEvent::Error(WorkerError::Hotkey(error)));
            }
            Err(error) if !error.is_cancelled() => {
                self.cancel_start().await;
                self.abort_run();
                self.emit(WorkerEvent::Error(WorkerError::Hotkey(
                    hotkey_settings_error(error),
                )));
            }
            Err(_) => {}
        }
    }

    async fn on_hotkey(&mut self, signal: Option<HotkeySignal>) {
        match signal {
            Some(HotkeySignal::Activated(activation, received))
                if activation.shortcut_id() == HOTKEY_ID && self.hotkey_bound =>
            {
                self.on_hotkey_pressed(received).await;
            }
            Some(HotkeySignal::Changed(changed)) => {
                self.on_hotkey_changed(bound_trigger(changed.shortcuts()))
                    .await;
            }
            Some(HotkeySignal::Activated(..)) => {}
            Some(HotkeySignal::Closed) | None => self.on_hotkey_closed().await,
        }
    }

    /// Toggle: stop a run, or reserve a start behind the start barrier.
    async fn on_hotkey_pressed(&mut self, received: Sequence) {
        if self.is_running() {
            // Any press stops, also one from before the latest run end.
            self.cancel_start().await;
            self.stop_run();
        } else if self.configure_task.is_none()
            && self
                .hotkey
                .as_mut()
                .is_some_and(|state| !state.start_barrier.is_old(received))
        {
            // Reserve Starting before asking Qt for current controls, so
            // a second press toggles off even while Qt is still busy.
            if self.machine.request_start() {
                let (request, response) = StartRequest::channel();
                self.start_request = Some(PendingStartRequest {
                    reply: response,
                    stops: self.control.borrow().stops,
                });
                self.emit(WorkerEvent::State(self.machine.state()));
                self.emit(WorkerEvent::StartRequested(request));
            }
        }
    }

    async fn on_hotkey_changed(&mut self, trigger: Option<String>) {
        if let Some(trigger) = trigger {
            // A reply for the previous shortcut configuration cannot start.
            if self.start_request.take().is_some() {
                self.stop_run();
            }
            self.hotkey_bound = true;
            self.emit(WorkerEvent::Hotkey(trigger));
            self.emit(WorkerEvent::HotkeyPhase(if self.configure_task.is_some() {
                HotkeyPhase::Configuring(true)
            } else {
                HotkeyPhase::Ready
            }));
            if self.configure_task.is_none() && !self.is_running() {
                self.emit(WorkerEvent::Status(WorkerStatus::Ready));
            }
        } else {
            self.cancel_start().await;
            self.abort_run();
            self.hotkey_bound = false;
            self.emit(WorkerEvent::HotkeyPhase(if self.configure_task.is_some() {
                HotkeyPhase::Configuring(false)
            } else {
                HotkeyPhase::Unavailable
            }));
            self.emit(WorkerEvent::Error(WorkerError::Hotkey(
                HotkeyError::Removed,
            )));
        }
    }

    async fn on_hotkey_closed(&mut self) {
        self.cancel_start().await;
        self.abort_run();
        if let Some(task) = self.configure_task.take() {
            task.abort();
        }
        self.hotkey_bound = false;
        self.emit(WorkerEvent::HotkeyPhase(HotkeyPhase::Unavailable));
        self.emit(WorkerEvent::Error(WorkerError::Hotkey(
            HotkeyError::SessionClosed,
        )));
        if let Some(state) = self.hotkey.take() {
            let _ = timeout(PORTAL_CLOSE_TIMEOUT, state.connection.close()).await;
        }
    }

    fn on_countdown_tick(&mut self) {
        let Some(pending) = self.countdown.as_mut() else {
            return;
        };
        if pending.remaining > 1 {
            pending.remaining -= 1;
            pending.next_tick += Duration::from_secs(1);
            let remaining = pending.remaining;
            self.emit(WorkerEvent::Countdown(remaining));
        } else {
            let Some(settings) = self.countdown.take().map(|pending| pending.settings) else {
                return;
            };
            self.start_run(settings);
        }
    }

    async fn on_click_tick(&mut self) {
        let Some(run) = self.active.as_mut() else {
            return;
        };
        let Some(session) = self.click_session.as_mut() else {
            self.fail_run();
            self.active = None;
            self.emit(WorkerEvent::Error(WorkerError::ClickSessionLost));
            return;
        };
        if !run.schedule.record_tick() {
            self.stop_run();
            return;
        }
        if let Err(error) = session.click(&run.settings).await {
            self.fail_run();
            self.active = None;
            self.emit(WorkerEvent::Error(WorkerError::Click(error)));
            // A revoked or failed session cannot safely be reused on retry.
            if let Some(failed_session) = self.click_session.take() {
                failed_session.close().await;
            }
            return;
        }
        if run.schedule.is_finished() {
            self.stop_run();
        } else {
            run.next_tick = run.schedule.next_deadline(run.next_tick, Instant::now());
        }
    }

    /// Keep the owner alive until its pending dialog/session has been closed.
    async fn cancel_start(&mut self) {
        if let Some(cancel) = self.start_cancel.take() {
            let _ = cancel.send(());
        }
        if let Some(task) = self.start_task.take()
            && let Ok(Ok(started)) = task.await
        {
            // The permission may have completed just before cancellation.
            started.session.close().await;
        }
    }

    /// Cancel registration and close even a session completed just before cancellation.
    async fn cancel_hotkey(&mut self) {
        if let Some(cancel) = self.hotkey_cancel.take() {
            let _ = cancel.send(());
        }
        if let Some(task) = self.hotkey_task.take()
            && let Ok(Ok(registration)) = task.await
        {
            let _ = timeout(PORTAL_CLOSE_TIMEOUT, registration.session.close()).await;
            let _ = timeout(PORTAL_CLOSE_TIMEOUT, registration.connection.close()).await;
        }
    }

    /// Report whether an active or pending run ended with the closed session.
    async fn discard_closed_click_session(&mut self) -> bool {
        let was_running = self.stop_run();
        if let Some(session) = self.click_session.take() {
            session.close().await;
        }
        if was_running {
            self.fail_run();
            self.emit(WorkerEvent::Error(WorkerError::Click(
                PlatformClickSession::closed_error(),
            )));
        } else {
            self.emit(WorkerEvent::Status(WorkerStatus::PermissionRequired));
        }
        was_running
    }

    fn fail_run(&mut self) {
        self.machine.fail();
        self.emit(WorkerEvent::State(self.machine.state()));
    }

    fn start_run(&mut self, settings: ClickSettings) {
        if !self.machine.started() {
            return;
        }
        let schedule = Schedule::new(settings.interval(), settings.repeat);
        self.active = Some(ActiveRun {
            settings,
            schedule,
            next_tick: Instant::now(),
        });
        self.emit(WorkerEvent::State(self.machine.state()));
        self.emit(WorkerEvent::Status(WorkerStatus::Clicking));
    }

    /// Start clicking now, or first give the user a cancellable countdown.
    fn begin_run(&mut self, settings: ClickSettings, delay: bool) {
        if delay {
            self.countdown = Some(PendingCountdown {
                settings,
                remaining: BUTTON_START_DELAY_SECS,
                next_tick: Instant::now() + Duration::from_secs(1),
            });
            self.emit(WorkerEvent::Countdown(BUTTON_START_DELAY_SECS));
        } else {
            self.start_run(settings);
        }
    }

    /// Report whether an active or pending run, including its countdown, was stopped.
    fn stop_run(&mut self) -> bool {
        if !self.machine.stop() {
            return false;
        }
        self.active = None;
        self.countdown = None;
        self.emit(WorkerEvent::State(self.machine.state()));
        self.emit(WorkerEvent::Status(WorkerStatus::Stopped));
        true
    }

    /// Fail only an active or pending run; hotkey problems use `HotkeyPhase` instead.
    fn abort_run(&mut self) {
        if self.stop_run() {
            self.fail_run();
        }
    }

    /// Mark the worker closed, then cancel pending requests and close the sessions.
    async fn close(mut self, closing: &AtomicBool) {
        self.machine.close();
        self.emit(WorkerEvent::State(self.machine.state()));
        closing.store(true, Ordering::Release);
        self.cancel_start().await;
        self.cancel_hotkey().await;
        if let Some(task) = self.configure_task.take() {
            task.abort();
        }
        if let Some(session) = self.click_session.take() {
            session.close().await;
        }
        if let Some(state) = self.hotkey.take() {
            let _ = timeout(PORTAL_CLOSE_TIMEOUT, state.session.close()).await;
            let _ = timeout(PORTAL_CLOSE_TIMEOUT, state.connection.close()).await;
        }
    }
}

/// Take a Stop or Shutdown that arrived since the worker last checked.
fn take_control_change(control: &mut watch::Receiver<Control>) -> ControlChange {
    match control.has_changed() {
        Ok(false) => ControlChange::None,
        Ok(true) if !control.borrow_and_update().shutdown => ControlChange::Stop,
        // A dropped handle ends the worker like Shutdown.
        _ => ControlChange::Shutdown,
    }
}

/// A start sent before the latest Stop must not run after it.
fn is_outdated(stops: u64, control: &watch::Receiver<Control>) -> bool {
    stops != control.borrow().stops
}

/// Ignore duplicate starts before they can change a run or its pending settings.
fn request_validated_start(
    machine: &mut StateMachine,
    settings: &ClickSettings,
    reserved: bool,
) -> Result<bool, ValidationError> {
    let accepted = if reserved {
        machine.state() == RunState::Starting
    } else {
        machine.request_start()
    };
    if !accepted {
        return Ok(false);
    }
    if let Err(error) = settings.validate() {
        machine.fail();
        return Err(error);
    }
    Ok(true)
}

/// The worker side of a handle, kept by a test so nothing drains the queue.
#[cfg(test)]
pub(crate) struct WorkerSide {
    commands: mpsc::Receiver<QueuedCommand>,
    control: watch::Receiver<Control>,
    _finished: std_mpsc::Sender<()>,
}

#[cfg(test)]
impl WorkerHandle {
    /// A handle without a worker loop; dropping the side ends the "worker".
    pub(crate) fn stalled(capacity: usize) -> (Self, WorkerSide) {
        let (tx, commands) = mpsc::channel(capacity);
        let (control, control_rx) = watch::channel(Control::default());
        let (finished_tx, finished) = std_mpsc::channel();
        let handle = Self {
            tx,
            control,
            closing: Arc::new(AtomicBool::new(false)),
            join: None,
            finished,
        };
        let side = WorkerSide {
            commands,
            control: control_rx,
            _finished: finished_tx,
        };
        (handle, side)
    }
}

#[cfg(test)]
impl WorkerSide {
    /// Stop requests the "worker" has been sent so far.
    pub(crate) fn stops(&self) -> u64 {
        self.control.borrow().stops
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn stalled_hotkey_registration_times_out() {
        let (_ready_tx, ready_rx) = oneshot::channel();
        let result = timeout(
            PORTAL_CLOSE_TIMEOUT * 2,
            register_hotkey_watcher(ready_rx, future::pending()),
        )
        .await;
        assert_eq!(result.ok(), Some(false));
    }

    #[tokio::test]
    async fn registered_hotkey_watcher_still_delivers_session_closure() {
        let (ready_tx, ready_rx) = oneshot::channel();
        let (close_tx, close_rx) = oneshot::channel();
        let mut closed = Box::pin(async move {
            let _ = ready_tx.send(());
            let _ = close_rx.await;
            HotkeySignal::Closed
        });
        assert!(register_hotkey_watcher(ready_rx, &mut closed).await);
        assert!(close_tx.send(()).is_ok());
        assert!(matches!(
            timeout(PORTAL_CLOSE_TIMEOUT, closed).await,
            Ok(HotkeySignal::Closed)
        ));
    }

    #[tokio::test]
    async fn session_closed_before_registration_is_not_ready() {
        let (_ready_tx, ready_rx) = oneshot::channel();
        assert!(!register_hotkey_watcher(ready_rx, async { HotkeySignal::Closed }).await);
    }

    #[tokio::test]
    async fn session_closed_at_successful_handoff_is_not_ready() {
        let (close_tx, close_rx) = oneshot::channel();
        let mut closed: HotkeyClosedWatcher = Box::pin(
            async move {
                let _ = close_rx.await;
                HotkeySignal::Closed
            }
            .fuse(),
        );
        let result = while_hotkey_open(&mut closed, async move {
            let _ = close_tx.send(());
            Ok(())
        })
        .await;
        assert_eq!(result, Err(HotkeyError::SessionClosed));
    }

    #[test]
    fn start_barrier_separates_presses_at_the_reply() {
        let mut barrier = StartBarrier::<u64>::new();
        // Before any run has ended, every press may start one.
        assert!(!barrier.is_old(1));
        // While the reply is outstanding, every press counts as old.
        let (reply_tx, reply_rx) = oneshot::channel();
        barrier.arm(Box::pin(async move { reply_rx.await.unwrap_or(u64::MAX) }));
        assert!(barrier.is_old(5));
        assert!(barrier.is_old(50));
        // A reply that is already there decides at once, without a wakeup.
        assert_eq!(reply_tx.send(10), Ok(()));
        assert!(barrier.is_old(9));
        assert!(!barrier.is_old(11));
        // A later run end moves the boundary on.
        barrier.arm(Box::pin(future::ready(20)));
        assert!(barrier.is_old(15));
        assert!(!barrier.is_old(21));
    }

    #[test]
    fn only_replies_from_the_signalling_portal_bound_old_presses() -> zbus::Result<()> {
        const PORTAL: &str = ":1.7";
        let portal = UniqueName::try_from(PORTAL)?;
        let call = Message::method_call("/org/freedesktop/portal/desktop", "Get")?
            .destination(PORTAL)?
            .build(&())?;
        let reply = |sender: &'static str| -> zbus::Result<Message> {
            Message::method_return(&call.header())?
                .sender(sender)?
                .build(&())
        };
        let error = |sender: &'static str, name: &'static str| -> zbus::Result<zbus::Error> {
            let message = Message::error(&call.header(), name)?
                .sender(sender)?
                .build(&("Testfehler",))?;
            Ok(zbus::Error::from(message))
        };
        // The portal's own replies, also errors, are ordered behind its signals.
        assert!(ordered_reply_position(Ok(reply(PORTAL)?), &portal).is_some());
        let unknown = "org.freedesktop.DBus.Error.UnknownProperty";
        assert!(ordered_reply_position(Err(error(PORTAL, unknown)?), &portal).is_some());
        // Errors from the bus daemon or replies from another connection are not.
        for name in [
            "org.freedesktop.DBus.Error.ServiceUnknown",
            "org.freedesktop.DBus.Error.NoReply",
        ] {
            let daemon = error("org.freedesktop.DBus", name)?;
            assert!(matches!(daemon, zbus::Error::MethodError(..)));
            assert!(ordered_reply_position(Err(daemon), &portal).is_none());
        }
        assert!(ordered_reply_position(Ok(reply(":1.8")?), &portal).is_none());
        assert!(ordered_reply_position(Err(error(":1.8", unknown)?), &portal).is_none());
        let lost = zbus::Error::InputOutput(Arc::new(io::Error::other("Verbindung getrennt")));
        assert!(ordered_reply_position(Err(lost), &portal).is_none());
        Ok(())
    }

    #[tokio::test]
    async fn start_barrier_settles_without_a_key_press() {
        let mut barrier = StartBarrier::<u64>::new();
        // Nothing to drive: settling must not return, or the hotkey wait would spin.
        assert!(barrier.settle().now_or_never().is_none());
        barrier.arm(Box::pin(future::ready(7)));
        barrier.settle().await;
        assert!(barrier.reply.is_none());
        assert!(barrier.is_old(6));
        assert!(!barrier.is_old(8));
        assert!(barrier.settle().now_or_never().is_none());
    }

    fn settings() -> ClickSettings {
        ClickSettings {
            interval_ms: 100,
            button: crate::model::MouseButton::Left,
            click_type: crate::model::ClickType::Single,
            repeat: Some(3),
            position: None,
            monitor: None,
        }
    }

    #[test]
    fn gui_reply_keeps_activation_stop_generation_and_bypasses_full_queue() {
        let (worker, side) = WorkerHandle::stalled(1);
        let (request, mut reply) = StartRequest::channel();
        let stops = side.control.borrow().stops;
        assert_eq!(worker.send(Command::Start(settings())), Ok(()));
        assert_eq!(worker.send(Command::Stop), Ok(()));
        // Qt answers after Stop, even while the command queue is full.
        request.start(settings());
        assert!(reply.try_recv().is_ok());
        assert!(is_outdated(stops, &side.control));
    }

    #[test]
    fn declining_or_revoking_gui_reply_is_terminal_only_for_that_request() {
        let (declined, mut reply) = StartRequest::channel();
        drop(declined);
        assert_eq!(reply.try_recv(), Err(oneshot::error::TryRecvError::Closed));
        let (cancelled, reply) = StartRequest::channel();
        drop(reply);
        assert!(cancelled.is_cancelled());
        let (fresh, mut reply) = StartRequest::channel();
        cancelled.start(settings());
        assert_eq!(reply.try_recv(), Err(oneshot::error::TryRecvError::Empty));
        fresh.start(settings());
        assert!(reply.try_recv().is_ok());
    }

    #[test]
    fn duplicate_start_keeps_pending_settings_and_active_run_stoppable() {
        for clicking in [false, true] {
            let mut machine = StateMachine::default();
            assert_eq!(
                request_validated_start(&mut machine, &settings(), false),
                Ok(true)
            );
            if clicking {
                assert!(machine.started());
            }
            let expected = machine.state();
            for interval_ms in [0, 250] {
                let duplicate = ClickSettings {
                    interval_ms,
                    ..settings()
                };
                assert_eq!(
                    request_validated_start(&mut machine, &duplicate, false),
                    Ok(false)
                );
                assert_eq!(machine.state(), expected);
            }
            assert!(machine.stop());
            assert!(!machine.started());
        }
    }

    #[test]
    fn invalid_initial_start_can_be_corrected() {
        let mut machine = StateMachine::default();
        let invalid = ClickSettings {
            interval_ms: 0,
            ..settings()
        };
        assert!(request_validated_start(&mut machine, &invalid, false).is_err());
        assert_eq!(machine.state(), RunState::Error);
        assert_eq!(
            request_validated_start(&mut machine, &settings(), false),
            Ok(true)
        );
        assert!(machine.started());
    }

    fn recording_emitter() -> (Emitter, std::sync::mpsc::Receiver<WorkerEvent>) {
        let (tx, rx) = std::sync::mpsc::channel();
        let emit: Emitter = Arc::new(move |event| {
            let _ = tx.send(event);
        });
        (emit, rx)
    }

    const WORKER_GONE: Result<(), SendError> = Err(SendError::Stopped);
    const QUEUE_FULL: Result<(), SendError> = Err(SendError::Busy);

    #[test]
    fn failed_thread_start_is_reported_and_commands_name_stopped_worker() {
        let (emit, events) = recording_emitter();
        // An impossible stack size makes spawning fail without exhausting resources.
        let builder = thread::Builder::new().stack_size(usize::MAX);
        let worker = WorkerHandle::spawn_with(builder, emit, CLICK_SESSION_IDLE);
        assert!(worker.join.is_none());
        let reported: Vec<_> = events.try_iter().collect();
        assert!(matches!(
            reported.as_slice(),
            [WorkerEvent::Error(WorkerError::WorkerStartFailed(_))]
        ));
        assert_eq!(worker.send(Command::Stop), WORKER_GONE);
        assert_eq!(worker.send(Command::Start(settings())), WORKER_GONE);
    }

    #[test]
    fn failed_runtime_closes_command_channel_before_reporting() {
        let (worker, side) = WorkerHandle::stalled(COMMAND_CAPACITY);
        let queue = worker.tx.clone();
        let stop = worker.control.clone();
        let (events_tx, events) = std::sync::mpsc::channel();
        let emit: Emitter = Arc::new(move |event| {
            let _ = events_tx.send((event, queue.is_closed() && stop.is_closed()));
        });
        block_on_runtime(Err(io::Error::other("Testfehler")), &emit, async move {
            let _worker_side = side;
            panic!("the worker must not run without a runtime");
        });
        let reported: Vec<_> = events.try_iter().collect();
        assert!(matches!(
            reported.as_slice(),
            [(WorkerEvent::Error(WorkerError::WorkerStartFailed(details)), true)] if details == "Testfehler"
        ));
        assert_eq!(worker.send(Command::Stop), WORKER_GONE);
        assert_eq!(worker.send(Command::Start(settings())), WORKER_GONE);
    }

    #[test]
    fn full_command_channel_is_distinguished_from_stopped_worker() {
        let (worker, side) = WorkerHandle::stalled(1);
        assert_eq!(worker.send(Command::Start(settings())), Ok(()));
        assert_eq!(worker.send(Command::Start(settings())), QUEUE_FULL);
        // Stop does not use the queue, so it still reaches a busy worker.
        assert_eq!(worker.send(Command::Stop), Ok(()));
        assert!(!worker.is_stopped());
        drop(side);
        assert_eq!(worker.send(Command::Start(settings())), WORKER_GONE);
        assert_eq!(worker.send(Command::Stop), WORKER_GONE);
        assert!(worker.is_stopped());
    }

    /// The next queued start and whether it came from the button.
    fn next_start(side: &mut WorkerSide) -> (u64, bool) {
        match side.commands.try_recv() {
            Ok(QueuedCommand::Start {
                stops, from_button, ..
            }) => (stops, from_button),
            _ => panic!("a start must be queued"),
        }
    }

    #[test]
    fn stop_bypasses_full_queue_and_outdates_queued_starts() {
        let (worker, mut side) = WorkerHandle::stalled(2);
        assert_eq!(worker.send(Command::Start(settings())), Ok(()));
        assert_eq!(worker.send(Command::StartFromButton(settings())), Ok(()));
        assert_eq!(worker.send(Command::ConfigureHotkey), QUEUE_FULL);
        assert_eq!(worker.send(Command::Stop), Ok(()));
        assert_eq!(take_control_change(&mut side.control), ControlChange::Stop);
        // Both kinds of start from before Stop are dropped by the worker.
        for from_button in [false, true] {
            let (stops, queued_from_button) = next_start(&mut side);
            assert_eq!(queued_from_button, from_button);
            assert!(is_outdated(stops, &side.control));
        }
        // Starts sent after Stop still run.
        assert_eq!(worker.send(Command::Start(settings())), Ok(()));
        assert_eq!(worker.send(Command::StartFromButton(settings())), Ok(()));
        for from_button in [false, true] {
            let (stops, queued_from_button) = next_start(&mut side);
            assert_eq!(queued_from_button, from_button);
            assert!(!is_outdated(stops, &side.control));
        }
    }

    #[test]
    fn pending_stop_is_taken_once_before_a_later_start() {
        let (worker, mut side) = WorkerHandle::stalled(1);
        assert_eq!(take_control_change(&mut side.control), ControlChange::None);
        // The worker reads this start before it has seen the Stop sent first.
        assert_eq!(worker.send(Command::Stop), Ok(()));
        assert_eq!(worker.send(Command::StartFromButton(settings())), Ok(()));
        let (stops, _) = next_start(&mut side);
        assert_eq!(take_control_change(&mut side.control), ControlChange::Stop);
        assert_eq!(take_control_change(&mut side.control), ControlChange::None);
        assert!(!is_outdated(stops, &side.control));

        assert_eq!(worker.send(Command::Shutdown), Ok(()));
        assert_eq!(
            take_control_change(&mut side.control),
            ControlChange::Shutdown
        );
        let (worker, mut side) = WorkerHandle::stalled(1);
        drop(worker);
        assert_eq!(
            take_control_change(&mut side.control),
            ControlChange::Shutdown
        );
    }

    #[test]
    fn shutdown_bypasses_full_queue_and_waits_bounded() {
        let (worker, side) = WorkerHandle::stalled(1);
        assert_eq!(worker.send(Command::Start(settings())), Ok(()));
        // The test keeps the worker side alive, so the worker never ends.
        // A blocking or unbounded shutdown fails here instead of hanging.
        let (done_tx, done) = std_mpsc::channel();
        thread::spawn(move || {
            let _ = done_tx.send(worker.shutdown_within(Duration::from_millis(50)));
        });
        assert_eq!(done.recv_timeout(Duration::from_secs(2)), Ok(false));
        assert!(side.control.borrow().shutdown);
    }

    /// Poll the worker's wait once with ticks due at the given deadlines.
    fn wake_now(
        side: &mut WorkerSide,
        countdown: Option<Instant>,
        tick: Option<Instant>,
    ) -> Option<Wake> {
        use futures_util::FutureExt;
        next_wake(WakeSources {
            control: &mut side.control,
            click_session: &mut None,
            commands: &mut side.commands,
            start_request: &mut None,
            start_task: &mut None,
            hotkey_task: &mut None,
            configure_task: &mut None,
            hotkey: &mut None,
            countdown,
            tick,
            session_idle: None,
        })
        .now_or_never()
    }

    /// Issue #117: the timer delayed a due tick to its next millisecond, which
    /// was most of a cycle at 2 ms.
    #[tokio::test]
    async fn due_tick_wakes_without_waiting_for_the_timer() {
        let (_worker, mut side) = WorkerHandle::stalled(1);
        let due = Instant::now();
        assert!(matches!(
            wake_now(&mut side, None, Some(due)),
            Some(Wake::ClickTick)
        ));
        assert!(wake_now(&mut side, None, Some(due + Duration::from_secs(1))).is_none());
    }

    #[tokio::test]
    async fn stop_wins_over_queued_commands_and_due_ticks() {
        let (worker, mut side) = WorkerHandle::stalled(1);
        let due = Instant::now();
        // Once the runtime's timer has passed the deadline, a timer for it
        // fires on its first poll, like a click that is already due.
        sleep_until(due + Duration::from_millis(5)).await;
        for (countdown, tick) in [(Some(due), None), (None, Some(due))] {
            assert!(matches!(
                wake_now(&mut side, countdown, tick),
                Some(Wake::CountdownTick | Wake::ClickTick)
            ));
            assert_eq!(worker.send(Command::Start(settings())), Ok(()));
            assert_eq!(worker.send(Command::Stop), Ok(()));
            assert!(matches!(
                wake_now(&mut side, countdown, tick),
                Some(Wake::Control(Ok(())))
            ));
            // The outdated start is next, still ahead of the due tick.
            assert!(matches!(
                wake_now(&mut side, countdown, tick),
                Some(Wake::Command(Some(QueuedCommand::Start { .. })))
            ));
        }
    }
}
