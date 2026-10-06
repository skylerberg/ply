//! The `dns` effect: the system's resolver, asked off the machine's thread. The resolver has its
//! own patience and cannot be interrupted, so a caller that stops waiting (`std.dns`'s `resolve`
//! cancels the task that asked) leaves the question on its thread until the system answers it.

use crate::pool::JobOutput;
use crate::tcp::TcpHost;
use crate::tcp::wire::{self, Endpoint};
use ply_eval::host::HostRegistry;
use ply_eval::{
    Determinism, Diagnostic, HostAnswer, HostHandler, HostOp, HostRequest, HostResource,
    HostRuntime, Linearity, Span, Symbol, Value, codes,
};
use std::net::IpAddr;
use std::sync::Arc;

pub const MODULE: &str = "std.dns";

pub const EFFECT: &str = "std.dns.dns";

/// Where the system's resolver keeps the servers it asks.
const RESOLVER_CONFIGURATION: &str = "/etc/resolv.conf";

/// The port a server named without one answers on.
const PORT: u16 = 53;

operations! {
    what "dns";
    path "dns";
    Lookup = "lookup" / 1,
    Reverse = "reverse" / 1,
    Servers = "servers" / 0,
}

/// What a name that is not ASCII is refused with: an internationalized name is looked up by its
/// A-labels, and which those are is not the resolver's to decide.
pub const NOT_ASCII: &str =
    "a name is ASCII: an internationalized name is looked up by its A-labels";

impl Op {
    pub fn declaration(self) -> HostOp {
        HostOp {
            effect: Symbol::new(EFFECT),
            op: Symbol::new(self.name()),
            resource: HostResource::Any,
            determinism: Determinism::Nondeterministic,
            // A question changes nothing it asks about.
            linearity: Linearity::Repeatable,
            blocking: matches!(self, Op::Lookup | Op::Reverse),
            secrets: false,
            path: self.path(),
        }
    }
}

/// A `std.dns.Failure` the system's resolver can answer with: a deadline is its caller's.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Failure {
    NoSuchName,
    NoData,
    ServerFailure,
    Other(i64, String),
}

impl Failure {
    fn value(&self) -> Value {
        let named = |name: &str, args: Vec<Value>| Value::ctor(format!("std.dns.{name}"), args);
        match self {
            Failure::NoSuchName => named("NoSuchName", Vec::new()),
            Failure::NoData => named("NoData", Vec::new()),
            Failure::ServerFailure => named("ServerFailure", Vec::new()),
            Failure::Other(code, text) => named("Other", vec![Value::Int(*code), Value::str(text)]),
        }
    }
}

fn failed(failure: &Failure) -> Value {
    Value::ctor("Err", vec![failure.value()])
}

/// A `std.dns.Found` of an address the system's resolver gave, which says nothing of how long it
/// holds.
fn found(address: IpAddr) -> Value {
    wire::record([("address", wire::ip_value(address)), ("ttl", wire::none())])
}

pub fn register(registry: &mut HostRegistry, net: Arc<TcpHost>) {
    for op in Op::ALL {
        registry.register(
            op.declaration(),
            Arc::new(Operation {
                op,
                net: Arc::clone(&net),
            }),
        );
    }
}

/// On the socket host's pool, so one runtime waits on a lookup as it waits on a connect.
struct Operation {
    op: Op,
    net: Arc<TcpHost>,
}

impl HostHandler for Operation {
    fn call(&self, _: &dyn HostRuntime, req: &HostRequest<'_>) -> Result<HostAnswer, Diagnostic> {
        let span = req.span;
        if req.args.len() != self.op.arity() {
            return Err(arity(self.op, req.args.len(), span));
        }
        match self.op {
            Op::Lookup => {
                let name = req.args[0].as_str(span, "a name")?.to_string();
                self.net.waiting(span, "lookup", self.op.what(), move || {
                    let answer = match name.parse::<IpAddr>() {
                        Ok(address) => Ok(vec![address]),
                        Err(_) if !name.is_ascii() => Err(Failure::Other(0, NOT_ASCII.to_string())),
                        Err(_) => system::addresses(&name),
                    };
                    JobOutput::Made(Box::new(move || match answer {
                        Ok(addresses) => {
                            wire::ok(Value::list(addresses.into_iter().map(found).collect()))
                        }
                        Err(failure) => failed(&failure),
                    }))
                })
            }
            Op::Reverse => {
                let address = wire::ip_of(&req.args[0], span)?;
                self.net.waiting(span, "reverse", self.op.what(), move || {
                    let answer = system::names(address);
                    JobOutput::Made(Box::new(move || match answer {
                        Ok(names) => {
                            wire::ok(Value::list(names.into_iter().map(Value::str).collect()))
                        }
                        Err(failure) => failed(&failure),
                    }))
                })
            }
            Op::Servers => Ok(HostAnswer::Value(Value::list(
                std::fs::read_to_string(RESOLVER_CONFIGURATION)
                    .map(|text| servers(&text))
                    .unwrap_or_default()
                    .iter()
                    .map(Endpoint::value)
                    .collect(),
            ))),
        }
    }
}

/// The servers a `resolv.conf` names, in its order, each at the port a server answers on.
pub fn servers(configuration: &str) -> Vec<Endpoint> {
    configuration
        .lines()
        .filter_map(|line| {
            let mut words = line.split_whitespace();
            (words.next() == Some("nameserver")).then(|| words.next())?
        })
        .filter_map(|server| {
            let (address, zone) = match server.split_once('%') {
                Some((address, zone)) => (address, Some(zone.to_string())),
                None => (server, None),
            };
            Some(Endpoint {
                ip: address.parse().ok()?,
                port: PORT,
                zone,
            })
        })
        .collect()
}

#[cold]
fn arity(op: Op, got: usize, span: Span) -> Diagnostic {
    Diagnostic::error(
        codes::INTERNAL_ERROR,
        format!(
            "{} was performed with {got} arguments and takes {}",
            op.what(),
            op.arity()
        ),
    )
    .primary(span, "this perform reached the host handler")
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
