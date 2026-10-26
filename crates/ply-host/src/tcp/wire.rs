//! What `std.net`'s and `std.udp`'s real handlers hand their natives and read back, in flat forms
//! the handlers' own readers and writers in `std.net` share. A socket address is parts: its
//! address's bytes, its port in two bytes and its zone, empty for none. An option is its setting's
//! byte, as `std.net` numbers settings, and then what it sets. A refusal is its kind's byte,
//! `Refused` 1 to `Other` 11 in declaration order, then `Handshake`'s reason, or `Other`'s code and
//! words as parts. An answer that may be refused is byte 1 and what it answers, or byte 0 and the
//! refusal.

use crate::dns::{address_bytes, address_of};
use crate::native::{int, int_of, parts, parts_of};
use std::io;
use std::net::{IpAddr, SocketAddr, SocketAddrV4, SocketAddrV6};
use std::time::Duration;

/// A `std.net.Refusal`.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Refusal {
    Refused,
    Unreachable,
    TimedOut,
    Reset,
    NameNotFound,
    AddressInUse,
    PermissionDenied,
    Injected,
    Untrusted,
    Handshake(String),
    Other(i64, String),
}

impl Refusal {
    pub fn of(error: &io::Error) -> Refusal {
        use io::ErrorKind as Kind;
        match error.kind() {
            Kind::ConnectionRefused => Refusal::Refused,
            Kind::HostUnreachable | Kind::NetworkUnreachable => Refusal::Unreachable,
            Kind::TimedOut | Kind::WouldBlock => Refusal::TimedOut,
            Kind::ConnectionReset
            | Kind::ConnectionAborted
            | Kind::BrokenPipe
            | Kind::NotConnected
            | Kind::UnexpectedEof => Refusal::Reset,
            Kind::AddrInUse => Refusal::AddressInUse,
            Kind::PermissionDenied => Refusal::PermissionDenied,
            _ => Refusal::Other(
                i64::from(error.raw_os_error().unwrap_or(0)),
                error.to_string(),
            ),
        }
    }

    pub fn written(&self) -> Vec<u8> {
        match self {
            Refusal::Refused => vec![1],
            Refusal::Unreachable => vec![2],
            Refusal::TimedOut => vec![3],
            Refusal::Reset => vec![4],
            Refusal::NameNotFound => vec![5],
            Refusal::AddressInUse => vec![6],
            Refusal::PermissionDenied => vec![7],
            Refusal::Injected => vec![8],
            Refusal::Untrusted => vec![9],
            Refusal::Handshake(why) => [&[10u8][..], why.as_bytes()].concat(),
            Refusal::Other(code, text) => {
                [&[11u8][..], &parts(&[&int(*code), text.as_bytes()])].concat()
            }
        }
    }
}

/// `outcome` as an answer that may be refused.
pub fn answer(outcome: Result<Vec<u8>, Refusal>) -> Vec<u8> {
    match outcome {
        Ok(answered) => [&[1u8][..], &answered].concat(),
        Err(refusal) => [&[0u8][..], &refusal.written()].concat(),
    }
}

/// A `std.ip.SocketAddress`: an address, a port, and the zone of a link-local IPv6 address.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Endpoint {
    pub ip: IpAddr,
    pub port: u16,
    pub zone: Option<String>,
}

impl Endpoint {
    pub fn of(address: SocketAddr) -> Endpoint {
        match address {
            SocketAddr::V4(a) => Endpoint {
                ip: IpAddr::V4(*a.ip()),
                port: a.port(),
                zone: None,
            },
            SocketAddr::V6(a) => Endpoint {
                ip: IpAddr::V6(*a.ip()),
                port: a.port(),
                zone: zone_name(a.scope_id()),
            },
        }
    }

    /// The address a socket call takes; a zone that names no interface of this machine is one
    /// nothing can be reached through.
    pub fn socket_addr(&self) -> Result<SocketAddr, Refusal> {
        match (self.ip, &self.zone) {
            (IpAddr::V4(ip), _) => Ok(SocketAddr::V4(SocketAddrV4::new(ip, self.port))),
            (IpAddr::V6(ip), None) => Ok(SocketAddr::V6(SocketAddrV6::new(ip, self.port, 0, 0))),
            (IpAddr::V6(ip), Some(zone)) => match zone_index(zone) {
                Some(scope) => Ok(SocketAddr::V6(SocketAddrV6::new(ip, self.port, 0, scope))),
                None => Err(Refusal::Other(
                    0,
                    format!("`{zone}` names no interface of this machine"),
                )),
            },
        }
    }

