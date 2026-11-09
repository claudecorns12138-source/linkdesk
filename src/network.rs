use crate::{
    discovery, identity,
    protocol::{self, Hello, VideoFrame, Welcome},
    source::{self, Mode, SourceControl},
};
use anyhow::{bail, ensure, Context, Result};
use openh264::{decoder::Decoder, formats::YUVSource};
use rustls::{pki_types::ServerName, ClientConnection, ServerConnection, StreamOwned};
use std::{
    collections::HashMap,
    net::{IpAddr, SocketAddr, TcpListener, TcpStream, ToSocketAddrs},
    sync::{
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
        mpsc::{self, SyncSender, TrySendError},
        Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};

pub enum Event {
    PairRequest {
        id: u64,
        name: String,
        decision: SyncSender<bool>,
    },
    PeerConnected {
        id: u64,
        name: String,
    },
    PeerDisconnected {
        id: u64,
    },
    Certificate {
        fingerprint: String,
        decision: SyncSender<bool>,
    },
    Status(String),
    SourceError(String),
}

pub struct DecodedFrame {
    pub rgb: Vec<u8>,
    pub width: usize,
    pub height: usize,
    pub sequence: u64,
}
pub type LatestFrame = Arc<Mutex<Option<DecodedFrame>>>;

struct Subscriber {
    id: u64,
    tx: SyncSender<Arc<VideoFrame>>,
}
#[derive(Default)]
struct Hub {
    subscribers: Vec<Subscriber>,
    gop: Vec<Arc<VideoFrame>>,
    gop_bytes: usize,
}
impl Hub {
    fn publish(&mut self, frame: VideoFrame) {
        let frame = Arc::new(frame);
        if frame.keyframe {
            self.gop.clear();
            self.gop_bytes = 0;
        }
        if frame.keyframe || !self.gop.is_empty() {
            self.gop_bytes += frame.data.len();
            self.gop.push(frame.clone());
            if self.gop.len() > 60 || self.gop_bytes > protocol::MAX_RECORD {
                self.gop.clear();
                self.gop_bytes = 0;
            }
        }
        // Never discard an inter-frame and continue with broken references.
        // A slow receiver is disconnected and can reconnect at lower resolution.
        self.subscribers
            .retain(|s| match s.tx.try_send(frame.clone()) {
                Ok(()) => true,
                Err(TrySendError::Full(_) | TrySendError::Disconnected(_)) => false,
            });
    }
    fn subscribe(&mut self, id: u64) -> mpsc::Receiver<Arc<VideoFrame>> {
        let (tx, rx) = mpsc::sync_channel(64);
        for frame in &self.gop {
            let _ = tx.try_send(frame.clone());
        }
        self.subscribers.push(Subscriber { id, tx });
        rx
    }
}

pub struct Host {
    pub address: SocketAddr,
    pub fingerprint: String,
    pub frames: Arc<AtomicU64>,
    pin: Arc<Mutex<(String, Instant)>>,
    stop: Arc<AtomicBool>,
    control: Arc<SourceControl>,
}
impl Host {
    pub fn pin(&self) -> String {
        self.pin.lock().unwrap().0.clone()
    }
    pub fn remaining(&self) -> u64 {
        self.pin
            .lock()
            .unwrap()
            .1
            .saturating_duration_since(Instant::now())
            .as_secs()
    }
    pub fn rotate(&self) {
        *self.pin.lock().unwrap() = new_pin();
    }
    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
        self.control.stop();
    }
}
impl Drop for Host {
    fn drop(&mut self) {
        self.stop();
    }
}
fn new_pin() -> (String, Instant) {
    (
        format!("{:06}", rand::random_range(0..1_000_000)),
        Instant::now() + Duration::from_secs(300),
    )
}

