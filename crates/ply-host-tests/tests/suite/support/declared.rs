//! The shipped effects the host's handlers bind to, declared by hand as `std` declares them, so a
//! binding is tested without compiling a program. That the host registers only what `std`
//! declares is the CLI suite's to hold, over the real declarations.

use ply_core::{EffectAtom, Mode, Resource, Symbol};
use ply_test_support::Sketch;

const NET: &[(&str, Mode, bool)] = &[
    ("listen", Mode::Write, true),
    ("listen_tls", Mode::Write, true),
    ("listen_on", Mode::Write, true),
    ("listen_unix", Mode::Write, true),
    ("connect", Mode::Write, true),
    ("connect_tls", Mode::Write, true),
    ("connect_to", Mode::Write, true),
    ("connect_unix", Mode::Write, true),
    ("handshake", Mode::Write, true),
    ("start_tls", Mode::Write, true),
    ("serve_tls", Mode::Write, true),
    ("accept", Mode::Write, true),
    ("recv", Mode::Write, true),
    ("send", Mode::Write, true),
    ("close_write", Mode::Write, true),
    ("close", Mode::Write, true),
    ("set_option", Mode::Write, true),
    ("local_port", Mode::Read, true),
    ("local_address", Mode::Read, true),
    ("peer_address", Mode::Read, true),
    ("peer_credentials", Mode::Read, true),
    ("peer_certificate", Mode::Read, true),
    ("protocol", Mode::Read, true),
    ("options", Mode::Read, true),
    ("send_secret", Mode::Write, true),
];

const UDP: &[(&str, Mode, bool)] = &[
    ("bind", Mode::Write, true),
    ("connect", Mode::Write, true),
    ("send_to", Mode::Write, true),
    ("send", Mode::Write, true),
    ("recv_from", Mode::Write, true),
    ("set_option", Mode::Write, true),
    ("close", Mode::Write, true),
    ("local_address", Mode::Read, true),
    ("options", Mode::Read, true),
];

const DNS: &[(&str, Mode, bool)] = &[
    ("lookup", Mode::Read, false),
    ("reverse", Mode::Read, false),
    ("servers", Mode::Read, false),
];

/// `sketch` with `std.net`, `std.udp` and `std.dns` declared.
pub fn shipped(sketch: Sketch) -> Sketch {
    sketch
        .effect("std.net", "net", true, NET)
        .effect("std.udp", "udp", true, UDP)
        .effect("std.dns", "dns", true, DNS)
}

/// The mode atom `effect.mode[label]`.
pub fn labelled(effect: &str, label: &str, mode: Mode) -> EffectAtom {
    EffectAtom::new(effect, Resource::Named(Symbol::new(label)), mode)
}

/// The operation atom `effect.op[label]`.
pub fn performed(effect: &str, label: &str, mode: Mode, op: &str) -> EffectAtom {
    EffectAtom::operation(effect, Resource::Named(Symbol::new(label)), mode, op)
}