    pub fn written(&self) -> Vec<u8> {
        parts(&[
            &address_bytes(&self.ip),
            &self.port.to_be_bytes(),
            self.zone.as_deref().unwrap_or_default().as_bytes(),
        ])
    }

    /// The socket address `flat` writes; `None` for bytes that write none.
    pub fn read(flat: &[u8]) -> Option<Endpoint> {
        let [ip, port, zone] = parts_of(flat)?[..] else {
            return None;
        };
        let zone = std::str::from_utf8(zone).ok()?;
        Some(Endpoint {
            ip: address_of(ip)?,
            port: u16::from_be_bytes(port.try_into().ok()?),
            zone: (!zone.is_empty()).then(|| zone.to_string()),
        })
    }
}

/// The interface a scope id names, as a zone is written; the number where it names none.
#[cfg(unix)]
fn zone_name(scope: u32) -> Option<String> {
    if scope == 0 {
        return None;
    }
    let mut name = [0 as libc::c_char; libc::IF_NAMESIZE];
    // SAFETY: the buffer is `IF_NAMESIZE` bytes, the most the call writes, terminator included.
    let found = unsafe { libc::if_indextoname(scope, name.as_mut_ptr()) };
    if found.is_null() {
        return Some(scope.to_string());
    }
    // SAFETY: a non-null answer is the buffer, holding a terminated name.
    let name = unsafe { std::ffi::CStr::from_ptr(name.as_ptr()) };
    Some(name.to_string_lossy().into_owned())
}

#[cfg(not(unix))]
fn zone_name(scope: u32) -> Option<String> {
    (scope != 0).then(|| scope.to_string())
}

#[cfg(unix)]
pub fn zone_index(zone: &str) -> Option<u32> {
    if let Ok(scope) = zone.parse::<u32>() {
        return Some(scope);
    }
    let name = std::ffi::CString::new(zone).ok()?;
    // SAFETY: `name` is a terminated string that outlives the call.
    match unsafe { libc::if_nametoindex(name.as_ptr()) } {
        0 => None,
        scope => Some(scope),
    }
}

#[cfg(not(unix))]
pub fn zone_index(zone: &str) -> Option<u32> {
    zone.parse().ok()
}

/// A `std.net.SocketOption`.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum SocketOption {
    NoDelay(bool),
    KeepAlive(Option<Probing>),
    ReuseAddress(bool),
    ReusePort(bool),
    SendBuffer(usize),
    ReceiveBuffer(usize),
    Linger(Option<Duration>),
    Broadcast(bool),
    MulticastLoop(bool),
    /// A multicast group, and the interface it is joined on where one is named.
    JoinGroup(IpAddr, Option<String>),
    LeaveGroup(IpAddr, Option<String>),
    /// The `--tls` credential a later `start_tls` presents where its server asks for a client
    /// certificate.
    Presenting(String),
}

/// A `std.net.Probing`: when an idle connection is probed, how often, and how many unanswered
/// probes end it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Probing {
    pub idle: Duration,
    pub interval: Option<Duration>,
    pub count: Option<u32>,
}

impl SocketOption {
    /// The option `flat` writes; `None` for bytes that write none.
    pub fn read(flat: &[u8]) -> Option<SocketOption> {
        let (setting, rest) = flat.split_first()?;
        let flag = || match rest {
            [0] => Some(false),
            [1] => Some(true),
            _ => None,
        };
        let size = || usize::try_from(int_of(rest)?).ok();
        let group = || -> Option<(IpAddr, Option<String>)> {
            let [group, interface] = parts_of(rest)?[..] else {
                return None;
            };
            let interface = std::str::from_utf8(interface).ok()?;
            Some((
                address_of(group)?,
                (!interface.is_empty()).then(|| interface.to_string()),
            ))
        };
        Some(match setting {
            0 => SocketOption::NoDelay(flag()?),
            1 => SocketOption::KeepAlive(match rest.split_first()? {
                (0, []) => None,
                (1, probing) => Some(Probing::read(probing)?),
                _ => return None,
            }),
            2 => SocketOption::ReuseAddress(flag()?),
            3 => SocketOption::ReusePort(flag()?),
            4 => SocketOption::SendBuffer(size()?),
            5 => SocketOption::ReceiveBuffer(size()?),
            6 => SocketOption::Linger(match rest.split_first()? {
                (0, []) => None,
                (1, nanos) => Some(duration_of(nanos)?),
                _ => return None,
            }),
            7 => SocketOption::Broadcast(flag()?),
            8 => SocketOption::MulticastLoop(flag()?),
            9 => {
                let (address, interface) = group()?;
                SocketOption::JoinGroup(address, interface)
            }
            10 => {
                let (address, interface) = group()?;
                SocketOption::LeaveGroup(address, interface)
            }
            11 => SocketOption::Presenting(std::str::from_utf8(rest).ok()?.to_string()),
            _ => return None,
        })
    }

