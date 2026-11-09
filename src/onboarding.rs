//! Input handling shared by the native pairing UI and its validation tests.
use crate::protocol::PORT;
use std::net::{IpAddr, SocketAddr};

pub fn address(input: &str) -> Result<String, &'static str> {
    let value = input.trim();
    if value.is_empty() {
        return Err("请填写主机地址，或从发现的设备中选择。");
    }
    if let Ok(socket) = value.parse::<SocketAddr>() {
        if socket.port() == 0 || socket.ip().is_unspecified() {
            return Err("请使用主机的实际网络地址和有效端口。");
        }
        return Ok(socket.to_string());
    }
    if let Ok(ip) = value
        .trim_start_matches('[')
        .trim_end_matches(']')
        .parse::<IpAddr>()
    {
        if ip.is_unspecified() {
            return Err("请使用主机的实际网络地址。");
        }
        return Ok(SocketAddr::new(ip, PORT).to_string());
    }
    let (host, port) = match value.split_once(':') {
        Some((host, port)) => (
            host,
            port.parse::<u16>()
                .ok()
                .filter(|p| *p != 0)
                .ok_or("端口应为 1 到 65535 之间的数字。")?,
        ),
        None => (value, PORT),
    };
    if host.len() > 253
        || host.split('.').any(|part| {
            part.is_empty()
                || part.len() > 63
                || part.starts_with('-')
                || part.ends_with('-')
                || !part.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-')
        })
    {
        return Err("地址格式不正确。请输入 IP 或主机名，不要粘贴网页链接。");
    }
    Ok(format!("{host}:{port}"))
}

pub fn pin(input: &str) -> Result<String, &'static str> {
    let result: String = input
        .chars()
        .filter(|c| !c.is_ascii_whitespace() && *c != '-')
        .collect();
    if result.len() != 6 || !result.bytes().all(|c| c.is_ascii_digit()) {
        return Err("请输入主机上显示的六位配对码。");
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn addresses_support_ipv4_ipv6_and_hostnames() {
        assert_eq!(address(" 127.0.0.1 ").unwrap(), "127.0.0.1:47831");
        assert_eq!(address("::1").unwrap(), "[::1]:47831");
        assert_eq!(address("[::1]:5000").unwrap(), "[::1]:5000");
        assert_eq!(address("host.local").unwrap(), "host.local:47831");
        assert_eq!(address("host.local:5000").unwrap(), "host.local:5000");
        for invalid in [
            "",
            "http://host.local",
            "host:0",
            "host:65536",
            "hello world",
            "0.0.0.0",
            "host..local",
        ] {
            assert!(address(invalid).is_err(), "{invalid}");
        }
    }
    #[test]
    fn pasted_pair_codes_keep_leading_zeroes() {
        assert_eq!(pin("012 345").unwrap(), "012345");
        assert_eq!(pin("012-345").unwrap(), "012345");
        assert!(pin("01234x").is_err());
        assert!(pin("12345").is_err());
    }
}