pub fn start_host(bind: &str, mode: Mode, events: mpsc::Sender<Event>) -> Result<Host> {
    let listener = TcpListener::bind(bind)
        .context("cannot listen; is another LinkDesk host already running?")?;
    listener.set_nonblocking(true)?;
    let address = listener.local_addr()?;
    let identity = identity::new_identity()?;
    let pin = Arc::new(Mutex::new(new_pin()));
    let stop = Arc::new(AtomicBool::new(false));
    let hub = Arc::new(Mutex::new(Hub::default()));
    let control = Arc::new(SourceControl::default());
    let frames = Arc::new(AtomicU64::new(0));
    let handle = Host {
        address,
        fingerprint: identity.fingerprint.clone(),
        pin: pin.clone(),
        stop: stop.clone(),
        control: control.clone(),
        frames: frames.clone(),
    };
    if !address.ip().is_loopback() {
        let _ = discovery::announce(address.port(), stop.clone());
    }
    {
        let stop = stop.clone();
        let hub = hub.clone();
        let events = events.clone();
        let control = control.clone();
        thread::spawn(move || {
            let result = source::run(mode, stop.clone(), control.clone(), |f| {
                frames.fetch_add(1, Ordering::Relaxed);
                hub.lock().unwrap().publish(f);
                Ok(())
            });
            control.stop();
            if !stop.load(Ordering::Relaxed) {
                if let Err(e) = result {
                    let _ = events.send(Event::SourceError(format!("{e:#}")));
                    stop.store(true, Ordering::Relaxed);
                }
            }
        });
    }
    thread::spawn(move || {
        let active = Arc::new(AtomicUsize::new(0));
        let next = AtomicU64::new(1);
        let mut attempts: HashMap<IpAddr, (Instant, u32)> = HashMap::new();
        while !stop.load(Ordering::Relaxed) {
            match listener.accept() {
                Ok((socket, remote)) => {
                    attempts.retain(|_, v| v.0.elapsed() < Duration::from_secs(60));
                    if attempts.len() >= 256 && !attempts.contains_key(&remote.ip()) {
                        continue;
                    }
                    let a = attempts.entry(remote.ip()).or_insert((Instant::now(), 0));
                    a.1 += 1;
                    if a.1 > 12 || active.load(Ordering::Relaxed) >= 12 {
                        continue;
                    }
                    active.fetch_add(1, Ordering::Relaxed);
                    let (config, pin, hub, stop, events, active) = (
                        identity.config.clone(),
                        pin.clone(),
                        hub.clone(),
                        stop.clone(),
                        events.clone(),
                        active.clone(),
                    );
                    let id = next.fetch_add(1, Ordering::Relaxed);
                    thread::spawn(move || {
                        let _ = serve_receiver(
                            socket,
                            config,
                            pin,
                            hub.clone(),
                            stop,
                            events.clone(),
                            id,
                        );
                        hub.lock().unwrap().subscribers.retain(|s| s.id != id);
                        let _ = events.send(Event::PeerDisconnected { id });
                        active.fetch_sub(1, Ordering::Relaxed);
                    });
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(50))
                }
                Err(_) => break,
            }
        }
    });
    Ok(handle)
}

fn serve_receiver(
    socket: TcpStream,
    config: Arc<rustls::ServerConfig>,
    pin: Arc<Mutex<(String, Instant)>>,
    hub: Arc<Mutex<Hub>>,
    stop: Arc<AtomicBool>,
    events: mpsc::Sender<Event>,
    id: u64,
) -> Result<()> {
    // Accepted sockets inherit nonblocking mode on Windows.
    socket.set_nonblocking(false)?;
    socket.set_nodelay(true)?;
    socket.set_read_timeout(Some(Duration::from_secs(8)))?;
    socket.set_write_timeout(Some(Duration::from_secs(3)))?;
    let mut tls = StreamOwned::new(ServerConnection::new(config)?, socket);
    let hello: Hello = protocol::read_json(&mut tls)?;
    protocol::validate_hello(&hello)?;
    let valid = {
        let p = pin.lock().unwrap();
        p.1 > Instant::now() && constant_equal(p.0.as_bytes(), hello.pin.as_bytes())
    };
    if !valid {
        protocol::write_json(
            &mut tls,
            &Welcome {
                accepted: false,
                message: "Incorrect or expired pairing code".into(),
                codec: "h264".into(),
            },
        )?;
        return Ok(());
    }
    let (tx, rx) = mpsc::sync_channel(1);
    events.send(Event::PairRequest {
        id,
        name: hello.name.clone(),
        decision: tx,
    })?;
    let deadline = Instant::now() + Duration::from_secs(60);
    let allow = loop {
        if stop.load(Ordering::Relaxed) || Instant::now() > deadline {
            break false;
        }
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(v) => break v,
            Err(mpsc::RecvTimeoutError::Disconnected) => break false,
            Err(_) => {}
        }
    };
    let rx = if allow {
        let mut hub = hub.lock().unwrap();
        if hub.subscribers.len() < 4 {
            Some(hub.subscribe(id))
        } else {
            None
        }
    } else {
        None
    };
    let allow = rx.is_some();
    protocol::write_json(
        &mut tls,
        &Welcome {
            accepted: allow,
            message: if allow {
                "Connected"
            } else {
                "Connection declined, capacity reached, or timed out"
            }
            .into(),
            codec: "h264".into(),
        },
    )?;
    if !allow {
        return Ok(());
    }
    events.send(Event::PeerConnected {
        id,
        name: hello.name,
    })?;
    let rx = rx.context("receiver was not approved")?;
    let mut initialized = false;
    while !stop.load(Ordering::Relaxed) {
        match rx.recv_timeout(Duration::from_millis(250)) {
            Ok(frame) => {
                if !initialized && !frame.keyframe {
                    continue;
                }
                initialized = true;
                protocol::write_video(&mut tls, &frame)?;
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(_) => bail!("receiver fell behind or host stopped"),
        }
    }
    Ok(())
}

