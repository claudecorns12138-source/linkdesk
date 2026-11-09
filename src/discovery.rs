//! Small LAN broadcast beacons. Manual IP pairing works when broadcasts are blocked.
use crate::protocol::{DISCOVERY_PORT, VERSION};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use socket2::{Domain, Protocol, Socket, Type};
use std::{
    collections::BTreeSet,
    net::{Ipv4Addr, SocketAddrV4, UdpSocket},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::SyncSender,
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

pub fn local_addresses(port: u16) -> Vec<String> {
    let mut result = BTreeSet::new();
    if let Ok(interfaces) = if_addrs::get_if_addrs() {
        for interface in interfaces {
            if let if_addrs::IfAddr::V4(address) = interface.addr {
                if !address.ip.is_loopback()
                    && !address.ip.is_unspecified()
                    && !address.ip.is_link_local()
                {
                    result.insert(format!("{}:{port}", address.ip));
                }
            }
        }
    }
    result.into_iter().collect()
}

fn broadcast_sockets() -> Vec<(UdpSocket, Ipv4Addr)> {
    let mut result = Vec::new();
    let mut seen = BTreeSet::new();
    if let Ok(interfaces) = if_addrs::get_if_addrs() {
        for interface in interfaces {
            if let if_addrs::IfAddr::V4(address) = interface.addr {
                if address.ip.is_loopback() || address.ip.is_unspecified() {
                    continue;
                }
                if let Some(broadcast) = address.broadcast {
                    if !seen.insert((address.ip, broadcast)) {
                        continue;
                    }
                    if let Ok(socket) = UdpSocket::bind((address.ip, 0)) {
                        if socket.set_broadcast(true).is_ok() {
                            result.push((socket, broadcast));
                        }
                    }
                }
            }
        }
    }
    if let Ok(socket) = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)) {
        if socket.set_broadcast(true).is_ok() {
            result.push((socket, Ipv4Addr::BROADCAST));
        }
    }
    result
}

fn parse_beacon(data: &[u8]) -> Option<Beacon> {
    let b: Beacon = serde_json::from_slice(data).ok()?;
    (b.service == "linkdesk-native"
        && b.version == VERSION
        && b.port != 0
        && !b.name.trim().is_empty()
        && b.name.len() <= 80
        && !b.name.chars().any(char::is_control))
    .then_some(b)
}

pub fn announce(port: u16, stop: Arc<AtomicBool>) -> Result<()> {
    let data = serde_json::to_vec(&Beacon {
        service: "linkdesk-native".into(),
        version: VERSION,
        port,
        name: "LinkDesk".into(),
    })?;
    thread::spawn(move || {
        let mut sockets = broadcast_sockets();
        let mut refreshed = Instant::now();
        while !stop.load(Ordering::Relaxed) {
            if refreshed.elapsed() > Duration::from_secs(10) {
                sockets = broadcast_sockets();
                refreshed = Instant::now();
            }
            for (socket, broadcast) in &sockets {
                let _ = socket.send_to(&data, (*broadcast, DISCOVERY_PORT));
            }
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

pub fn listen(tx: SyncSender<Device>, stop: Arc<AtomicBool>) -> Result<()> {
    let socket = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
    socket.set_reuse_address(true)?;
    socket.bind(&SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, DISCOVERY_PORT).into())?;
    let socket: UdpSocket = socket.into();
    socket.set_read_timeout(Some(Duration::from_millis(500)))?;
    thread::spawn(move || {
        let mut data = [0; 1024];
        while !stop.load(Ordering::Relaxed) {
            if let Ok((n, addr)) = socket.recv_from(&mut data) {
                if let Some(b) = parse_beacon(&data[..n]) {
                    let _ = tx.try_send(Device {
                        address: format!("{}:{}", addr.ip(), b.port),
                        name: b.name,
                        seen: Instant::now(),
                    });
                }
            }
        }
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn discovery_rejects_wrong_protocol_and_control_characters() {
        let mut beacon = Beacon {
            service: "linkdesk-native".into(),
            version: VERSION,
            port: 47831,
            name: "LinkDesk".into(),
        };
        assert!(parse_beacon(&serde_json::to_vec(&beacon).unwrap()).is_some());
        beacon.name = "Fake\nApproved".into();
        assert!(parse_beacon(&serde_json::to_vec(&beacon).unwrap()).is_none());
        beacon.name = "LinkDesk".into();
        beacon.version += 1;
        assert!(parse_beacon(&serde_json::to_vec(&beacon).unwrap()).is_none());
    }
}
