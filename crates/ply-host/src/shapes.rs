//! One value of each `std` type the socket handlers build, under that type's name. A handler builds
//! a constructor or a record by its name and fields, so a rename in the library breaks no compile:
//! the CLI suite types each of these against the library's declaration instead.

use crate::dns;
use crate::tcp::wire::{self, Credentials, Endpoint, Probing, Refusal, SocketOption};
use ply_eval::Value;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::time::Duration;

pub fn shapes() -> Vec<(&'static str, Value)> {
    let v4 = IpAddr::V4(Ipv4Addr::new(192, 0, 2, 1));
    let v6 = IpAddr::V6(Ipv6Addr::LOCALHOST);
    let probing = Probing {
        idle: Duration::from_secs(60),
        interval: Some(Duration::from_secs(5)),
        count: Some(3),
    };
    let refusals = [
        Refusal::Refused,
        Refusal::Unreachable,
        Refusal::TimedOut,
        Refusal::Reset,
        Refusal::NameNotFound,
        Refusal::AddressInUse,
        Refusal::PermissionDenied,
        Refusal::Injected,
        Refusal::Untrusted,
        Refusal::Handshake("no shared cipher".to_string()),
        Refusal::Other(61, "the system's own words".to_string()),
    ];
    let options = [
        SocketOption::NoDelay(true),
        SocketOption::KeepAlive(Some(probing)),
        SocketOption::ReuseAddress(true),
        SocketOption::ReusePort(false),
        SocketOption::SendBuffer(65536),
        SocketOption::ReceiveBuffer(65536),
        SocketOption::Linger(Some(Duration::from_secs(1))),
        SocketOption::Broadcast(false),
        SocketOption::MulticastLoop(true),
        SocketOption::JoinGroup(v4, Some("en0".to_string())),
        SocketOption::LeaveGroup(v6, None),
        SocketOption::Presenting("client".to_string()),
    ];
    let mut shapes = vec![
        (
            "std.ip.SocketAddress",
            Endpoint {
                ip: v4,
                port: 80,
                zone: None,
            }
            .value(),
        ),
        (
            "std.ip.SocketAddress",
            Endpoint {
                ip: v6,
                port: 443,
                zone: Some("1".to_string()),
            }
            .value(),
        ),
        ("std.net.Probing", probing.value()),
        (
            "std.net.PeerCredentials",
            Credentials {
                user: 501,
                group: 20,
                process: Some(7),
            }
            .value(),
        ),
        ("std.net.Secured", wire::secured(Some("h2".to_string()))),
    ];
    shapes.extend(refusals.iter().map(|r| ("std.net.Refusal", r.value())));
    shapes.extend(options.iter().map(|o| ("std.net.SocketOption", o.value())));
    shapes.extend(dns::shapes());
    shapes
}