    /// The options `flat` writes as parts, one each.
    pub fn list(flat: &[u8]) -> Option<Vec<SocketOption>> {
        parts_of(flat)?
            .into_iter()
            .map(SocketOption::read)
            .collect()
    }

    pub fn written(&self) -> Vec<u8> {
        let flag = |setting: u8, on: bool| vec![setting, u8::from(on)];
        let size = |setting: u8, n: usize| {
            [&[setting][..], &int(i64::try_from(n).unwrap_or(i64::MAX))].concat()
        };
        let group = |setting: u8, address: &IpAddr, interface: &Option<String>| {
            let named = interface.as_deref().unwrap_or_default();
            [
                &[setting][..],
                &parts(&[&address_bytes(address), named.as_bytes()]),
            ]
            .concat()
        };
        match self {
            SocketOption::NoDelay(on) => flag(0, *on),
            SocketOption::KeepAlive(None) => vec![1, 0],
            SocketOption::KeepAlive(Some(probing)) => [&[1u8, 1][..], &probing.written()].concat(),
            SocketOption::ReuseAddress(on) => flag(2, *on),
            SocketOption::ReusePort(on) => flag(3, *on),
            SocketOption::SendBuffer(n) => size(4, *n),
            SocketOption::ReceiveBuffer(n) => size(5, *n),
            SocketOption::Linger(None) => vec![6, 0],
            SocketOption::Linger(Some(d)) => [&[6u8, 1][..], &nanos(*d)].concat(),
            SocketOption::Broadcast(on) => flag(7, *on),
            SocketOption::MulticastLoop(on) => flag(8, *on),
            SocketOption::JoinGroup(address, interface) => group(9, address, interface),
            SocketOption::LeaveGroup(address, interface) => group(10, address, interface),
            SocketOption::Presenting(name) => [&[11u8][..], name.as_bytes()].concat(),
        }
    }

    /// `options` as parts, one each.
    pub fn listed(options: &[SocketOption]) -> Vec<u8> {
        let written: Vec<Vec<u8>> = options.iter().map(SocketOption::written).collect();
        let each: Vec<&[u8]> = written.iter().map(Vec::as_slice).collect();
        parts(&each)
    }

    /// Whether `options` reads the option back: a group joined or left, and a credential to
    /// present, are acts rather than settings of the socket.
    pub fn is_setting(&self) -> bool {
        !matches!(
            self,
            SocketOption::JoinGroup(..)
                | SocketOption::LeaveGroup(..)
                | SocketOption::Presenting(_)
        )
    }
}

impl Probing {
    /// Parts: the idle wait, then the interval and the count, each empty for none.
    fn read(flat: &[u8]) -> Option<Probing> {
        let [idle, interval, count] = parts_of(flat)?[..] else {
            return None;
        };
        Some(Probing {
            idle: duration_of(idle)?,
            interval: if interval.is_empty() {
                None
            } else {
                Some(duration_of(interval)?)
            },
            count: if count.is_empty() {
                None
            } else {
                Some(u32::try_from(int_of(count)?).ok()?)
            },
        })
    }

    fn written(&self) -> Vec<u8> {
        let interval = self.interval.map(nanos);
        let count = self.count.map(|n| int(i64::from(n)));
        parts(&[
            &nanos(self.idle),
            interval.as_ref().map_or(&[][..], |n| &n[..]),
            count.as_ref().map_or(&[][..], |n| &n[..]),
        ])
    }
}

/// A duration as its nanoseconds' eight bytes.
fn nanos(d: Duration) -> [u8; 8] {
    int(i64::try_from(d.as_nanos()).unwrap_or(i64::MAX))
}

/// The duration of nanoseconds eight bytes count, which is not negative.
fn duration_of(bytes: &[u8]) -> Option<Duration> {
    u64::try_from(int_of(bytes)?).ok().map(Duration::from_nanos)
}

/// A `std.net.PeerCredentials`: who holds the other end of a Unix socket.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Credentials {
    pub user: i64,
    pub group: i64,
    pub process: Option<i64>,
}

impl Credentials {
    /// Parts: the user, the group, and the process, empty where the platform does not say.
    pub fn written(&self) -> Vec<u8> {
        let process = self.process.map(int);
        parts(&[
            &int(self.user),
            &int(self.group),
            process.as_ref().map_or(&[][..], |n| &n[..]),
        ])
    }
}
