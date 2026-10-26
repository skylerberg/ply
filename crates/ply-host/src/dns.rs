//! The system's resolver, which `std.dns`'s real handler asks off the program's thread. The resolver
//! has its own patience and cannot be interrupted, so a caller that stops waiting (`std.dns`'s
//! `resolve` cancels the task that asked) leaves the question on its thread until the system
//! answers it.
//!
//! An answer is flat: byte 1 and then each address's bytes or each name as parts, or byte 0, the
//! failure's kind (`NoSuchName` 1, `NoData` 2, `ServerFailure` 3, `Other` 4), and for `Other` its
//! code and the resolver's words as parts.

use crate::native::{int, parts};
use std::net::IpAddr;

/// Where the system's resolver keeps the servers it asks.
pub const RESOLVER_CONFIGURATION: &str = "/etc/resolv.conf";

/// A `std.dns.Failure` the system's resolver can answer with: a deadline is its caller's.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Failure {
    NoSuchName,
    NoData,
    ServerFailure,
    Other(i64, String),
}

impl Failure {
    fn written(&self) -> Vec<u8> {
        match self {
            Failure::NoSuchName => vec![0, 1],
            Failure::NoData => vec![0, 2],
            Failure::ServerFailure => vec![0, 3],
            Failure::Other(code, text) => {
                [&[0u8, 4][..], &parts(&[&int(*code), text.as_bytes()])].concat()
            }
        }
    }
}

/// The addresses the system's resolver gives `name`, each once and in its order.
pub fn lookup(name: &str) -> Vec<u8> {
    match system::addresses(name) {
        Ok(found) => {
            let bytes: Vec<Vec<u8>> = found.iter().map(address_bytes).collect();
            let each: Vec<&[u8]> = bytes.iter().map(Vec::as_slice).collect();
            [&[1u8][..], &parts(&each)].concat()
        }
        Err(failure) => failure.written(),
    }
}

/// The names the system's resolver says `address` answers to.
pub fn reverse(address: IpAddr) -> Vec<u8> {
    match system::names(address) {
        Ok(names) => {
            let each: Vec<&[u8]> = names.iter().map(|n| n.as_bytes()).collect();
            [&[1u8][..], &parts(&each)].concat()
        }
        Err(failure) => failure.written(),
    }
}

/// The address four or sixteen bytes are, in network order.
pub fn address_of(bytes: &[u8]) -> Option<IpAddr> {
    match bytes.len() {
        4 => Some(IpAddr::from(<[u8; 4]>::try_from(bytes).ok()?)),
        16 => Some(IpAddr::from(<[u8; 16]>::try_from(bytes).ok()?)),
        _ => None,
    }
}

/// `address` in network order.
pub fn address_bytes(address: &IpAddr) -> Vec<u8> {
    match address {
        IpAddr::V4(four) => four.octets().to_vec(),
        IpAddr::V6(six) => six.octets().to_vec(),
    }
}

