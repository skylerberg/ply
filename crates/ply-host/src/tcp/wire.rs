//! The values `std.net`, `std.udp` and `std.dns` exchange with their handlers, each built and read
//! by the name its Ply declaration gives it.

use ply_eval::{Diagnostic, Fixed, IntTy, Span, Symbol, Value, codes};
use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4, SocketAddrV6};
use std::sync::Arc;
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

    pub fn value(&self) -> Value {
        let named = |name: &str, args: Vec<Value>| Value::ctor(format!("std.net.{name}"), args);
        match self {
            Refusal::Refused => named("Refused", Vec::new()),
            Refusal::Unreachable => named("Unreachable", Vec::new()),
            Refusal::TimedOut => named("TimedOut", Vec::new()),
            Refusal::Reset => named("Reset", Vec::new()),
            Refusal::NameNotFound => named("NameNotFound", Vec::new()),
            Refusal::AddressInUse => named("AddressInUse", Vec::new()),
            Refusal::PermissionDenied => named("PermissionDenied", Vec::new()),
            Refusal::Injected => named("Injected", Vec::new()),
            Refusal::Untrusted => named("Untrusted", Vec::new()),
            Refusal::Handshake(why) => named("Handshake", vec![Value::str(why)]),
            Refusal::Other(code, text) => named("Other", vec![Value::Int(*code), Value::str(text)]),
        }
    }
}

pub fn ok(value: Value) -> Value {
    Value::ctor("Ok", vec![value])
}

pub fn err(refusal: &Refusal) -> Value {
    Value::ctor("Err", vec![refusal.value()])
}

pub fn some(value: Value) -> Value {
    Value::ctor("Some", vec![value])
}

pub fn none() -> Value {
    Value::ctor("None", Vec::new())
}

pub fn option(value: Option<Value>) -> Value {
    value.map_or_else(none, some)
}

pub fn record<const N: usize>(fields: [(&str, Value); N]) -> Value {
    Value::Record(Arc::new(
        fields
            .into_iter()
            .map(|(name, value)| (Symbol::new(name), value))
            .collect(),
    ))
}

/// The prelude's `Duration`, which counts nanoseconds.
pub fn duration(d: Duration) -> Value {
    Value::ctor(
        "Duration",
        vec![Value::Int(i64::try_from(d.as_nanos()).unwrap_or(i64::MAX))],
    )
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

    pub fn value(&self) -> Value {
        Value::ctor(
            "std.ip.SocketAddress",
            vec![
                ip_value(self.ip),
                Value::Int(i64::from(self.port)),
                option(self.zone.as_deref().map(Value::str)),
            ],
        )
    }

    pub fn read(value: &Value, span: Span) -> Result<Endpoint, Diagnostic> {
        let args = fields_of(value, "std.ip.SocketAddress", 3)
            .ok_or_else(|| misshapen("a `std.ip.SocketAddress`", span))?;
        let port = u16::try_from(args[1].as_int(span, "a port")?)
            .map_err(|_| misshapen("a port", span))?;
        let zone = match maybe(&args[2]).ok_or_else(|| misshapen("a zone", span))? {
            Some(zone) => Some(zone.as_str(span, "a zone")?.to_string()),
            None => None,
        };
        Ok(Endpoint {
            ip: ip_of(&args[0], span)?,
            port,
            zone,
        })
    }
}

/// A `std.ip.Ip`.
pub fn ip_value(ip: IpAddr) -> Value {
    match ip {
        IpAddr::V4(four) => Value::ctor(
            "std.ip.V4",
            vec![Value::ctor(
                "std.ip.Ipv4",
                vec![Value::Int(i64::from(u32::from(four)))],
            )],
        ),
        IpAddr::V6(six) => Value::ctor(
            "std.ip.V6",
            vec![Value::ctor(
                "std.ip.Ipv6",
                vec![Value::Fixed(Fixed::new(IntTy::U128, u128::from(six)))],
            )],
        ),
    }
}