fn constant_equal(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (a, b)| acc | (a ^ b)) == 0
}

fn connect_tls(
    address: &str,
    pin: Option<String>,
) -> Result<StreamOwned<ClientConnection, TcpStream>> {
    let address = address
        .to_socket_addrs()?
        .next()
        .context("host address not found")?;
    let socket = TcpStream::connect_timeout(&address, Duration::from_secs(5))
        .context("cannot reach host; check IP, network isolation and firewall")?;
    socket.set_nodelay(true)?;
    socket.set_read_timeout(Some(Duration::from_secs(8)))?;
    socket.set_write_timeout(Some(Duration::from_secs(5)))?;
    let connection = ClientConnection::new(
        identity::client_config(pin),
        ServerName::try_from("linkdesk.local")?,
    )?;
    let mut tls = StreamOwned::new(connection, socket);
    while tls.conn.is_handshaking() {
        tls.conn.complete_io(&mut tls.sock)?;
    }
    Ok(tls)
}

pub fn probe_fingerprint(address: &str) -> Result<String> {
    let tls = connect_tls(address, None)?;
    let cert = tls
        .conn
        .peer_certificates()
        .and_then(|c| c.first())
        .context("host did not provide a certificate")?;
    Ok(identity::fingerprint(cert.as_ref()))
}

pub struct Receiver {
    stop: Arc<AtomicBool>,
    socket: Arc<Mutex<Option<TcpStream>>>,
}
impl Receiver {
    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(socket) = self.socket.lock().unwrap().take() {
            let _ = socket.shutdown(std::net::Shutdown::Both);
        }
    }
}
impl Drop for Receiver {
    fn drop(&mut self) {
        self.stop();
    }
}

pub fn start_receiver(
    address: String,
    pin: String,
    events: mpsc::Sender<Event>,
    latest: LatestFrame,
) -> Receiver {
    let stop = Arc::new(AtomicBool::new(false));
    let socket: Arc<Mutex<Option<TcpStream>>> = Arc::default();
    let handle = Receiver {
        stop: stop.clone(),
        socket: socket.clone(),
    };
    thread::spawn(move || {
        let result = (|| -> Result<()> {
            let fingerprint = probe_fingerprint(&address)?;
            let (tx, rx) = mpsc::sync_channel(1);
            events.send(Event::Certificate {
                fingerprint: fingerprint.clone(),
                decision: tx,
            })?;
            let deadline = Instant::now() + Duration::from_secs(90);
            loop {
                ensure!(
                    !stop.load(Ordering::Relaxed) && Instant::now() < deadline,
                    "Pairing cancelled"
                );
                match rx.recv_timeout(Duration::from_millis(100)) {
                    Ok(true) => break,
                    Ok(false) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                        bail!("Fingerprint was not confirmed")
                    }
                    Err(_) => {}
                }
            }
            // Reconnect with exact pinning before sending any application identity.
            let mut tls = connect_tls(&address, Some(fingerprint))?;
            *socket.lock().unwrap() = Some(tls.sock.try_clone()?);
            tls.sock.set_read_timeout(Some(Duration::from_secs(65)))?;
            protocol::write_json(
                &mut tls,
                &Hello {
                    protocol: protocol::VERSION,
                    name: "LinkDesk receiver".into(),
                    pin,
                },
            )?;
            events.send(Event::Status("Waiting for approval on the host…".into()))?;
            let welcome: Welcome = protocol::read_json(&mut tls)?;
            ensure!(welcome.accepted, "{}", welcome.message);
            ensure!(welcome.codec == "h264", "unsupported video codec");
            events.send(Event::Status("Connected · native H.264 / TLS".into()))?;
            // Static screens can legitimately produce no new capture frames.
            tls.sock.set_read_timeout(None)?;
            let keepalive = socket2::TcpKeepalive::new()
                .with_time(Duration::from_secs(10))
                .with_interval(Duration::from_secs(5));
            let _ = socket2::SockRef::from(&tls.sock).set_tcp_keepalive(&keepalive);
            receive_frames(&mut tls, stop.clone(), latest)
        })();
        if !stop.load(Ordering::Relaxed) {
            if let Err(e) = result {
                let _ = events.send(Event::Status(format!("{e:#}")));
            }
        }
    });
    handle
}

