use crate::native::{int, parts};
use std::net::IpAddr;

pub const RESOLVER_CONFIGURATION: &str = "/etc/resolv.conf";

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

pub fn reverse(address: IpAddr) -> Vec<u8> {
    match system::names(address) {
        Ok(names) => {
            let each: Vec<&[u8]> = names.iter().map(|n| n.as_bytes()).collect();
            [&[1u8][..], &parts(&each)].concat()
        }
        Err(failure) => failure.written(),
    }
}

pub fn ip_address_from_bytes(bytes: &[u8]) -> Option<IpAddr> {
    match bytes.len() {
        4 => Some(IpAddr::from(<[u8; 4]>::try_from(bytes).ok()?)),
        16 => Some(IpAddr::from(<[u8; 16]>::try_from(bytes).ok()?)),
        _ => None,
    }
}

pub fn address_bytes(address: &IpAddr) -> Vec<u8> {
    match address {
        IpAddr::V4(four) => four.octets().to_vec(),
        IpAddr::V6(six) => six.octets().to_vec(),
    }
}

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
                let text = unsafe { CStr::from_ptr(libc::gai_strerror(other)) };
                Failure::Other(i64::from(other), text.to_string_lossy().into_owned())
            }
        }
    }

    pub fn addresses(name: &str) -> Result<Vec<IpAddr>, Failure> {
        let Ok(name) = CString::new(name) else {
            return Err(Failure::NoSuchName);
        };
        let mut hints: libc::addrinfo = unsafe { std::mem::zeroed() };
        hints.ai_family = libc::AF_UNSPEC;
        hints.ai_socktype = libc::SOCK_STREAM;
        let mut list: *mut libc::addrinfo = std::ptr::null_mut();
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
            let info = unsafe { &*entry };
            let address = match info.ai_family {
                libc::AF_INET => {
                    let four = unsafe { &*info.ai_addr.cast::<libc::sockaddr_in>() };
                    Some(IpAddr::V4(Ipv4Addr::from(u32::from_be(
                        four.sin_addr.s_addr,
                    ))))
                }
                libc::AF_INET6 => {
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
        unsafe { libc::freeaddrinfo(list) };
        if found.is_empty() {
            return Err(Failure::NoData);
        }
        Ok(found)
    }

    pub fn names(address: IpAddr) -> Result<Vec<String>, Failure> {
        let socket = socket2::SockAddr::from(match address {
            IpAddr::V4(four) => SocketAddr::V4(SocketAddrV4::new(four, 0)),
            IpAddr::V6(six) => SocketAddr::V6(SocketAddrV6::new(six, 0, 0, 0)),
        });
        let mut host = [0 as libc::c_char; libc::NI_MAXHOST as usize];
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
        let name = unsafe { CStr::from_ptr(host.as_ptr()) };
        Ok(vec![name.to_string_lossy().into_owned()])
    }
}

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
            "this platform's resolver does no reverse lookup".to_string(),
        ))
    }
}
