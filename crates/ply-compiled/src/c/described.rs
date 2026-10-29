//! What this runtime says of itself to a compiler that emits code for it, in the form
//! `compiler.emit.abi.runtime_of` reads: what a unit emitted against it is a function of, each
//! native and helper it binds with what its function's type takes and answers, and where it lays
//! out what emitted code reads in place, each as a named integer.

use super::prelude::{machine_natives, own_helpers};
use crate::blocks::{self, Filling, Page, Segment};
use crate::emitted::{PlyFnDesc, PlyKept, PlyOp, PlyType};
use crate::frames::PlyFrame;
use crate::heap::{self, Alloc, Obj};
use crate::native::{Machine, Signature};
use crate::rt::Ctx;
use std::fmt::Write;
use std::mem::{offset_of, size_of};

/// Each native compiled code calls, under the name the compiler declares it by.
pub fn natives() -> impl Iterator<Item = (&'static str, Signature)> {
    machine_natives().iter().map(|n| (n.name, n.signature))
}

/// The size of the field `field` borrows out of an `S`.
fn field_bytes<S, F>(_field: fn(&S) -> &F) -> usize {
    size_of::<F>()
}

/// Each named field of `$s` as `$name.field.at` and `$name.field.bytes`, then `$name.bytes`.
macro_rules! laid {
    ($out:ident, $s:ty as $name:literal: $($f:ident),* $(,)?) => {
        $(
            $out.push((concat!($name, ".", stringify!($f), ".at"), offset_of!($s, $f)));
            $out.push((
                concat!($name, ".", stringify!($f), ".bytes"),
                field_bytes::<$s, _>(|s| &s.$f),
            ));
        )*
        $out.push((concat!($name, ".bytes"), size_of::<$s>()));
    };
}

/// Where this runtime lays out what emitted code reads and writes in place, and the constants both
/// read, each under the name `compiler.emit.abi.facts` gives it. Of the context, only the fields
/// emitted code reads: a field it reads anew is named here.
fn facts() -> Vec<(&'static str, usize)> {
    let mut out = Vec::new();
    // Only the released compiler's code reads `fuel`.
    laid!(out, Ctx as "PlyCtx": failed, fuel, stack_floor, site_root, site_start, site_end,
        frames, release_depth, release_waiting, target, entering_desc, entering_words,
        recording, bridge, hosted, unserved, performed_root,
        performed_start, performed_end, task, alloc);
    laid!(out, Obj as "PlyObj": rc, made_by_scheduler);
    laid!(out, Page as "PlyPage": free, fresh, end, owner, handing, next, prev, block, live, class,
        listed, spare);
    laid!(out, Filling as "PlyFilling": pages, name, reused);
    laid!(out, Alloc as "PlyAlloc": blocks, count, slow);
    laid!(out, PlyFrame as "PlyFrame": prev, each);
    laid!(out, PlyType as "PlyType": identity, spelled, spelled_len, inc, dec, lower, lift);
    laid!(out, PlyKept as "PlyKept": ty, equal, order);
    laid!(out, PlyOp as "PlyOp": effect, op, answer, own, params_len, params, vars_len, vars);
    laid!(out, PlyFnDesc as "PlyFnDesc": uniform, typed, convention, release, share, keep);
    out.extend([
        ("alloc.segment", blocks::SEGMENT),
        ("alloc.page", blocks::PAGE),
        ("alloc.pages_at", offset_of!(Segment, pages)),
        ("alloc.word_classes", blocks::WORD_CLASSES),
        ("alloc.link_at", blocks::LINK_AT),
        ("count.frame", heap::FRAME as usize),
        ("count.immortal", heap::IMMORTAL as usize),
        ("count.shared", heap::SHARED as usize),
        ("pointer_floor", heap::POINTER_FLOOR as usize),
        ("release_depth", crate::rt::RELEASE_DEPTH as usize),
        ("hold.held", crate::rt::HOLD_HELD),
        ("closure.descriptor", heap::CLOSURE_CODE),
        ("failed.target", crate::rt::FAILED_TARGET as usize),
        ("failed.pending", crate::rt::FAILED_PENDING as usize),
        ("unwinding.words", crate::rt::UNWINDING_WORDS),
        ("instance.head", crate::detached::INSTANCE_HEAD),
        ("instance.detached", crate::detached::INSTANCE_DETACHED),
        ("instance.vars", crate::detached::INSTANCE_VARS),
        ("record.closure", crate::detached::RECORD_CLOSURE),
        ("record.instance", crate::detached::RECORD_INSTANCE),
        ("record.string", crate::detached::RECORD_STRING),
    ]);
    out
}

fn function_line(out: &mut String, kind: &str, name: &str, s: Signature) {
    let answer = s.answer.map_or("-", Machine::name);
    let _ = write!(out, "{kind} {name} {} {answer}", u8::from(s.raises));
    for p in s.params {
        let _ = write!(out, " {}", p.name());
    }
    out.push('\n');
}

/// This runtime, described: a line naming the form, then its identity, each native, each helper
/// of its own and each fact, a line each, its words apart by one space.
pub fn described() -> &'static str {
    static TEXT: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    TEXT.get_or_init(|| {
        let mut out = String::from("ply runtime 1\n");
        let _ = writeln!(out, "identity {}", super::runtime_identity());
        for (name, s) in natives() {
            function_line(&mut out, "native", name, s);
        }
        for h in own_helpers() {
            function_line(&mut out, "helper", h.name, h.signature);
        }
        for (name, value) in facts() {
            let _ = writeln!(out, "fact {name} {value}");
        }
        out
    })
}