fn receive_frames(
    reader: &mut impl std::io::Read,
    stop: Arc<AtomicBool>,
    latest: LatestFrame,
) -> Result<()> {
    let mut decoder = Decoder::new()?;
    let mut last = None;
    while !stop.load(Ordering::Relaxed) {
        let frame = protocol::next_frame(reader)?;
        if stop.load(Ordering::Relaxed) {
            break;
        }
        if let Some(last) = last {
            ensure!(frame.sequence > last, "reordered video frame");
        }
        last = Some(frame.sequence);
        if let Some(decoded) = decoder.decode(&frame.data)? {
            let (w, h) = decoded.dimensions();
            protocol::validate_dimensions(w as u32, h as u32)?;
            ensure!(
                w == frame.width as usize && h == frame.height as usize,
                "video dimensions do not match stream header"
            );
            let mut rgb = vec![0; w * h * 3];
            decoded.write_rgb8(&mut rgb);
            *latest.lock().unwrap() = Some(DecodedFrame {
                rgb,
                width: w,
                height: h,
                sequence: frame.sequence,
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    #[test]
    fn encrypted_approved_native_video_and_wrong_pin() {
        let (tx, rx) = mpsc::channel();
        let host = start_host("127.0.0.1:0", Mode::Test, tx).unwrap();
        let address = host.address.to_string();
        assert_eq!(probe_fingerprint(&address).unwrap(), host.fingerprint);
        assert!(connect_tls(&address, Some("wrong fingerprint".into())).is_err());
        let mut wrong = connect_tls(&address, Some(host.fingerprint.clone())).unwrap();
        protocol::write_json(
            &mut wrong,
            &Hello {
                protocol: protocol::VERSION,
                name: "Test receiver".into(),
                pin: "not-a-code".into(),
            },
        )
        .unwrap();
        assert!(protocol::read_json::<Welcome>(&mut wrong).is_err());
        let wrong_pin = if host.pin() == "000000" {
            "000001"
        } else {
            "000000"
        };
        let mut wrong = connect_tls(&address, Some(host.fingerprint.clone())).unwrap();
        protocol::write_json(
            &mut wrong,
            &Hello {
                protocol: protocol::VERSION,
                name: "Test receiver".into(),
                pin: wrong_pin.into(),
            },
        )
        .unwrap();
        assert!(!protocol::read_json::<Welcome>(&mut wrong).unwrap().accepted);
        let mut tls = connect_tls(&address, Some(host.fingerprint.clone())).unwrap();
        protocol::write_json(
            &mut tls,
            &Hello {
                protocol: protocol::VERSION,
                name: "Test receiver".into(),
                pin: host.pin(),
            },
        )
        .unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if let Ok(Event::PairRequest { decision, .. }) =
                rx.recv_timeout(Duration::from_millis(100))
            {
                decision.send(true).unwrap();
                break;
            }
        }
        assert!(protocol::read_json::<Welcome>(&mut tls).unwrap().accepted);
        let frame = protocol::next_frame(&mut tls).unwrap();
        assert!(frame.keyframe);
        let mut decoder = Decoder::new().unwrap();
        let decoded = decoder.decode(&frame.data).unwrap().unwrap();
        assert_eq!(decoded.dimensions(), (640, 360));
        host.stop();
    }
    #[test]
    fn slow_receivers_do_not_keep_broken_interframe_streams() {
        let mut hub = Hub::default();
        let rx = hub.subscribe(1);
        for sequence in 0..66 {
            hub.publish(VideoFrame {
                sequence,
                width: 640,
                height: 360,
                keyframe: sequence == 0,
                data: vec![1],
            });
        }
        assert!(hub.subscribers.is_empty());
        assert_eq!(rx.try_iter().count(), 64);
    }
    #[test]
    fn latest_decoded_frame_is_replaced_instead_of_queued() {
        let mut encoder = openh264::encoder::Encoder::new().unwrap();
        let mut stream = Vec::new();
        for seq in 0..3 {
            protocol::write_video(&mut stream, &source::test_frame(seq, &mut encoder).unwrap())
                .unwrap();
        }
        let latest: LatestFrame = Arc::default();
        let _ = receive_frames(
            &mut Cursor::new(stream),
            Arc::new(AtomicBool::new(false)),
            latest.clone(),
        );
        assert_eq!(latest.lock().unwrap().as_ref().unwrap().sequence, 2);
    }
}
