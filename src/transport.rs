//! Cancellable, bounded receiver setup; no application identity is sent here.
use anyhow::{bail, ensure, Context, Result};
use mio::{Events, Interest, Poll, Token, Waker};
use rustls::{pki_types::ServerName, ClientConnection, StreamOwned};
use std::{
    io::{self, Read, Write},
    net::{SocketAddr, TcpStream, ToSocketAddrs},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc, Mutex, OnceLock,
    },
    time::{Duration, Instant},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureStage {
    Resolve,
    Connect,
    Handshake,
    Trust,
    Approval,
    Video,
}
impl FailureStage {
    pub fn title(self) -> &'static str {
        match self {
            Self::Resolve => "主机名称解析未完成",
            Self::Connect => "无法连接主机",
            Self::Handshake => "安全握手未完成",
            Self::Trust => "身份指纹尚未确认",
            Self::Approval => "配对未完成",
            Self::Video => "尚未收到有效画面",
        }
    }
    pub fn hint(self) -> &'static str {
        match self {
        Self::Resolve=>"主机名暂时无法解析，可从主机复制实际 IP 地址后重试。",
        Self::Connect=>"检查主机是否正在共享、地址是否正确。同一热点也可能隔离设备，可用可互通的局域网作对照。",
        Self::Handshake=>"已连接到端口，但安全握手未完成。请确认这是 LinkDesk 主机端口，并检查主机状态后重试。",
        Self::Trust=>"请在两端核对完整身份指纹；确认一致后再继续配对。",
        Self::Approval=>"查看主机的配对结果；配对码过期时刷新，并在主机允许本次连接。",
        Self::Video=>"主机已批准，但未收到有效首帧。请检查主机采集状态、录屏权限和网络后重试。",
    }
    }
}
impl std::fmt::Display for FailureStage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.title())
    }
}
impl std::error::Error for FailureStage {}
pub(crate) fn stage_of(error: &anyhow::Error, fallback: FailureStage) -> FailureStage {
    error
        .downcast_ref::<FailureStage>()
        .copied()
        .unwrap_or(fallback)
}

