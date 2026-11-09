//! Small LAN broadcast beacons. Manual IP pairing works when broadcasts are blocked.
use crate::protocol::{DISCOVERY_PORT, VERSION};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use socket2::{Domain, Protocol, Socket, Type};
use std::{
    net::{Ipv4Addr, SocketAddrV4, UdpSocket},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::Sender,
        Arc,
    },
    thread,
    time::{Duration, Instant},
};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Beacon {
    pub service: String,
    pub version: u16,
    pub port: u16,
    pub name: String,
}
#[derive(Debug, Clone)]
pub struct Device {
    pub address: String,
    pub name: String,
    pub seen: Instant,
}

pub fn announce(port: u16, stop: Arc<AtomicBool>) -> Result<()> {
    let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0))?;
    socket.set_broadcast(true)?;
    let data = serde_json::to_vec(&Beacon {
        service: "linkdesk-native".into(),
        version: VERSION,
        port,
        name: "LinkDesk".into(),
    })?;
    thread::spawn(move || {
        while !stop.load(Ordering::Relaxed) {
            let _ = socket.send_to(&data, (Ipv4Addr::BROADCAST, DISCOVERY_PORT));
            for _ in 0..20 {
                if stop.load(Ordering::Relaxed) {
                    break;
                }
                thread::sleep(Duration::from_millis(100));
            }
        }
    });
    Ok(())
}

pub fn listen(tx: Sender<Device>, stop: Arc<AtomicBool>) -> Result<()> {
    let socket = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
    socket.set_reuse_address(true)?;
    socket.bind(&SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, DISCOVERY_PORT).into())?;
    let socket: UdpSocket = socket.into();
    socket.set_read_timeout(Some(Duration::from_millis(500)))?;
    thread::spawn(move || {
        let mut data = [0; 1024];
        while !stop.load(Ordering::Relaxed) {
            if let Ok((n, addr)) = socket.recv_from(&mut data) {
                if let Ok(b) = serde_json::from_slice::<Beacon>(&data[..n]) {
                    if b.service == "linkdesk-native"
                        && b.version == VERSION
                        && b.port != 0
                        && b.name.len() <= 80
                    {
                        let _ = tx.send(Device {
                            address: format!("{}:{}", addr.ip(), b.port),
                            name: b.name,
                            seen: Instant::now(),
                        });
                    }
                }
            }
        }
    });
    Ok(())
}
