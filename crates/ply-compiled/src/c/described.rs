use super::prelude::{machine_natives, own_helpers};
use crate::blocks::{self, Filling, Page, Segment};
use crate::emitted::{PlyFnDesc, PlyKept, PlyOp, PlyType};
use crate::frames::PlyFrame;
use crate::heap::{self, Alloc, Obj};
use crate::native::{Machine, Signature};
use crate::rt::Ctx;
use std::fmt::Write;
use std::mem::{offset_of, size_of};

pub fn natives() -> impl Iterator<Item = (&'static str, Signature)> {
    machine_natives().iter().map(|n| (n.name, n.signature))
}

fn field_bytes<S, F>(_field: fn(&S) -> &F) -> usize {
    size_of::<F>()
}

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

fn facts() -> Vec<(&'static str, usize)> {
    let mut out = Vec::new();
    laid!(out, Ctx as "PlyCtx": failed, stack_floor, site_root, site_start, site_end,
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
        ("hold.held", crate::rt::SCOPED_VALUE),
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
    let result = s.result.map_or("-", Machine::name);
    let _ = write!(out, "{kind} {name} {} {result}", u8::from(s.raises));
    for p in s.params {
        let _ = write!(out, " {}", p.name());
    }
    out.push('\n');
}

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
        for code in ply_core::codes::ALL {
            let _ = writeln!(out, "code {code}");
        }
        out
    })
}