#[derive(Default)]
struct Active {
    socket: Option<TcpStream>,
    wake: Option<Arc<Waker>>,
}
#[derive(Default)]
pub(crate) struct Control {
    stopped: AtomicBool,
    active: Mutex<Active>,
}
impl Control {
    pub fn stopped(&self) -> bool {
        self.stopped.load(Ordering::Acquire)
    }
    fn check(&self) -> Result<()> {
        ensure!(!self.stopped(), "Connection cancelled");
        Ok(())
    }
    pub fn cancel(&self) {
        self.stopped.store(true, Ordering::Release);
        let mut active = self.active.lock().unwrap();
        if let Some(socket) = active.socket.take() {
            let _ = socket.shutdown(std::net::Shutdown::Both);
        }
        if let Some(wake) = active.wake.take() {
            let _ = wake.wake();
        }
    }
    fn interrupt_socket(&self) {
        if let Some(socket) = self.active.lock().unwrap().socket.as_ref() {
            let _ = socket.shutdown(std::net::Shutdown::Both);
        }
    }
    fn watch(self: &Arc<Self>, poll: &Poll) -> Result<WakeLease> {
        let wake = Arc::new(Waker::new(poll.registry(), Token(1))?);
        let mut active = self.active.lock().unwrap();
        self.check()?;
        ensure!(
            active.wake.is_none(),
            "A connection setup is already active"
        );
        active.wake = Some(wake);
        Ok(WakeLease(self.clone()))
    }
    fn register(self: &Arc<Self>, socket: TcpStream) -> Result<SocketLease> {
        let mut active = self.active.lock().unwrap();
        self.check()?;
        ensure!(active.socket.is_none(), "A connection is already active");
        active.socket = Some(socket);
        Ok(SocketLease(self.clone()))
    }
}
struct WakeLease(Arc<Control>);
impl Drop for WakeLease {
    fn drop(&mut self) {
        self.0.active.lock().unwrap().wake = None;
    }
}
struct SocketLease(Arc<Control>);
impl Drop for SocketLease {
    fn drop(&mut self) {
        if let Some(socket) = self.0.active.lock().unwrap().socket.take() {
            let _ = socket.shutdown(std::net::Shutdown::Both);
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum FirstFrameState {
    Pending,
    Complete,
    Expired,
}
pub(crate) struct FirstFrameDeadline {
    state: Arc<(Mutex<FirstFrameState>, std::sync::Condvar)>,
    deadline: Instant,
}
impl FirstFrameDeadline {
    pub fn start(control: Arc<Control>, timeout: Duration) -> Result<Self> {
        let state = Arc::new((
            Mutex::new(FirstFrameState::Pending),
            std::sync::Condvar::new(),
        ));
        let worker = state.clone();
        let deadline = Instant::now() + timeout;
        std::thread::Builder::new()
            .name("linkdesk-first-frame".into())
            .spawn(move || {
                let (lock, wake) = &*worker;
                let mut state = lock.lock().unwrap();
                while *state == FirstFrameState::Pending {
                    let remaining = deadline.saturating_duration_since(Instant::now());
                    if remaining.is_zero() {
                        *state = FirstFrameState::Expired;
                        control.interrupt_socket();
                        break;
                    }
                    state = wake.wait_timeout(state, remaining).unwrap().0;
                }
            })?;
        Ok(Self { state, deadline })
    }
    pub fn complete(&self) -> Result<()> {
        let (lock, wake) = &*self.state;
        let mut state = lock.lock().unwrap();
        if *state == FirstFrameState::Pending {
            *state = if Instant::now() < self.deadline {
                FirstFrameState::Complete
            } else {
                FirstFrameState::Expired
            };
        }
        wake.notify_one();
        ensure!(
            *state == FirstFrameState::Complete,
            "First decoded frame deadline exceeded"
        );
        Ok(())
    }
}
impl Drop for FirstFrameDeadline {
    fn drop(&mut self) {
        let (lock, wake) = &*self.state;
        let mut state = lock.lock().unwrap();
        if *state == FirstFrameState::Pending {
            *state = FirstFrameState::Complete;
        }
        wake.notify_one();
    }
}

struct Lookup {
    address: String,
    control: Arc<Control>,
    deadline: Instant,
    reply: mpsc::SyncSender<Result<Vec<SocketAddr>, String>>,
}
struct Resolver {
    tx: mpsc::SyncSender<Lookup>,
}
impl Resolver {
    fn start(
        lookup: impl Fn(&str) -> Result<Vec<SocketAddr>, String> + Send + 'static,
    ) -> io::Result<Self> {
        let (tx, rx) = mpsc::sync_channel::<Lookup>(4);
        std::thread::Builder::new()
            .name("linkdesk-resolver".into())
            .spawn(move || {
                for request in rx {
                    if request.control.stopped() || Instant::now() >= request.deadline {
                        continue;
                    }
                    let result = lookup(&request.address);
                    if !request.control.stopped() {
                        let _ = request.reply.try_send(result);
                    }
                }
            })?;
        Ok(Self { tx })
    }
    fn resolve(
        &self,
        address: &str,
        control: &Arc<Control>,
        timeout: Duration,
    ) -> Result<Vec<SocketAddr>> {
        control.check()?;
        let deadline = Instant::now() + timeout;
        let (tx, rx) = mpsc::sync_channel(1);
        self.tx
            .try_send(Lookup {
                address: address.into(),
                control: control.clone(),
                deadline,
                reply: tx,
            })
            .map_err(|_| {
                anyhow::anyhow!("Name resolver is busy; use the host IP or retry shortly")
            })?;
        loop {
            control.check()?;
            let remaining = deadline.saturating_duration_since(Instant::now());
            ensure!(
                !remaining.is_zero(),
                "Name resolution timed out; use the host IP"
            );
            match rx.recv_timeout(remaining.min(Duration::from_millis(50))) {
                Ok(result) => {
                    control.check()?;
                    return result.map_err(anyhow::Error::msg);
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => bail!("Name resolver stopped"),
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
        }
    }
}
fn resolve(address: &str, control: &Arc<Control>, timeout: Duration) -> Result<Vec<SocketAddr>> {
    control.check()?;
    ensure!(address.len() <= 512, "Host address is too long");
    if let Ok(address) = address.parse::<SocketAddr>() {
        return Ok(vec![address]);
    }
    static RESOLVER: OnceLock<Result<Resolver, String>> = OnceLock::new();
    let resolver = RESOLVER
        .get_or_init(|| {
            Resolver::start(|s| {
                s.to_socket_addrs()
                    .map(|a| a.take(8).collect())
                    .map_err(|e| e.to_string())
            })
            .map_err(|e| e.to_string())
        })
        .as_ref()
        .map_err(|e| anyhow::anyhow!("Cannot start name resolver: {e}"))?;
    resolver.resolve(address, control, timeout)
}

#[derive(Clone, Copy)]
pub(crate) struct Timeouts {
    pub resolve: Duration,
    pub connect: Duration,
    pub handshake: Duration,
}
impl Default for Timeouts {
    fn default() -> Self {
        Self {
            resolve: Duration::from_secs(5),
            connect: Duration::from_secs(5),
            handshake: Duration::from_secs(8),
        }
    }
}

pub(crate) struct ClientSession {
    pub stream: StreamOwned<ClientConnection, TcpStream>,
    _socket: SocketLease,
}
impl Read for ClientSession {
    fn read(&mut self, b: &mut [u8]) -> io::Result<usize> {
        self.stream.read(b)
    }
}
impl Write for ClientSession {
    fn write(&mut self, b: &[u8]) -> io::Result<usize> {
        self.stream.write(b)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.stream.flush()
    }
}

fn wait(poll: &mut Poll, events: &mut Events, control: &Control, deadline: Instant) -> Result<()> {
    control.check()?;
    let remaining = deadline.saturating_duration_since(Instant::now());
    ensure!(!remaining.is_zero(), "Connection setup deadline exceeded");
    match poll.poll(events, Some(remaining)) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
        Err(e) => return Err(e.into()),
    }
    control.check()
}

pub(crate) fn connect(
    address: &str,
    pin: Option<String>,
    control: &Arc<Control>,
    timeouts: Timeouts,
) -> Result<ClientSession> {
    let addresses = resolve(address, control, timeouts.resolve).context(FailureStage::Resolve)?;
    connect_resolved(addresses, pin, control, timeouts)
}

fn connect_resolved(
    addresses: Vec<SocketAddr>,
    pin: Option<String>,
    control: &Arc<Control>,
    timeouts: Timeouts,
) -> Result<ClientSession> {
    let mut unique = Vec::new();
    for address in addresses.into_iter().take(8) {
        if !unique.contains(&address) {
            unique.push(address);
        }
    }
    let mut poll = Poll::new().context(FailureStage::Connect)?;
    let _wake = control.watch(&poll).context(FailureStage::Connect)?;
    let mut events = Events::with_capacity(8);
    let deadline = Instant::now() + timeouts.connect;
    let count = unique.len();
    let mut connected = None;
    let mut last = None;
    for (index, address) in unique.into_iter().enumerate() {
        control.check()?;
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break;
        }
        // Keep a bounded share for later IPv4/IPv6 candidates instead of
        // spending the complete budget on the first route.
        let attempt_deadline = Instant::now() + remaining / (count - index) as u32;
        let attempt = (|| -> Result<mio::net::TcpStream> {
            let mut socket = mio::net::TcpStream::connect(address)?;
            poll.registry().register(
                &mut socket,
                Token(0),
                Interest::READABLE | Interest::WRITABLE,
            )?;
            let result = (|| -> Result<()> {
                loop {
                    wait(&mut poll, &mut events, control, attempt_deadline)?;
                    if let Some(error) = socket.take_error()? {
                        return Err(error.into());
                    }
                    if socket.peer_addr().is_ok() {
                        return Ok(());
                    }
                }
            })();
            if result.is_err() {
                let _ = poll.registry().deregister(&mut socket);
            }
            result?;
            Ok(socket)
        })();
        match attempt {
            Ok(socket) => {
                connected = Some(socket);
                break;
            }
            Err(error) => last = Some(error),
        }
    }
    control.check()?;
    let socket = connected
        .ok_or_else(|| {
            last.unwrap_or_else(|| {
                anyhow::anyhow!("Host address not found or connection budget exhausted")
            })
        })
        .context(FailureStage::Connect)?;
    let duplicate: TcpStream = socket2::SockRef::from(&socket)
        .try_clone()
        .context(FailureStage::Connect)?
        .into();
    let lease = control.register(duplicate).context(FailureStage::Connect)?;
    let connection = ClientConnection::new(
        crate::identity::client_config(pin),
        ServerName::try_from("linkdesk.local")?,
    )
    .context(FailureStage::Handshake)?;
    let mut tls = StreamOwned::new(connection, socket);
    let deadline = Instant::now() + timeouts.handshake;
    (|| -> Result<()> {
        while tls.conn.is_handshaking() {
            control.check()?;
            ensure!(Instant::now() < deadline, "TLS handshake deadline exceeded");
            match tls.conn.complete_io(&mut tls.sock) {
                Ok((0, 0)) if tls.conn.is_handshaking() => {
                    bail!("Host closed during TLS handshake")
                }
                Ok(_) => {}
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                    wait(&mut poll, &mut events, control, deadline)?
                }
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => return Err(e.into()),
            }
        }
        control.check()?;
        poll.registry().deregister(&mut tls.sock)?;
        Ok(())
    })()
    .context(FailureStage::Handshake)?;
    let socket: TcpStream = tls.sock.into();
    (|| -> io::Result<()> {
        socket.set_nonblocking(false)?;
        socket.set_nodelay(true)?;
        socket.set_read_timeout(Some(Duration::from_secs(8)))?;
        socket.set_write_timeout(Some(Duration::from_secs(5)))
    })()
    .context(FailureStage::Handshake)?;
    Ok(ClientSession {
        stream: StreamOwned::new(tls.conn, socket),
        _socket: lease,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{net::TcpListener, thread};

    #[test]
    fn cancellation_wakes_a_blocked_connection_poll() {
        let control = Arc::new(Control::default());
        let active = control.clone();
        let (ready_tx, ready) = mpsc::channel();
        let (done_tx, done) = mpsc::channel();
        let worker = thread::spawn(move || {
            let mut poll = Poll::new().unwrap();
            let _wake = active.watch(&poll).unwrap();
            let mut events = Events::with_capacity(2);
            ready_tx.send(()).unwrap();
            done_tx
                .send(
                    wait(
                        &mut poll,
                        &mut events,
                        &active,
                        Instant::now() + Duration::from_secs(5),
                    )
                    .is_err(),
                )
                .unwrap();
        });
        ready.recv_timeout(Duration::from_secs(2)).unwrap();
        thread::sleep(Duration::from_millis(50));
        control.cancel();
        assert!(done.recv_timeout(Duration::from_secs(2)).unwrap());
        worker.join().unwrap();
    }

    #[test]
    fn cancellation_does_not_wait_for_an_inflight_system_lookup() {
        let (entered_tx, entered) = mpsc::channel();
        let (release_tx, release) = mpsc::channel();
        let resolver = Arc::new(
            Resolver::start(move |_| {
                entered_tx.send(()).unwrap();
                release.recv().unwrap();
                Ok(vec!["127.0.0.1:1234".parse().unwrap()])
            })
            .unwrap(),
        );
        let control = Arc::new(Control::default());
        let worker_control = control.clone();
        let worker_resolver = resolver.clone();
        let (done_tx, done) = mpsc::channel();
        let worker = thread::spawn(move || {
            done_tx
                .send(
                    worker_resolver
                        .resolve("held.invalid:1234", &worker_control, Duration::from_secs(5))
                        .is_err(),
                )
                .unwrap();
        });
        entered.recv_timeout(Duration::from_secs(2)).unwrap();
        control.cancel();
        let cancelled = done.recv_timeout(Duration::from_secs(2));
        release_tx.send(()).unwrap();
        worker.join().unwrap();
        assert!(
            cancelled.unwrap(),
            "UI cancellation must not wait for the OS resolver"
        );
    }
    #[test]
    fn cancelled_and_expired_queued_names_are_not_resolved_later() {
        let calls = Arc::new(Mutex::new(Vec::<String>::new()));
        let recorded = calls.clone();
        let (entered_tx, entered) = mpsc::channel();
        let (release_tx, release) = mpsc::channel();
        let resolver = Arc::new(
            Resolver::start(move |name| {
                recorded.lock().unwrap().push(name.into());
                if name == "held.invalid:1234" {
                    entered_tx.send(()).unwrap();
                    release.recv().unwrap();
                }
                Ok(vec!["127.0.0.1:1234".parse().unwrap()])
            })
            .unwrap(),
        );
        let first = resolver.clone();
        let worker = thread::spawn(move || {
            first.resolve(
                "held.invalid:1234",
                &Arc::new(Control::default()),
                Duration::from_secs(5),
            )
        });
        entered.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(resolver
            .resolve(
                "expired.invalid:1234",
                &Arc::new(Control::default()),
                Duration::from_millis(50)
            )
            .is_err());
        let cancelled = Arc::new(Control::default());
        cancelled.cancel();
        let (reply, _) = mpsc::sync_channel(1);
        resolver
            .tx
            .try_send(Lookup {
                address: "cancelled.invalid:1234".into(),
                control: cancelled,
                deadline: Instant::now() + Duration::from_secs(5),
                reply,
            })
            .unwrap();
        let (reply, last) = mpsc::sync_channel(1);
        resolver
            .tx
            .try_send(Lookup {
                address: "active.invalid:1234".into(),
                control: Arc::new(Control::default()),
                deadline: Instant::now() + Duration::from_secs(5),
                reply,
            })
            .unwrap();
        release_tx.send(()).unwrap();
        worker.join().unwrap().unwrap();
        last.recv_timeout(Duration::from_secs(2)).unwrap().unwrap();
        assert_eq!(
            *calls.lock().unwrap(),
            vec!["held.invalid:1234", "active.invalid:1234"]
        );
    }
    fn silent_peer() -> (
        String,
        mpsc::Receiver<()>,
        mpsc::Receiver<bool>,
        thread::JoinHandle<()>,
    ) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap().to_string();
        let (entered_tx, entered) = mpsc::channel();
        let (closed_tx, closed) = mpsc::channel();
        let worker = thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(4)))
                .unwrap();
            let mut first = [0; 4096];
            let n = socket.read(&mut first).unwrap();
            assert!(n > 0);
            entered_tx.send(()).unwrap();
            closed_tx
                .send(socket.read_to_end(&mut Vec::new()).is_ok())
                .unwrap();
        });
        (address, entered, closed, worker)
    }
    #[test]
    fn cancel_interrupts_nonresponsive_tls_before_its_handshake_deadline() {
        let (address, entered, closed, server) = silent_peer();
        let control = Arc::new(Control::default());
        let active = control.clone();
        let (done_tx, done) = mpsc::channel();
        let client = thread::spawn(move || {
            done_tx
                .send(connect(&address, None, &active, Timeouts::default()).is_err())
                .unwrap();
        });
        entered.recv_timeout(Duration::from_secs(2)).unwrap();
        control.cancel();
        assert!(done.recv_timeout(Duration::from_secs(2)).unwrap());
        assert!(closed.recv_timeout(Duration::from_secs(2)).unwrap());
        client.join().unwrap();
        server.join().unwrap();
    }
    #[test]
    fn silent_tls_has_an_absolute_deadline_and_reports_the_handshake_stage() {
        let (address, _entered, closed, server) = silent_peer();
        let timeouts = Timeouts {
            handshake: Duration::from_millis(200),
            ..Timeouts::default()
        };
        let started = Instant::now();
        let error = connect(&address, None, &Arc::new(Control::default()), timeouts)
            .err()
            .unwrap();
        assert_eq!(
            stage_of(&error, FailureStage::Connect),
            FailureStage::Handshake
        );
        assert!(started.elapsed() < Duration::from_secs(2));
        assert!(closed.recv_timeout(Duration::from_secs(2)).unwrap());
        server.join().unwrap();
    }
    #[test]
    fn refused_first_address_falls_back_to_the_next_pinned_tls_endpoint() {
        let refused = socket2::Socket::new(
            socket2::Domain::IPV4,
            socket2::Type::STREAM,
            Some(socket2::Protocol::TCP),
        )
        .unwrap();
        refused
            .bind(&"127.0.0.1:0".parse::<SocketAddr>().unwrap().into())
            .unwrap();
        let bad = refused.local_addr().unwrap().as_socket().unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let good = listener.local_addr().unwrap();
        let identity = crate::identity::new_identity().unwrap();
        let fingerprint = identity.fingerprint.clone();
        let server = thread::spawn(move || {
            let (socket, _) = listener.accept().unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(4)))
                .unwrap();
            let mut tls = StreamOwned::new(
                rustls::ServerConnection::new(identity.config).unwrap(),
                socket,
            );
            assert!(
                crate::protocol::read_json::<crate::protocol::Hello>(&mut tls).is_err(),
                "TLS setup alone must not send a PIN"
            );
        });
        let session = connect_resolved(
            vec![bad, good],
            Some(fingerprint.clone()),
            &Arc::new(Control::default()),
            Timeouts::default(),
        )
        .unwrap();
        assert_eq!(
            crate::identity::fingerprint(
                session.stream.conn.peer_certificates().unwrap()[0].as_ref()
            ),
            fingerprint
        );
        drop(session);
        server.join().unwrap();
    }
}