pub fn ip_of(value: &Value, span: Span) -> Result<IpAddr, Diagnostic> {
    let wrong = || misshapen("a `std.ip.Ip`", span);
    if let Some(four) = fields_of(value, "std.ip.V4", 1) {
        let bits = fields_of(&four[0], "std.ip.Ipv4", 1).ok_or_else(wrong)?[0]
            .as_int(span, "an IPv4 address")?;
        let bits = u32::try_from(bits).map_err(|_| wrong())?;
        return Ok(IpAddr::V4(Ipv4Addr::from(bits)));
    }
    let six = fields_of(value, "std.ip.V6", 1).ok_or_else(wrong)?;
    let bits = fields_of(&six[0], "std.ip.Ipv6", 1).ok_or_else(wrong)?[0]
        .as_fixed(span, "an IPv6 address")?;
    Ok(IpAddr::V6(Ipv6Addr::from(bits.bits())))
}

/// The arguments of a constructor named `name` that holds `arity` of them.
pub fn fields_of<'a>(value: &'a Value, name: &str, arity: usize) -> Option<&'a [Value]> {
    match value {
        Value::Ctor { name: found, args } if found.as_str() == name && args.len() == arity => {
            Some(args.as_slice())
        }
        _ => None,
    }
}

/// An `Option`'s content; `None` for a value that is no `Option`.
pub fn maybe(value: &Value) -> Option<Option<&Value>> {
    if let Some(inside) = fields_of(value, "Some", 1) {
        return Some(Some(&inside[0]));
    }
    fields_of(value, "None", 0).map(|_| None)
}

/// A `Duration` that is not negative.
pub fn duration_of(value: &Value, span: Span) -> Result<Duration, Diagnostic> {
    let nanos = fields_of(value, "Duration", 1).ok_or_else(|| misshapen("a `Duration`", span))?[0]
        .as_int(span, "a duration")?;
    u64::try_from(nanos).map(Duration::from_nanos).map_err(|_| {
        Diagnostic::error(
            codes::RUNTIME_ERROR,
            format!("a socket was given a duration of {nanos} nanoseconds"),
        )
        .primary(span, "a duration here is zero or more")
    })
}

pub fn strings_of(value: &Value, span: Span, what: &str) -> Result<Vec<String>, Diagnostic> {
    value
        .as_list(span, what)?
        .iter()
        .map(|item| item.as_str(span, what).map(str::to_string))
        .collect()
}

