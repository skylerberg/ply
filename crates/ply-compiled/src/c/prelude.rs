use crate::emitted::{Block, DropRoot, Handed, Handle, PlyOp, PlyType};
use crate::heap::Word;
use crate::native::{Addr, F64, I64, Native, Signature};
use crate::rt::Ctx;

#[derive(Clone, Copy)]
pub struct Helper {
    pub name: &'static str,
    pub signature: Signature,
    pub address: *const (),
}

unsafe impl Send for Helper {}
unsafe impl Sync for Helper {}

impl Helper {
    fn of<F: Native>(name: &'static str, f: F) -> Helper {
        Helper {
            name,
            signature: F::SIGNATURE,
            address: f.address(),
        }
    }

    pub fn args(&self) -> usize {
        self.signature.words()
    }

    pub fn results(&self) -> bool {
        self.signature.results()
    }
}

pub struct MachineNative {
    pub name: &'static str,
    pub signature: Signature,
    pub helper: Helper,
}

macro_rules! machine {
    ($($name:literal => $f:ident($($t:ty),*) $(-> $r:ty)?;)*) => {
        fn runtime_native_table() -> Vec<MachineNative> {
            vec![$({
                let f: unsafe extern "C" fn(*mut Ctx $(, $t)*) $(-> $r)? = crate::rt::$f;
                let helper = Box::leak(builtin_helper_name($name).into_boxed_str());
                MachineNative {
                    name: $name,
                    signature: signature_of(f),
                    helper: Helper::of(helper, f),
                }
            }),*]
        }
    };
}

fn signature_of<F: Native>(_: F) -> Signature {
    F::SIGNATURE
}

pub(crate) fn runtime_native<F: Native>(name: &'static str, f: F) -> MachineNative {
    let helper = Box::leak(builtin_helper_name(name).into_boxed_str());
    MachineNative {
        name,
        signature: F::SIGNATURE,
        helper: Helper::of(helper, f),
    }
}

machine! {
    "?out_tag" => rt_out_tag(I64);
    "?out_int" => rt_out_int(I64);
    "?out_float" => rt_out_float(F64);
    "?out_decimal" => rt_out_decimal(I64, I64);
    "?out_fixed" => rt_out_fixed(I64, I64, I64);
    "?out_char" => rt_out_char(I64);
    "?out_str" => rt_out_str(Addr, I64);
    "?out_bytes" => rt_out_bytes(Addr, I64);
    "?out_count" => rt_out_count(I64, I64);
    "?out_ctor" => rt_out_ctor(Addr, I64, I64);
    "?out_name" => rt_out_name(Addr, I64);
    "?out_handle" => rt_out_handle(Handle);
    "?out_carried" => rt_out_carried(Handed, *const PlyType);
    "?out_fun" => rt_out_fun(Handed, Addr, I64);
    "?out_again" => rt_out_again(Handed) -> I64;
    "?in_tag" => rt_in_tag() -> I64;
    "?in_int" => rt_in_int() -> I64;
    "?in_float" => rt_in_float() -> F64;
    "?in_decimal" => rt_in_decimal() -> I64;
    "?in_fixed" => rt_in_fixed(I64) -> I64;
    "?in_high" => rt_in_high() -> I64;
    "?in_char" => rt_in_char() -> I64;
    "?in_str" => rt_in_str() -> I64;
    "?in_bytes" => rt_in_bytes() -> I64;
    "?in_count" => rt_in_count(I64) -> I64;
    "?in_field" => rt_in_field(Addr, I64);
    "?in_name_is" => rt_in_name_is(Addr, I64) -> I64;
    "?shown_open" => rt_shown_open();
    "?lower_close" => rt_lower_close() -> I64;
    "?lift_close" => rt_lift_close();
    "?taken" => rt_taken(Addr, I64);
    "?hold_made" => rt_hold_made(Handed) -> Handle;
    "?hold_read" => rt_hold_read(Handle) -> Handed;
    "?hold_taken" => rt_hold_taken(Handle) -> Handed;
}

pub fn runtime_natives() -> &'static [MachineNative] {
    static ALL: std::sync::OnceLock<Vec<MachineNative>> = std::sync::OnceLock::new();
    ALL.get_or_init(|| {
        let mut all = runtime_native_table();
        all.extend(crate::machine::natives());
        all.extend(crate::secret::natives());
        all.extend(crate::hosted::natives());
        all.extend(crate::entry::natives());
        all.extend(crate::tasks::natives());
        all
    })
}

