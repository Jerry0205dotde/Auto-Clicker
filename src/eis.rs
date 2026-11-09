//! Pointer input over libei for a RemoteDesktop session that answered
//! `ConnectToEIS`. Events go straight to the compositor instead of one D-Bus
//! call each, and one `ei_connection.sync` round trip confirms every cycle.
//! A session connected this way never sends `Notify*` calls.
use std::{
    collections::HashMap,
    io,
    os::{fd::OwnedFd, unix::net::UnixStream},
    time::Duration,
};

use futures_util::{FutureExt, StreamExt};
use reis::{Interface, PendingRequestResult, ei, tokio::EiEventStream};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum EisError {
    #[error("libei-Verbindung gestört: {0}")]
    Io(#[from] io::Error),
    #[error("libei-Protokollfehler: {0}")]
    Protocol(String),
    #[error("libei-Verbindung beendet")]
    Disconnected,
    #[error("kein passendes libei-Zeigegerät")]
    NoDevice,
    #[error("kein libei-Bereich für den Bildschirm")]
    NoRegion,
}

/// The monitor stream a fixed position refers to.
pub struct RegionTarget {
    pub mapping_id: Option<String>,
    pub position: (i32, i32),
    pub size: (i32, i32),
}

#[derive(Debug, Clone, PartialEq)]
struct Region {
    x: u32,
    y: u32,
    width: u32,
    height: u32,
    mapping_id: Option<String>,
}

#[derive(Default)]
struct DeviceState {
    interfaces: HashMap<String, reis::Object>,
    regions: Vec<Region>,
    /// `region_mapping_id` names the region that follows it.
    next_mapping_id: Option<String>,
    done: bool,
    resumed: bool,
}

struct Pointer {
    device: ei::Device,
    button: ei::Button,
    /// The absolute interface and the region offset of a fixed position.
    absolute: Option<(ei::PointerAbsolute, (u32, u32))>,
}

pub struct EisInput {
    context: ei::Context,
    events: EiEventStream,
    connection: ei::Connection,
    /// The last serial the compositor sent; every request echoes it.
    serial: u32,
    sequence: u32,
    target: Option<RegionTarget>,
    seats: HashMap<ei::Seat, HashMap<String, u64>>,
    bound: bool,
    devices: HashMap<ei::Device, DeviceState>,
    pointer: Option<Pointer>,
    pending_sync: Option<ei::Callback>,
    lost: bool,
}

impl EisInput {
    /// Handshake and wait for a resumed device with a button, plus an absolute
    /// pointer and a region of `target` for a fixed position.
    pub async fn connect(fd: OwnedFd, target: Option<RegionTarget>) -> Result<Self, EisError> {
        let context = ei::Context::new(UnixStream::from(fd))?;
        let mut events = EiEventStream::new(context.clone())?;
        let mut handshaker =
            reis::handshake::EiHandshaker::new("Klickmeister", ei::handshake::ContextType::Sender);
        let response = loop {
            let event = next_event(&mut events).await?;
            if let Some(response) = handshaker
                .handle_event(event)
                .map_err(|error| EisError::Protocol(error.to_string()))?
            {
                break response;
            }
        };
        let pointer = if target.is_some() {
            ei::PointerAbsolute::NAME
        } else {
            ei::Pointer::NAME
        };
        if [ei::Button::NAME, pointer]
            .iter()
            .any(|name| !response.negotiated_interfaces.contains_key(*name))
        {
            return Err(EisError::NoDevice);
        }
        let mut input = Self {
            context,
            events,
            connection: response.connection,
            serial: response.serial,
            sequence: 0,
            target,
            seats: HashMap::new(),
            bound: false,
            devices: HashMap::new(),
            pointer: None,
            pending_sync: None,
            lost: false,
        };
        input.flush().await?;
        while !input.ready() {
            input.receive().await?;
        }
        Ok(input)
    }

    /// Emit a bounded click cycle and wait until the compositor processed it.
    pub async fn click(
        &mut self,
        button: u32,
        clicks: u8,
        position: Option<(u32, u32)>,
    ) -> Result<(), EisError> {
        self.dispatch_received()?;
        // A paused device must not emulate; wait for the compositor to resume it.
        while !self.ready() {
            self.receive().await?;
        }
        let pointer = self.pointer.as_ref().ok_or(EisError::Disconnected)?;
        let serial = self.serial;
        self.sequence = self.sequence.wrapping_add(1);
        pointer.device.start_emulating(serial, self.sequence);
        if let Some((x, y)) = position {
            let (absolute, (left, top)) = pointer.absolute.as_ref().ok_or(EisError::NoRegion)?;
            absolute.motion_absolute(left.saturating_add(x) as f32, top.saturating_add(y) as f32);
            pointer.device.frame(serial, now_micros());
        }
        for _ in 0..clicks {
            for state in [
                ei::button::ButtonState::Press,
                ei::button::ButtonState::Released,
            ] {
                pointer.button.button(button, state);
                pointer.device.frame(serial, now_micros());
            }
        }
        pointer.device.stop_emulating(serial);
        self.pending_sync = Some(self.connection.sync(1));
        self.flush().await?;
        while self.pending_sync.is_some() {
            self.receive().await?;
        }
        Ok(())
    }

    /// Resolve when the compositor ends the connection or removes the device.
    /// Keeps answering pings meanwhile, also between runs.
    pub async fn wait_lost(&mut self) {
        while !self.lost {
            if self.receive().await.is_err() {
                self.lost = true;
            }
        }
    }

    /// Whether the connection already ended, without waiting.
    pub fn is_lost(&mut self) -> bool {
        if self.dispatch_received().is_err() {
            self.lost = true;
        }
        // Best effort: replies to pings must not wait for the next click.
        let _ = self.context.flush();
        self.lost
    }

    pub async fn close(self) {
        self.connection.disconnect();
        let _ = self.flush().await;
    }

    fn ready(&self) -> bool {
        self.pointer
            .as_ref()
            .and_then(|pointer| self.devices.get(&pointer.device))
            .is_some_and(|device| device.resumed)
    }

    /// Wait for and handle one event, then send any replies.
    async fn receive(&mut self) -> Result<(), EisError> {
        if self.lost {
            return Err(EisError::Disconnected);
        }
        let event = next_event(&mut self.events).await?;
        self.handle(event)?;
        self.flush().await?;
        if self.lost {
            return Err(EisError::Disconnected);
        }
        Ok(())
    }

    /// Handle events that have already arrived, without waiting.
    fn dispatch_received(&mut self) -> Result<(), EisError> {
        while !self.lost {
            match self.events.next().now_or_never() {
                None => break,
                Some(None) => self.lost = true,
                Some(Some(result)) => {
                    if let Some(event) = request(result)? {
                        self.handle(event)?;
                    }
                }
            }
        }
        if self.lost {
            return Err(EisError::Disconnected);
        }
        Ok(())
    }

    async fn flush(&self) -> Result<(), EisError> {
        loop {
            match self.context.flush() {
                Ok(()) => return Ok(()),
                // Unsent bytes stay buffered until KWin reads again. Callers
                // bound the wait with their own timeout.
                Err(error) if error == rustix::io::Errno::AGAIN => {
                    tokio::time::sleep(Duration::from_millis(1)).await;
                }
                Err(error) => return Err(io::Error::from(error).into()),
            }
        }
    }

    fn handle(&mut self, event: ei::Event) -> Result<(), EisError> {
        match event {
            ei::Event::Connection(_, event) => match event {
                ei::connection::Event::Disconnected { .. } => self.lost = true,
                ei::connection::Event::Seat { seat } => {
                    self.seats.insert(seat, HashMap::new());
                }
                ei::connection::Event::Ping { ping } => ping.done(0),
                _ => {}
            },
            ei::Event::Callback(callback, ei::callback::Event::Done { .. }) => {
                // `done` destroys the callback, but reis keeps it registered;
                // one per cycle would pile up for the session's lifetime.
                let object = callback.as_object();
                if let Some(backend) = object.backend() {
                    backend.remove_id(object.id());
                }
                if self.pending_sync.as_ref() == Some(&callback) {
                    self.pending_sync = None;
                }
            }
            ei::Event::Seat(seat, event) => match event {
                ei::seat::Event::Capability { mask, interface } => {
                    if let Some(capabilities) = self.seats.get_mut(&seat) {
                        capabilities.insert(interface, mask);
                    }
                }
                ei::seat::Event::Done => self.bind(&seat),
                ei::seat::Event::Device { device } => {
                    self.devices.insert(device, DeviceState::default());
                }
                ei::seat::Event::Destroyed { serial } => {
                    self.serial = serial;
                    self.seats.remove(&seat);
                }
                _ => {}
            },
            ei::Event::Device(device, event) => self.handle_device(device, event)?,
            _ => {}
        }
        Ok(())
    }

    /// Bind the first seat that offers a button and the pointer this mode
    /// needs. KWin creates no device for a button alone.
    fn bind(&mut self, seat: &ei::Seat) {
        if self.bound {
            return;
        }
        let pointer = if self.target.is_some() {
            ei::PointerAbsolute::NAME
        } else {
            ei::Pointer::NAME
        };
        let Some(capabilities) = self.seats.get(seat) else {
            return;
        };
        if let (Some(button), Some(pointer)) = (
            capabilities.get(ei::Button::NAME),
            capabilities.get(pointer),
        ) {
            seat.bind(button | pointer);
            self.bound = true;
        }
    }

    fn handle_device(
        &mut self,
        device: ei::Device,
        event: ei::device::Event,
    ) -> Result<(), EisError> {
        let Some(state) = self.devices.get_mut(&device) else {
            return Ok(());
        };
        match event {
            ei::device::Event::RegionMappingId { mapping_id } => {
                state.next_mapping_id = Some(mapping_id);
            }
            ei::device::Event::Region {
                offset_x,
                offset_y,
                width,
                hight,
                ..
            } => {
                let mapping_id = state.next_mapping_id.take();
                state.regions.push(Region {
                    x: offset_x,
                    y: offset_y,
                    width,
                    height: hight,
                    mapping_id,
                });
            }
            ei::device::Event::Interface { object } => {
                state
                    .interfaces
                    .insert(object.interface().to_owned(), object);
            }
            ei::device::Event::Done => {
                state.done = true;
                // Version 3 devices wait for this before the compositor resumes them.
                if device.version() >= 3 {
                    device.ready();
                }
            }
            ei::device::Event::Resumed { serial } => {
                self.serial = serial;
                state.resumed = true;
            }
            ei::device::Event::Paused { serial } => {
                self.serial = serial;
                state.resumed = false;
            }
            ei::device::Event::Destroyed { serial } => {
                self.serial = serial;
                self.devices.remove(&device);
                if self
                    .pointer
                    .as_ref()
                    .is_some_and(|pointer| pointer.device == device)
                {
                    self.pointer = None;
                    self.lost = true;
                }
                return Ok(());
            }
            _ => {}
        }
        if self.pointer.is_none() && !self.lost && state.done {
            self.pointer = select_pointer(&device, state, self.target.as_ref())?;
        }
        Ok(())
    }
}

/// Use a device only with every interface the mode needs. A fixed position
/// also needs exactly one region for its monitor.
fn select_pointer(
    device: &ei::Device,
    state: &DeviceState,
    target: Option<&RegionTarget>,
) -> Result<Option<Pointer>, EisError> {
    let interface = |name: &str| state.interfaces.get(name).cloned();
    let Some(button) = interface(ei::Button::NAME).and_then(|object| object.downcast()) else {
        return Ok(None);
    };
    let absolute = match target {
        None => None,
        Some(target) => {
            let Some(absolute) =
                interface(ei::PointerAbsolute::NAME).and_then(|object| object.downcast())
            else {
                return Ok(None);
            };
            let offset = select_region(&state.regions, target).ok_or(EisError::NoRegion)?;
            Some((absolute, offset))
        }
    };
    Ok(Some(Pointer {
        device: device.clone(),
        button,
        absolute,
    }))
}

/// Match a region by the stream's mapping ID where both carry one, otherwise
/// by its origin; the size must match too. Never guess by order.
fn select_region(regions: &[Region], target: &RegionTarget) -> Option<(u32, u32)> {
    let by_id = target.mapping_id.is_some() && regions.iter().any(|r| r.mapping_id.is_some());
    let mut matching = regions.iter().filter(|region| {
        let place = if by_id {
            region.mapping_id == target.mapping_id
        } else {
            (i64::from(region.x), i64::from(region.y))
                == (i64::from(target.position.0), i64::from(target.position.1))
        };
        place
            && (i64::from(region.width), i64::from(region.height))
                == (i64::from(target.size.0), i64::from(target.size.1))
    });
    let region = matching.next()?;
    matching.next().is_none().then_some((region.x, region.y))
}

/// Wait for the next event; cancelling loses none, since a returned event
/// is handled before the caller awaits again.
async fn next_event(events: &mut EiEventStream) -> Result<ei::Event, EisError> {
    loop {
        let result = events.next().await.ok_or(EisError::Disconnected)?;
        if let Some(event) = request(result)? {
            return Ok(event);
        }
    }
}

fn request(
    result: io::Result<PendingRequestResult<ei::Event>>,
) -> Result<Option<ei::Event>, EisError> {
    match result? {
        PendingRequestResult::Request(event) => Ok(Some(event)),
        PendingRequestResult::ParseError(error) => Err(EisError::Protocol(error.to_string())),
        // Events for objects this client already released.
        PendingRequestResult::InvalidObject(_) => Ok(None),
    }
}

/// libei timestamps are `CLOCK_MONOTONIC` microseconds.
fn now_micros() -> u64 {
    let now = rustix::time::clock_gettime(rustix::time::ClockId::Monotonic);
    u64::try_from(now.tv_sec).unwrap_or(0) * 1_000_000
        + u64::try_from(now.tv_nsec).unwrap_or(0) / 1_000
}

#[cfg(test)]
mod tests {
    use super::*;

    fn region(x: u32, mapping_id: Option<&str>) -> Region {
        Region {
            x,
            y: 0,
            width: 1920,
            height: 1080,
            mapping_id: mapping_id.map(ToOwned::to_owned),
        }
    }

    fn target(x: i32, mapping_id: Option<&str>) -> RegionTarget {
        RegionTarget {
            mapping_id: mapping_id.map(ToOwned::to_owned),
            position: (x, 0),
            size: (1920, 1080),
        }
    }

    #[test]
    fn matches_regions_by_mapping_id_independently_of_order() {
        // IDs win over geometry, so a region may even sit elsewhere than its stream.
        let regions = [region(1920, Some("HDMI-A-1")), region(0, Some("DP-1"))];
        assert_eq!(
            select_region(&regions, &target(-1920, Some("DP-1"))),
            Some((0, 0))
        );
        assert_eq!(
            select_region(&regions, &target(0, Some("HDMI-A-1"))),
            Some((1920, 0))
        );
        assert_eq!(select_region(&regions, &target(0, Some("eDP-1"))), None);
    }

    #[test]
    fn falls_back_to_the_origin_without_mapping_ids() {
        let regions = [region(1920, None), region(0, None)];
        assert_eq!(
            select_region(&regions, &target(1920, None)),
            Some((1920, 0))
        );
        assert_eq!(
            select_region(&regions, &target(1920, Some("DP-1"))),
            Some((1920, 0))
        );
        assert_eq!(select_region(&regions, &target(-1920, None)), None);
    }

    #[test]
    fn rejects_ambiguous_or_differently_sized_regions() {
        let mirrored = [region(0, Some("DP-1")), region(0, Some("DP-1"))];
        assert_eq!(select_region(&mirrored, &target(0, Some("DP-1"))), None);
        let smaller = [Region {
            width: 1280,
            ..region(0, Some("DP-1"))
        }];
        assert_eq!(select_region(&smaller, &target(0, Some("DP-1"))), None);
        assert_eq!(select_region(&[], &target(0, None)), None);
    }
}