/// The system's resolver, through the C library, which is where its failures have codes.
#[cfg(unix)]
mod system {
    use super::Failure;
    use std::ffi::{CStr, CString};
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4, SocketAddrV6};

    fn failure(code: libc::c_int) -> Failure {
        match code {
            libc::EAI_NONAME => Failure::NoSuchName,
            #[cfg(any(target_os = "linux", target_os = "android", target_os = "macos"))]
            libc::EAI_NODATA => Failure::NoData,
            libc::EAI_AGAIN | libc::EAI_FAIL => Failure::ServerFailure,
            other => {
                // SAFETY: the answer is a static, terminated string for any code.
                let text = unsafe { CStr::from_ptr(libc::gai_strerror(other)) };
                Failure::Other(i64::from(other), text.to_string_lossy().into_owned())
            }
        }
    }

    /// Each address of `name` once, in the resolver's order.
    pub fn addresses(name: &str) -> Result<Vec<IpAddr>, Failure> {
        let Ok(name) = CString::new(name) else {
            return Err(Failure::NoSuchName);
        };
        // SAFETY: zeroed is the documented way to start a hints structure.
        let mut hints: libc::addrinfo = unsafe { std::mem::zeroed() };
        hints.ai_family = libc::AF_UNSPEC;
        // One socket type, or every address comes back once for each.
        hints.ai_socktype = libc::SOCK_STREAM;
        let mut list: *mut libc::addrinfo = std::ptr::null_mut();
        // SAFETY: `name` is terminated, `hints` is initialised, and `list` receives the answer.
        let code = unsafe {
            libc::getaddrinfo(
                name.as_ptr(),
                std::ptr::null(),
                &raw const hints,
                &raw mut list,
            )
        };
        if code != 0 {
            return Err(failure(code));
        }
        let mut found = Vec::new();
        let mut entry = list;
        while !entry.is_null() {
            // SAFETY: `entry` is a node of the list the call answered, valid until it is freed.
            let info = unsafe { &*entry };
            let address = match info.ai_family {
                libc::AF_INET => {
                    // SAFETY: an `AF_INET` entry's address is a `sockaddr_in`.
                    let four = unsafe { &*info.ai_addr.cast::<libc::sockaddr_in>() };
                    Some(IpAddr::V4(Ipv4Addr::from(u32::from_be(
                        four.sin_addr.s_addr,
                    ))))
                }
                libc::AF_INET6 => {
                    // SAFETY: an `AF_INET6` entry's address is a `sockaddr_in6`.
                    let six = unsafe { &*info.ai_addr.cast::<libc::sockaddr_in6>() };
                    Some(IpAddr::V6(Ipv6Addr::from(six.sin6_addr.s6_addr)))
                }
                _ => None,
            };
            if let Some(address) = address
                && !found.contains(&address)
            {
                found.push(address);
            }
            entry = info.ai_next;
        }
        // SAFETY: `list` is what the call answered, freed once.
        unsafe { libc::freeaddrinfo(list) };
        if found.is_empty() {
            return Err(Failure::NoData);
        }
        Ok(found)
    }

    /// The name `address` answers to; an address with none is `NoSuchName`.
    pub fn names(address: IpAddr) -> Result<Vec<String>, Failure> {
        let socket = socket2::SockAddr::from(match address {
            IpAddr::V4(four) => SocketAddr::V4(SocketAddrV4::new(four, 0)),
            IpAddr::V6(six) => SocketAddr::V6(SocketAddrV6::new(six, 0, 0, 0)),
        });
        let mut host = [0 as libc::c_char; libc::NI_MAXHOST as usize];
        // SAFETY: `socket` is an address of its own length, and `host` the buffer of the length
        // given; no service name is asked for.
        let code = unsafe {
            libc::getnameinfo(
                socket.as_ptr().cast(),
                socket.len(),
                host.as_mut_ptr(),
                host.len() as libc::socklen_t,
                std::ptr::null_mut(),
                0,
                libc::NI_NAMEREQD,
            )
        };
        if code != 0 {
            return Err(failure(code));
        }
        // SAFETY: a successful call left a terminated name in the buffer.
        let name = unsafe { CStr::from_ptr(host.as_ptr()) };
        Ok(vec![name.to_string_lossy().into_owned()])
    }
}

/// Without the C library's resolver there are no codes, so a failure is only what it says.
#[cfg(not(unix))]
mod system {
    use super::Failure;
    use std::net::{IpAddr, ToSocketAddrs};

    pub fn addresses(name: &str) -> Result<Vec<IpAddr>, Failure> {
        let found: Vec<IpAddr> = (name, 0)
            .to_socket_addrs()
            .map_err(|e| Failure::Other(0, e.to_string()))?
            .map(|a| a.ip())
            .collect();
        if found.is_empty() {
            return Err(Failure::NoData);
        }
        Ok(found)
    }

    pub fn names(_: IpAddr) -> Result<Vec<String>, Failure> {
        Err(Failure::Other(
            0,
            "this platform's resolver answers no reverse lookup".to_string(),
        ))
    }
}
