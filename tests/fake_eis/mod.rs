//! A small EIS server that plays KWin's part behind the fake `ConnectToEIS`.
//! It offers one seat with a button and both pointer kinds, creates one device
//! per bind like KWin, and gives absolute pointers the configured regions.
use std::{
    io,
    os::{fd::OwnedFd, unix::net::UnixStream},
    sync::{
        Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    thread,
    time::Duration,
};

use reis::{
    PendingRequestResult, eis,
    handshake::EisHandshaker,
    request::{Device, DeviceCapability, EisRequest, EisRequestConverter},
};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Input {
    Button(u32, bool),
    Motion(f32, f32),
}

pub struct Region {
    pub x: u32,
    pub width: u32,
    pub mapping_id: &'static str,
}

#[derive(Default)]
pub struct FakeEis {
    regions: Vec<Region>,
    /// Close each socket instead of answering the handshake.
    pub refuse: AtomicBool,
    /// Hold every sync reply, so the client stays inside its click.
    pub hold_sync: AtomicBool,
    /// Connections up to this number end with `disconnected`.
    disconnect_up_to: AtomicUsize,
    opened: AtomicUsize,
    closed: AtomicUsize,
    inputs: Mutex<Vec<Input>>,
}

impl FakeEis {
    pub fn new(regions: Vec<Region>) -> Self {
        Self {
            regions,
            ..Self::default()
        }
    }

    /// Accept a client like KWin does for `ConnectToEIS`.
    pub fn connect(self: &std::sync::Arc<Self>) -> io::Result<OwnedFd> {
        let (client, server) = UnixStream::pair()?;
        let id = self.opened.fetch_add(1, Ordering::SeqCst) + 1;
        let fake = self.clone();
        thread::spawn(move || {
            let _ = fake.serve(server, id);
            fake.closed.fetch_add(1, Ordering::SeqCst);
        });
        Ok(client.into())
    }

    /// End every open connection, as KWin does when the session ends.
    pub fn disconnect(&self) {
        self.disconnect_up_to
            .store(self.opened.load(Ordering::SeqCst), Ordering::SeqCst);
    }

    /// Connections opened so far and those of them that ended.
    pub fn connections(&self) -> (usize, usize) {
        (
            self.opened.load(Ordering::SeqCst),
            self.closed.load(Ordering::SeqCst),
        )
    }

    pub fn take_inputs(&self) -> Vec<Input> {
        std::mem::take(&mut *self.inputs.lock().unwrap_or_else(|e| e.into_inner()))
    }

    pub fn input_count(&self) -> usize {
        self.inputs.lock().unwrap_or_else(|e| e.into_inner()).len()
    }

    fn revoked(&self, id: usize) -> bool {
        id <= self.disconnect_up_to.load(Ordering::SeqCst)
    }

    fn serve(&self, socket: UnixStream, id: usize) -> Result<(), Box<dyn std::error::Error>> {
        if self.refuse.load(Ordering::SeqCst) {
            return Ok(());
        }
        let context = eis::Context::new(socket)?;
        let mut handshaker = EisHandshaker::new(&context, 1);
        let response = loop {
            let Some(request) = self.next(&context, id)? else {
                return Ok(());
            };
            if let Some(response) = handshaker.handle_request(request)? {
                break response;
            }
        };
        let mut converter = EisRequestConverter::new(&context, response, 1);
        let _seat = converter.handle().add_seat(
            Some("seat0"),
            DeviceCapability::Pointer
                | DeviceCapability::PointerAbsolute
                | DeviceCapability::Button,
        );
        context.flush()?;
        // Removing a dropped device would tell the client it is gone.
        let mut devices: Vec<Device> = Vec::new();
        loop {
            let Some(request) = self.next(&context, id)? else {
                if self.revoked(id) {
                    converter.handle().disconnected(
                        eis::connection::DisconnectReason::Disconnected,
                        Some("session closed"),
                    );
                    let _ = context.flush();
                }
                return Ok(());
            };
            if let eis::Request::Connection(_, eis::connection::Request::Sync { callback }) =
                &request
            {
                while self.hold_sync.load(Ordering::SeqCst) && !self.revoked(id) {
                    thread::sleep(Duration::from_millis(1));
                }
                callback.done(0);
                context.flush()?;
                continue;
            }
            converter.handle_request(request)?;
            while let Some(request) = converter.next_request() {
                match request {
                    EisRequest::Disconnect => return Ok(()),
                    EisRequest::Bind(bind) => {
                        let absolute = bind
                            .capabilities
                            .contains(DeviceCapability::PointerAbsolute);
                        let pointer = if absolute {
                            DeviceCapability::PointerAbsolute
                        } else {
                            DeviceCapability::Pointer
                        };
                        let regions = if absolute { &self.regions[..] } else { &[] };
                        let device = bind.seat.add_device(
                            Some("fake pointer"),
                            eis::device::DeviceType::Virtual,
                            pointer | DeviceCapability::Button,
                            |device| {
                                for region in regions {
                                    device.device().region_mapping_id(region.mapping_id);
                                    device.device().region(region.x, 0, region.width, 1080, 1.0);
                                }
                            },
                        );
                        device.resumed();
                        devices.push(device);
                    }
                    EisRequest::Button(button) => self.record(Input::Button(
                        button.button,
                        button.state == eis::button::ButtonState::Press,
                    )),
                    EisRequest::PointerMotionAbsolute(motion) => {
                        self.record(Input::Motion(motion.dx_absolute, motion.dy_absolute));
                    }
                    _ => {}
                }
            }
            context.flush()?;
        }
    }

    fn record(&self, input: Input) {
        self.inputs
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(input);
    }

    /// The next request, or `None` once the client left or the test revoked
    /// this connection.
    fn next(
        &self,
        context: &eis::Context,
        id: usize,
    ) -> Result<Option<eis::Request>, Box<dyn std::error::Error>> {
        loop {
            if let Some(result) = context.pending_request() {
                match result {
                    PendingRequestResult::Request(request) => return Ok(Some(request)),
                    PendingRequestResult::ParseError(error) => {
                        return Err(error.to_string().into());
                    }
                    PendingRequestResult::InvalidObject(_) => continue,
                }
            }
            if self.revoked(id) {
                return Ok(None);
            }
            match context.read() {
                Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
                Err(error) => return Err(error.into()),
                Ok(0) => thread::sleep(Duration::from_millis(1)),
                Ok(_) => {}
            }
        }
    }
}