macro_rules! taken {
    ($p:ident) => {
        Word
    };
    ($p:ident: $t:ty) => {
        $t
    };
}

macro_rules! helpers {
    ($($f:ident($($p:ident $(: $t:ty)?),*) $(-> $r:ty)?;)*) => {
        fn own() -> Vec<Helper> {
            vec![$({
                let f: unsafe extern "C" fn(*mut Ctx $(, taken!($p $(: $t)?))*) $(-> $r)? =
                    crate::rt::$f;
                Helper::of(stringify!($f), f)
            }),*]
        }
    };
}

helpers! {
    rt_stack_more() -> Word;
    rt_inc_shared(w);
    rt_note_update(in_place, copied);
    rt_failed(raises: I64, texts: *const Word, notes: I64, shown: I64);

    rt_alloc_bytes(bytes) -> Block;
    rt_alloc_kept(bytes) -> Block;
    rt_free(o: Block, bytes);
    rt_dec_shared_last(o: Block) -> Word;
    rt_handle_free(h: Block);
    rt_released_twice(o: Block);
    rt_release_later(w: Handed, root: DropRoot);
    rt_release_drain();
    rt_performed(op: *const PlyOp, resource: I64, mode: I64);
    rt_scheduler_entered() -> Word;
    rt_scheduler_left(was);
    rt_pending_raised_at(effect: I64, op: I64) -> Handed;
    rt_label_string(resource: I64, string: *const PlyType) -> Handed;
    rt_task_abandons(op: *const PlyOp) -> Word;
    rt_task_captures(op: *const PlyOp) -> Word;
    rt_task_raises(op: *const PlyOp, args: *const Handed) -> Word;
    rt_detached_open(body: Block, forwards: Block, instance: Block) -> Handed;
    rt_detached_perform(
        record: *const Word,
        op: *const PlyOp,
        resource: I64,
        args: *const Handed
    ) -> Handed;
    rt_detached_stop(
        record: *const Word,
        op: *const PlyOp,
        resource: I64,
        args: *const Handed,
        again: I64
    ) -> Handed;
    rt_pending_raised(string: *const PlyType, bridged: I64) -> Handed;
    rt_with_cleanup(
        acquire: Block,
        body: Block,
        cleanup: Block,
        held: *const PlyType,
        result: *const PlyType
    ) -> Handed;
    rt_unwind_hold(held) -> Word;
    rt_unwind_resume(held);
    rt_branches(slots: *const Handed, n: I64);
    rt_keep_constant(index: I64, kept);
    rt_const_failed();
    rt_never_lifted();
    rt_never_shown();
}

macro_rules! never_called {
    ($($name:literal($($t:ty),*) $(-> $r:ty)?;)*) => {
        fn never_called() -> Vec<Helper> {
            vec![$({
                unsafe extern "C" fn never(_: *mut Ctx $(, _: $t)*) $(-> $r)? {
                    unreachable!(concat!($name, " is bound and never called"))
                }
                let f: unsafe extern "C" fn(*mut Ctx $(, $t)*) $(-> $r)? = never;
                Helper::of($name, f)
            }),*]
        }
    };
}

never_called! {
    "builtin_elaborated_refused"(I64, I64, I64, I64);
    "builtin_elaborated_refused_text"(I64, Addr, I64, Addr, I64);
    "builtin_elaborated_assert_failed"(Addr, I64, Addr, I64);
    "builtin_elaborated_scalar_encoded"(I64, I64, I64, I64) -> I64;
    "builtin_elaborated_text_encoded"(I64, Addr, I64) -> I64;
    "builtin_elaborated_functions_compared"() -> I64;
}

pub fn builtin_helper_name(builtin: &str) -> String {
    match (builtin.strip_prefix('?'), builtin.split_once("::")) {
        (Some(written), _) => format!("builtin_elaborated_{written}"),
        (None, Some((of, name))) => format!("builtin_{}_{name}", of.to_ascii_lowercase()),
        (None, None) => format!("builtin_{builtin}"),
    }
}

pub fn helpers() -> &'static [Helper] {
    static ALL: std::sync::OnceLock<Vec<Helper>> = std::sync::OnceLock::new();
    ALL.get_or_init(|| {
        let machine = runtime_natives().iter().map(|n| n.helper);
        own()
            .into_iter()
            .chain(never_called())
            .chain(machine)
            .collect()
    })
}

pub fn runtime_helpers() -> &'static [Helper] {
    let all = helpers();
    &all[..all.len() - runtime_natives().len() - never_called().len()]
}