/// Inference checks an argument's type, so one of another shape was never checked.
#[cold]
pub fn misshapen(what: &str, span: Span) -> Diagnostic {
    Diagnostic::error(
        codes::INTERNAL_ERROR,
        format!("a host operation was handed something that is not {what}"),
    )
    .primary(span, "this perform reached the host handler")
    .note("the declaration and the handler disagree about a type's shape; this is Ply's fault")
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
    pub fn read(value: &Value, span: Span) -> Result<SocketOption, Diagnostic> {
        let wrong = || misshapen("a `std.net.SocketOption`", span);
        let Value::Ctor { name, args } = value else {
            return Err(wrong());
        };
        let group = |args: &[Value]| -> Result<(IpAddr, Option<String>), Diagnostic> {
            let [group, interface] = args else {
                return Err(wrong());
            };
            let interface = match maybe(interface).ok_or_else(wrong)? {
                Some(name) => Some(name.as_str(span, "an interface")?.to_string()),
                None => None,
            };
            Ok((ip_of(group, span)?, interface))
        };
        match name.as_str() {
            "std.net.JoinGroup" => {
                let (address, interface) = group(args)?;
                return Ok(SocketOption::JoinGroup(address, interface));
            }
            "std.net.LeaveGroup" => {
                let (address, interface) = group(args)?;
                return Ok(SocketOption::LeaveGroup(address, interface));
            }
            _ => {}
        }
        let [arg] = args.as_slice() else {
            return Err(wrong());
        };
        let size = |arg: &Value| -> Result<usize, Diagnostic> {
            let bytes = arg.as_int(span, "a buffer size")?;
            usize::try_from(bytes).map_err(|_| {
                Diagnostic::error(
                    codes::RUNTIME_ERROR,
                    format!("a socket buffer was sized at {bytes} bytes"),
                )
                .primary(span, "a buffer holds zero or more bytes")
            })
        };
        Ok(match name.as_str() {
            "std.net.NoDelay" => SocketOption::NoDelay(arg.as_bool(span, "a flag")?),
            "std.net.KeepAlive" => SocketOption::KeepAlive(match maybe(arg).ok_or_else(wrong)? {
                Some(probing) => Some(Probing::read(probing, span)?),
                None => None,
            }),
            "std.net.ReuseAddress" => SocketOption::ReuseAddress(arg.as_bool(span, "a flag")?),
            "std.net.ReusePort" => SocketOption::ReusePort(arg.as_bool(span, "a flag")?),
            "std.net.SendBuffer" => SocketOption::SendBuffer(size(arg)?),
            "std.net.ReceiveBuffer" => SocketOption::ReceiveBuffer(size(arg)?),
            "std.net.Linger" => SocketOption::Linger(match maybe(arg).ok_or_else(wrong)? {
                Some(d) => Some(duration_of(d, span)?),
                None => None,
            }),
            "std.net.Broadcast" => SocketOption::Broadcast(arg.as_bool(span, "a flag")?),
            "std.net.MulticastLoop" => SocketOption::MulticastLoop(arg.as_bool(span, "a flag")?),
            "std.net.Presenting" => {
                SocketOption::Presenting(arg.as_str(span, "a credential")?.to_string())
            }
            _ => return Err(wrong()),
        })
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

    pub fn list(value: &Value, span: Span) -> Result<Vec<SocketOption>, Diagnostic> {
        value
            .as_list(span, "socket options")?
            .iter()
            .map(|option| SocketOption::read(option, span))
            .collect()
    }

    pub fn value(&self) -> Value {
        let named = |name: &str, arg: Value| Value::ctor(format!("std.net.{name}"), vec![arg]);
        let size = |n: &usize| Value::Int(i64::try_from(*n).unwrap_or(i64::MAX));
        match self {
            SocketOption::NoDelay(on) => named("NoDelay", Value::Bool(*on)),
            SocketOption::KeepAlive(probing) => {
                named("KeepAlive", option(probing.map(|p| p.value())))
            }
            SocketOption::ReuseAddress(on) => named("ReuseAddress", Value::Bool(*on)),
            SocketOption::ReusePort(on) => named("ReusePort", Value::Bool(*on)),
            SocketOption::SendBuffer(n) => named("SendBuffer", size(n)),
            SocketOption::ReceiveBuffer(n) => named("ReceiveBuffer", size(n)),
            SocketOption::Linger(d) => named("Linger", option(d.map(duration))),
            SocketOption::Broadcast(on) => named("Broadcast", Value::Bool(*on)),
            SocketOption::MulticastLoop(on) => named("MulticastLoop", Value::Bool(*on)),
            SocketOption::JoinGroup(group, interface) => membership("JoinGroup", group, interface),
            SocketOption::LeaveGroup(group, interface) => {
                membership("LeaveGroup", group, interface)
            }
            SocketOption::Presenting(name) => named("Presenting", Value::str(name)),
        }
    }
}

fn membership(name: &str, group: &IpAddr, interface: &Option<String>) -> Value {
    Value::ctor(
        format!("std.net.{name}"),
        vec![
            ip_value(*group),
            option(interface.as_deref().map(Value::str)),
        ],
    )
}

impl Probing {
    fn read(value: &Value, span: Span) -> Result<Probing, Diagnostic> {
        let wrong = || misshapen("a `std.net.Probing`", span);
        let Value::Record(fields) = value else {
            return Err(wrong());
        };
        let field = |name: &str| fields.named(name).ok_or_else(wrong);
        let interval = match maybe(field("interval")?).ok_or_else(wrong)? {
            Some(d) => Some(duration_of(d, span)?),
            None => None,
        };
        let count = match maybe(field("count")?).ok_or_else(wrong)? {
            Some(n) => Some(
                u32::try_from(n.as_int(span, "a probe count")?)
                    .map_err(|_| misshapen("a probe count", span))?,
            ),
            None => None,
        };
        Ok(Probing {
            idle: duration_of(field("idle")?, span)?,
            interval,
            count,
        })
    }

    fn value(&self) -> Value {
        record([
            (
                "count",
                option(self.count.map(|n| Value::Int(i64::from(n)))),
            ),
            ("idle", duration(self.idle)),
            ("interval", option(self.interval.map(duration))),
        ])
    }
}

/// A `std.net.PeerCredentials`: who holds the other end of a Unix socket.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Credentials {
    pub user: i64,
    pub group: i64,
    pub process: Option<i64>,
}

impl Credentials {
    pub fn value(&self) -> Value {
        record([
            ("group", Value::Int(self.group)),
            ("process", option(self.process.map(Value::Int))),
            ("user", Value::Int(self.user)),
        ])
    }
}

/// A `std.net.Secured`.
pub fn secured(protocol: Option<String>) -> Value {
    record([("protocol", option(protocol.map(Value::str)))])
}
