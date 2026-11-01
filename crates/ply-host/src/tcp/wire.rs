use crate::dns::{address_bytes, ip_address_from_bytes};
use crate::native::{int, int_of, parts, parts_of};
use std::io;
use std::net::{IpAddr, SocketAddr, SocketAddrV4, SocketAddrV6};
use std::time::Duration;

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum NetError {
    Rejected,
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

impl NetError {
    pub fn of(error: &io::Error) -> NetError {
        use io::ErrorKind as Kind;
        match error.kind() {
            Kind::ConnectionRefused => NetError::Rejected,
            Kind::HostUnreachable | Kind::NetworkUnreachable => NetError::Unreachable,
            Kind::TimedOut | Kind::WouldBlock => NetError::TimedOut,
            Kind::ConnectionReset
            | Kind::ConnectionAborted
            | Kind::BrokenPipe
            | Kind::NotConnected
            | Kind::UnexpectedEof => NetError::Reset,
            Kind::AddrInUse => NetError::AddressInUse,
            Kind::PermissionDenied => NetError::PermissionDenied,
            _ => NetError::Other(
                i64::from(error.raw_os_error().unwrap_or(0)),
                error.to_string(),
            ),
        }
    }

    pub fn written(&self) -> Vec<u8> {
        match self {
            NetError::Rejected => vec![1],
            NetError::Unreachable => vec![2],
            NetError::TimedOut => vec![3],
            NetError::Reset => vec![4],
            NetError::NameNotFound => vec![5],
            NetError::AddressInUse => vec![6],
            NetError::PermissionDenied => vec![7],
            NetError::Injected => vec![8],
            NetError::Untrusted => vec![9],
            NetError::Handshake(reason) => [&[10u8][..], reason.as_bytes()].concat(),
            NetError::Other(code, text) => {
                [&[11u8][..], &parts(&[&int(*code), text.as_bytes()])].concat()
            }
        }
    }
}

pub fn result(outcome: Result<Vec<u8>, NetError>) -> Vec<u8> {
    match outcome {
        Ok(returned) => [&[1u8][..], &returned].concat(),
        Err(rejection) => [&[0u8][..], &rejection.written()].concat(),
    }
}

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

    pub fn socket_address(&self) -> Result<SocketAddr, NetError> {
        match (self.ip, &self.zone) {
            (IpAddr::V4(ip), _) => Ok(SocketAddr::V4(SocketAddrV4::new(ip, self.port))),
            (IpAddr::V6(ip), None) => Ok(SocketAddr::V6(SocketAddrV6::new(ip, self.port, 0, 0))),
            (IpAddr::V6(ip), Some(zone)) => match zone_index(zone) {
                Some(scope) => Ok(SocketAddr::V6(SocketAddrV6::new(ip, self.port, 0, scope))),
                None => Err(NetError::Other(
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

    pub fn read(flat: &[u8]) -> Option<Endpoint> {
        let [ip, port, zone] = parts_of(flat)?[..] else {
            return None;
        };
        let zone = std::str::from_utf8(zone).ok()?;
        Some(Endpoint {
            ip: ip_address_from_bytes(ip)?,
            port: u16::from_be_bytes(port.try_into().ok()?),
            zone: (!zone.is_empty()).then(|| zone.to_string()),
        })
    }
}

#[cfg(unix)]
fn zone_name(scope: u32) -> Option<String> {
    if scope == 0 {
        return None;
    }
    let mut name = [0 as libc::c_char; libc::IF_NAMESIZE];
    let found = unsafe { libc::if_indextoname(scope, name.as_mut_ptr()) };
    if found.is_null() {
        return Some(scope.to_string());
    }
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
    match unsafe { libc::if_nametoindex(name.as_ptr()) } {
        0 => None,
        scope => Some(scope),
    }
}

#[cfg(not(unix))]
pub fn zone_index(zone: &str) -> Option<u32> {
    zone.parse().ok()
}

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
    JoinGroup(IpAddr, Option<String>),
    LeaveGroup(IpAddr, Option<String>),
    Presenting(String),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Probing {
    pub idle: Duration,
    pub interval: Option<Duration>,
    pub count: Option<u32>,
}

impl SocketOption {
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
                ip_address_from_bytes(group)?,
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

    pub fn listed(options: &[SocketOption]) -> Vec<u8> {
        let written: Vec<Vec<u8>> = options.iter().map(SocketOption::written).collect();
        let each: Vec<&[u8]> = written.iter().map(Vec::as_slice).collect();
        parts(&each)
    }

    pub fn can_read_back(&self) -> bool {
        !matches!(
            self,
            SocketOption::JoinGroup(..)
                | SocketOption::LeaveGroup(..)
                | SocketOption::Presenting(_)
        )
    }
}

impl Probing {
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

fn nanos(d: Duration) -> [u8; 8] {
    int(i64::try_from(d.as_nanos()).unwrap_or(i64::MAX))
}

fn duration_of(bytes: &[u8]) -> Option<Duration> {
    u64::try_from(int_of(bytes)?).ok().map(Duration::from_nanos)
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Credentials {
    pub user: i64,
    pub group: i64,
    pub process: Option<i64>,
}

impl Credentials {
    pub fn written(&self) -> Vec<u8> {
        let process = self.process.map(int);
        parts(&[
            &int(self.user),
            &int(self.group),
            process.as_ref().map_or(&[][..], |n| &n[..]),
        ])
    }
}
