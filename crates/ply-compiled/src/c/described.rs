use super::prelude::{runtime_helpers, runtime_natives};
use crate::blocks::{self, ActivePages, Page, Segment};
use crate::emitted::{PlyFunctionDescriptor, PlyKept, PlyOp, PlyType};
use crate::frames::PlyFrame;
use crate::heap::{self, Alloc, Object};
use crate::native::{MachineType, Signature};
use crate::rt::Context;
use std::fmt::Write;
use std::mem::{offset_of, size_of};

pub fn natives() -> impl Iterator<Item = (&'static str, Signature)> {
    runtime_natives().iter().map(|n| (n.name, n.signature))
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
    laid!(out, Context as "PlyCtx": failed, stack_floor, site_root, site_start, site_end,
        frames, release_depth, release_waiting, target, entering_descriptor, entering_words,
        recording, bridge, hosted, unserved, performed_root,
        performed_start, performed_end, task, alloc, constants);
    laid!(out, Object as "PlyObj": rc, made_by_scheduler);
    laid!(out, Page as "PlyPage": free, fresh, end, owner, remote_free_state, next, previous, block, live, class,
        listed, spare);
    laid!(out, ActivePages as "PlyFilling": pages, name, reused);
    laid!(out, Alloc as "PlyAlloc": blocks, count, slow);
    laid!(out, PlyFrame as "PlyFrame": previous, each);
    laid!(out, PlyType as "PlyType": identity, spelled, spelled_len, inc, dec, lower, lift);
    laid!(out, PlyKept as "PlyKept": ty, equal, order);
    laid!(out, PlyOp as "PlyOp": effect, op, answer, own, params_len, params, variables_length, variables);
    laid!(out, PlyFunctionDescriptor as "PlyFnDesc": uniform, typed, convention, release, share, keep);
    out.extend([
        ("alloc.segment", blocks::SEGMENT),
        ("alloc.page", blocks::PAGE),
        ("alloc.pages_at", offset_of!(Segment, pages)),
        ("alloc.word_classes", blocks::WORD_STEP_SIZE_CLASSES),
        ("alloc.link_at", blocks::LINK_OFFSET),
        ("count.frame", heap::FRAME as usize),
        ("count.immortal", heap::IMMORTAL as usize),
        ("count.shared", heap::SHARED as usize),
        ("pointer_floor", heap::POINTER_FLOOR as usize),
        ("release_depth", crate::rt::RELEASE_DEPTH as usize),
        ("hold.held", crate::rt::SCOPED_VALUE),
        ("closure.descriptor", heap::CLOSURE_CODE),
        (
            "failed.target",
            crate::rt::FAILED_UNWOUND_TO_HANDLER as usize,
        ),
        ("evidence.one_slot", crate::rt::EVIDENCE_ONE_SLOT as usize),
        ("evidence.shared", crate::rt::EVIDENCE_SHARED as usize),
        ("failed.pending", crate::rt::FAILED_PENDING_RAISE as usize),
        ("unwinding.words", crate::rt::UNWINDING_WORDS),
        ("instance.head", crate::detached::INSTANCE_HEADER_WORDS),
        ("instance.detached", crate::detached::INSTANCE_DETACHED),
        ("instance.variables", crate::detached::INSTANCE_VARIABLES),
        ("record.closure", crate::detached::RECORD_CLOSURE),
        ("record.instance", crate::detached::RECORD_INSTANCE),
        ("record.string", crate::detached::RECORD_STRING),
    ]);
    out
}

fn function_line(out: &mut String, kind: &str, name: &str, s: Signature) {
    let result = s.result.map_or("-", MachineType::name);
    let _ = write!(out, "{kind} {name} 0 {result}");
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
        for h in runtime_helpers() {
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
