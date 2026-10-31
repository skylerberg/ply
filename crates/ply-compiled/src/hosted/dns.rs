//! `std.dns`'s natives: the system's resolver, asked off the program's thread. A question changes
//! nothing it asks about, so each reads.

use super::{Nondeterministic, asked};
use crate::c::{MachineNative, machine_native};
use crate::native::{Addr, I64, TakenBytes};
use crate::rt::{Ctx, borrowed};
use ply_core::{Diagnostic, codes};
use ply_host::jobs::Work;
use ply_host::native::found;

pub(super) fn natives() -> Vec<MachineNative> {
    vec![
        machine_native(
            "dns_lookup",
            rt_dns_lookup as unsafe extern "C" fn(_, _, _) -> _,
        ),
        machine_native(
            "dns_reverse",
            rt_dns_reverse as unsafe extern "C" fn(_, _, _) -> _,
        ),
        machine_native(
            "dns_configuration",
            rt_dns_configuration as unsafe extern "C" fn(_) -> _,
        ),
    ]
}

/// The ticket of the addresses the system's resolver gives the name at `p`.
pub unsafe extern "C" fn rt_dns_lookup(ctx: *mut Ctx, p: Addr, n: I64) -> I64 {
    asked(
        ctx,
        "dns_lookup",
        Nondeterministic,
        I64(0),
        |c: &mut Ctx| {
            let Some(name) = c.text(unsafe { borrowed(p, n) }) else {
                return I64(0);
            };
            let name = name.to_string();
            c.submitted("dns_lookup", "`dns.lookup`", Work::Waiting, move || {
                Ok(ply_host::dns::lookup(&name))
            })
        },
    )
}

/// The ticket of the names the system's resolver says the address at `p` answers to.
pub unsafe extern "C" fn rt_dns_reverse(ctx: *mut Ctx, p: Addr, n: I64) -> I64 {
    asked(
        ctx,
        "dns_reverse",
        Nondeterministic,
        I64(0),
        |c: &mut Ctx| {
            let Some(address) = ply_host::dns::address_of(unsafe { borrowed(p, n) }) else {
                c.fail(Diagnostic::error(
                    codes::INTERNAL_ERROR,
                    "`dns.reverse` was handed bytes that are no address",
                ));
                return I64(0);
            };
            c.submitted("dns_reverse", "`dns.reverse`", Work::Waiting, move || {
                Ok(ply_host::dns::reverse(address))
            })
        },
    )
}

/// The system's resolver configuration, found where the host has one to read.
pub unsafe extern "C" fn rt_dns_configuration(ctx: *mut Ctx) -> TakenBytes {
    asked(
        ctx,
        "dns_configuration",
        Nondeterministic,
        TakenBytes(0),
        |c: &mut Ctx| {
            let text = std::fs::read(ply_host::dns::RESOLVER_CONFIGURATION).ok();
            c.given(found(text))
        },
    )
}
