//! The fragment of the spike's fragment, compiled with Cranelift.

use crate::heap::{HEADER, Heap, KIND_BYTES, KIND_LIST, KIND_RECORD, Layouts, Word};
use crate::rt::{self, Ctx, Tables};
use crate::source::Source;
use anyhow::{Result, anyhow};
use cranelift_codegen::Context as ClifContext;
use cranelift_codegen::ir::condcodes::IntCC;
use cranelift_codegen::ir::{
    AbiParam, Block, BlockArg, InstBuilder, MemFlags, Signature, StackSlotData, StackSlotKind,
    types,
};
use cranelift_codegen::settings::{self, Configurable};
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext};
use cranelift_jit::{JITBuilder, JITModule};
use cranelift_module::{FuncId, Linkage, Module, default_libcall_names};
use ply_eval::code::{Arm, Pat, Stmt, lower_fn};
use ply_eval::rc::Own;
use ply_eval::{Builtin, Code, NodeKind, Value};
use ply_span::Symbol;
use ply_syntax::ast::{BinOp, IntTy, Lit, QName, UnOp};
use ply_syntax::resolve::Namespace;
use std::cell::RefCell;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::rc::Rc;

/// A compiled function: `extern "C" fn(ctx, args) -> handle`.
pub type Entry = unsafe extern "C" fn(*mut Ctx, *const i64) -> i64;

/// What the fragment refused, and where.
#[derive(Debug)]
pub struct Refused {
    pub function: String,
    pub construct: String,
}

impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "`{}` is outside the compiled fragment: {}",
            self.function, self.construct
        )
    }
}

impl std::error::Error for Refused {}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Kind {
    Int,
    Bool,
    Boxed,
    /// A fixed-width integer, held in a Cranelift register of its own width — which is the whole
    /// point of the family (ADR 0039): at `U32` an add is `add w`, a rotate is one instruction,
    /// and there is nothing to mask.
    ///
    /// Only the six widths below sixty-four appear. A value of one of those always fits the
    /// sixty-three bits an immediate carries, so boxing one is a shift and an or with no branch
    /// and no allocator, and unboxing needs no tag test. `U64` and `I64` do not have that
    /// property and are left outside the fragment ([`carried_width`]).
    Num(IntTy),
}

/// Whether the fragment carries values of this width, and the reason it does not carry the other
/// two: a `U64` past `2^62` does not fit an immediate, so it would need a heap object of its own
/// kind and an unboxing that tests for it — the cost the family exists to remove.
fn carried_width(t: IntTy) -> bool {
    t.bits() < 64
}

#[derive(Clone, Copy)]
struct Val {
    kind: Kind,
    v: cranelift_codegen::ir::Value,
    /// What the value is known to be at compile time, by index into `Jit::tys`; `0` is unknown.
    ty: u32,
    /// The binding this value lives in, as one past its index in `Fx::homes`; `0` for a temporary.
    home: u32,
}

struct Helpers {
    box_int: FuncId,
    unbox_int: FuncId,
    unbox_bool: FuncId,
    arith: FuncId,
    overflow: FuncId,
    not_that_width: FuncId,
    no_match: FuncId,
    lit: FuncId,
    equal: FuncId,
    concat: FuncId,
    record_fits: FuncId,
    record_has: FuncId,
    builtin: FuncId,
    list: FuncId,
    list_fits: FuncId,
    list_at: FuncId,
    list_rest: FuncId,
    ctor_arg: FuncId,
    map_lookup: FuncId,
    list_index: FuncId,
    list_lookup: FuncId,
    push: FuncId,
    map_insert: FuncId,
    map_contains: FuncId,
    map_get: FuncId,
    compare: FuncId,
    byte_of_int: FuncId,
    bytes_scan: FuncId,
    bytes_scan_until: FuncId,
    bytes_slice: FuncId,
    bytes_concat: FuncId,
    record_update: FuncId,
    field: FuncId,
    no_fuel: FuncId,
    closure: FuncId,
    builtin_value: FuncId,
    ctor_value: FuncId,
    call: FuncId,
    map: FuncId,
    filter: FuncId,
    fold: FuncId,
    map_fold: FuncId,
    iterate: FuncId,
    iterate_bad: FuncId,
    bad_range: FuncId,
    bytes_join: FuncId,
    list_get: FuncId,
    list_push: FuncId,
    not_a_list: FuncId,
    shift_count: FuncId,
    dup: FuncId,
    dec: FuncId,
    reset: FuncId,
    alloc: FuncId,
    constant: FuncId,
}

/// A lambda met while lowering a body: declared where it was met, so the closure that names it
/// can be built, and defined after its owner so one `FunctionBuilder` is live at a time.
struct Pending {
    /// The top-level function the lambda is inside, which a refusal in its body is charged to.
    owner: String,
    id: FuncId,
    /// The captured names first, then the lambda's own parameters.
    params: Vec<Symbol>,
    body: Code,
    module_index: usize,
}

/// What a compiled function takes and answers in registers: the kind of each parameter and of
/// the result, read off the checker's scheme. `Boxed` wherever the type is one the fragment keeps
/// as a handle, a type variable included. Beside each kind, what the checker knows of the type,
/// by index into [`Jit::tys`].
#[derive(Clone, Debug)]
struct Sig {
    params: Vec<Kind>,
    ret: Kind,
    param_tys: Vec<u32>,
    ret_ty: u32,
    /// Per parameter, whether the body only reads it — every mention the base of a field read,
    /// and no literal of its width that would take its memory — so the caller keeps its hold and
    /// the callee neither counts it nor lets it go: no increment and release around the call,
    /// and a record that dies in the caller dies where the caller's tokens are.
    borrowed: Vec<bool>,
}

/// What the code generator knows of a value's type at compile time: enough to read a record's
/// field at its offset rather than by name. A record type is its fields sorted, each with its own
/// type, which is the order the shape lays them out in.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Ty {
    Unknown,
    Int,
    Bool,
    Num(IntTy),
    Record(Vec<(Symbol, u32)>),
}

/// A fused `iterate` loop's blocks and values, for the step's answer to continue or leave it.
#[derive(Clone, Copy)]
struct Loop {
    go: u32,
    stop: u32,
    header: Block,
    exit: Block,
    left: cranelift_codegen::ir::Value,
    state: cranelift_codegen::ir::Value,
}

/// A compiled top-level function: the typed body other compiled code calls directly, and the
/// entry the seam and a closure reach it through, over the handle ABI.
#[derive(Clone)]
struct Func {
    typed: FuncId,
    entry: FuncId,
    arity: usize,
    module_index: usize,
    sig: Sig,
}

/// The Cranelift type a width is held in. Cranelift has exactly `I8 I16 I32 I64 I128` and puts
/// signedness in the *operation* rather than the type, which is why `U32` and `I32` share one here
/// and differ only in which instruction is selected.
/// The Cranelift type a value of this kind is held in. Every join carrying a value of a computed
/// kind must give its block parameter *this* type: a `Kind::Num` is narrower than a word, and a
/// block parameter that says `I64` where the branches pass an `I32` is an instruction the
/// assembler encodes and the processor refuses.
fn clif_kind(k: Kind) -> types::Type {
    match k {
        Kind::Num(t) => clif_int(t),
        _ => types::I64,
    }
}

fn clif_int(t: IntTy) -> types::Type {
    match t.bits() {
        8 => types::I8,
        16 => types::I16,
        32 => types::I32,
        _ => types::I64,
    }
}

fn kind_of_type(t: &ply_core::ty::Type) -> Kind {
    match t {
        ply_core::ty::Type::Con(name, args) if args.is_empty() => match name.as_str() {
            "Int" => Kind::Int,
            "Bool" => Kind::Bool,
            other => match IntTy::from_name(other) {
                Some(t) if carried_width(t) => Kind::Num(t),
                _ => Kind::Boxed,
            },
        },
        _ => Kind::Boxed,
    }
}

/// The signature the checker published for `name`, or all-boxed for one it did not.
fn sig_of(jit: &mut Jit, loaded: &Source, name: &str, arity: usize) -> Sig {
    use ply_core::ty::Type;
    let boxed = Sig {
        params: vec![Kind::Boxed; arity],
        ret: Kind::Boxed,
        param_tys: vec![0; arity],
        ret_ty: 0,
        borrowed: vec![false; arity],
    };
    let Some(def) = loaded.check.defs.get(&Symbol::new(name)) else {
        return boxed;
    };
    match &def.scheme.ty {
        Type::Fn { params, ret, .. } if params.len() == arity => Sig {
            params: params.iter().map(kind_of_type).collect(),
            ret: kind_of_type(ret),
            param_tys: params.iter().map(|p| jit.ty_of_type(p)).collect(),
            ret_ty: jit.ty_of_type(ret),
            borrowed: vec![false; arity],
        },
        ty if arity == 0 => Sig {
            params: Vec::new(),
            ret: kind_of_type(ty),
            param_tys: Vec::new(),
            ret_ty: jit.ty_of_type(ty),
            borrowed: Vec::new(),
        },
        _ => boxed,
    }
}

/// One compiled program, and the tables its runtime context needs.
pub struct Unit {
    module: JITModule,
    entries: HashMap<String, (FuncId, usize)>,
    /// The pure nullary roots, by the index the runtime's memo is keyed on, so the seam can
    /// remember their answers as compiled code does.
    constants: HashMap<String, usize>,
    tables: Rc<Tables>,
    pub nodes: HashMap<String, usize>,
    /// Nanoseconds spent in `cranelift`, from the first declaration to `finalize_definitions`.
    pub compile_nanos: u128,
}

impl Unit {
    pub fn entry(&self, name: &str) -> Option<Entry> {
        let (id, _) = self.entries.get(name)?;
        let ptr = self.module.get_finalized_function(*id);
        Some(unsafe { std::mem::transmute::<*const u8, Entry>(ptr) })
    }

    /// The memo index of a pure nullary root, if `name` is one.
    pub fn constant_index(&self, name: &str) -> Option<usize> {
        self.constants.get(name).copied()
    }

    pub fn arity(&self, name: &str) -> Option<usize> {
        self.entries.get(name).map(|(_, a)| *a)
    }

    pub fn tables(&self) -> &Rc<Tables> {
        &self.tables
    }

    /// A context wired to this program's tables.
    pub fn context(&self) -> Ctx {
        Ctx::new(self.tables.clone())
    }
}

/// What the compiler is allowed to do beyond lowering the fragment.
#[derive(Clone, Copy)]
pub struct Opts {
    /// Whether a `Str`, `Bytes` or nullary-constructor literal becomes a constant in the code
    /// object.
    pub fold_literals: bool,
}

impl Default for Opts {
    fn default() -> Opts {
        Opts {
            fold_literals: true,
        }
    }
}

pub struct Jit {
    module: JITModule,
    opts: Opts,
    consts: Vec<Value>,
    /// The constants as immortal words, one per entry of `consts`, built as they are met.
    const_words: Vec<Word>,
    layouts: Layouts,
    /// Owns the constant pool's objects until the tables take it over.
    immortals: Heap,
    /// Per constructor index, the immortal singleton a nullary one is, or `0`; and the empty
    /// list and map, each made once — none of these is ever allocated by a body.
    nullaries: Vec<Word>,
    empty_list: Word,
    empty_map: Word,
    /// Every compile-time type a value has been given, interned; index `0` is unknown.
    tys: Vec<Ty>,
    ty_ids: HashMap<Ty, u32>,
    fields: Vec<Symbol>,
    builtins: Vec<Builtin>,
    funcs: HashMap<String, Func>,
    /// The nullary functions whose published row is pure: called through `rt_constant`, which
    /// remembers their value the way the machine's memo does.
    constants: HashSet<FuncId>,
    helpers: Helpers,
    nodes: HashMap<String, usize>,
    /// Every function a closure may name, by the index `rt_closure` is handed.
    functions: Vec<FuncId>,
    pending: Vec<Pending>,
}

fn helper_sig(module: &JITModule, params: usize, returns: bool) -> Signature {
    let mut sig = module.make_signature();
    for _ in 0..params {
        sig.params.push(AbiParam::new(types::I64));
    }
    if returns {
        sig.returns.push(AbiParam::new(types::I64));
    }
    sig
}

/// A lowered body, held between declaration and definition.
type Body = (String, Vec<Symbol>, Code, usize);

impl Jit {
    /// Compiles `names` as one unit: a call between two of them is a direct call, and a call to
    /// anything else refuses the caller.
    pub fn compile(loaded: &'static Source, names: &[&str]) -> Result<Unit> {
        Jit::compile_with(loaded, names, Opts::default())
    }

    pub fn compile_with(loaded: &'static Source, names: &[&str], opts: Opts) -> Result<Unit> {
        let (mut jit, bodies, started) = Jit::prepare(loaded, names, opts)?;
        let mut clif = ClifContext::new();
        let mut fctx = FunctionBuilderContext::new();
        for (name, params, body, module_index) in &bodies {
            jit.nodes.insert(name.clone(), count_nodes(body));
            clif.clear();
            let func = jit.funcs[name].clone();
            clif.func.signature = jit.typed_signature(func.arity);
            jit.define(
                &mut clif,
                &mut fctx,
                loaded,
                name,
                params,
                body,
                *module_index,
                Some(&func.sig),
            )?;
            // `PLY_CODEGEN_ASM=<program-wide name>` prints what one definition compiles to, which
            // is the only way to see the costs the source does not show: spills, rematerialised
            // constants, and the untagging a field read expands into. `benches/value-model` used
            // it to find that the integer kernel's gap is register pressure rather than the
            // arithmetic, after two cheaper instruments said nothing.
            let asm = std::env::var("PLY_CODEGEN_ASM").is_ok_and(|want| want == *name);
            if asm {
                clif.set_disasm(true);
            }
            jit.module.define_function(func.typed, &mut clif)?;
            if asm && let Some(compiled) = clif.compiled_code() {
                eprintln!(
                    "compiled `{name}`: {} bytes\n{}",
                    compiled.code_buffer().len(),
                    compiled.vcode.as_deref().unwrap_or("<no listing>")
                );
            }
            clif.clear();
            clif.func.signature = jit.entry_signature();
            jit.define_entry(&mut clif, &mut fctx, loaded, &func);
            jit.module.define_function(func.entry, &mut clif)?;
            while let Some(lambda) = jit.pending.pop() {
                clif.clear();
                clif.func.signature = jit.entry_signature();
                jit.define(
                    &mut clif,
                    &mut fctx,
                    loaded,
                    &lambda.owner,
                    &lambda.params,
                    &lambda.body,
                    lambda.module_index,
                    None,
                )?;
                jit.module.define_function(lambda.id, &mut clif)?;
            }
        }
        jit.module.finalize_definitions()?;
        let compile_nanos = started.elapsed().as_nanos();
        let functions = jit
            .functions
            .iter()
            .map(|id| jit.module.get_finalized_function(*id) as usize)
            .collect();

        let entries = jit
            .funcs
            .iter()
            .map(|(name, f)| (name.clone(), (f.entry, f.arity)))
            .collect();
        let constants = jit
            .funcs
            .iter()
            .filter(|(_, f)| jit.constants.contains(&f.typed))
            .filter_map(|(name, f)| {
                let index = jit.functions.iter().position(|id| *id == f.entry)?;
                Some((name.clone(), index))
            })
            .collect();
        Ok(Unit {
            module: jit.module,
            entries,
            constants,
            tables: Rc::new(Tables {
                consts: jit.consts,
                const_words: jit.const_words,
                layouts: jit.layouts,
                fields: jit.fields,
                builtins: jit.builtins,
                functions,
                memo: RefCell::new(Vec::new()),
                immortals: RefCell::new(jit.immortals),
                bytes: RefCell::new([0; 256]),
                nullaries: jit.nullaries,
                empty_list: jit.empty_list,
                empty_map: jit.empty_map,
                memo_values: RefCell::new(HashMap::new()),
                memo_words: RefCell::new(HashMap::new()),
                calls: RefCell::new(HashMap::new()),
            }),
            nodes: jit.nodes,
            compile_nanos,
        })
    }

    /// Which of `name`'s handle parameters its body only reads: every mention the base of a field
    /// read, and none captured by a lambda.
    ///
    /// A parameter whose width the body also *builds* would rather stay owned, so that a record
    /// dying here donates its memory to the literal instead of the allocator — but **only one
    /// donor per width is any use**, since there is one token slot for each. Keeping every such
    /// parameter owned charges the rest a duplicate in the caller and a release here for a
    /// donation that can never happen: `round(s, m)` in `std.hash` has one state record that dies
    /// at the call and one message record the caller needs again, and the second was paying the
    /// first's price. The first of each width stays owned; the rest borrow.
    fn borrowed_params(&self, name: &str, params: &[Symbol], code: &Code) -> Vec<bool> {
        let Some(func) = self.funcs.get(name) else {
            return vec![false; params.len()];
        };
        let mut widths = BTreeSet::new();
        record_widths(code, &mut widths);
        let mut donor_taken: BTreeSet<usize> = BTreeSet::new();
        params
            .iter()
            .enumerate()
            .map(|(i, p)| {
                if func.sig.params[i] != Kind::Boxed {
                    return false;
                }
                let width = match &self.tys[func.sig.param_tys[i] as usize] {
                    Ty::Record(fields) => Some(fields.len()),
                    _ => None,
                };
                if let Some(w) = width
                    && widths.contains(&w)
                    && donor_taken.insert(w)
                {
                    return false;
                }
                let mut only_read = true;
                read_only(code, p, &mut only_read);
                only_read
            })
            .collect()
    }

    /// The values a body builds without allocating: every nullary constructor, the empty list
    /// and the empty map, each made once and immortal.
    fn make_singletons(&mut self) {
        let mut nullaries = Vec::with_capacity(self.layouts.ctors.len());
        for (index, (_, arity)) in self.layouts.ctors.iter().enumerate() {
            let w = if *arity == 0 {
                let w = self
                    .immortals
                    .alloc(crate::heap::KIND_CTOR, 0, 0, index as u32)
                    as Word;
                crate::heap::mark_immortal(w);
                w
            } else {
                0
            };
            nullaries.push(w);
        }
        self.nullaries = nullaries;
        let list = self.immortals.list_from(&[]);
        crate::heap::mark_immortal(list);
        self.empty_list = list;
        let map = self.immortals.map_new();
        crate::heap::mark_immortal(map);
        self.empty_map = map;
    }

    /// Every refusal `names` produces when they are offered **as one unit**, rather than the first
    /// one.
    pub fn refusals(loaded: &'static Source, names: &[&str], opts: Opts) -> Result<Vec<Refused>> {
        let (mut jit, bodies, _) = Jit::prepare(loaded, names, opts)?;
        let mut out = Vec::new();
        for (name, params, body, module_index) in &bodies {
            // A fresh context per function: `FunctionBuilder::finalize` is what returns a
            // `FunctionBuilderContext` to a reusable state, and a refused body never reaches it.
            let mut clif = ClifContext::new();
            let mut fctx = FunctionBuilderContext::new();
            let func = jit.funcs[name].clone();
            clif.func.signature = jit.typed_signature(func.arity);
            if let Err(e) = jit.define(
                &mut clif,
                &mut fctx,
                loaded,
                name,
                params,
                body,
                *module_index,
                Some(&func.sig),
            ) {
                // A refusal is the answer this is asking for; anything else — a cranelift failure,
                // a name that does not resolve — is a bug in the spike and must not be read as
                // "outside the fragment".
                let Some(refused) = e.downcast_ref::<Refused>() else {
                    return Err(e);
                };
                out.push(Refused {
                    function: refused.function.clone(),
                    construct: refused.construct.clone(),
                });
                jit.pending.clear();
                continue;
            }
            while let Some(lambda) = jit.pending.pop() {
                let mut clif = ClifContext::new();
                let mut fctx = FunctionBuilderContext::new();
                clif.func.signature = jit.entry_signature();
                if let Err(e) = jit.define(
                    &mut clif,
                    &mut fctx,
                    loaded,
                    &lambda.owner,
                    &lambda.params,
                    &lambda.body,
                    lambda.module_index,
                    None,
                ) {
                    let Some(refused) = e.downcast_ref::<Refused>() else {
                        return Err(e);
                    };
                    out.push(Refused {
                        function: refused.function.clone(),
                        construct: refused.construct.clone(),
                    });
                    jit.pending.clear();
                    break;
                }
            }
        }
        Ok(out)
    }

    /// `(ctx, args) -> handle`, the one shape every compiled function has.
    fn entry_signature(&self) -> Signature {
        let mut sig = self.module.make_signature();
        sig.params.push(AbiParam::new(types::I64));
        sig.params.push(AbiParam::new(types::I64));
        sig.returns.push(AbiParam::new(types::I64));
        sig
    }

    /// The index of a compile-time type, interned.
    fn ty_id(&mut self, ty: Ty) -> u32 {
        if let Some(id) = self.ty_ids.get(&ty) {
            return *id;
        }
        let id = self.tys.len() as u32;
        self.tys.push(ty.clone());
        self.ty_ids.insert(ty, id);
        id
    }

    /// What the code generator keeps of a type the checker published: scalars, and a closed
    /// record with its fields' types; a type variable, a row or anything else is unknown.
    fn ty_of_type(&mut self, t: &ply_core::ty::Type) -> u32 {
        use ply_core::ty::Type;
        let ty = match t {
            Type::Con(name, args) if args.is_empty() => match name.as_str() {
                "Int" => Ty::Int,
                "Bool" => Ty::Bool,
                other => match IntTy::from_name(other) {
                    Some(t) if carried_width(t) => Ty::Num(t),
                    _ => Ty::Unknown,
                },
            },
            Type::Record(fields) => {
                let fields: Vec<(Symbol, u32)> = fields
                    .iter()
                    .map(|(name, t)| (name.clone(), self.ty_of_type(t)))
                    .collect();
                Ty::Record(fields)
            }
            _ => Ty::Unknown,
        };
        self.ty_id(ty)
    }

    /// `(ctx, p1, .., pn) -> r`: every parameter and the result in a register, each an `Int` or a
    /// `Bool` as itself or anything else as its handle, which [`Sig`] says per position.
    fn typed_signature(&self, arity: usize) -> Signature {
        let mut sig = self.module.make_signature();
        for _ in 0..=arity {
            sig.params.push(AbiParam::new(types::I64));
        }
        sig.returns.push(AbiParam::new(types::I64));
        sig
    }

    /// Everything both drivers do before a body is lowered to instructions: build the module,
    /// register the runtime symbols, declare every function so that a call between two of them
    /// resolves, and lower each body.
    fn prepare(
        loaded: &'static Source,
        names: &[&str],
        opts: Opts,
    ) -> Result<(Jit, Vec<Body>, std::time::Instant)> {
        let mut flags = settings::builder();
        flags.set("use_colocated_libcalls", "false")?;
        flags.set("is_pic", "false")?;
        flags.set("opt_level", "speed")?;
        // The verifier checks the IR this generator emits, which the test suites run in debug
        // builds; a release binary compiles a unit for its speed.
        flags.set(
            "enable_verifier",
            if cfg!(debug_assertions) {
                "true"
            } else {
                "false"
            },
        )?;
        let isa = cranelift_native::builder()
            .map_err(|e| anyhow!("this host has no Cranelift backend: {e}"))?
            .finish(settings::Flags::new(flags))?;
        let mut builder = JITBuilder::with_isa(isa, default_libcall_names());
        for (name, ptr) in rt::symbols() {
            builder.symbol(name, ptr);
        }
        let mut module = JITModule::new(builder);

        let started = std::time::Instant::now();
        let helpers = Helpers {
            box_int: declare(&mut module, "rt_box_int", 2, true)?,
            unbox_int: declare(&mut module, "rt_unbox_int", 2, true)?,
            unbox_bool: declare(&mut module, "rt_unbox_bool", 2, true)?,
            arith: declare(&mut module, "rt_arith", 4, true)?,
            overflow: declare(&mut module, "rt_overflow", 2, false)?,
            not_that_width: declare(&mut module, "rt_not_that_width", 3, false)?,
            no_match: declare(&mut module, "rt_no_match", 1, false)?,
            lit: declare(&mut module, "rt_lit", 2, true)?,
            equal: declare(&mut module, "rt_equal", 3, true)?,
            concat: declare(&mut module, "rt_concat", 3, true)?,
            record_fits: declare(&mut module, "rt_record_fits", 4, true)?,
            record_has: declare(&mut module, "rt_record_has", 3, true)?,
            builtin: declare(&mut module, "rt_builtin", 4, true)?,
            list: declare(&mut module, "rt_list", 3, true)?,
            list_fits: declare(&mut module, "rt_list_fits", 4, true)?,
            list_at: declare(&mut module, "rt_list_at", 3, true)?,
            list_rest: declare(&mut module, "rt_list_rest", 3, true)?,
            ctor_arg: declare(&mut module, "rt_ctor_arg", 4, true)?,
            map_lookup: declare(&mut module, "rt_map_lookup", 3, true)?,
            list_index: declare(&mut module, "rt_list_index", 3, true)?,
            list_lookup: declare(&mut module, "rt_list_lookup", 3, true)?,
            push: declare(&mut module, "rt_push", 3, true)?,
            map_insert: declare(&mut module, "rt_map_insert", 4, true)?,
            map_contains: declare(&mut module, "rt_map_contains", 3, true)?,
            map_get: declare(&mut module, "rt_map_get", 3, true)?,
            compare: declare(&mut module, "rt_compare", 3, true)?,
            byte_of_int: declare(&mut module, "rt_byte_of_int", 2, true)?,
            bytes_scan: declare(&mut module, "rt_bytes_scan", 5, true)?,
            bytes_scan_until: declare(&mut module, "rt_bytes_scan_until", 5, true)?,
            bytes_slice: declare(&mut module, "rt_bytes_slice", 4, true)?,
            bytes_concat: declare(&mut module, "rt_bytes_concat", 3, true)?,
            record_update: declare(&mut module, "rt_record_update", 6, true)?,
            field: declare(&mut module, "rt_field", 4, true)?,
            no_fuel: declare(&mut module, "rt_no_fuel", 1, false)?,
            closure: declare(&mut module, "rt_closure", 5, true)?,
            builtin_value: declare(&mut module, "rt_builtin_value", 2, true)?,
            ctor_value: declare(&mut module, "rt_ctor_value", 2, true)?,
            call: declare(&mut module, "rt_call", 4, true)?,
            map: declare(&mut module, "rt_map", 3, true)?,
            filter: declare(&mut module, "rt_filter", 3, true)?,
            fold: declare(&mut module, "rt_fold", 4, true)?,
            map_fold: declare(&mut module, "rt_map_fold", 4, true)?,
            iterate: declare(&mut module, "rt_iterate", 4, true)?,
            iterate_bad: declare(&mut module, "rt_iterate_bad", 3, false)?,
            bad_range: declare(&mut module, "rt_bad_range", 3, false)?,
            bytes_join: declare(&mut module, "rt_bytes_join", 3, true)?,
            list_get: declare(&mut module, "rt_list_get", 3, true)?,
            list_push: declare(&mut module, "rt_list_push", 3, true)?,
            not_a_list: declare(&mut module, "rt_not_a_list", 3, false)?,
            shift_count: declare(&mut module, "rt_shift_count", 2, false)?,
            dup: declare(&mut module, "rt_dup", 2, true)?,
            dec: declare(&mut module, "rt_dec", 2, false)?,
            reset: declare(&mut module, "rt_reset", 2, true)?,
            alloc: declare(&mut module, "rt_alloc", 5, true)?,
            constant: declare(&mut module, "rt_constant", 2, true)?,
        };

        let mut jit = Jit {
            module,
            opts,
            consts: Vec::new(),
            const_words: Vec::new(),
            layouts: Layouts::new(loaded.ctors()),
            immortals: Heap::persistent(),
            nullaries: Vec::new(),
            empty_list: 0,
            empty_map: 0,
            tys: vec![Ty::Unknown],
            ty_ids: HashMap::from([(Ty::Unknown, 0)]),
            fields: Vec::new(),
            builtins: Vec::new(),
            funcs: HashMap::new(),
            constants: HashSet::new(),
            helpers,
            nodes: HashMap::new(),
            functions: Vec::new(),
            pending: Vec::new(),
        };
        jit.make_singletons();

        let mut bodies = Vec::new();
        for name in names {
            let (def, module_index) = loaded
                .definition(name)
                .ok_or_else(|| anyhow!("no definition named `{name}`"))?;
            let arity = def.params.len();
            let typed_sig = jit.typed_signature(arity);
            let typed = jit
                .module
                .declare_function(&mangle(name), Linkage::Local, &typed_sig)?;
            let entry_sig = jit.entry_signature();
            let entry = jit.module.declare_function(
                &mangle(&format!("{name}$entry")),
                Linkage::Export,
                &entry_sig,
            )?;
            let sig = sig_of(&mut jit, loaded, name, arity);
            jit.funcs.insert(
                (*name).to_string(),
                Func {
                    typed,
                    entry,
                    arity,
                    module_index,
                    sig,
                },
            );
            jit.functions.push(entry);
            if arity == 0
                && ply_eval::memo::pure_by_published_row(Some(loaded.check), &Symbol::new(name))
            {
                jit.constants.insert(typed);
            }
            let params: Vec<Symbol> = def.params.iter().map(|p| p.name.name.clone()).collect();
            // With the parameters as the window's leading slots, so a lambda that reads one
            // captures it rather than reading a global that does not exist.
            let body =
                crate::opt::optimize(loaded, module_index, def, crate::opt::Inlining::IN_PROCESS);
            let code = lower_fn(&params, &body).code;
            let borrowed = jit.borrowed_params(name, &params, &code);
            if let Some(func) = jit.funcs.get_mut(*name) {
                func.sig.borrowed = borrowed;
            }
            bodies.push(((*name).to_string(), params, code, module_index));
        }
        Ok((jit, bodies, started))
    }

    #[allow(clippy::too_many_arguments)]
    fn define(
        &mut self,
        clif: &mut ClifContext,
        fctx: &mut FunctionBuilderContext,
        loaded: &'static Source,
        name: &str,
        params: &[Symbol],
        body: &Code,
        module_index: usize,
        sig: Option<&Sig>,
    ) -> Result<()> {
        let mut builder = FunctionBuilder::new(&mut clif.func, fctx);
        let entry = builder.create_block();
        builder.append_block_params_for_function_params(entry);
        builder.switch_to_block(entry);
        builder.seal_block(entry);
        let block_params: Vec<cranelift_codegen::ir::Value> = builder.block_params(entry).to_vec();
        let ctx_ptr = block_params[0];

        let failure = builder.create_block();

        let mut fx = Fx {
            jit: self,
            builder,
            loaded,
            ctx: ctx_ptr,
            failure,
            function: name.to_string(),
            module_index,
            homes: Vec::new(),
            tokens: Vec::new(),
            pinned: Vec::new(),
            kinds: HashMap::new(),
        };
        fx.arm_tokens(body);

        // The prologue `ply_eval::limit` needs and the fragment's gaps item 6 records as missing: one
        // nested call is spent here and given back on the normal return, so a compiled recursion is
        // bounded by the same number the machine bounds an interpreted one by.
        let fuel = fx.load_fuel();
        let spent = fx.builder.ins().iadd_imm(fuel, -1);
        let go = fx.builder.create_block();
        let exhausted = fx.builder.create_block();
        let none_left = fx.builder.ins().icmp_imm(IntCC::SignedLessThan, spent, 0);
        fx.builder.ins().brif(none_left, exhausted, &[], go, &[]);
        fx.builder.switch_to_block(exhausted);
        fx.builder.seal_block(exhausted);
        fx.helper_void(fx.jit.helpers.no_fuel, &[]);
        fx.builder.ins().jump(failure, &[]);
        fx.builder.switch_to_block(go);
        fx.builder.seal_block(go);
        fx.store_fuel(spent);

        let mut scope = Vec::new();
        match sig {
            // A typed body: each parameter is the register it arrived in, of its own kind.
            Some(sig) => {
                for (i, (p, kind)) in params.iter().zip(&sig.params).enumerate() {
                    // Every parameter arrives in a sixty-four-bit register whatever it means, so a
                    // fixed-width one is narrowed here and widened again at each call: the typed
                    // ABI carries the *value*, extended, not the tagged word.
                    let v = match kind {
                        Kind::Num(t) => fx.builder.ins().ireduce(clif_int(*t), block_params[i + 1]),
                        _ => block_params[i + 1],
                    };
                    let val = Val {
                        kind: *kind,
                        v,
                        ty: sig.param_tys[i],
                        home: 0,
                    };
                    // A borrowed parameter is the caller's: read here, never moved or let go.
                    let v = if sig.borrowed[i] {
                        fx.pinned.push(p.clone());
                        val
                    } else {
                        fx.home(val)
                    };
                    scope.push((p.clone(), v));
                }
            }
            // A lambda: its captures and parameters arrive as handles through the array.
            None => {
                let args_ptr = block_params[1];
                for (i, p) in params.iter().enumerate() {
                    let handle = fx.builder.ins().load(
                        types::I64,
                        MemFlags::trusted(),
                        args_ptr,
                        (i * 8) as i32,
                    );
                    let v = fx.home(Val {
                        kind: Kind::Boxed,
                        v: handle,
                        ty: 0,
                        home: 0,
                    });
                    scope.push((p.clone(), v));
                }
            }
        }

        let result = fx.consumed(body, &mut scope)?;
        let answer = fx.abi_out(result, sig.map_or(Kind::Boxed, |s| s.ret));
        fx.release_tokens();
        fx.release_homes_from(0);
        // The only path that gives the nested call back.
        let left = fx.load_fuel();
        let restored = fx.builder.ins().iadd_imm(left, 1);
        fx.store_fuel(restored);
        fx.builder.ins().return_(&[answer]);

        fx.builder.switch_to_block(failure);
        let zero = fx.builder.ins().iconst(types::I64, 0);
        fx.builder.ins().return_(&[zero]);

        fx.builder.seal_all_blocks();
        fx.builder.finalize();
        Ok(())
    }

    /// The handle-ABI entry of a typed function: each parameter unboxed to the register kind the
    /// body takes, the body called, and its answer boxed for whoever entered.
    fn define_entry(
        &mut self,
        clif: &mut ClifContext,
        fctx: &mut FunctionBuilderContext,
        loaded: &'static Source,
        func: &Func,
    ) {
        let mut builder = FunctionBuilder::new(&mut clif.func, fctx);
        let entry = builder.create_block();
        builder.append_block_params_for_function_params(entry);
        builder.switch_to_block(entry);
        builder.seal_block(entry);
        let ctx = builder.block_params(entry)[0];
        let args_ptr = builder.block_params(entry)[1];
        let failure = builder.create_block();
        let mut fx = Fx {
            jit: self,
            builder,
            loaded,
            ctx,
            failure,
            function: String::new(),
            module_index: func.module_index,
            homes: Vec::new(),
            tokens: Vec::new(),
            pinned: Vec::new(),
            kinds: HashMap::new(),
        };
        let mut args = vec![fx.ctx];
        let mut lent = Vec::new();
        for (i, kind) in func.sig.params.iter().enumerate() {
            let handle =
                fx.builder
                    .ins()
                    .load(types::I64, MemFlags::trusted(), args_ptr, (i * 8) as i32);
            // The entry owns every handle it is given; one the body borrows is let go here.
            if func.sig.borrowed[i] {
                lent.push(handle);
            }
            // `abi_out`, not `coerce`: the typed body takes every position in a sixty-four-bit
            // register, so a fixed-width one travels extended. Nothing the seam admits has a
            // width in its signature (`Compiled::carries` refuses it), but a compiled closure
            // reaches its neighbour's entry and this path has to be right for that.
            let v = fx.abi_out(
                Val {
                    kind: Kind::Boxed,
                    v: handle,
                    ty: 0,
                    home: 0,
                },
                *kind,
            );
            args.push(v);
        }
        let callee = fx
            .jit
            .module
            .declare_func_in_func(func.typed, fx.builder.func);
        let call = fx.builder.ins().call(callee, &args);
        let r = fx.builder.inst_results(call)[0];
        fx.check();
        for w in lent {
            fx.dec_inline(w);
        }
        let narrowed = fx.abi_in(r, func.sig.ret);
        let handle = fx.boxed(Val {
            kind: func.sig.ret,
            v: narrowed,
            ty: 0,
            home: 0,
        });
        fx.builder.ins().return_(&[handle]);

        fx.builder.switch_to_block(failure);
        let zero = fx.builder.ins().iconst(types::I64, 0);
        fx.builder.ins().return_(&[zero]);
        fx.builder.seal_all_blocks();
        fx.builder.finalize();
    }
}

fn declare(module: &mut JITModule, name: &str, params: usize, returns: bool) -> Result<FuncId> {
    let sig = helper_sig(module, params, returns);
    Ok(module.declare_function(name, Linkage::Import, &sig)?)
}

/// A JIT symbol may not collide with a runtime helper's, and a Ply name holds dots.
fn mangle(name: &str) -> String {
    format!("ply${}", name.replace('.', "$"))
}

fn count_nodes(code: &Code) -> usize {
    let mut n = 1;
    match &code.kind {
        NodeKind::Lit(..) | NodeKind::Var { .. } => {}
        NodeKind::Unary { operand, .. } => n += count_nodes(operand),
        NodeKind::Binary { lhs, rhs, .. } => n += count_nodes(lhs) + count_nodes(rhs),
        NodeKind::Lambda { body, .. } => n += count_nodes(body),
        NodeKind::App { func, args, .. } => {
            n += count_nodes(func);
            n += args.iter().map(count_nodes).sum::<usize>();
        }
        NodeKind::If {
            cond,
            then_branch,
            else_branch,
        } => n += count_nodes(cond) + count_nodes(then_branch) + count_nodes(else_branch),
        NodeKind::Match { scrutinee, arms } => {
            n += count_nodes(scrutinee);
            for arm in arms.iter() {
                n += count_nodes(&arm.body);
                if let Some(g) = &arm.guard {
                    n += count_nodes(g);
                }
            }
        }
        NodeKind::Block { stmts, tail } => {
            for s in stmts.iter() {
                n += match s {
                    Stmt::Let { value, .. } => count_nodes(value),
                    Stmt::Expr { code, .. } => count_nodes(code),
                };
            }
            if let Some(t) = tail {
                n += count_nodes(t);
            }
        }
        NodeKind::Record { fields, .. } => {
            n += fields.iter().map(|(_, e)| count_nodes(e)).sum::<usize>()
        }
        NodeKind::RecordUpdate { base, sets, .. } => {
            n += count_nodes(base) + sets.iter().map(|(_, e)| count_nodes(e)).sum::<usize>()
        }
        NodeKind::Field { base, .. } => n += count_nodes(base),
        NodeKind::List { items } => n += items.iter().map(count_nodes).sum::<usize>(),
        NodeKind::Perform { args, .. } => n += args.iter().map(count_nodes).sum::<usize>(),
        NodeKind::Handle { body, .. } => n += count_nodes(body),
        NodeKind::WithCell { init, body, .. } => n += count_nodes(init) + count_nodes(body),
        NodeKind::Simulate { body, .. } => n += count_nodes(body),
        NodeKind::WithRegion { body } => n += count_nodes(body),
    }
    n
}

/// The widths of the record literals a body builds itself — a lambda's are its own function's.
fn record_widths(code: &Code, out: &mut BTreeSet<usize>) {
    match &code.kind {
        NodeKind::Lit(..) | NodeKind::Var { .. } | NodeKind::Lambda { .. } => {}
        NodeKind::Unary { operand, .. } => record_widths(operand, out),
        NodeKind::Binary { lhs, rhs, .. } => {
            record_widths(lhs, out);
            record_widths(rhs, out);
        }
        NodeKind::App { func, args, .. } => {
            record_widths(func, out);
            args.iter().for_each(|a| record_widths(a, out));
        }
        NodeKind::If {
            cond,
            then_branch,
            else_branch,
        } => {
            record_widths(cond, out);
            record_widths(then_branch, out);
            record_widths(else_branch, out);
        }
        NodeKind::Match { scrutinee, arms } => {
            record_widths(scrutinee, out);
            for arm in arms.iter() {
                record_widths(&arm.body, out);
                if let Some(g) = &arm.guard {
                    record_widths(g, out);
                }
            }
        }
        NodeKind::Block { stmts, tail } => {
            for s in stmts.iter() {
                match s {
                    Stmt::Let { value, .. } => record_widths(value, out),
                    Stmt::Expr { code, .. } => record_widths(code, out),
                }
            }
            if let Some(t) = tail {
                record_widths(t, out);
            }
        }
        NodeKind::Record { fields, .. } => {
            out.insert(fields.len());
            fields.iter().for_each(|(_, e)| record_widths(e, out));
        }
        NodeKind::RecordUpdate { base, copies, sets } => {
            // A fully written literal is built as one, in the token of its width.
            if copies.is_empty() {
                out.insert(sets.len());
            }
            record_widths(base, out);
            sets.iter().for_each(|(_, e)| record_widths(e, out));
        }
        NodeKind::Field { base, .. } => record_widths(base, out),
        NodeKind::List { items } => items.iter().for_each(|i| record_widths(i, out)),
        NodeKind::Perform { args, .. } => args.iter().for_each(|a| record_widths(a, out)),
        NodeKind::Handle { body, .. }
        | NodeKind::Simulate { body, .. }
        | NodeKind::WithRegion { body } => record_widths(body, out),
        NodeKind::WithCell { init, body, .. } => {
            record_widths(init, out);
            record_widths(body, out);
        }
    }
}

/// Whether evaluating `code` can move the local `name` out of its binding: a read marked its
/// last use, or a lambda capturing it.
fn moves(code: &Code, name: &Symbol) -> bool {
    let mut hit = false;
    let mut check = |c: &Code| {
        if moves(c, name) {
            hit = true;
        }
    };
    match &code.kind {
        NodeKind::Var { name: q, .. } => {
            return q.is_bare() && q.symbol() == name && code.own == Own::Owned;
        }
        NodeKind::Lambda { captures, .. } => return captures.names.contains(name),
        NodeKind::Lit(..) => return false,
        NodeKind::Field { base, .. } => {
            // A field read at the record's last use takes the record with it.
            if let NodeKind::Var { name: q, .. } = &base.kind {
                return q.is_bare() && q.symbol() == name && base.own == Own::Owned;
            }
            check(base);
        }
        NodeKind::Unary { operand, .. } => check(operand),
        NodeKind::Binary { lhs, rhs, .. } => {
            check(lhs);
            check(rhs);
        }
        NodeKind::App { func, args, .. } => {
            check(func);
            args.iter().for_each(&mut check);
        }
        NodeKind::If {
            cond,
            then_branch,
            else_branch,
        } => {
            check(cond);
            check(then_branch);
            check(else_branch);
        }
        NodeKind::Match { scrutinee, arms } => {
            check(scrutinee);
            for arm in arms.iter() {
                check(&arm.body);
                if let Some(g) = &arm.guard {
                    check(g);
                }
            }
        }
        NodeKind::Block { stmts, tail } => {
            for s in stmts.iter() {
                match s {
                    Stmt::Let { value, .. } => check(value),
                    Stmt::Expr { code, .. } => check(code),
                }
            }
            if let Some(t) = tail {
                check(t);
            }
        }
        NodeKind::Record { fields, .. } => fields.iter().for_each(|(_, e)| check(e)),
        NodeKind::RecordUpdate { base, sets, .. } => {
            check(base);
            sets.iter().for_each(|(_, e)| check(e));
        }
        NodeKind::List { items } => items.iter().for_each(&mut check),
        NodeKind::Perform { args, .. } => args.iter().for_each(&mut check),
        NodeKind::Handle { body, .. }
        | NodeKind::Simulate { body, .. }
        | NodeKind::WithRegion { body } => check(body),
        NodeKind::WithCell { init, body, .. } => {
            check(init);
            check(body);
        }
    }
    hit
}

/// Clears `ok` where `code` mentions `name` other than as the base of a field read — as a value,
/// an update's base, a scrutinee or a lambda's capture.
fn read_only(code: &Code, name: &Symbol, ok: &mut bool) {
    if !*ok {
        return;
    }
    let mut go = |c: &Code| read_only(c, name, ok);
    match &code.kind {
        NodeKind::Lit(..) => {}
        NodeKind::Var { name: q, .. } => {
            if q.is_bare() && q.symbol() == name {
                *ok = false;
            }
        }
        NodeKind::Field { base, .. } => {
            if !matches!(&base.kind, NodeKind::Var { name: q, .. } if q.is_bare() && q.symbol() == name)
            {
                go(base);
            }
        }
        NodeKind::Lambda { captures, body, .. } => {
            if captures.names.contains(name) {
                *ok = false;
            } else {
                go(body);
            }
        }
        NodeKind::Unary { operand, .. } => go(operand),
        NodeKind::Binary { lhs, rhs, .. } => {
            go(lhs);
            go(rhs);
        }
        NodeKind::App { func, args, .. } => {
            go(func);
            args.iter().for_each(go);
        }
        NodeKind::If {
            cond,
            then_branch,
            else_branch,
        } => {
            go(cond);
            go(then_branch);
            go(else_branch);
        }
        NodeKind::Match { scrutinee, arms } => {
            go(scrutinee);
            for arm in arms.iter() {
                go(&arm.body);
                if let Some(g) = &arm.guard {
                    go(g);
                }
            }
        }
        NodeKind::Block { stmts, tail } => {
            for s in stmts.iter() {
                match s {
                    Stmt::Let { value, .. } => go(value),
                    Stmt::Expr { code, .. } => go(code),
                }
            }
            if let Some(t) = tail {
                go(t);
            }
        }
        NodeKind::Record { fields, .. } => fields.iter().for_each(|(_, e)| go(e)),
        NodeKind::RecordUpdate { base, sets, .. } => {
            go(base);
            sets.iter().for_each(|(_, e)| go(e));
        }
        NodeKind::List { items } => items.iter().for_each(go),
        NodeKind::Perform { args, .. } => args.iter().for_each(go),
        NodeKind::Handle { body, .. }
        | NodeKind::Simulate { body, .. }
        | NodeKind::WithRegion { body } => go(body),
        NodeKind::WithCell { init, body, .. } => {
            go(init);
            go(body);
        }
    }
}

/// Where compiled code finds `Ctx::failed`, taken from the type rather than written down:
/// `#[repr(C)]` fixes the layout and `offset_of!` reads it, so the two cannot drift when a field is
/// added.
const FAILED_OFFSET: i32 = std::mem::offset_of!(Ctx, failed) as i32;
const FUEL_OFFSET: i32 = std::mem::offset_of!(Ctx, fuel) as i32;

/// Whether a compiled body may call this builtin, and what to say when it may not.
/// Whether a call of `b` with `arity` arguments is answered inline, in an `Int` register: a
/// length or a byte read is a load once the argument's kind is checked, and the runtime is
/// entered only for anything else — another kind, an index out of range — which it refuses as
/// the interpreter would.
fn inline_builtin_answers(b: Builtin, arity: usize) -> bool {
    matches!(
        (b, arity),
        (Builtin::BytesAt, 2) | (Builtin::BytesLen, 1) | (Builtin::Len, 1)
    ) || scalar_builtin_answers(b, arity)
        || width_builtin(b, arity).is_some()
}

/// The width a conversion builtin answers at, if it is one the fragment carries. `int_of_u32`
/// answers an `Int` and appears here as `None` for the width and `true` for being one.
fn width_builtin(b: Builtin, arity: usize) -> Option<Option<IntTy>> {
    if arity != 1 {
        return None;
    }
    if let Some(t) = b.converts_into() {
        return carried_width(t).then_some(Some(t));
    }
    if let Some(t) = b.converts_from() {
        return carried_width(t).then_some(None);
    }
    None
}

/// The builtins over two `Int`s that are one instruction each: the wrapping arithmetic and the
/// rotate of the low word.
fn scalar_builtin_answers(b: Builtin, arity: usize) -> bool {
    arity == 2
        && matches!(
            b,
            Builtin::WrapAdd
                | Builtin::WrapSub
                | Builtin::WrapMul
                | Builtin::Rotr32
                | Builtin::Rotr
        )
}

fn admissible_builtin(b: Builtin) -> Result<(), String> {
    if b.higher_order() && !lowered_callback(b) {
        return Err(format!("`{}`, a builtin that calls user code", b.name()));
    }
    // `U64` and `I64` are outside the fragment, so their conversions are too: a value of either
    // may not fit an immediate, which is the property every carried width has.
    if let Some(t) = b.converts_into().or_else(|| b.converts_from())
        && !carried_width(t)
    {
        return Err(format!(
            "`{}`, whose width the fragment does not carry",
            b.name()
        ));
    }
    match b {
        Builtin::CellGet | Builtin::CellSet => Err(format!(
            "`{}`, which reaches a cell arena compiled code is not given",
            b.name()
        )),
        Builtin::SecretOfString => Err(format!(
            "`{}`, which would put a credential in the fragment's value arena",
            b.name()
        )),
        _ => Ok(()),
    }
}

/// The callback builtins compiled code runs as a loop of its own, calling the callback through
/// `rt_call`; the others that call user code stay outside the fragment.
fn lowered_callback(b: Builtin) -> bool {
    matches!(
        b,
        Builtin::Map | Builtin::Filter | Builtin::Fold | Builtin::MapFold | Builtin::Iterate
    )
}

/// What a name denotes, decided at compile time in the order `Machine::lookup` decides it at run
/// time.
/// The step a fused loop calls directly: a compiled function through its typed body, or a
/// lambda literal through its own entry, with the words it captured as its leading arguments.
enum Step {
    Typed(Func),
    /// A lambda literal, lowered in the loop's own body: its parameters bound to the loop's
    /// values and its captures read as the loop's locals.
    Inline {
        params: Vec<Symbol>,
        body: Code,
        captures: Vec<Symbol>,
    },
}

/// What a fused loop walks.
enum Walk {
    Range {
        lo: cranelift_codegen::ir::Value,
        hi: cranelift_codegen::ir::Value,
    },
    List {
        list: cranelift_codegen::ir::Value,
        len: cranelift_codegen::ir::Value,
    },
}

enum Denotes {
    Local(Val),
    Compiled(Func),
    /// A Ply function this unit did not compile.
    Uncompiled(String),
    Ctor(usize, usize),
    Builtin(usize),
    Constant(usize),
}

struct Fx<'a, 'b> {
    jit: &'a mut Jit,
    builder: FunctionBuilder<'b>,
    loaded: &'static Source,
    ctx: cranelift_codegen::ir::Value,
    failure: cranelift_codegen::ir::Block,
    function: String,
    module_index: usize,
    /// The stack slot of every binding whose value is a word, released at the function's exit
    /// unless a move emptied it first.
    homes: Vec<cranelift_codegen::ir::StackSlot>,
    /// Perceus's reuse tokens: per width of record literal the body builds, a slot holding a
    /// record of that width that died at its last use in this body — its memory the next such
    /// literal's — or `0`. Whatever is left in one at the exit is released there.
    tokens: Vec<(usize, cranelift_codegen::ir::StackSlot)>,
    /// The locals an inlined step reads as captures: never moved out by a mark computed for
    /// the lambda's own frame, since the loop still holds them.
    pinned: Vec<Symbol>,
    /// `kind_of`'s answer per node: a branch is asked about before it is compiled and again at
    /// every join above it, and the answer is fixed by the node's place in the tree.
    kinds: HashMap<*const ply_eval::code::Node, Kind>,
}

type Scope = Vec<(Symbol, Val)>;

impl Fx<'_, '_> {
    fn refuse<T>(&self, what: impl Into<String>) -> Result<T> {
        Err(Refused {
            function: self.function.clone(),
            construct: what.into(),
        }
        .into())
    }

    /// The index of a field name in the unit's table, added on first use.
    fn field_index(&mut self, name: &Symbol) -> i64 {
        if let Some(i) = self.jit.fields.iter().position(|f| f == name) {
            return i as i64;
        }
        self.jit.fields.push(name.clone());
        (self.jit.fields.len() - 1) as i64
    }

    /// A constant's index in the pool, its word built immortal on the way in.
    fn intern(&mut self, value: Value) -> usize {
        let w = self.jit.immortals.immortal(&self.jit.layouts, &value);
        self.jit.consts.push(value);
        self.jit.const_words.push(w);
        self.jit.consts.len() - 1
    }

    fn load_fuel(&mut self) -> cranelift_codegen::ir::Value {
        self.builder
            .ins()
            .load(types::I64, MemFlags::trusted(), self.ctx, FUEL_OFFSET)
    }

    fn store_fuel(&mut self, v: cranelift_codegen::ir::Value) {
        self.builder
            .ins()
            .store(MemFlags::trusted(), v, self.ctx, FUEL_OFFSET);
    }

    /// After every call that can fail: one load and one branch, which is what a real backend pays
    /// for a fallible runtime call too.
    fn check(&mut self) {
        let failed =
            self.builder
                .ins()
                .load(types::I64, MemFlags::trusted(), self.ctx, FAILED_OFFSET);
        let next = self.builder.create_block();
        self.builder
            .ins()
            .brif(failed, self.failure, &[], next, &[]);
        self.builder.switch_to_block(next);
        self.builder.seal_block(next);
    }

    /// One more holder of a word, in place: nothing for an immediate or an immortal object, a
    /// bumped count for anything else.
    fn inc_inline(&mut self, w: cranelift_codegen::ir::Value) {
        let tagged = self.builder.ins().band_imm(w, 1);
        let pointer = self.builder.create_block();
        let bump = self.builder.create_block();
        let done = self.builder.create_block();
        self.builder.ins().brif(tagged, done, &[], pointer, &[]);
        self.builder.switch_to_block(pointer);
        self.builder.seal_block(pointer);
        let rc = self
            .builder
            .ins()
            .load(types::I32, MemFlags::trusted(), w, 0);
        let immortal = self.builder.ins().icmp_imm(IntCC::Equal, rc, -1);
        self.builder.ins().brif(immortal, done, &[], bump, &[]);
        self.builder.switch_to_block(bump);
        self.builder.seal_block(bump);
        let bumped = self.builder.ins().iadd_imm(rc, 1);
        self.builder.ins().store(MemFlags::trusted(), bumped, w, 0);
        self.builder.ins().jump(done, &[]);
        self.builder.switch_to_block(done);
        self.builder.seal_block(done);
    }

    /// The field `w` at offset `at` of the record `base` leaves it: the slot is emptied when the
    /// record has one holder, so the field keeps its count and the record's end lets nothing go;
    /// otherwise the field is held once more. What `rt_field` does for a taken field, inline.
    fn take_field_inline(
        &mut self,
        base: cranelift_codegen::ir::Value,
        at: usize,
        w: cranelift_codegen::ir::Value,
    ) {
        let rc = self
            .builder
            .ins()
            .load(types::I32, MemFlags::trusted(), base, 0);
        let unique = self.builder.ins().icmp_imm(IntCC::Equal, rc, 1);
        let take = self.builder.create_block();
        let share = self.builder.create_block();
        let done = self.builder.create_block();
        self.builder.ins().brif(unique, take, &[], share, &[]);
        self.builder.switch_to_block(take);
        self.builder.seal_block(take);
        let unit = self.builder.ins().iconst(types::I64, crate::heap::unit());
        self.builder
            .ins()
            .store(MemFlags::trusted(), unit, base, (HEADER + 8 * at) as i32);
        self.builder.ins().jump(done, &[]);
        self.builder.switch_to_block(share);
        self.builder.seal_block(share);
        self.inc_inline(w);
        self.builder.ins().jump(done, &[]);
        self.builder.switch_to_block(done);
        self.builder.seal_block(done);
    }

    /// The field a value of compile-time type `ty` has at `name`, as its offset and its own type.
    fn static_field(&self, ty: u32, name: &Symbol) -> Option<(usize, u32)> {
        match &self.jit.tys[ty as usize] {
            Ty::Record(fields) => fields
                .iter()
                .position(|(n, _)| n == name)
                .map(|at| (at, fields[at].1)),
            _ => None,
        }
    }

    /// A register's value as a word: an `Int` that fits is tagged in place and one that does not
    /// is boxed by the runtime; a `Bool` is one of the two singletons.
    fn boxed(&mut self, val: Val) -> cranelift_codegen::ir::Value {
        match val.kind {
            Kind::Boxed => val.v,
            Kind::Int => {
                let shifted = self.builder.ins().ishl_imm(val.v, 1);
                let back = self.builder.ins().sshr_imm(shifted, 1);
                let fits = self.builder.ins().icmp(IntCC::Equal, back, val.v);
                let fast = self.builder.create_block();
                let slow = self.builder.create_block();
                let join = self.builder.create_block();
                self.builder.append_block_param(join, types::I64);
                self.builder.ins().brif(fits, fast, &[], slow, &[]);
                self.builder.switch_to_block(fast);
                self.builder.seal_block(fast);
                let tagged = self.builder.ins().bor_imm(shifted, 1);
                self.builder.ins().jump(join, &[BlockArg::Value(tagged)]);
                self.builder.switch_to_block(slow);
                self.builder.seal_block(slow);
                let heavy = self.helper(self.jit.helpers.box_int, &[val.v]);
                self.builder.ins().jump(join, &[BlockArg::Value(heavy)]);
                self.builder.switch_to_block(join);
                self.builder.seal_block(join);
                self.builder.block_params(join)[0]
            }
            // Always an immediate, so no fits test and no allocator: a value of a carried width
            // is at most thirty-two bits and an immediate holds sixty-three.
            Kind::Num(t) => {
                let wide = if t.signed() {
                    self.builder.ins().sextend(types::I64, val.v)
                } else {
                    self.builder.ins().uextend(types::I64, val.v)
                };
                let shifted = self.builder.ins().ishl_imm(wide, 1);
                self.builder.ins().bor_imm(shifted, 1)
            }
            Kind::Bool => {
                let t = self
                    .builder
                    .ins()
                    .iconst(types::I64, crate::heap::bool(true));
                let f = self
                    .builder
                    .ins()
                    .iconst(types::I64, crate::heap::bool(false));
                let c = self.builder.ins().icmp_imm(IntCC::NotEqual, val.v, 0);
                self.builder.ins().select(c, t, f)
            }
        }
    }

    /// A word as an `Int` register: an immediate is untagged in place, and anything else is the
    /// runtime's to unbox or refuse.
    fn as_int(&mut self, val: Val) -> cranelift_codegen::ir::Value {
        match val.kind {
            Kind::Int => val.v,
            _ => {
                let w = self.boxed(val);
                let tagged = self.builder.ins().band_imm(w, 1);
                let fast = self.builder.create_block();
                let slow = self.builder.create_block();
                let join = self.builder.create_block();
                self.builder.append_block_param(join, types::I64);
                self.builder.ins().brif(tagged, fast, &[], slow, &[]);
                self.builder.switch_to_block(fast);
                self.builder.seal_block(fast);
                let v = self.builder.ins().sshr_imm(w, 1);
                self.builder.ins().jump(join, &[BlockArg::Value(v)]);
                self.builder.switch_to_block(slow);
                self.builder.seal_block(slow);
                let v = self.helper(self.jit.helpers.unbox_int, &[w]);
                self.check();
                self.builder.ins().jump(join, &[BlockArg::Value(v)]);
                self.builder.switch_to_block(join);
                self.builder.seal_block(join);
                self.builder.block_params(join)[0]
            }
        }
    }

    /// The width a binary operation is at, if either operand is already at one. The checker
    /// unified the two operands, so one side knowing the width is the whole of the question.
    fn num_width(l: Val, r: Val) -> Option<IntTy> {
        match (l.kind, r.kind) {
            (Kind::Num(t), _) | (_, Kind::Num(t)) => Some(t),
            _ => None,
        }
    }

    /// `v`, refused unless it is one of `t`'s values. The check `Int` gets from `sadd_overflow`,
    /// at a width Cranelift has no overflow instruction for.
    fn checked_narrow(&mut self, wide: cranelift_codegen::ir::Value, t: IntTy, sub: bool) -> Val {
        let low = self.builder.ins().iconst(types::I64, t.min() as i64);
        let high = self.builder.ins().iconst(types::I64, t.max() as i64);
        let cc = if t.signed() {
            (IntCC::SignedLessThan, IntCC::SignedGreaterThan)
        } else {
            (IntCC::SignedLessThan, IntCC::UnsignedGreaterThan)
        };
        let under = self.builder.ins().icmp(cc.0, wide, low);
        let over = self.builder.ins().icmp(cc.1, wide, high);
        let bad = self.builder.ins().bor(under, over);
        let overflowed = self.builder.create_block();
        let ok = self.builder.create_block();
        self.builder.ins().brif(bad, overflowed, &[], ok, &[]);
        self.builder.switch_to_block(overflowed);
        self.builder.seal_block(overflowed);
        let what = self.builder.ins().iconst(types::I64, i64::from(sub));
        self.helper_void(self.jit.helpers.overflow, &[what]);
        self.builder.ins().jump(self.failure, &[]);
        self.builder.switch_to_block(ok);
        self.builder.seal_block(ok);
        let v = self.builder.ins().ireduce(clif_int(t), wide);
        Val {
            kind: Kind::Num(t),
            v,
            ty: 0,
            home: 0,
        }
    }

    /// A value as a register of width `t`. From a word this is an untag and a narrowing with **no
    /// tag test**, which is what the type buys over `as_int`: a value of a carried width is an
    /// immediate by construction, so there is nothing to branch on.
    fn as_num(&mut self, val: Val, t: IntTy) -> cranelift_codegen::ir::Value {
        let width = clif_int(t);
        match val.kind {
            Kind::Num(have) if have == t => val.v,
            Kind::Num(_) | Kind::Int => {
                let wide = self.as_wide(val);
                self.builder.ins().ireduce(width, wide)
            }
            _ => {
                let w = self.boxed(val);
                let raw = self.builder.ins().sshr_imm(w, 1);
                self.builder.ins().ireduce(width, raw)
            }
        }
    }

    /// A numeric register widened to the sixty-four bits the typed ABI passes, still untagged.
    fn as_wide(&mut self, val: Val) -> cranelift_codegen::ir::Value {
        match val.kind {
            Kind::Num(t) => {
                if t.signed() {
                    self.builder.ins().sextend(types::I64, val.v)
                } else {
                    self.builder.ins().uextend(types::I64, val.v)
                }
            }
            _ => self.as_int(val),
        }
    }

    /// A word as a `Bool` register: a compare against the two singletons, and the runtime's
    /// refusal for anything else.
    fn as_bool(&mut self, val: Val) -> cranelift_codegen::ir::Value {
        match val.kind {
            Kind::Bool => val.v,
            _ => {
                let w = self.boxed(val);
                let is_true = self
                    .builder
                    .ins()
                    .icmp_imm(IntCC::Equal, w, crate::heap::bool(true));
                let is_false =
                    self.builder
                        .ins()
                        .icmp_imm(IntCC::Equal, w, crate::heap::bool(false));
                let known = self.builder.ins().bor(is_true, is_false);
                let fast = self.builder.create_block();
                let slow = self.builder.create_block();
                let join = self.builder.create_block();
                self.builder.append_block_param(join, types::I64);
                self.builder.ins().brif(known, fast, &[], slow, &[]);
                self.builder.switch_to_block(fast);
                self.builder.seal_block(fast);
                let v = self.builder.ins().uextend(types::I64, is_true);
                self.builder.ins().jump(join, &[BlockArg::Value(v)]);
                self.builder.switch_to_block(slow);
                self.builder.seal_block(slow);
                let v = self.helper(self.jit.helpers.unbox_bool, &[w]);
                self.check();
                self.builder.ins().jump(join, &[BlockArg::Value(v)]);
                self.builder.switch_to_block(join);
                self.builder.seal_block(join);
                self.builder.block_params(join)[0]
            }
        }
    }

    fn helper(
        &mut self,
        id: FuncId,
        args: &[cranelift_codegen::ir::Value],
    ) -> cranelift_codegen::ir::Value {
        let func = self.jit.module.declare_func_in_func(id, self.builder.func);
        let mut all = vec![self.ctx];
        all.extend_from_slice(args);
        let call = self.builder.ins().call(func, &all);
        self.builder.inst_results(call)[0]
    }

    fn helper_void(&mut self, id: FuncId, args: &[cranelift_codegen::ir::Value]) {
        let func = self.jit.module.declare_func_in_func(id, self.builder.func);
        let mut all = vec![self.ctx];
        all.extend_from_slice(args);
        self.builder.ins().call(func, &all);
    }

    /// A value about to be consumed — handed to a helper that takes its arguments, bound by a
    /// `let`, captured by a closure. A local read at the binding's last use (`Own::Owned`, the
    /// lowering's mark) is the slot's own handle and is taken; a local read anywhere else is
    /// first duplicated into a fresh slot, Perceus's `dup`, so the binding's slot survives the
    /// take. Everything else is a temporary already. A read that only looks — a field, an
    /// unbox, a comparison, a pattern test — takes nothing and uses the slot as it is, so it pins
    /// no record: a duplicate that stayed in a slot would hold the record shared until the entry
    /// ended and cost every later update its reuse.
    fn consumed(&mut self, code: &Code, scope: &mut Scope) -> Result<Val> {
        let val = self.expr(code, scope)?;
        if val.kind != Kind::Boxed {
            return Ok(val);
        }
        if self.is_borrowed_local(code, scope) {
            self.inc_inline(val.v);
            let v = val.v;
            return Ok(Val {
                kind: Kind::Boxed,
                v,
                ty: val.ty,
                home: 0,
            });
        }
        // A local at its last use leaves its binding: the binding's slot is emptied, so the
        // exit releases nothing for it and whoever now holds the word is its only holder.
        if self.last_use_of_local(code, scope) {
            self.moved(val);
        }
        Ok(Val { home: 0, ..val })
    }

    /// A captured value, by the capture's own mark.
    fn captured(&mut self, own: Own, val: Val) -> Val {
        if val.kind != Kind::Boxed {
            return val;
        }
        if own == Own::Owned {
            self.moved(val);
            return Val { home: 0, ..val };
        }
        let v = self.helper(self.jit.helpers.dup, &[val.v]);
        Val {
            kind: Kind::Boxed,
            v,
            ty: val.ty,
            home: 0,
        }
    }

    /// A binding's value, given a stack slot its scope releases at its end unless a move empties
    /// it first; a register value has no count and needs none. The slot is written here, on
    /// every path through the scope that reads it, so it needs no clearing.
    fn home(&mut self, val: Val) -> Val {
        if val.kind != Kind::Boxed {
            return val;
        }
        let slot = self.builder.create_sized_stack_slot(StackSlotData::new(
            StackSlotKind::ExplicitSlot,
            8,
            3,
        ));
        self.builder.ins().stack_store(val.v, slot, 0);
        self.homes.push(slot);
        Val {
            home: self.homes.len() as u32,
            ..val
        }
    }

    /// A binding's value has left it: the slot is emptied so the exit does not release it.
    fn moved(&mut self, val: Val) {
        if val.home == 0 {
            return;
        }
        let slot = self.homes[val.home as usize - 1];
        let zero = self.builder.ins().iconst(types::I64, 0);
        self.builder.ins().stack_store(zero, slot, 0);
    }

    /// One holder fewer of a word, in place: nothing for an empty slot, an immediate or an
    /// immortal object, a decrement for an object others still hold, and the runtime's
    /// dismantling only for the last holder.
    fn dec_inline(&mut self, w: cranelift_codegen::ir::Value) {
        let done = self.builder.create_block();
        let pointer = self.builder.create_block();
        let counted = self.builder.create_block();
        let shared = self.builder.create_block();
        let last = self.builder.create_block();
        let tagged = self.builder.ins().band_imm(w, 1);
        let tagged = self.builder.ins().icmp_imm(IntCC::NotEqual, tagged, 0);
        let empty = self.builder.ins().icmp_imm(IntCC::Equal, w, 0);
        let skip = self.builder.ins().bor(empty, tagged);
        self.builder.ins().brif(skip, done, &[], pointer, &[]);
        self.builder.switch_to_block(pointer);
        self.builder.seal_block(pointer);
        let rc = self
            .builder
            .ins()
            .load(types::I32, MemFlags::trusted(), w, 0);
        let immortal = self.builder.ins().icmp_imm(IntCC::Equal, rc, -1);
        self.builder.ins().brif(immortal, done, &[], counted, &[]);
        self.builder.switch_to_block(counted);
        self.builder.seal_block(counted);
        let more = self
            .builder
            .ins()
            .icmp_imm(IntCC::UnsignedGreaterThan, rc, 1);
        self.builder.ins().brif(more, shared, &[], last, &[]);
        self.builder.switch_to_block(shared);
        self.builder.seal_block(shared);
        let fewer = self.builder.ins().iadd_imm(rc, -1);
        self.builder.ins().store(MemFlags::trusted(), fewer, w, 0);
        self.builder.ins().jump(done, &[]);
        self.builder.switch_to_block(last);
        self.builder.seal_block(last);
        self.helper_void(self.jit.helpers.dec, &[w]);
        self.builder.ins().jump(done, &[]);
        self.builder.switch_to_block(done);
        self.builder.seal_block(done);
    }

    /// Every binding made since `mark` and still holding a word is released, and forgotten:
    /// Perceus's drop at the scope's end, for a block's `let`s at the block's end, an arm's
    /// pattern at the arm's, and the parameters at the function's.
    fn release_homes_from(&mut self, mark: usize) {
        for slot in self.homes.split_off(mark) {
            let w = self.builder.ins().stack_load(types::I64, slot, 0);
            self.dec_inline(w);
        }
    }

    /// The same releases, emitted on a path that leaves the scope by a jump, with the bindings
    /// kept for the paths still to be emitted.
    fn release_homes_since(&mut self, mark: usize) {
        for i in mark..self.homes.len() {
            let slot = self.homes[i];
            let w = self.builder.ins().stack_load(types::I64, slot, 0);
            self.dec_inline(w);
        }
    }

    /// A block's statements: each `let` bound into `inner` as a home, each bare expression's
    /// answer released.
    fn block_stmts(&mut self, stmts: &[Stmt], inner: &mut Scope) -> Result<()> {
        for s in stmts.iter() {
            match s {
                Stmt::Let { pat, value, .. } => {
                    let v = self.consumed(value, inner)?;
                    match pat {
                        Pat::Var { name, .. } => {
                            let v = self.home(v);
                            inner.push((name.name.clone(), v));
                        }
                        Pat::Wildcard => {}
                        other if self.binds_without_test(other) => {
                            self.bind_pattern(other, v, true, inner)?;
                        }
                        other => {
                            return self.refuse(format!(
                                "a `let` binding a refutable {} pattern",
                                pattern_name(other)
                            ));
                        }
                    }
                }
                Stmt::Expr { code, .. } => {
                    // An answer nobody binds is released here unless it is a local's.
                    let v = self.expr(code, inner)?;
                    if v.kind == Kind::Boxed && !self.is_local(code, inner) {
                        self.dec_inline(v.v);
                    }
                }
            }
        }
        Ok(())
    }

    /// One token slot per width of record literal in `body`, empty on entry.
    fn arm_tokens(&mut self, body: &Code) {
        let mut widths = BTreeSet::new();
        record_widths(body, &mut widths);
        for width in widths {
            let slot = self.builder.create_sized_stack_slot(StackSlotData::new(
                StackSlotKind::ExplicitSlot,
                8,
                3,
            ));
            let zero = self.builder.ins().iconst(types::I64, 0);
            self.builder.ins().stack_store(zero, slot, 0);
            self.tokens.push((width, slot));
        }
    }

    fn token_for(&self, width: usize) -> Option<cranelift_codegen::ir::StackSlot> {
        self.tokens
            .iter()
            .find(|(w, _)| *w == width)
            .map(|(_, slot)| *slot)
    }

    /// The token slot a dying value of compile-time type `ty` can fill — a record of a width this
    /// body builds — and whether every field of it is a scalar, so that keeping it lets nothing go.
    fn token_of(&self, ty: u32) -> Option<(cranelift_codegen::ir::StackSlot, bool)> {
        match &self.jit.tys[ty as usize] {
            Ty::Record(fields) => {
                // A fixed width belongs here beside `Int` and `Bool`: a carried width is always
                // boxed as an immediate, so a record of them holds no count and its release walks
                // nothing. Leaving it out cost more than the whole family bought — sixteen tag
                // tests and sixteen conditional decrements per `round` of the integer kernel.
                let flat = fields.iter().all(|(_, t)| {
                    matches!(self.jit.tys[*t as usize], Ty::Int | Ty::Bool | Ty::Num(_))
                });
                self.token_for(fields.len()).map(|slot| (slot, flat))
            }
            _ => None,
        }
    }

    /// A dying record's word released into its width's token when it has one, and let go
    /// otherwise.
    fn release_record(&mut self, w: cranelift_codegen::ir::Value, ty: u32) {
        match self.token_of(ty) {
            Some((slot, flat)) => self.reset_inline(w, slot, flat),
            None => self.dec_inline(w),
        }
    }

    /// Whatever a token still holds at the exit is released: its fields already let go and its
    /// length zero, the release walks nothing.
    fn release_tokens(&mut self) {
        for (_, slot) in std::mem::take(&mut self.tokens) {
            let w = self.builder.ins().stack_load(types::I64, slot, 0);
            self.dec_inline(w);
        }
    }

    /// [`Fx::dec_inline`] for a record at its last use in a body that builds another of its width:
    /// the last holder resets it into `slot` when the slot is empty — with no call at all when
    /// the record is `flat`, its fields all scalars with nothing to let go — and dismantles it
    /// otherwise.
    fn reset_inline(
        &mut self,
        w: cranelift_codegen::ir::Value,
        slot: cranelift_codegen::ir::StackSlot,
        flat: bool,
    ) {
        let done = self.builder.create_block();
        let pointer = self.builder.create_block();
        let counted = self.builder.create_block();
        let shared = self.builder.create_block();
        let last = self.builder.create_block();
        let free = self.builder.create_block();
        let keep = self.builder.create_block();
        let tagged = self.builder.ins().band_imm(w, 1);
        let tagged = self.builder.ins().icmp_imm(IntCC::NotEqual, tagged, 0);
        let empty = self.builder.ins().icmp_imm(IntCC::Equal, w, 0);
        let skip = self.builder.ins().bor(empty, tagged);
        self.builder.ins().brif(skip, done, &[], pointer, &[]);
        self.builder.switch_to_block(pointer);
        self.builder.seal_block(pointer);
        let rc = self
            .builder
            .ins()
            .load(types::I32, MemFlags::trusted(), w, 0);
        let immortal = self.builder.ins().icmp_imm(IntCC::Equal, rc, -1);
        self.builder.ins().brif(immortal, done, &[], counted, &[]);
        self.builder.switch_to_block(counted);
        self.builder.seal_block(counted);
        let more = self
            .builder
            .ins()
            .icmp_imm(IntCC::UnsignedGreaterThan, rc, 1);
        self.builder.ins().brif(more, shared, &[], last, &[]);
        self.builder.switch_to_block(shared);
        self.builder.seal_block(shared);
        let fewer = self.builder.ins().iadd_imm(rc, -1);
        self.builder.ins().store(MemFlags::trusted(), fewer, w, 0);
        self.builder.ins().jump(done, &[]);
        self.builder.switch_to_block(last);
        self.builder.seal_block(last);
        let held = self.builder.ins().stack_load(types::I64, slot, 0);
        let occupied = self.builder.ins().icmp_imm(IntCC::NotEqual, held, 0);
        self.builder.ins().brif(occupied, free, &[], keep, &[]);
        self.builder.switch_to_block(free);
        self.builder.seal_block(free);
        self.helper_void(self.jit.helpers.dec, &[w]);
        self.builder.ins().jump(done, &[]);
        self.builder.switch_to_block(keep);
        self.builder.seal_block(keep);
        let kept = if flat {
            w
        } else {
            self.helper(self.jit.helpers.reset, &[w])
        };
        self.builder.ins().stack_store(kept, slot, 0);
        self.builder.ins().jump(done, &[]);
        self.builder.switch_to_block(done);
        self.builder.seal_block(done);
    }

    /// A fresh record of `shape` over `ordered`, built in the token a dying record of this width
    /// left when there is one — the header rewritten, the fields stored — and by the runtime
    /// otherwise.
    fn record_of(
        &mut self,
        shape: u32,
        ordered: &[cranelift_codegen::ir::Value],
    ) -> cranelift_codegen::ir::Value {
        let Some(slot) = self.token_for(ordered.len()) else {
            return self.record_fresh(shape, ordered);
        };
        let reuse = self.builder.create_block();
        let fresh = self.builder.create_block();
        let done = self.builder.create_block();
        self.builder.append_block_param(done, types::I64);
        let held = self.builder.ins().stack_load(types::I64, slot, 0);
        let have = self.builder.ins().icmp_imm(IntCC::NotEqual, held, 0);
        self.builder.ins().brif(have, reuse, &[], fresh, &[]);
        self.builder.switch_to_block(reuse);
        self.builder.seal_block(reuse);
        let zero = self.builder.ins().iconst(types::I64, 0);
        self.builder.ins().stack_store(zero, slot, 0);
        let flags = MemFlags::trusted();
        let one = self.builder.ins().iconst(types::I32, 1);
        self.builder.ins().store(flags, one, held, 0);
        // The kind, and beside it the flat flag when every field is an immediate.
        let flat = self.flat_over(ordered);
        let flat = self.builder.ins().ishl_imm(flat, 8);
        let kind = self
            .builder
            .ins()
            .iconst(types::I64, i64::from(KIND_RECORD));
        let kind = self.builder.ins().bor(kind, flat);
        let kind = self.builder.ins().ireduce(types::I32, kind);
        self.builder.ins().store(flags, kind, held, 4);
        let len = self.builder.ins().iconst(types::I32, ordered.len() as i64);
        self.builder.ins().store(flags, len, held, 8);
        let layout = self.builder.ins().iconst(types::I32, i64::from(shape));
        self.builder.ins().store(flags, layout, held, 12);
        for (i, h) in ordered.iter().enumerate() {
            self.builder
                .ins()
                .store(flags, *h, held, (HEADER + 8 * i) as i32);
        }
        self.builder.ins().jump(done, &[BlockArg::Value(held)]);
        self.builder.switch_to_block(fresh);
        self.builder.seal_block(fresh);
        let v = self.record_fresh(shape, ordered);
        self.builder.ins().jump(done, &[BlockArg::Value(v)]);
        self.builder.switch_to_block(done);
        self.builder.seal_block(done);
        self.builder.block_params(done)[0]
    }

    fn record_fresh(
        &mut self,
        shape: u32,
        ordered: &[cranelift_codegen::ir::Value],
    ) -> cranelift_codegen::ir::Value {
        self.built_fresh(KIND_RECORD, shape, ordered)
    }

    /// A fresh object with its header written by the runtime and its fields stored here, with
    /// no argument array between.
    fn built_fresh(
        &mut self,
        kind: u8,
        layout: u32,
        ordered: &[cranelift_codegen::ir::Value],
    ) -> cranelift_codegen::ir::Value {
        let flat = self.flat_over(ordered);
        let kind = self.builder.ins().iconst(types::I64, i64::from(kind));
        let len = self.builder.ins().iconst(types::I64, ordered.len() as i64);
        let layout = self.builder.ins().iconst(types::I64, i64::from(layout));
        let p = self.helper(self.jit.helpers.alloc, &[kind, len, layout, flat]);
        let flags = MemFlags::trusted();
        for (i, h) in ordered.iter().enumerate() {
            self.builder
                .ins()
                .store(flags, *h, p, (HEADER + 8 * i) as i32);
        }
        p
    }

    /// `1` when every word is an immediate, `0` otherwise: the flat flag of an object built over
    /// them.
    fn flat_over(
        &mut self,
        words: &[cranelift_codegen::ir::Value],
    ) -> cranelift_codegen::ir::Value {
        let mut flat = self.builder.ins().iconst(types::I64, 1);
        for w in words {
            let tag = self.builder.ins().band_imm(*w, 1);
            flat = self.builder.ins().band(flat, tag);
        }
        flat
    }

    fn is_local(&self, code: &Code, scope: &Scope) -> bool {
        matches!(&code.kind, NodeKind::Var { name: q, .. }
            if q.is_bare() && scope.iter().any(|(s, _)| s == q.symbol()))
    }

    fn is_borrowed_local(&self, code: &Code, scope: &Scope) -> bool {
        (code.own != Own::Owned || self.is_pinned(code)) && self.is_local(code, scope)
    }

    /// Whether `code` reads a local an inlined step captured, which its marks may not move.
    fn is_pinned(&self, code: &Code) -> bool {
        matches!(&code.kind, NodeKind::Var { name: q, .. }
            if q.is_bare() && self.pinned.contains(q.symbol()))
    }

    /// Whether `arg` is a local whose read here is the binding's last.
    fn last_use_of_local(&self, arg: &Code, scope: &Scope) -> bool {
        arg.own == Own::Owned
            && !self.is_pinned(arg)
            && matches!(&arg.kind, NodeKind::Var { name: q, .. }
                if q.is_bare() && scope.iter().any(|(s, _)| s == q.symbol()))
    }

    /// The argument array a call is handed: one stack slot, one store per argument, and every value
    /// boxed at the boundary.
    fn spill(&mut self, handles: &[cranelift_codegen::ir::Value]) -> cranelift_codegen::ir::Value {
        let bytes = (handles.len().max(1) * 8) as u32;
        let slot = self.builder.create_sized_stack_slot(StackSlotData::new(
            StackSlotKind::ExplicitSlot,
            bytes,
            3,
        ));
        for (i, h) in handles.iter().enumerate() {
            self.builder.ins().stack_store(*h, slot, (i * 8) as i32);
        }
        self.builder.ins().stack_addr(types::I64, slot, 0)
    }

    fn denotation(&mut self, q: &QName, scope: &Scope) -> Result<Denotes> {
        if q.is_bare()
            && let Some((_, val)) = scope.iter().rev().find(|(s, _)| s == q.symbol())
        {
            return Ok(Denotes::Local(*val));
        }
        let global = if q.is_bare() {
            self.loaded
                .resolved
                .scopes
                .get(self.module_index)
                .and_then(|s| s.get(Namespace::Value, q.symbol()))
                .map(|b| b.qualified.clone())
        } else {
            self.loaded
                .resolved
                .lookup(self.module_index, Namespace::Value, q)
                .ok()
                .map(|b| b.qualified.clone())
        };
        if let Some(name) = &global
            && let Some(f) = self.jit.funcs.get(name.as_str())
        {
            return Ok(Denotes::Compiled(f.clone()));
        }
        if let Some(name) = &global
            && self.loaded.definition(name.as_str()).is_some()
        {
            return Ok(Denotes::Uncompiled(name.to_string()));
        }
        let ctor = global.clone().or_else(|| {
            if q.is_bare() && self.jit.layouts.ctors.iter().any(|(n, _)| n == q.symbol()) {
                Some(q.symbol().clone())
            } else {
                None
            }
        });
        if let Some(name) = ctor
            && let Some(index) = self.jit.layouts.ctors.iter().position(|(n, _)| *n == name)
        {
            let arity = self.jit.layouts.ctors[index].1;
            if arity == 0 {
                let handle = self.intern(Value::ctor(name, Vec::new()));
                return Ok(Denotes::Constant(handle));
            }
            return Ok(Denotes::Ctor(index, arity));
        }
        if q.is_bare()
            && let Some(b) = Builtin::from_name(q.symbol())
        {
            if let Err(why) = admissible_builtin(b) {
                return self.refuse(why);
            }
            let index = match self.jit.builtins.iter().position(|x| *x == b) {
                Some(i) => i,
                None => {
                    self.jit.builtins.push(b);
                    self.jit.builtins.len() - 1
                }
            };
            return Ok(Denotes::Builtin(index));
        }
        self.refuse(format!(
            "the name `{}` denotes nothing this spike knows",
            q.symbol()
        ))
    }

    /// The representation an expression will produce, decided before its branches are compiled so a
    /// join can be given a block parameter.
    fn kind_of(&mut self, code: &Code, scope: &Scope) -> Result<Kind> {
        let key = std::rc::Rc::as_ptr(code);
        if let Some(kind) = self.kinds.get(&key) {
            return Ok(*kind);
        }
        let kind = self.kind_of_uncached(code, scope)?;
        self.kinds.insert(key, kind);
        Ok(kind)
    }

    fn kind_of_uncached(&mut self, code: &Code, scope: &Scope) -> Result<Kind> {
        Ok(match &code.kind {
            NodeKind::Lit(Lit::Int(_), _) => Kind::Int,
            NodeKind::Lit(Lit::Fixed { ty, .. }, _) if carried_width(*ty) => Kind::Num(*ty),
            NodeKind::Lit(Lit::Bool(_), _) => Kind::Bool,
            NodeKind::Lit(..) => Kind::Boxed,
            NodeKind::Var { name: q, .. } => match self.denotation(q, scope)? {
                Denotes::Local(v) => v.kind,
                _ => Kind::Boxed,
            },
            // A direct call answers in the callee's register kind; anything else answers a handle.
            NodeKind::App { func, args } => match &func.kind {
                NodeKind::Var { name: q, .. } => match self.denotation(q, scope)? {
                    Denotes::Compiled(f) if f.arity == args.len() => f.sig.ret,
                    Denotes::Builtin(index)
                        if inline_builtin_answers(self.jit.builtins[index], args.len()) =>
                    {
                        let b = self.jit.builtins[index];
                        match width_builtin(b, args.len()) {
                            Some(Some(t)) => Kind::Num(t),
                            Some(None) => Kind::Int,
                            // The wrapping family answers at its operand's width, which the
                            // argument's own kind is what says.
                            None if scalar_builtin_answers(b, args.len()) => {
                                match self.kind_of(&args[0], scope)? {
                                    Kind::Num(t) => Kind::Num(t),
                                    _ => Kind::Int,
                                }
                            }
                            None => Kind::Int,
                        }
                    }
                    _ => Kind::Boxed,
                },
                _ => Kind::Boxed,
            },
            // A scalar field of a local whose record type the checker fixed, as the field arm
            // of `expr` answers it.
            NodeKind::Field { base, field } => match &base.kind {
                NodeKind::Var { name: q, .. } => match self.denotation(q, scope)? {
                    Denotes::Local(v) => match self.static_field(v.ty, &field.name) {
                        Some((_, ty)) => match self.jit.tys[ty as usize] {
                            Ty::Int => Kind::Int,
                            Ty::Bool => Kind::Bool,
                            Ty::Num(t) => Kind::Num(t),
                            _ => Kind::Boxed,
                        },
                        None => Kind::Boxed,
                    },
                    _ => Kind::Boxed,
                },
                _ => Kind::Boxed,
            },
            NodeKind::Unary { op, operand } => match op {
                UnOp::Not => Kind::Bool,
                UnOp::Neg | UnOp::BitNot => match self.kind_of(operand, scope)? {
                    Kind::Num(t) => Kind::Num(t),
                    _ => Kind::Int,
                },
            },
            NodeKind::Binary { op, lhs, rhs } => match op {
                BinOp::BitAnd
                | BinOp::BitOr
                | BinOp::BitXor
                | BinOp::Shl
                | BinOp::Shr
                | BinOp::Ushr
                | BinOp::Add
                | BinOp::Sub
                | BinOp::Mul
                | BinOp::Div
                | BinOp::Rem => {
                    // The answer has the operands' type. A shift's right operand is a count and
                    // never says the width, so the left is asked first.
                    match self.kind_of(lhs, scope)? {
                        Kind::Num(t) => Kind::Num(t),
                        _ => match self.kind_of(rhs, scope)? {
                            Kind::Num(t)
                                if !matches!(op, BinOp::Shl | BinOp::Shr | BinOp::Ushr) =>
                            {
                                Kind::Num(t)
                            }
                            _ => Kind::Int,
                        },
                    }
                }
                BinOp::Eq
                | BinOp::Ne
                | BinOp::Lt
                | BinOp::Le
                | BinOp::Gt
                | BinOp::Ge
                | BinOp::And
                | BinOp::Or => Kind::Bool,
                BinOp::Concat => Kind::Boxed,
            },
            NodeKind::If {
                then_branch,
                else_branch,
                ..
            } => {
                let a = self.kind_of(then_branch, scope)?;
                let b = self.kind_of(else_branch, scope)?;
                if a == b { a } else { Kind::Boxed }
            }
            // A block with no tail answers `Unit`, which is boxed, so it falls through to the arm
            // below rather than restating it.
            NodeKind::Block {
                stmts,
                tail: Some(t),
            } => {
                let mut inner = scope.clone();
                for s in stmts.iter() {
                    if let Stmt::Let { pat, value, .. } = s
                        && let Pat::Var { name, .. } = pat
                    {
                        let kind = self.kind_of(value, &inner)?;
                        inner.push((
                            name.name.clone(),
                            Val {
                                kind,
                                v: cranelift_codegen::ir::Value::from_u32(0),
                                ty: 0,
                                home: 0,
                            },
                        ));
                    }
                }
                self.kind_of(t, &inner)?
            }
            _ => Kind::Boxed,
        })
    }

    /// [`Fx::coerce`] into the sixty-four-bit register the typed ABI passes every position in.
    /// A fixed-width value travels there as its *value*, extended, rather than as a tagged word:
    /// two instructions at each boundary and none in between.
    fn abi_out(&mut self, val: Val, to: Kind) -> cranelift_codegen::ir::Value {
        let v = self.coerce(val, to);
        match to {
            Kind::Num(t) => {
                if t.signed() {
                    self.builder.ins().sextend(types::I64, v)
                } else {
                    self.builder.ins().uextend(types::I64, v)
                }
            }
            _ => v,
        }
    }

    /// The other side of [`Fx::abi_out`]: a value arriving from one.
    fn abi_in(
        &mut self,
        v: cranelift_codegen::ir::Value,
        kind: Kind,
    ) -> cranelift_codegen::ir::Value {
        match kind {
            Kind::Num(t) => self.builder.ins().ireduce(clif_int(t), v),
            _ => v,
        }
    }

    fn coerce(&mut self, val: Val, to: Kind) -> cranelift_codegen::ir::Value {
        match to {
            Kind::Boxed => self.boxed(val),
            Kind::Int => self.as_int(val),
            Kind::Bool => self.as_bool(val),
            Kind::Num(t) => self.as_num(val, t),
        }
    }

    fn expr(&mut self, code: &Code, scope: &mut Scope) -> Result<Val> {
        match &code.kind {
            NodeKind::Lit(lit, _) => self.literal(lit),

            NodeKind::Var { name: q, .. } => match self.denotation(q, scope)? {
                Denotes::Local(v) => Ok(v),
                Denotes::Constant(handle) => Ok(self.constant(handle)),
                // A compiled function used as a value is a closure over nothing, through its
                // handle-ABI entry.
                Denotes::Compiled(f) => {
                    let index = self.function_index(f.entry);
                    let index = self.builder.ins().iconst(types::I64, index as i64);
                    let arity = self.builder.ins().iconst(types::I64, f.arity as i64);
                    let ptr = self.spill(&[]);
                    let zero = self.builder.ins().iconst(types::I64, 0);
                    let v = self.helper(self.jit.helpers.closure, &[index, arity, ptr, zero]);
                    Ok(Val {
                        kind: Kind::Boxed,
                        v,
                        ty: 0,
                        home: 0,
                    })
                }
                Denotes::Builtin(index) => {
                    let index = self.builder.ins().iconst(types::I64, index as i64);
                    let v = self.helper(self.jit.helpers.builtin_value, &[index]);
                    Ok(Val {
                        kind: Kind::Boxed,
                        v,
                        ty: 0,
                        home: 0,
                    })
                }
                Denotes::Ctor(index, _) => {
                    let index = self.builder.ins().iconst(types::I64, index as i64);
                    let v = self.helper(self.jit.helpers.ctor_value, &[index]);
                    Ok(Val {
                        kind: Kind::Boxed,
                        v,
                        ty: 0,
                        home: 0,
                    })
                }
                Denotes::Uncompiled(target) => self.refuse(format!(
                    "`{target}` is used as a value, and it is not in this compiled unit"
                )),
            },

            NodeKind::Unary { op, operand } => {
                let value = self.expr(operand, scope)?;
                match op {
                    UnOp::BitNot => {
                        // The pattern flipped is the *type's* pattern, so `~0u8` is `255u8`.
                        if let Kind::Num(t) = value.kind {
                            let a = self.as_num(value, t);
                            let v = self.builder.ins().bnot(a);
                            return Ok(Val {
                                kind: Kind::Num(t),
                                v,
                                ty: 0,
                                home: 0,
                            });
                        }
                        let a = self.as_int(value);
                        let v = self.builder.ins().bnot(a);
                        Ok(Val {
                            kind: Kind::Int,
                            v,
                            ty: 0,
                            home: 0,
                        })
                    }
                    UnOp::Not => {
                        let b = self.as_bool(value);
                        let one = self.builder.ins().iconst(types::I64, 1);
                        let v = self.builder.ins().bxor(b, one);
                        Ok(Val {
                            kind: Kind::Bool,
                            v,
                            ty: 0,
                            home: 0,
                        })
                    }
                    // `Int` like the rest of the fragment: `as_int` refuses a `Float` or `Decimal`
                    // operand at run time and the entry declines, which is what every other
                    // arithmetic node here already does.
                    UnOp::Neg => {
                        // Checked at the width like every other arithmetic node, which at an
                        // unsigned type refuses every operand but zero — what the type says.
                        if let Kind::Num(t) = value.kind {
                            let a = self.as_num(value, t);
                            let wide = if t.signed() {
                                self.builder.ins().sextend(types::I64, a)
                            } else {
                                self.builder.ins().uextend(types::I64, a)
                            };
                            let negated = self.builder.ins().ineg(wide);
                            return Ok(self.checked_narrow(negated, t, true));
                        }
                        let a = self.as_int(value);
                        let overflowed = self.builder.create_block();
                        let ok = self.builder.create_block();
                        let is_min = self.builder.ins().icmp_imm(IntCC::Equal, a, i64::MIN);
                        self.builder.ins().brif(is_min, overflowed, &[], ok, &[]);
                        self.builder.switch_to_block(overflowed);
                        self.builder.seal_block(overflowed);
                        let what = self.builder.ins().iconst(types::I64, 2);
                        self.helper_void(self.jit.helpers.overflow, &[what]);
                        self.builder.ins().jump(self.failure, &[]);
                        self.builder.switch_to_block(ok);
                        self.builder.seal_block(ok);
                        let v = self.builder.ins().ineg(a);
                        Ok(Val {
                            kind: Kind::Int,
                            v,
                            ty: 0,
                            home: 0,
                        })
                    }
                }
            }

            NodeKind::Binary { op, lhs, rhs } => self.binary(*op, lhs, rhs, scope),

            NodeKind::If {
                cond,
                then_branch,
                else_branch,
            } => {
                let kind = {
                    let a = self.kind_of(then_branch, scope)?;
                    let b = self.kind_of(else_branch, scope)?;
                    if a == b { a } else { Kind::Boxed }
                };
                let c = self.expr(cond, scope)?;
                let c = self.as_bool(c);
                let then_block = self.builder.create_block();
                let else_block = self.builder.create_block();
                let join = self.builder.create_block();
                self.builder.append_block_param(join, clif_kind(kind));
                self.builder.ins().brif(c, then_block, &[], else_block, &[]);

                self.builder.switch_to_block(then_block);
                self.builder.seal_block(then_block);
                let mut inner = scope.clone();
                // A branch's answer is owned: a local returned here is duplicated unless the
                // read is its last, so the join never aliases a binding at one count.
                let t = self.consumed(then_branch, &mut inner)?;
                let t_ty = t.ty;
                let t = self.coerce(t, kind);
                self.builder.ins().jump(join, &[BlockArg::Value(t)]);

                self.builder.switch_to_block(else_block);
                self.builder.seal_block(else_block);
                let mut inner = scope.clone();
                let e = self.consumed(else_branch, &mut inner)?;
                let e_ty = e.ty;
                let e = self.coerce(e, kind);
                self.builder.ins().jump(join, &[BlockArg::Value(e)]);

                self.builder.switch_to_block(join);
                self.builder.seal_block(join);
                Ok(Val {
                    kind,
                    v: self.builder.block_params(join)[0],
                    ty: if t_ty == e_ty { t_ty } else { 0 },
                    home: 0,
                })
            }

            NodeKind::Block { stmts, tail } => {
                let mut inner = scope.clone();
                let mark = self.homes.len();
                self.block_stmts(stmts, &mut inner)?;
                let answer = match tail {
                    Some(t) => self.consumed(t, &mut inner)?,
                    None => {
                        let handle = self.intern(Value::Unit);
                        self.constant(handle)
                    }
                };
                self.release_homes_from(mark);
                Ok(answer)
            }

            NodeKind::Match { scrutinee, arms } => self.match_expr(scrutinee, arms, scope),

            NodeKind::RecordUpdate { base, copies, sets } => {
                // The written fields in source order, then the base at its last use; the result's
                // shape is the whole field set, and each written field goes to its offset in it.
                let mut handles = Vec::with_capacity(sets.len());
                let mut set_tys = Vec::with_capacity(sets.len());
                for (_, value) in sets.iter() {
                    let v = self.consumed(value, scope)?;
                    set_tys.push(v.ty);
                    let h = self.boxed(v);
                    handles.push(h);
                }
                // A fully written literal copies nothing out of its base: the base is a hint that
                // a record dies here, released into its width's token when it does — and left
                // alone when it is still held elsewhere — and the literal is built as one, in the
                // token of its own width when there is one.
                if copies.is_empty() {
                    if self.last_use_of_local(base, scope) {
                        let dying = self.expr(base, scope)?;
                        self.moved(dying);
                        let w = self.boxed(dying);
                        self.release_record(w, dying.ty);
                    }
                    let mut names: Vec<Symbol> = sets.iter().map(|(n, _)| n.clone()).collect();
                    names.sort();
                    let shape = self.jit.layouts.shape(names);
                    let sorted = self.jit.layouts.shape_names(shape);
                    let at_of = |name: &Symbol| {
                        sets.iter()
                            .position(|(n, _)| n == name)
                            .expect("every field of the shape was written")
                    };
                    let ordered: Vec<cranelift_codegen::ir::Value> =
                        sorted.iter().map(|name| handles[at_of(name)]).collect();
                    let ty = self.jit.ty_id(Ty::Record(
                        sorted
                            .iter()
                            .map(|name| (name.clone(), set_tys[at_of(name)]))
                            .collect(),
                    ));
                    let v = self.record_of(shape, &ordered);
                    return Ok(Val {
                        kind: Kind::Boxed,
                        v,
                        ty,
                        home: 0,
                    });
                }
                let base = self.consumed(base, scope)?;
                // The result's type is the base's with the written fields' types replaced, when
                // the base's is known and names exactly these fields.
                let ty = match &self.jit.tys[base.ty as usize] {
                    Ty::Record(fields)
                        if fields.len() == sets.len() + copies.len()
                            && sets
                                .iter()
                                .map(|(n, _)| n)
                                .chain(copies.iter().map(|c| &c.name))
                                .all(|n| fields.iter().any(|(f, _)| f == n)) =>
                    {
                        let updated: Vec<(Symbol, u32)> = fields
                            .iter()
                            .map(|(f, t)| match sets.iter().position(|(n, _)| n == f) {
                                Some(i) => (f.clone(), set_tys[i]),
                                None => (f.clone(), *t),
                            })
                            .collect();
                        Some(Ty::Record(updated))
                    }
                    _ => None,
                };
                let ty = ty.map_or(0, |t| self.jit.ty_id(t));
                let base = self.boxed(base);
                let mut all: Vec<Symbol> = sets.iter().map(|(n, _)| n.clone()).collect();
                all.extend(copies.iter().map(|c| c.name.clone()));
                let shape = self.jit.layouts.shape(all);
                let offsets: Vec<cranelift_codegen::ir::Value> = sets
                    .iter()
                    .map(|(name, _)| {
                        let at = self
                            .jit
                            .layouts
                            .offset(shape, name)
                            .expect("a written field is in the shape it was interned into");
                        self.builder.ins().iconst(types::I64, at as i64)
                    })
                    .collect();
                let ptr = self.spill(&handles);
                let offsets = self.spill(&offsets);
                let shape = self.builder.ins().iconst(types::I64, i64::from(shape));
                let n = self.builder.ins().iconst(types::I64, handles.len() as i64);
                let v = self.helper(
                    self.jit.helpers.record_update,
                    &[shape, base, ptr, offsets, n],
                );
                self.check();
                Ok(Val {
                    kind: Kind::Boxed,
                    v,
                    ty,
                    home: 0,
                })
            }

            NodeKind::App { func, args, .. } => self.app(func, args, scope),

            NodeKind::Record { fields, .. } => {
                // Evaluated in source order, laid out in the shape's sorted order.
                let mut names = Vec::with_capacity(fields.len());
                let mut handles = Vec::with_capacity(fields.len());
                let mut tys = Vec::with_capacity(fields.len());
                for (name, value) in fields.iter() {
                    let v = self.consumed(value, scope)?;
                    tys.push(v.ty);
                    let h = self.boxed(v);
                    names.push(name.clone());
                    handles.push(h);
                }
                let shape = self.jit.layouts.shape(names.clone());
                let sorted = self.jit.layouts.shape_names(shape);
                let positions: Vec<usize> = sorted
                    .iter()
                    .map(|name| {
                        names
                            .iter()
                            .position(|n| n == name)
                            .expect("every field of the shape was written")
                    })
                    .collect();
                let ordered: Vec<cranelift_codegen::ir::Value> =
                    positions.iter().map(|at| handles[*at]).collect();
                let ty = self.jit.ty_id(Ty::Record(
                    sorted
                        .iter()
                        .zip(&positions)
                        .map(|(name, at)| (name.clone(), tys[*at]))
                        .collect(),
                ));
                let v = self.record_of(shape, &ordered);
                Ok(Val {
                    kind: Kind::Boxed,
                    v,
                    ty,
                    home: 0,
                })
            }

            NodeKind::Lambda {
                params,
                body,
                captures,
                ..
            } => {
                // The captured values are the closure's environment and the body's leading
                // parameters, in the order the lowering named them.
                let mut env = Vec::with_capacity(captures.len());
                for (name, own) in captures.names.iter().zip(&captures.owns) {
                    let Some((_, val)) = scope.iter().rev().find(|(s, _)| s == name) else {
                        return self.refuse(format!(
                            "a lambda capturing `{name}`, which is not a local of its body"
                        ));
                    };
                    let own = if self.pinned.contains(name) {
                        Own::Borrowed
                    } else {
                        *own
                    };
                    let val = self.captured(own, *val);
                    let handle = self.boxed(val);
                    env.push(handle);
                }
                let index = self.jit.functions.len();
                let sig = self.jit.entry_signature();
                let id = self.jit.module.declare_function(
                    &mangle(&format!("{}$lambda{index}", self.function)),
                    Linkage::Local,
                    &sig,
                )?;
                self.jit.functions.push(id);
                let mut full: Vec<Symbol> = captures.names.clone();
                full.extend(params.iter().cloned());
                self.jit.pending.push(Pending {
                    owner: self.function.clone(),
                    id,
                    params: full,
                    body: body.clone(),
                    module_index: self.module_index,
                });
                let ptr = self.spill(&env);
                let index = self.builder.ins().iconst(types::I64, index as i64);
                let arity = self.builder.ins().iconst(types::I64, params.len() as i64);
                let n = self.builder.ins().iconst(types::I64, env.len() as i64);
                let v = self.helper(self.jit.helpers.closure, &[index, arity, ptr, n]);
                Ok(Val {
                    kind: Kind::Boxed,
                    v,
                    ty: 0,
                    home: 0,
                })
            }

            NodeKind::Field { base: b, field } => {
                // 0: a read of a local's record, which stays; 1: the local's last use, so the
                // record is taken; 2: this field's last use while the record stays; 3: the base
                // is a temporary, taken so it pins nothing.
                let own = if self.last_use_of_local(b, scope) {
                    1
                } else if code.own == Own::OwnedField
                    && self.is_local(b, scope)
                    && !self.is_pinned(b)
                {
                    2
                } else if self.is_local(b, scope) {
                    0
                } else {
                    3
                };
                let base = self.expr(b, scope)?;
                let base_ty = base.ty;
                let known = self.static_field(base.ty, &field.name);
                if own == 1 {
                    self.moved(base);
                }
                let base = self.boxed(base);
                // A read of a record whose shape the checker fixed is a load at the field's
                // offset. A scalar field answers in a register whatever the mark, since moving
                // a scalar out of a dying record is the same as reading it; any other field is
                // held once more when the record stays, and taken out of the record — its slot
                // emptied when nobody else holds the record, held once more otherwise — when
                // the mark says the field or the record is at its last use.
                if let Some((at, ty)) = known {
                    let kind = match self.jit.tys[ty as usize] {
                        Ty::Int => Kind::Int,
                        Ty::Bool => Kind::Bool,
                        // A field of a carried width is a scalar like the two above it: read
                        // straight into a register of its own width, with no tag test — the
                        // 142 of them ADR 0036 counted per `round` are this line.
                        Ty::Num(t) => Kind::Num(t),
                        _ => Kind::Boxed,
                    };
                    let w = self.builder.ins().load(
                        types::I64,
                        MemFlags::trusted(),
                        base,
                        (HEADER + 8 * at) as i32,
                    );
                    let v = match kind {
                        Kind::Boxed if own == 0 => {
                            self.inc_inline(w);
                            w
                        }
                        Kind::Boxed => {
                            self.take_field_inline(base, at, w);
                            w
                        }
                        _ => self.coerce(
                            Val {
                                kind: Kind::Boxed,
                                v: w,
                                ty,
                                home: 0,
                            },
                            kind,
                        ),
                    };
                    // A record dying here whose width this body rebuilds is the next such
                    // literal's memory rather than the allocator's.
                    if own == 1 || own == 3 {
                        self.release_record(base, base_ty);
                    }
                    return Ok(Val {
                        kind,
                        v,
                        ty,
                        home: 0,
                    });
                }
                let index = self.field_index(&field.name);
                let index = self.builder.ins().iconst(types::I64, index);
                let own = self.builder.ins().iconst(types::I64, own);
                let v = self.helper(self.jit.helpers.field, &[base, index, own]);
                self.check();
                Ok(Val {
                    kind: Kind::Boxed,
                    v,
                    ty: known.map_or(0, |(_, ty)| ty),
                    home: 0,
                })
            }

            NodeKind::List { items } => {
                if items.is_empty() {
                    let v = self.builder.ins().iconst(types::I64, self.jit.empty_list);
                    return Ok(Val {
                        kind: Kind::Boxed,
                        v,
                        ty: 0,
                        home: 0,
                    });
                }
                let mut handles = Vec::with_capacity(items.len());
                for item in items.iter() {
                    let v = self.consumed(item, scope)?;
                    let h = self.boxed(v);
                    handles.push(h);
                }
                let ptr = self.spill(&handles);
                let n = self.builder.ins().iconst(types::I64, handles.len() as i64);
                let v = self.helper(self.jit.helpers.list, &[ptr, n]);
                Ok(Val {
                    kind: Kind::Boxed,
                    v,
                    ty: 0,
                    home: 0,
                })
            }
            NodeKind::Perform { effect, op, .. } => {
                self.refuse(format!("`perform {}.{}`", effect.symbol(), op))
            }
            NodeKind::Handle { .. } => self.refuse("a `handle`"),
            NodeKind::WithCell { .. } => self.refuse("a `with cell`"),
            NodeKind::Simulate { .. } => self.refuse("a `simulate`"),
            NodeKind::WithRegion { .. } => self.refuse("a `region` block"),
        }
    }

    /// A constant: its immortal word as an immediate when folding is on, and a rebuilt allocation
    /// when it is off.
    fn constant(&mut self, index: usize) -> Val {
        if self.jit.opts.fold_literals {
            let v = self
                .builder
                .ins()
                .iconst(types::I64, self.jit.const_words[index]);
            return Val {
                kind: Kind::Boxed,
                v,
                ty: 0,
                home: 0,
            };
        }
        let index = self.builder.ins().iconst(types::I64, index as i64);
        let v = self.helper(self.jit.helpers.lit, &[index]);
        Val {
            kind: Kind::Boxed,
            v,
            ty: 0,
            home: 0,
        }
    }

    /// A literal, or a refusal for the two the fragment has no arithmetic for.
    fn literal(&mut self, lit: &Lit) -> Result<Val> {
        match lit {
            Lit::Int(i) => Ok(Val {
                kind: Kind::Int,
                v: self.builder.ins().iconst(types::I64, *i),
                ty: 0,
                home: 0,
            }),
            Lit::Fixed { ty, bits } if carried_width(*ty) => Ok(Val {
                kind: Kind::Num(*ty),
                // `iconst` takes the pattern; at a narrow type Cranelift wants it inside the
                // width, which `raw` is and a sign-extended `bits` would not be.
                v: self.builder.ins().iconst(
                    clif_int(*ty),
                    (bits & (u64::MAX >> (64 - ty.bits()))) as i64,
                ),
                ty: 0,
                home: 0,
            }),
            Lit::Fixed { ty, .. } => self.refuse(format!(
                "a `{ty}` literal, whose width the fragment does not carry"
            )),
            Lit::Bool(b) => Ok(Val {
                kind: Kind::Bool,
                v: self.builder.ins().iconst(types::I64, i64::from(*b)),
                ty: 0,
                home: 0,
            }),
            Lit::Float(_) => self.refuse("a `Float` literal, which the fragment has no path for"),
            Lit::Decimal { .. } => {
                self.refuse("a `Decimal` literal, which the fragment has no path for")
            }
            other => {
                let value = match other {
                    Lit::Str(s) => Value::str(s),
                    Lit::Bytes(b) => Value::bytes(b),
                    _ => Value::Unit,
                };
                let handle = self.intern(value);
                Ok(self.constant(handle))
            }
        }
    }

    fn binary(&mut self, op: BinOp, lhs: &Code, rhs: &Code, scope: &mut Scope) -> Result<Val> {
        if matches!(op, BinOp::And | BinOp::Or) {
            let l = self.expr(lhs, scope)?;
            let l = self.as_bool(l);
            let rhs_block = self.builder.create_block();
            let join = self.builder.create_block();
            self.builder.append_block_param(join, types::I64);
            let short = self
                .builder
                .ins()
                .iconst(types::I64, i64::from(matches!(op, BinOp::Or)));
            if matches!(op, BinOp::And) {
                self.builder
                    .ins()
                    .brif(l, rhs_block, &[], join, &[BlockArg::Value(short)]);
            } else {
                self.builder
                    .ins()
                    .brif(l, join, &[BlockArg::Value(short)], rhs_block, &[]);
            }
            self.builder.switch_to_block(rhs_block);
            self.builder.seal_block(rhs_block);
            let mut inner = scope.clone();
            let r = self.expr(rhs, &mut inner)?;
            let r = self.as_bool(r);
            self.builder.ins().jump(join, &[BlockArg::Value(r)]);
            self.builder.switch_to_block(join);
            self.builder.seal_block(join);
            return Ok(Val {
                kind: Kind::Bool,
                v: self.builder.block_params(join)[0],
                ty: 0,
                home: 0,
            });
        }

        // `++` takes both operands, so a string built by appending to it grows in place.
        if matches!(op, BinOp::Concat) {
            let l = self.consumed(lhs, scope)?;
            let r = self.consumed(rhs, scope)?;
            let a = self.boxed(l);
            let b = self.boxed(r);
            let v = self.helper(self.jit.helpers.concat, &[a, b]);
            self.check();
            return Ok(Val {
                kind: Kind::Boxed,
                v,
                ty: 0,
                home: 0,
            });
        }
        let l = self.expr(lhs, scope)?;
        let r = self.expr(rhs, scope)?;
        match op {
            BinOp::BitAnd | BinOp::BitOr | BinOp::BitXor => {
                // At a fixed width the operators are the register's own, and the answer has the
                // operands' type: `~0u8` is `255u8` because the register is eight bits wide.
                if let Some(t) = Fx::num_width(l, r) {
                    let a = self.as_num(l, t);
                    let b = self.as_num(r, t);
                    let v = match op {
                        BinOp::BitAnd => self.builder.ins().band(a, b),
                        BinOp::BitOr => self.builder.ins().bor(a, b),
                        _ => self.builder.ins().bxor(a, b),
                    };
                    return Ok(Val {
                        kind: Kind::Num(t),
                        v,
                        ty: 0,
                        home: 0,
                    });
                }
                let a = self.as_int(l);
                let b = self.as_int(r);
                let v = match op {
                    BinOp::BitAnd => self.builder.ins().band(a, b),
                    BinOp::BitOr => self.builder.ins().bor(a, b),
                    _ => self.builder.ins().bxor(a, b),
                };
                Ok(Val {
                    kind: Kind::Int,
                    v,
                    ty: 0,
                    home: 0,
                })
            }
            BinOp::Shl | BinOp::Shr | BinOp::Ushr => {
                // The count is an `Int` whatever the word is, and the bound it must sit inside is
                // the *word's* width, as the interpreter refuses it.
                let width = match l.kind {
                    Kind::Num(t) => i64::from(t.bits()),
                    _ => 64,
                };
                let a = match l.kind {
                    Kind::Num(t) => self.as_num(l, t),
                    _ => self.as_int(l),
                };
                let n = self.as_int(r);
                let bad = self
                    .builder
                    .ins()
                    .icmp_imm(IntCC::UnsignedGreaterThanOrEqual, n, width);
                let refused = self.builder.create_block();
                let ok = self.builder.create_block();
                self.builder.ins().brif(bad, refused, &[], ok, &[]);
                self.builder.switch_to_block(refused);
                self.builder.seal_block(refused);
                self.helper_void(self.jit.helpers.shift_count, &[n]);
                self.builder.ins().jump(self.failure, &[]);
                self.builder.switch_to_block(ok);
                self.builder.seal_block(ok);
                if let Kind::Num(t) = l.kind {
                    let count = self.builder.ins().ireduce(clif_int(t), n);
                    // The two right shifts differ exactly where the type is signed; `>>` on an
                    // unsigned register fills with zeros either way, which is what the value says.
                    let v = match op {
                        BinOp::Shl => self.builder.ins().ishl(a, count),
                        BinOp::Shr if t.signed() => self.builder.ins().sshr(a, count),
                        _ => self.builder.ins().ushr(a, count),
                    };
                    return Ok(Val {
                        kind: Kind::Num(t),
                        v,
                        ty: 0,
                        home: 0,
                    });
                }
                let v = match op {
                    BinOp::Shl => self.builder.ins().ishl(a, n),
                    BinOp::Shr => self.builder.ins().sshr(a, n),
                    _ => self.builder.ins().ushr(a, n),
                };
                Ok(Val {
                    kind: Kind::Int,
                    v,
                    ty: 0,
                    home: 0,
                })
            }
            BinOp::Add | BinOp::Sub => {
                // Checked at the width, as `Int`'s is at sixty-four: the sum is taken wide, where
                // no pair of values of a carried width can overflow, and refused if it left the
                // type. Two instructions more than the wrapping spelling, and never taken.
                if let Some(t) = Fx::num_width(l, r) {
                    let a = self.as_num(l, t);
                    let b = self.as_num(r, t);
                    let (wa, wb) = if t.signed() {
                        (
                            self.builder.ins().sextend(types::I64, a),
                            self.builder.ins().sextend(types::I64, b),
                        )
                    } else {
                        (
                            self.builder.ins().uextend(types::I64, a),
                            self.builder.ins().uextend(types::I64, b),
                        )
                    };
                    let wide = if matches!(op, BinOp::Add) {
                        self.builder.ins().iadd(wa, wb)
                    } else {
                        self.builder.ins().isub(wa, wb)
                    };
                    return Ok(self.checked_narrow(wide, t, matches!(op, BinOp::Sub)));
                }
                let a = self.as_int(l);
                let b = self.as_int(r);
                let (v, carry) = if matches!(op, BinOp::Add) {
                    self.builder.ins().sadd_overflow(a, b)
                } else {
                    self.builder.ins().ssub_overflow(a, b)
                };
                let overflowed = self.builder.create_block();
                let ok = self.builder.create_block();
                self.builder.ins().brif(carry, overflowed, &[], ok, &[]);
                self.builder.switch_to_block(overflowed);
                self.builder.seal_block(overflowed);
                let what = self
                    .builder
                    .ins()
                    .iconst(types::I64, i64::from(matches!(op, BinOp::Sub)));
                self.helper_void(self.jit.helpers.overflow, &[what]);
                self.builder.ins().jump(self.failure, &[]);
                self.builder.switch_to_block(ok);
                self.builder.seal_block(ok);
                Ok(Val {
                    kind: Kind::Int,
                    v,
                    ty: 0,
                    home: 0,
                })
            }
            BinOp::Mul | BinOp::Div | BinOp::Rem => {
                // Widened to sixty-four and given to the same helper `Int` uses — which is where
                // the zero divisor is reported — then refused if the answer left the type. A
                // product of two values of a carried width is at most sixty-four bits, so the
                // helper's own overflow check cannot fire and the narrowing is the whole of it.
                if let Some(t) = Fx::num_width(l, r) {
                    let a = self.as_num(l, t);
                    let b = self.as_num(r, t);
                    let (wa, wb) = if t.signed() {
                        (
                            self.builder.ins().sextend(types::I64, a),
                            self.builder.ins().sextend(types::I64, b),
                        )
                    } else {
                        (
                            self.builder.ins().uextend(types::I64, a),
                            self.builder.ins().uextend(types::I64, b),
                        )
                    };
                    let code = match op {
                        BinOp::Mul => 0,
                        BinOp::Div => 1,
                        _ => 2,
                    };
                    let code = self.builder.ins().iconst(types::I64, code);
                    let wide = self.helper(self.jit.helpers.arith, &[code, wa, wb]);
                    self.check();
                    return Ok(self.checked_narrow(wide, t, false));
                }
                let a = self.as_int(l);
                let b = self.as_int(r);
                let code = match op {
                    BinOp::Mul => 0,
                    BinOp::Div => 1,
                    _ => 2,
                };
                let code = self.builder.ins().iconst(types::I64, code);
                let v = self.helper(self.jit.helpers.arith, &[code, a, b]);
                self.check();
                Ok(Val {
                    kind: Kind::Int,
                    v,
                    ty: 0,
                    home: 0,
                })
            }
            BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => {
                // By value, so an unsigned width compares unsigned and a signed one signed:
                // `255u8 > 0u8` and `i8_of_int(-128) < 0i8` are both true.
                let (a, b, signed) = match Fx::num_width(l, r) {
                    Some(t) => (self.as_num(l, t), self.as_num(r, t), t.signed()),
                    None => (self.as_int(l), self.as_int(r), true),
                };
                let cc = match (op, signed) {
                    (BinOp::Lt, true) => IntCC::SignedLessThan,
                    (BinOp::Le, true) => IntCC::SignedLessThanOrEqual,
                    (BinOp::Gt, true) => IntCC::SignedGreaterThan,
                    (_, true) => IntCC::SignedGreaterThanOrEqual,
                    (BinOp::Lt, false) => IntCC::UnsignedLessThan,
                    (BinOp::Le, false) => IntCC::UnsignedLessThanOrEqual,
                    (BinOp::Gt, false) => IntCC::UnsignedGreaterThan,
                    (_, false) => IntCC::UnsignedGreaterThanOrEqual,
                };
                let v = self.builder.ins().icmp(cc, a, b);
                let v = self.builder.ins().uextend(types::I64, v);
                Ok(Val {
                    kind: Kind::Bool,
                    v,
                    ty: 0,
                    home: 0,
                })
            }
            BinOp::Eq | BinOp::Ne => {
                let width = Fx::num_width(l, r);
                let native = (l.kind == Kind::Int && r.kind == Kind::Int)
                    || (l.kind == Kind::Bool && r.kind == Kind::Bool)
                    || width.is_some();
                let v = if native {
                    let cc = if matches!(op, BinOp::Eq) {
                        IntCC::Equal
                    } else {
                        IntCC::NotEqual
                    };
                    let (a, b) = match width {
                        Some(t) => (self.as_num(l, t), self.as_num(r, t)),
                        None if l.kind == Kind::Int => (self.as_int(l), self.as_int(r)),
                        None => (self.as_bool(l), self.as_bool(r)),
                    };
                    let c = self.builder.ins().icmp(cc, a, b);
                    self.builder.ins().uextend(types::I64, c)
                } else {
                    let a = self.boxed(l);
                    let b = self.boxed(r);
                    let eq = self.helper(self.jit.helpers.equal, &[a, b]);
                    self.check();
                    if matches!(op, BinOp::Eq) {
                        eq
                    } else {
                        let one = self.builder.ins().iconst(types::I64, 1);
                        self.builder.ins().bxor(eq, one)
                    }
                };
                Ok(Val {
                    kind: Kind::Bool,
                    v,
                    ty: 0,
                    home: 0,
                })
            }
            BinOp::Concat | BinOp::And | BinOp::Or => unreachable!("handled above"),
        }
    }

    /// Whether `step_of` will answer for this argument, decided before anything is evaluated so
    /// the generic path can still take the call whole.
    fn fusable_step(&mut self, code: &Code, arity: usize, scope: &mut Scope) -> Result<bool> {
        Ok(match &code.kind {
            NodeKind::Var { name: q, .. } => matches!(
                self.denotation(q, scope)?,
                Denotes::Compiled(f)
                    if f.arity == arity
                        && f.sig.params.iter().all(|k| *k != Kind::Bool)
                        && f.sig.ret != Kind::Bool
            ),
            NodeKind::Lambda { params, .. } => params.len() == arity,
            _ => false,
        })
    }

    /// The callback a fused loop calls directly, when the argument is one it can: a compiled
    /// function of the right arity, or a lambda literal of it, which is compiled as its own
    /// function and called through its entry with the captured values as leading arguments —
    /// held for the loop's duration and given to the callee once more per call, as a closure
    /// would hand them over.
    fn step_of(&mut self, code: &Code, arity: usize, scope: &mut Scope) -> Result<Option<Step>> {
        match &code.kind {
            NodeKind::Var { name: q, .. } => match self.denotation(q, scope)? {
                Denotes::Compiled(f)
                    if f.arity == arity
                        && f.sig.params.iter().all(|k| *k != Kind::Bool)
                        && f.sig.ret != Kind::Bool =>
                {
                    Ok(Some(Step::Typed(f)))
                }
                _ => Ok(None),
            },
            NodeKind::Lambda {
                params,
                body,
                captures,
                ..
            } if params.len() == arity => {
                for name in &captures.names {
                    if !scope.iter().any(|(s, _)| s == name) {
                        return self.refuse(format!(
                            "a lambda capturing `{name}`, which is not a local of its body"
                        ));
                    }
                }
                Ok(Some(Step::Inline {
                    params: params.to_vec(),
                    body: body.clone(),
                    captures: captures.names.clone(),
                }))
            }
            _ => Ok(None),
        }
    }

    /// One call of a fused loop's step, which owns its arguments and answers an owned value.
    fn call_step(&mut self, step: &Step, args: &[Val], scope: &Scope) -> Result<Val> {
        match step {
            Step::Inline {
                params,
                body,
                captures,
            } => {
                let mark = self.homes.len();
                let pin = self.pinned.len();
                self.pinned.extend(captures.iter().cloned());
                let mut inner = scope.clone();
                for (p, a) in params.iter().zip(args) {
                    let v = self.home(*a);
                    inner.push((p.clone(), v));
                }
                let r = self.consumed(body, &mut inner)?;
                self.release_homes_from(mark);
                self.pinned.truncate(pin);
                Ok(r)
            }
            Step::Typed(f) => {
                let mut vals = Vec::with_capacity(args.len() + 1);
                vals.push(self.ctx);
                let mut lent = Vec::new();
                for (i, (a, kind)) in args.iter().zip(&f.sig.params).enumerate() {
                    if f.sig.borrowed[i] && a.kind == Kind::Boxed {
                        lent.push(a.v);
                    }
                    let v = self.coerce(*a, *kind);
                    vals.push(v);
                }
                let callee = self
                    .jit
                    .module
                    .declare_func_in_func(f.typed, self.builder.func);
                let call = self.builder.ins().call(callee, &vals);
                let v = self.builder.inst_results(call)[0];
                self.check();
                for w in lent {
                    self.dec_inline(w);
                }
                Ok(Val {
                    kind: f.sig.ret,
                    v,
                    ty: f.sig.ret_ty,
                    home: 0,
                })
            }
        }
    }

    /// What a fused loop walks: a `range(lo, hi)` as the integers themselves, checked against
    /// the interpreter's limit, or any other expression as the list it must be — checked to be
    /// one, with its length read — walked by index.
    fn source_of(&mut self, code: &Code, scope: &mut Scope) -> Result<Walk> {
        if let NodeKind::App { func, args } = &code.kind
            && let NodeKind::Var { name: q, .. } = &func.kind
            && args.len() == 2
            && matches!(self.denotation(q, scope)?, Denotes::Builtin(i) if self.jit.builtins[i] == Builtin::Range)
        {
            let lo = self.consumed(&args[0], scope)?;
            let lo = self.as_int(lo);
            let hi = self.consumed(&args[1], scope)?;
            let hi = self.as_int(hi);
            let span = self.builder.ins().isub(hi, lo);
            let too_big =
                self.builder
                    .ins()
                    .icmp_imm(IntCC::SignedGreaterThan, span, rt::RANGE_LIMIT);
            let refuse = self.builder.create_block();
            let fine = self.builder.create_block();
            self.builder.ins().brif(too_big, refuse, &[], fine, &[]);
            self.builder.switch_to_block(refuse);
            self.builder.seal_block(refuse);
            self.helper_void(self.jit.helpers.bad_range, &[lo, hi]);
            self.check();
            self.builder.ins().jump(fine, &[]);
            self.builder.switch_to_block(fine);
            self.builder.seal_block(fine);
            return Ok(Walk::Range { lo, hi });
        }
        let list = self.consumed(code, scope)?;
        let list = self.boxed(list);
        let pointer = self.builder.create_block();
        let bad = self.builder.create_block();
        let fine = self.builder.create_block();
        let tagged = self.builder.ins().band_imm(list, 1);
        self.builder.ins().brif(tagged, bad, &[], pointer, &[]);
        self.builder.switch_to_block(pointer);
        self.builder.seal_block(pointer);
        let kind = self
            .builder
            .ins()
            .uload8(types::I32, MemFlags::trusted(), list, 4);
        let is = self
            .builder
            .ins()
            .icmp_imm(IntCC::Equal, kind, i64::from(KIND_LIST));
        self.builder.ins().brif(is, fine, &[], bad, &[]);
        self.builder.switch_to_block(bad);
        self.builder.seal_block(bad);
        let which = self.builder.ins().iconst(types::I64, 0);
        self.helper_void(self.jit.helpers.not_a_list, &[which, list]);
        self.check();
        self.builder.ins().jump(fine, &[]);
        self.builder.switch_to_block(fine);
        self.builder.seal_block(fine);
        let len = self.builder.ins().uload32(MemFlags::trusted(), list, 8);
        Ok(Walk::List { list, len })
    }

    /// `fold` over a `range` and `iterate`, as loops in this body calling their step directly
    /// when `step_of` admits it, rather than a list built and a closure called back through the
    /// runtime's loop. The interpreter's limits and refusals hold: a range past its limit and
    /// an `iterate` that runs out or answers the wrong constructor raise, and so decline.
    fn fused_loop(&mut self, b: Builtin, args: &[Code], scope: &mut Scope) -> Result<Option<Val>> {
        match b {
            // The pieces of a list literal are joined without the list.
            Builtin::BytesConcatAll if args.len() == 1 => {
                let NodeKind::List { items } = &args[0].kind else {
                    return Ok(None);
                };
                let mut handles = Vec::with_capacity(items.len());
                for item in items.iter() {
                    let v = self.consumed(item, scope)?;
                    let h = self.boxed(v);
                    handles.push(h);
                }
                let ptr = self.spill(&handles);
                let n = self.builder.ins().iconst(types::I64, handles.len() as i64);
                let v = self.helper(self.jit.helpers.bytes_join, &[ptr, n]);
                self.check();
                Ok(Some(Val {
                    kind: Kind::Boxed,
                    v,
                    ty: 0,
                    home: 0,
                }))
            }
            Builtin::Fold | Builtin::Map | Builtin::Filter
                if args.len() == if b == Builtin::Fold { 3 } else { 2 } =>
            {
                let step_at = args.len() - 1;
                let arity = if b == Builtin::Fold { 2 } else { 1 };
                if !self.fusable_step(&args[step_at], arity, scope)? {
                    return Ok(None);
                }
                // Left to right, as the interpreter evaluates them: the source, the seed, then
                // the step — whose captures are read when the lambda is.
                let walk = self.source_of(&args[0], scope)?;
                let init = if b == Builtin::Fold {
                    Some(self.consumed(&args[1], scope)?)
                } else {
                    None
                };
                let Some(step) = self.step_of(&args[step_at], arity, scope)? else {
                    // The source and the seed are already evaluated, so the generic path must
                    // not evaluate them again.
                    return self.refuse(format!(
                        "a `{}` whose step is not a call this body can make",
                        b.name()
                    ));
                };
                let acc_kind = match (&step, b) {
                    (Step::Typed(f), Builtin::Fold) => f.sig.params[0],
                    _ => Kind::Boxed,
                };
                let acc = match init {
                    Some(init) => self.coerce(init, acc_kind),
                    None => {
                        let ptr = self.spill(&[]);
                        let zero = self.builder.ins().iconst(types::I64, 0);
                        self.helper(self.jit.helpers.list, &[ptr, zero])
                    }
                };
                let (lo, hi) = match &walk {
                    Walk::Range { lo, hi } => (*lo, *hi),
                    Walk::List { len, .. } => (self.builder.ins().iconst(types::I64, 0), *len),
                };

                let header = self.builder.create_block();
                self.builder.append_block_param(header, types::I64);
                self.builder.append_block_param(header, types::I64);
                let body = self.builder.create_block();
                let exit = self.builder.create_block();
                self.builder.append_block_param(exit, types::I64);
                self.builder
                    .ins()
                    .jump(header, &[BlockArg::Value(lo), BlockArg::Value(acc)]);
                self.builder.switch_to_block(header);
                let i = self.builder.block_params(header)[0];
                let acc = self.builder.block_params(header)[1];
                let more = self.builder.ins().icmp(IntCC::SignedLessThan, i, hi);
                self.builder
                    .ins()
                    .brif(more, body, &[], exit, &[BlockArg::Value(acc)]);
                self.builder.switch_to_block(body);
                self.builder.seal_block(body);
                let x = match &walk {
                    Walk::Range { .. } => Val {
                        kind: Kind::Int,
                        v: i,
                        ty: 0,
                        home: 0,
                    },
                    Walk::List { list, .. } => {
                        let w = self.helper(self.jit.helpers.list_get, &[*list, i]);
                        Val {
                            kind: Kind::Boxed,
                            v: w,
                            ty: 0,
                            home: 0,
                        }
                    }
                };
                let acc_val = Val {
                    kind: acc_kind,
                    v: acc,
                    ty: 0,
                    home: 0,
                };
                let next = match b {
                    Builtin::Fold => {
                        let next = self.call_step(&step, &[acc_val, x], scope)?;
                        self.coerce(next, acc_kind)
                    }
                    Builtin::Map => {
                        let r = self.call_step(&step, &[x], scope)?;
                        let r = self.boxed(r);
                        self.helper(self.jit.helpers.list_push, &[acc, r])
                    }
                    _ => {
                        // The element is held once for the predicate, which takes it, and once
                        // for the answer, which keeps it or lets it go.
                        let xw = self.boxed(x);
                        self.inc_inline(xw);
                        let r = self.call_step(
                            &step,
                            &[Val {
                                kind: Kind::Boxed,
                                v: xw,
                                ty: 0,
                                home: 0,
                            }],
                            scope,
                        )?;
                        let keep = self.as_bool(r);
                        let kept = self.builder.create_block();
                        let dropped = self.builder.create_block();
                        let joined = self.builder.create_block();
                        self.builder.append_block_param(joined, types::I64);
                        self.builder.ins().brif(keep, kept, &[], dropped, &[]);
                        self.builder.switch_to_block(kept);
                        self.builder.seal_block(kept);
                        let pushed = self.helper(self.jit.helpers.list_push, &[acc, xw]);
                        self.builder.ins().jump(joined, &[BlockArg::Value(pushed)]);
                        self.builder.switch_to_block(dropped);
                        self.builder.seal_block(dropped);
                        self.dec_inline(xw);
                        self.builder.ins().jump(joined, &[BlockArg::Value(acc)]);
                        self.builder.switch_to_block(joined);
                        self.builder.seal_block(joined);
                        self.builder.block_params(joined)[0]
                    }
                };
                let i1 = self.builder.ins().iadd_imm(i, 1);
                self.builder
                    .ins()
                    .jump(header, &[BlockArg::Value(i1), BlockArg::Value(next)]);
                self.builder.seal_block(header);
                self.builder.switch_to_block(exit);
                self.builder.seal_block(exit);
                let v = self.builder.block_params(exit)[0];
                if let Walk::List { list, .. } = &walk {
                    self.dec_inline(*list);
                }
                Ok(Some(Val {
                    kind: acc_kind,
                    v,
                    ty: 0,
                    home: 0,
                }))
            }
            Builtin::Iterate if args.len() == 3 => {
                let (Some(stop), Some(go)) = (self.jit.layouts.stop, self.jit.layouts.go) else {
                    return Ok(None);
                };
                if !self.fusable_step(&args[2], 1, scope)? {
                    return Ok(None);
                }
                let seed = self.consumed(&args[0], scope)?;
                let seed_ty = seed.ty;
                let seed = self.boxed(seed);
                let budget = self.consumed(&args[1], scope)?;
                let budget = self.as_int(budget);
                let Some(step) = self.step_of(&args[2], 1, scope)? else {
                    return self.refuse("an `iterate` whose step is not a call this body can make");
                };

                let header = self.builder.create_block();
                self.builder.append_block_param(header, types::I64);
                self.builder.append_block_param(header, types::I64);
                let exit = self.builder.create_block();
                self.builder.append_block_param(exit, types::I64);
                let small = self.builder.create_block();
                let under = self
                    .builder
                    .ins()
                    .icmp_imm(IntCC::SignedLessThan, budget, 1);
                self.builder.ins().brif(
                    under,
                    small,
                    &[],
                    header,
                    &[BlockArg::Value(budget), BlockArg::Value(seed)],
                );
                self.builder.switch_to_block(small);
                self.builder.seal_block(small);
                let zero = self.builder.ins().iconst(types::I64, 0);
                self.helper_void(self.jit.helpers.iterate_bad, &[zero, budget]);
                self.check();
                self.builder.ins().jump(exit, &[BlockArg::Value(seed)]);

                self.builder.switch_to_block(header);
                let left = self.builder.block_params(header)[0];
                let state = self.builder.block_params(header)[1];
                let spent = self.builder.create_block();
                let body = self.builder.create_block();
                let none_left = self
                    .builder
                    .ins()
                    .icmp_imm(IntCC::SignedLessThanOrEqual, left, 0);
                self.builder.ins().brif(none_left, spent, &[], body, &[]);
                self.builder.switch_to_block(spent);
                self.builder.seal_block(spent);
                let one = self.builder.ins().iconst(types::I64, 1);
                self.helper_void(self.jit.helpers.iterate_bad, &[one, budget]);
                self.check();
                self.builder.ins().jump(exit, &[BlockArg::Value(state)]);

                self.builder.switch_to_block(body);
                self.builder.seal_block(body);
                if let Step::Inline {
                    params,
                    body: step_body,
                    captures,
                } = &step
                {
                    // The step's body in the loop, its `Continue` and `Stop` as jumps: no
                    // constructor is built and no call is made per step.
                    let mark = self.homes.len();
                    let pin = self.pinned.len();
                    self.pinned.extend(captures.iter().cloned());
                    let mut inner = scope.clone();
                    let v = self.home(Val {
                        kind: Kind::Boxed,
                        v: state,
                        ty: seed_ty,
                        home: 0,
                    });
                    inner.push((params[0].clone(), v));
                    let loop_ = Loop {
                        go,
                        stop,
                        header,
                        exit,
                        left,
                        state,
                    };
                    self.iterate_tail(step_body, &mut inner, mark, &loop_)?;
                    self.homes.truncate(mark);
                    self.pinned.truncate(pin);
                    self.builder.seal_block(header);
                    self.builder.switch_to_block(exit);
                    self.builder.seal_block(exit);
                    let v = self.builder.block_params(exit)[0];
                    return Ok(Some(Val {
                        kind: Kind::Boxed,
                        v,
                        ty: 0,
                        home: 0,
                    }));
                }
                let r = self.call_step(
                    &step,
                    &[Val {
                        kind: Kind::Boxed,
                        v: state,
                        ty: 0,
                        home: 0,
                    }],
                    scope,
                )?;
                let r = self.boxed(r);
                let loop_ = Loop {
                    go,
                    stop,
                    header,
                    exit,
                    left,
                    state,
                };
                self.unwrap_step(r, &loop_);
                self.builder.seal_block(header);
                self.builder.switch_to_block(exit);
                self.builder.seal_block(exit);
                let v = self.builder.block_params(exit)[0];
                Ok(Some(Val {
                    kind: Kind::Boxed,
                    v,
                    ty: 0,
                    home: 0,
                }))
            }
            _ => Ok(None),
        }
    }

    /// `r`, a step's answer — `Continue(x)` or `Stop(x)` — unwrapped in place: the payload is
    /// held once more, the constructor let go, and the loop continued or left.
    fn unwrap_step(&mut self, r: cranelift_codegen::ir::Value, loop_: &Loop) {
        let Loop {
            go,
            stop,
            header,
            exit,
            left,
            state,
        } = *loop_;
        let unwrap = self.builder.create_block();
        let check_ctor = self.builder.create_block();
        let bad = self.builder.create_block();
        let tagged = self.builder.ins().band_imm(r, 1);
        self.builder.ins().brif(tagged, bad, &[], check_ctor, &[]);
        self.builder.switch_to_block(check_ctor);
        self.builder.seal_block(check_ctor);
        let kind = self
            .builder
            .ins()
            .uload8(types::I32, MemFlags::trusted(), r, 4);
        let is_ctor =
            self.builder
                .ins()
                .icmp_imm(IntCC::Equal, kind, i64::from(crate::heap::KIND_CTOR));
        let len = self.builder.ins().uload32(MemFlags::trusted(), r, 8);
        let one_field = self.builder.ins().icmp_imm(IntCC::Equal, len, 1);
        let index = self.builder.ins().uload32(MemFlags::trusted(), r, 12);
        let is_stop = self
            .builder
            .ins()
            .icmp_imm(IntCC::Equal, index, i64::from(stop));
        let is_go = self
            .builder
            .ins()
            .icmp_imm(IntCC::Equal, index, i64::from(go));
        let named = self.builder.ins().bor(is_stop, is_go);
        let shaped = self.builder.ins().band(is_ctor, one_field);
        let ok = self.builder.ins().band(shaped, named);
        self.builder.ins().brif(ok, unwrap, &[], bad, &[]);
        self.builder.switch_to_block(bad);
        self.builder.seal_block(bad);
        let two = self.builder.ins().iconst(types::I64, 2);
        self.helper_void(self.jit.helpers.iterate_bad, &[two, r]);
        self.check();
        self.builder.ins().jump(exit, &[BlockArg::Value(state)]);
        self.builder.switch_to_block(unwrap);
        self.builder.seal_block(unwrap);
        let payload = self
            .builder
            .ins()
            .load(types::I64, MemFlags::trusted(), r, HEADER as i32);
        self.inc_inline(payload);
        self.dec_inline(r);
        let again = self.builder.create_block();
        self.builder
            .ins()
            .brif(is_stop, exit, &[BlockArg::Value(payload)], again, &[]);
        self.builder.switch_to_block(again);
        self.builder.seal_block(again);
        let left1 = self.builder.ins().iadd_imm(left, -1);
        self.builder
            .ins()
            .jump(header, &[BlockArg::Value(left1), BlockArg::Value(payload)]);
    }

    /// An inlined `iterate` step's body in tail position: `Continue(x)` jumps to the loop's
    /// header with `x` as the next state and `Stop(x)` to its exit, through an `if` or a block;
    /// anything else is evaluated and unwrapped as a called step's answer is. The step's own
    /// bindings are released on every path out.
    fn iterate_tail(
        &mut self,
        code: &Code,
        scope: &mut Scope,
        base: usize,
        loop_: &Loop,
    ) -> Result<()> {
        let answered = match &code.kind {
            NodeKind::App { func, args } if args.len() == 1 => match &func.kind {
                NodeKind::Var { name: q, .. } => match self.denotation(q, scope)? {
                    Denotes::Ctor(index, 1)
                        if index as u32 == loop_.go || index as u32 == loop_.stop =>
                    {
                        Some((index as u32, &args[0]))
                    }
                    _ => None,
                },
                _ => None,
            },
            _ => None,
        };
        if let Some((index, arg)) = answered {
            let v = self.consumed(arg, scope)?;
            let w = self.boxed(v);
            self.release_homes_since(base);
            if index == loop_.go {
                let left1 = self.builder.ins().iadd_imm(loop_.left, -1);
                self.builder
                    .ins()
                    .jump(loop_.header, &[BlockArg::Value(left1), BlockArg::Value(w)]);
            } else {
                self.builder.ins().jump(loop_.exit, &[BlockArg::Value(w)]);
            }
            return Ok(());
        }
        match &code.kind {
            NodeKind::If {
                cond,
                then_branch,
                else_branch,
            } => {
                let c = self.expr(cond, scope)?;
                let c = self.as_bool(c);
                let then_block = self.builder.create_block();
                let else_block = self.builder.create_block();
                self.builder.ins().brif(c, then_block, &[], else_block, &[]);
                self.builder.switch_to_block(then_block);
                self.builder.seal_block(then_block);
                let mut inner = scope.clone();
                self.iterate_tail(then_branch, &mut inner, base, loop_)?;
                self.builder.switch_to_block(else_block);
                self.builder.seal_block(else_block);
                let mut inner = scope.clone();
                self.iterate_tail(else_branch, &mut inner, base, loop_)
            }
            NodeKind::Block {
                stmts,
                tail: Some(t),
            } => {
                let mut inner = scope.clone();
                let mark = self.homes.len();
                self.block_stmts(stmts, &mut inner)?;
                self.iterate_tail(t, &mut inner, base, loop_)?;
                self.homes.truncate(mark);
                Ok(())
            }
            _ => {
                let r = self.consumed(code, scope)?;
                let r = self.boxed(r);
                self.release_homes_since(base);
                self.unwrap_step(r, loop_);
                Ok(())
            }
        }
    }

    /// A builtin `inline_builtin_answers` admits, in a register: the argument's kind is checked
    /// inline and its length or byte loaded; any other kind, or an index out of range, goes
    /// through the runtime's own path so the diagnostic is the interpreter's. Takes the
    /// arguments as the runtime would.
    fn inline_builtin(&mut self, index: usize, args: &[Code], scope: &mut Scope) -> Result<Val> {
        let b = self.jit.builtins[index];
        let mut vals = Vec::with_capacity(args.len());
        for a in args {
            vals.push(self.consumed(a, scope)?);
        }
        // `u32_of_int` and its siblings: a range test and a narrowing, with the refusal cold and
        // off the path. `int_of_u32` and its siblings are a widening and nothing else, since every
        // carried width fits an `Int`.
        if args.len() == 1
            && let Some(t) = b.converts_into()
            && carried_width(t)
        {
            let n = self.as_int(vals[0]);
            let low = self.builder.ins().iconst(types::I64, t.min() as i64);
            let high = self.builder.ins().iconst(types::I64, t.max() as i64);
            let under = self.builder.ins().icmp(IntCC::SignedLessThan, n, low);
            let over = self.builder.ins().icmp(IntCC::SignedGreaterThan, n, high);
            let bad = self.builder.ins().bor(under, over);
            let refused = self.builder.create_block();
            let ok = self.builder.create_block();
            self.builder.ins().brif(bad, refused, &[], ok, &[]);
            self.builder.switch_to_block(refused);
            self.builder.seal_block(refused);
            let which = self.builder.ins().iconst(types::I64, t as i64);
            self.helper_void(self.jit.helpers.not_that_width, &[which, n]);
            self.builder.ins().jump(self.failure, &[]);
            self.builder.switch_to_block(ok);
            self.builder.seal_block(ok);
            let v = self.builder.ins().ireduce(clif_int(t), n);
            return Ok(Val {
                kind: Kind::Num(t),
                v,
                ty: 0,
                home: 0,
            });
        }
        if args.len() == 1
            && let Some(t) = b.converts_from()
            && carried_width(t)
        {
            let v = self.as_wide(vals[0]);
            return Ok(Val {
                kind: Kind::Int,
                v,
                ty: 0,
                home: 0,
            });
        }
        if scalar_builtin_answers(b, args.len()) {
            // At a fixed width the whole family is one instruction on the register: wrapping is
            // what the register does, and a rotate turns the word it is.
            if let Kind::Num(t) = vals[0].kind {
                let a = self.as_num(vals[0], t);
                let v = match b {
                    Builtin::WrapAdd => {
                        let n = self.as_num(vals[1], t);
                        self.builder.ins().iadd(a, n)
                    }
                    Builtin::WrapSub => {
                        let n = self.as_num(vals[1], t);
                        self.builder.ins().isub(a, n)
                    }
                    Builtin::WrapMul => {
                        let n = self.as_num(vals[1], t);
                        self.builder.ins().imul(a, n)
                    }
                    // The count is an `Int`, taken modulo the width so every count names a
                    // rotation, and narrowed to the register the rotate turns.
                    _ => {
                        let n = self.as_int(vals[1]);
                        let count = self.builder.ins().srem_imm(n, i64::from(t.bits()));
                        let count = self.builder.ins().iadd_imm(count, i64::from(t.bits()));
                        let count = self.builder.ins().srem_imm(count, i64::from(t.bits()));
                        let count = self.builder.ins().ireduce(clif_int(t), count);
                        self.builder.ins().rotr(a, count)
                    }
                };
                return Ok(Val {
                    kind: Kind::Num(t),
                    v,
                    ty: 0,
                    home: 0,
                });
            }
            let a = self.as_int(vals[0]);
            let n = self.as_int(vals[1]);
            let v = match b {
                Builtin::WrapAdd => self.builder.ins().iadd(a, n),
                Builtin::WrapSub => self.builder.ins().isub(a, n),
                Builtin::WrapMul => self.builder.ins().imul(a, n),
                _ => {
                    let word = self.builder.ins().ireduce(types::I32, a);
                    let count = self.builder.ins().band_imm(n, 31);
                    let count = self.builder.ins().ireduce(types::I32, count);
                    let turned = self.builder.ins().rotr(word, count);
                    self.builder.ins().uextend(types::I64, turned)
                }
            };
            return Ok(Val {
                kind: Kind::Int,
                v,
                ty: 0,
                home: 0,
            });
        }
        let target = self.boxed(vals[0]);
        let at = (b == Builtin::BytesAt).then(|| self.as_int(vals[1]));
        let want = if b == Builtin::Len {
            KIND_LIST
        } else {
            KIND_BYTES
        };

        let pointer = self.builder.create_block();
        let fast = self.builder.create_block();
        let slow = self.builder.create_block();
        let join = self.builder.create_block();
        self.builder.append_block_param(join, types::I64);

        let tagged = self.builder.ins().band_imm(target, 1);
        self.builder.ins().brif(tagged, slow, &[], pointer, &[]);
        self.builder.switch_to_block(pointer);
        self.builder.seal_block(pointer);
        let kind = self
            .builder
            .ins()
            .uload8(types::I32, MemFlags::trusted(), target, 4);
        let is = self
            .builder
            .ins()
            .icmp_imm(IntCC::Equal, kind, i64::from(want));
        let len = self.builder.ins().uload32(MemFlags::trusted(), target, 8);
        match at {
            Some(i) => {
                // A negative index is a huge one unsigned, so one compare covers both bounds.
                let bounds = self.builder.create_block();
                self.builder.ins().brif(is, bounds, &[], slow, &[]);
                self.builder.switch_to_block(bounds);
                self.builder.seal_block(bounds);
                let inside = self.builder.ins().icmp(IntCC::UnsignedLessThan, i, len);
                self.builder.ins().brif(inside, fast, &[], slow, &[]);
            }
            None => {
                self.builder.ins().brif(is, fast, &[], slow, &[]);
            }
        }

        self.builder.switch_to_block(fast);
        self.builder.seal_block(fast);
        let answer = match at {
            Some(i) => {
                let base = self.builder.ins().iadd_imm(target, HEADER as i64);
                let addr = self.builder.ins().iadd(base, i);
                self.builder
                    .ins()
                    .uload8(types::I64, MemFlags::trusted(), addr, 0)
            }
            None => len,
        };
        self.dec_inline(target);
        self.builder.ins().jump(join, &[BlockArg::Value(answer)]);

        self.builder.switch_to_block(slow);
        self.builder.seal_block(slow);
        let mut handles = vec![target];
        if let Some(i) = at {
            handles.push(self.boxed(Val {
                kind: Kind::Int,
                v: i,
                ty: 0,
                home: 0,
            }));
        }
        let n = self.builder.ins().iconst(types::I64, handles.len() as i64);
        let ptr = self.spill(&handles);
        let which = self.builder.ins().iconst(types::I64, index as i64);
        let v = self.helper(self.jit.helpers.builtin, &[which, ptr, n]);
        self.check();
        let v = self.as_int(Val {
            kind: Kind::Boxed,
            v,
            ty: 0,
            home: 0,
        });
        self.builder.ins().jump(join, &[BlockArg::Value(v)]);

        self.builder.switch_to_block(join);
        self.builder.seal_block(join);
        Ok(Val {
            kind: Kind::Int,
            v: self.builder.block_params(join)[0],
            ty: 0,
            home: 0,
        })
    }

    fn app(&mut self, func: &Code, args: &[Code], scope: &mut Scope) -> Result<Val> {
        let NodeKind::Var { name: q, .. } = &func.kind else {
            // The callee is a value: evaluate it, then the arguments, then call through it.
            let callee = self.consumed(func, scope)?;
            let callee = self.boxed(callee);
            return self.call_value(callee, args, scope);
        };
        let denotes = self.denotation(q, scope)?;
        if let Denotes::Local(v) = denotes {
            let own = if self.is_pinned(func) {
                Own::Borrowed
            } else {
                func.own
            };
            let callee = self.captured(own, v);
            let callee = self.boxed(callee);
            return self.call_value(callee, args, scope);
        }
        if let Denotes::Compiled(f) = &denotes {
            if f.arity != args.len() {
                return self.refuse(format!(
                    "`{}` is called with {} arguments and takes {}",
                    q.symbol(),
                    args.len(),
                    f.arity
                ));
            }
            // A pure nullary definition whose value is a handle is remembered by the runtime's
            // memo; one that answers a register is cheaper to call than to look up.
            if f.arity == 0 && f.sig.ret == Kind::Boxed && self.jit.constants.contains(&f.typed) {
                let index = self.function_index(f.entry);
                let index = self.builder.ins().iconst(types::I64, index as i64);
                let v = self.helper(self.jit.helpers.constant, &[index]);
                self.check();
                return Ok(Val {
                    kind: Kind::Boxed,
                    v,
                    ty: 0,
                    home: 0,
                });
            }
            // A direct call: each argument in the register kind the callee's signature names. A
            // compiled callee owns its handle parameters — each is a last use or a fresh duplicate
            // by the read rule — and takes each at its own last use.
            let mut vals = Vec::with_capacity(args.len() + 1);
            vals.push(self.ctx);
            // A borrowed parameter takes a local as it is, the caller's hold outliving the
            // call, and a temporary with the hold this body has on it, let go afterwards.
            let mut lent = Vec::new();
            for (i, (a, kind)) in args.iter().zip(&f.sig.params).enumerate() {
                // A local passed as it is must outlive the call: a later argument that moves
                // it out — its last use — would free it first, so that case holds it.
                let moved_later = matches!(&a.kind, NodeKind::Var { name: q, .. }
                    if args[i + 1..].iter().any(|later| moves(later, q.symbol())));
                let v = if f.sig.borrowed[i] && self.is_local(a, scope) && !moved_later {
                    self.expr(a, scope)?
                } else {
                    let v = self.consumed(a, scope)?;
                    if f.sig.borrowed[i] && v.kind == Kind::Boxed {
                        lent.push(v.v);
                    }
                    v
                };
                let v = self.abi_out(v, *kind);
                vals.push(v);
            }
            let callee = self
                .jit
                .module
                .declare_func_in_func(f.typed, self.builder.func);
            let call = self.builder.ins().call(callee, &vals);
            let v = self.builder.inst_results(call)[0];
            let v = self.abi_in(v, f.sig.ret);
            self.check();
            for w in lent {
                self.dec_inline(w);
            }
            return Ok(Val {
                kind: f.sig.ret,
                v,
                ty: f.sig.ret_ty,
                home: 0,
            });
        }
        if let Denotes::Builtin(index) = &denotes
            && let Some(v) = self.fused_loop(self.jit.builtins[*index], args, scope)?
        {
            return Ok(v);
        }
        if let Denotes::Builtin(index) = &denotes
            && inline_builtin_answers(self.jit.builtins[*index], args.len())
        {
            return self.inline_builtin(*index, args, scope);
        }
        let mut handles = Vec::with_capacity(args.len());
        for a in args {
            let v = self.consumed(a, scope)?;
            let h = self.boxed(v);
            handles.push(h);
        }
        let n = self.builder.ins().iconst(types::I64, handles.len() as i64);
        let v = match denotes {
            Denotes::Compiled(_) => unreachable!("a compiled callee is called directly above"),
            Denotes::Uncompiled(target) => {
                return self.refuse(format!(
                    "a call to `{target}`, which is not in this compiled unit"
                ));
            }
            Denotes::Builtin(index) => {
                let b = self.jit.builtins[index];
                let want = match b {
                    Builtin::Map | Builtin::Filter => 2,
                    Builtin::Fold | Builtin::MapFold | Builtin::Iterate => 3,
                    _ => handles.len(),
                };
                if handles.len() != want {
                    return self.refuse(format!(
                        "`{}` is called with {} arguments and takes {want}",
                        b.name(),
                        handles.len()
                    ));
                }
                let h = &self.jit.helpers;
                let (list_at, push, map_insert, map_contains, map_get, compare, byte_of_int) = (
                    h.list_index,
                    h.push,
                    h.map_insert,
                    h.map_contains,
                    h.map_get,
                    h.compare,
                    h.byte_of_int,
                );
                let (bytes_scan, bytes_scan_until, bytes_slice, bytes_concat) = (
                    h.bytes_scan,
                    h.bytes_scan_until,
                    h.bytes_slice,
                    h.bytes_concat,
                );
                let v = match b {
                    // The values no body allocates, and the builtins called directly: no
                    // dispatch on the index, no argument array.
                    Builtin::MapNew if handles.is_empty() => {
                        self.builder.ins().iconst(types::I64, self.jit.empty_map)
                    }
                    Builtin::ListAt if handles.len() == 2 => {
                        self.helper(list_at, &[handles[0], handles[1]])
                    }
                    Builtin::Push if handles.len() == 2 => {
                        self.helper(push, &[handles[0], handles[1]])
                    }
                    Builtin::MapInsert if handles.len() == 3 => {
                        self.helper(map_insert, &[handles[0], handles[1], handles[2]])
                    }
                    Builtin::MapContains if handles.len() == 2 => {
                        self.helper(map_contains, &[handles[0], handles[1]])
                    }
                    Builtin::MapGet if handles.len() == 2 => {
                        self.helper(map_get, &[handles[0], handles[1]])
                    }
                    Builtin::Compare if handles.len() == 2 => {
                        self.helper(compare, &[handles[0], handles[1]])
                    }
                    Builtin::ByteOfInt if handles.len() == 1 => {
                        self.helper(byte_of_int, &[handles[0]])
                    }
                    Builtin::BytesScan if handles.len() == 4 => self.helper(
                        bytes_scan,
                        &[handles[0], handles[1], handles[2], handles[3]],
                    ),
                    Builtin::BytesScanUntil if handles.len() == 4 => self.helper(
                        bytes_scan_until,
                        &[handles[0], handles[1], handles[2], handles[3]],
                    ),
                    Builtin::BytesSlice if handles.len() == 3 => {
                        self.helper(bytes_slice, &[handles[0], handles[1], handles[2]])
                    }
                    Builtin::BytesConcat if handles.len() == 2 => {
                        self.helper(bytes_concat, &[handles[0], handles[1]])
                    }
                    Builtin::Map => self.helper(self.jit.helpers.map, &[handles[0], handles[1]]),
                    Builtin::Filter => {
                        self.helper(self.jit.helpers.filter, &[handles[0], handles[1]])
                    }
                    Builtin::Fold => {
                        self.helper(self.jit.helpers.fold, &[handles[0], handles[1], handles[2]])
                    }
                    Builtin::MapFold => self.helper(
                        self.jit.helpers.map_fold,
                        &[handles[0], handles[1], handles[2]],
                    ),
                    Builtin::Iterate => self.helper(
                        self.jit.helpers.iterate,
                        &[handles[0], handles[1], handles[2]],
                    ),
                    _ => {
                        let ptr = self.spill(&handles);
                        let index = self.builder.ins().iconst(types::I64, index as i64);
                        self.helper(self.jit.helpers.builtin, &[index, ptr, n])
                    }
                };
                self.check();
                v
            }
            Denotes::Ctor(index, arity) => {
                if arity != handles.len() {
                    return self.refuse(format!(
                        "the constructor `{}` is applied to {} of its {arity} fields",
                        q.symbol(),
                        handles.len()
                    ));
                }
                if arity == 0 {
                    self.builder
                        .ins()
                        .iconst(types::I64, self.jit.nullaries[index])
                } else {
                    self.built_fresh(crate::heap::KIND_CTOR, index as u32, &handles)
                }
            }
            Denotes::Local(_) => unreachable!("a local callee is called through `call_value`"),
            Denotes::Constant(_) => {
                return self.refuse(format!("`{}` is not a function", q.symbol()));
            }
        };
        Ok(Val {
            kind: Kind::Boxed,
            v,
            ty: 0,
            home: 0,
        })
    }

    /// A call through a closure handle: the arguments, then `rt_call`, which enters a native
    /// closure directly and answers a builtin or a constructor the way the interpreter would.
    fn call_value(
        &mut self,
        callee: cranelift_codegen::ir::Value,
        args: &[Code],
        scope: &mut Scope,
    ) -> Result<Val> {
        let mut handles = Vec::with_capacity(args.len());
        for a in args {
            let v = self.consumed(a, scope)?;
            let h = self.boxed(v);
            handles.push(h);
        }
        let ptr = self.spill(&handles);
        let n = self.builder.ins().iconst(types::I64, handles.len() as i64);
        let v = self.helper(self.jit.helpers.call, &[callee, ptr, n]);
        self.check();
        Ok(Val {
            kind: Kind::Boxed,
            v,
            ty: 0,
            home: 0,
        })
    }

    /// The index `rt_closure` names a compiled function by.
    fn function_index(&mut self, id: FuncId) -> usize {
        match self.jit.functions.iter().position(|f| *f == id) {
            Some(i) => i,
            None => {
                self.jit.functions.push(id);
                self.jit.functions.len() - 1
            }
        }
    }

    /// The constructor a name in *pattern* position denotes, resolved the way
    /// `ply_eval::Machine::ctor_name` resolves it, and `None` for a name that denotes nothing —
    /// which is what makes it a binder.
    fn ctor_of(&self, q: &QName) -> Option<(usize, usize)> {
        let global = if q.is_bare() {
            self.loaded
                .resolved
                .scopes
                .get(self.module_index)
                .and_then(|s| s.get(Namespace::Value, q.symbol()))
                .map(|b| b.qualified.clone())
        } else {
            self.loaded
                .resolved
                .lookup(self.module_index, Namespace::Value, q)
                .ok()
                .map(|b| b.qualified.clone())
        };
        let name = global.or_else(|| {
            if q.is_bare() && self.jit.layouts.ctors.iter().any(|(n, _)| n == q.symbol()) {
                Some(q.symbol().clone())
            } else {
                None
            }
        })?;
        let index = self
            .jit
            .layouts
            .ctors
            .iter()
            .position(|(n, _)| *n == name)?;
        Some((index, self.jit.layouts.ctors[index].1))
    }

    /// Whether a sub-pattern binds without being able to fail.
    fn irrefutable(&self, pat: &Pat) -> bool {
        match pat {
            Pat::Wildcard => true,
            Pat::Var { name: id, .. } => self
                .ctor_of(&QName::bare(id.clone()))
                .is_none_or(|(_, arity)| arity != 0),
            _ => false,
        }
    }

    /// Whether a pattern can be bound with no test at all — what a `let` may carry, since the
    /// checker has already matched the value's shape to it: a product of such patterns, or a
    /// list pattern that is only a rest.
    fn binds_without_test(&self, pat: &Pat) -> bool {
        match pat {
            Pat::Record { fields, .. } => {
                fields.iter().all(|(_, sub)| self.binds_without_test(sub))
            }
            Pat::List { items, rest } => {
                items.is_empty() && rest.as_ref().is_some_and(|r| self.binds_without_test(r))
            }
            _ => self.irrefutable(pat),
        }
    }

    /// Whether a pattern binds anything at all, at any depth, so [`Fx::bind_pattern`] can skip
    /// extracting a sub-value nothing will read.
    fn binds_any(&self, pat: &Pat) -> bool {
        match pat {
            Pat::Wildcard | Pat::Lit(_) => false,
            Pat::Var { .. } => self.binder(pat).is_some(),
            Pat::Ctor { args, .. } => args.iter().any(|a| self.binds_any(a)),
            Pat::Record { fields, .. } => fields.iter().any(|(_, p)| self.binds_any(p)),
            Pat::List { items, rest } => {
                items.iter().any(|p| self.binds_any(p))
                    || rest.as_ref().is_some_and(|r| self.binds_any(r))
            }
        }
    }

    /// The name a sub-pattern binds, and `None` for one that binds nothing — a wildcard, or a bare
    /// nullary constructor, which is a test rather than a binder ([`Fx::test_pattern`]).
    fn binder(&self, pat: &Pat) -> Option<Symbol> {
        let Pat::Var { name: id, .. } = pat else {
            return None;
        };
        match self.ctor_of(&QName::bare(id.clone())) {
            Some((_, 0)) => None,
            _ => Some(id.name.clone()),
        }
    }

    /// The refutable half of a pattern: leave the current block for `hit` when it matches and
    /// `miss` when it does not.
    fn test_pattern(&mut self, pat: &Pat, value: Val, hit: Block, miss: Block) -> Result<()> {
        match pat {
            Pat::Wildcard => {
                self.builder.ins().jump(hit, &[]);
            }
            Pat::Var { name: id, .. } => match self.ctor_of(&QName::bare(id.clone())) {
                Some((index, 0)) => self.test_ctor(value, index, 0, hit, miss),
                _ => {
                    self.builder.ins().jump(hit, &[]);
                }
            },
            Pat::Lit(lit) => {
                let literal = self.literal(lit)?;
                let native = (literal.kind == Kind::Int && value.kind == Kind::Int)
                    || (literal.kind == Kind::Bool && value.kind == Kind::Bool);
                let eq = if native {
                    let a = if value.kind == Kind::Int {
                        self.as_int(value)
                    } else {
                        self.as_bool(value)
                    };
                    let b = if literal.kind == Kind::Int {
                        self.as_int(literal)
                    } else {
                        self.as_bool(literal)
                    };
                    self.builder.ins().icmp(IntCC::Equal, a, b)
                } else {
                    let a = self.boxed(value);
                    let b = self.boxed(literal);
                    let eq = self.helper(self.jit.helpers.equal, &[a, b]);
                    self.check();
                    self.builder.ins().icmp_imm(IntCC::NotEqual, eq, 0)
                };
                self.builder.ins().brif(eq, hit, &[], miss, &[]);
            }
            Pat::List { items, rest } => {
                // A refutable `rest` would need the tail built before it could be tested;
                // the corpus asks for none.
                if let Some(bad) = rest.iter().find(|p| !self.irrefutable(p)) {
                    return self.refuse(format!(
                        "a {} pattern as a list pattern's rest",
                        pattern_name(bad)
                    ));
                }
                let base = self.boxed(value);
                let len = self.builder.ins().iconst(types::I64, items.len() as i64);
                let exact = self
                    .builder
                    .ins()
                    .iconst(types::I64, i64::from(rest.is_none()));
                let fits = self.helper(self.jit.helpers.list_fits, &[base, len, exact]);
                let fits = self.builder.ins().icmp_imm(IntCC::NotEqual, fits, 0);
                let mut cur = self.builder.create_block();
                self.builder.ins().brif(fits, cur, &[], miss, &[]);
                for (i, item) in items.iter().enumerate() {
                    if self.irrefutable(item) {
                        continue;
                    }
                    self.builder.switch_to_block(cur);
                    self.builder.seal_block(cur);
                    let at = self.builder.ins().iconst(types::I64, i as i64);
                    let sub = self.helper(self.jit.helpers.list_at, &[base, at]);
                    self.check();
                    let next = self.builder.create_block();
                    self.test_pattern(
                        item,
                        Val {
                            kind: Kind::Boxed,
                            v: sub,
                            ty: 0,
                            home: 0,
                        },
                        next,
                        miss,
                    )?;
                    cur = next;
                }
                self.builder.switch_to_block(cur);
                self.builder.seal_block(cur);
                self.builder.ins().jump(hit, &[]);
            }
            Pat::Ctor { name, args } => {
                let Some((index, arity)) = self.ctor_of(name) else {
                    return self.refuse(format!(
                        "the constructor pattern `{}`, which names nothing this unit knows",
                        name.symbol()
                    ));
                };
                if args.len() != arity {
                    return self.refuse(format!(
                        "the constructor pattern `{}` binds {} of its {arity} fields",
                        name.symbol(),
                        args.len()
                    ));
                }
                // The tag first, then each argument that can still fail, in
                // `Machine::match_pattern`'s order: a sub-pattern may raise, and must not
                // run under a tag that already failed.
                let base = self.boxed(value);
                let boxed = Val {
                    kind: Kind::Boxed,
                    v: base,
                    ty: 0,
                    home: 0,
                };
                let mut cur = self.builder.create_block();
                self.test_ctor(boxed, index, arity, cur, miss);
                for (i, arg) in args.iter().enumerate() {
                    if self.irrefutable(arg) {
                        continue;
                    }
                    self.builder.switch_to_block(cur);
                    self.builder.seal_block(cur);
                    let sub = self.ctor_arg_inline(base, i, false);
                    let next = self.builder.create_block();
                    self.test_pattern(
                        arg,
                        Val {
                            kind: Kind::Boxed,
                            v: sub,
                            ty: 0,
                            home: 0,
                        },
                        next,
                        miss,
                    )?;
                    cur = next;
                }
                self.builder.switch_to_block(cur);
                self.builder.seal_block(cur);
                self.builder.ins().jump(hit, &[]);
            }
            Pat::Record { fields, rest } => {
                // Refutable despite the shape: a record pattern fails on a non-record, on
                // a field count without `..`, and on a missing field.
                let base = self.boxed(value);
                let len = self.builder.ins().iconst(types::I64, fields.len() as i64);
                let exact = self.builder.ins().iconst(types::I64, i64::from(!*rest));
                let fits = self.helper(self.jit.helpers.record_fits, &[base, len, exact]);
                let fits = self.builder.ins().icmp_imm(IntCC::NotEqual, fits, 0);
                let mut cur = self.builder.create_block();
                self.builder.ins().brif(fits, cur, &[], miss, &[]);
                for (name, sub) in fields {
                    self.builder.switch_to_block(cur);
                    self.builder.seal_block(cur);
                    let index = self.field_index(&name.name);
                    let index = self.builder.ins().iconst(types::I64, index);
                    let has = self.helper(self.jit.helpers.record_has, &[base, index]);
                    let has = self.builder.ins().icmp_imm(IntCC::NotEqual, has, 0);
                    let present = self.builder.create_block();
                    self.builder.ins().brif(has, present, &[], miss, &[]);
                    // Sealed by whoever ends up owning it: the next iteration's `cur`, or the
                    // tail. Sealing here as well is a double seal, which cranelift only rejects
                    // under `debug_assertions`.
                    if self.irrefutable(sub) {
                        cur = present;
                        continue;
                    }
                    self.builder.switch_to_block(present);
                    self.builder.seal_block(present);
                    let field = {
                        let borrowed = self.builder.ins().iconst(types::I64, 0);
                        self.helper(self.jit.helpers.field, &[base, index, borrowed])
                    };
                    self.check();
                    let next = self.builder.create_block();
                    self.test_pattern(
                        sub,
                        Val {
                            kind: Kind::Boxed,
                            v: field,
                            ty: 0,
                            home: 0,
                        },
                        next,
                        miss,
                    )?;
                    cur = next;
                }
                self.builder.switch_to_block(cur);
                self.builder.seal_block(cur);
                self.builder.ins().jump(hit, &[]);
            }
        }
        Ok(())
    }

    /// Whether `value` is the constructor at `index` with `arity` arguments: a pointer whose
    /// header says so, read inline.
    fn test_ctor(&mut self, value: Val, index: usize, arity: usize, hit: Block, miss: Block) {
        let v = self.boxed(value);
        let pointer = self.builder.create_block();
        let tagged = self.builder.ins().band_imm(v, 1);
        let tagged = self.builder.ins().icmp_imm(IntCC::NotEqual, tagged, 0);
        let empty = self.builder.ins().icmp_imm(IntCC::Equal, v, 0);
        let skip = self.builder.ins().bor(empty, tagged);
        self.builder.ins().brif(skip, miss, &[], pointer, &[]);
        self.builder.switch_to_block(pointer);
        self.builder.seal_block(pointer);
        let flags = MemFlags::trusted();
        let kind = self.builder.ins().uload8(types::I32, flags, v, 4);
        let is_ctor =
            self.builder
                .ins()
                .icmp_imm(IntCC::Equal, kind, i64::from(crate::heap::KIND_CTOR));
        let len = self.builder.ins().load(types::I32, flags, v, 8);
        let has_arity = self.builder.ins().icmp_imm(IntCC::Equal, len, arity as i64);
        let layout = self.builder.ins().load(types::I32, flags, v, 12);
        let is_index = self
            .builder
            .ins()
            .icmp_imm(IntCC::Equal, layout, index as i64);
        let is = self.builder.ins().band(is_ctor, has_arity);
        let is = self.builder.ins().band(is, is_index);
        self.builder.ins().brif(is, hit, &[], miss, &[]);
    }

    /// Argument `i` of a constructor value: a load, taken out of a constructor nothing else
    /// holds when `take` is set and held once more otherwise. A value whose header says it is
    /// not a constructor with that argument enters the runtime's path, which raises.
    fn ctor_arg_inline(
        &mut self,
        base: cranelift_codegen::ir::Value,
        i: usize,
        take: bool,
    ) -> cranelift_codegen::ir::Value {
        let fast = self.builder.create_block();
        let slow = self.builder.create_block();
        let done = self.builder.create_block();
        self.builder.append_block_param(done, types::I64);
        let flags = MemFlags::trusted();
        let tagged = self.builder.ins().band_imm(base, 1);
        let tagged = self.builder.ins().icmp_imm(IntCC::NotEqual, tagged, 0);
        let empty = self.builder.ins().icmp_imm(IntCC::Equal, base, 0);
        let bad = self.builder.ins().bor(empty, tagged);
        let check = self.builder.create_block();
        self.builder.ins().brif(bad, slow, &[], check, &[]);
        self.builder.switch_to_block(check);
        self.builder.seal_block(check);
        let kind = self.builder.ins().uload8(types::I32, flags, base, 4);
        let is_ctor =
            self.builder
                .ins()
                .icmp_imm(IntCC::Equal, kind, i64::from(crate::heap::KIND_CTOR));
        let len = self.builder.ins().load(types::I32, flags, base, 8);
        let in_range = self
            .builder
            .ins()
            .icmp_imm(IntCC::UnsignedGreaterThan, len, i as i64);
        let ok = self.builder.ins().band(is_ctor, in_range);
        self.builder.ins().brif(ok, fast, &[], slow, &[]);
        self.builder.switch_to_block(fast);
        self.builder.seal_block(fast);
        let w = self
            .builder
            .ins()
            .load(types::I64, flags, base, (HEADER + 8 * i) as i32);
        if take {
            self.take_field_inline(base, i, w);
        } else {
            self.inc_inline(w);
        }
        self.builder.ins().jump(done, &[BlockArg::Value(w)]);
        self.builder.switch_to_block(slow);
        self.builder.seal_block(slow);
        let at = self.builder.ins().iconst(types::I64, i as i64);
        let taking = self.builder.ins().iconst(types::I64, i64::from(take));
        let v = self.helper(self.jit.helpers.ctor_arg, &[base, at, taking]);
        self.check();
        self.builder.ins().jump(done, &[BlockArg::Value(v)]);
        self.builder.switch_to_block(done);
        self.builder.seal_block(done);
        self.builder.block_params(done)[0]
    }

    /// The bindings a pattern makes, emitted in the block that has already committed to the arm.
    /// Binds a pattern's names. `moving`: the value is a temporary or a local at its last use,
    /// so a bound field or constructor argument is moved out of a record or constructor nothing
    /// else holds rather than cloned — a bound value is itself a temporary, so the binds below it
    /// always move.
    fn bind_pattern(
        &mut self,
        pat: &Pat,
        value: Val,
        moving: bool,
        scope: &mut Scope,
    ) -> Result<()> {
        match pat {
            Pat::Wildcard => {}
            Pat::Var { name: id, .. } => {
                if !matches!(self.ctor_of(&QName::bare(id.clone())), Some((_, 0))) {
                    let value = self.home(value);
                    scope.push((id.name.clone(), value));
                }
            }
            Pat::Lit(_) => {}
            Pat::List { items, rest } => {
                let base = self.boxed(value);
                for (i, item) in items.iter().enumerate() {
                    if !self.binds_any(item) {
                        continue;
                    }
                    let i = self.builder.ins().iconst(types::I64, i as i64);
                    let v = self.helper(self.jit.helpers.list_at, &[base, i]);
                    self.check();
                    self.bind_pattern(
                        item,
                        Val {
                            kind: Kind::Boxed,
                            v,
                            ty: 0,
                            home: 0,
                        },
                        true,
                        scope,
                    )?;
                }
                if let Some(rest) = rest
                    && let Some(name) = self.binder(rest)
                {
                    let from = self.builder.ins().iconst(types::I64, items.len() as i64);
                    let v = self.helper(self.jit.helpers.list_rest, &[base, from]);
                    self.check();
                    let v = self.home(Val {
                        kind: Kind::Boxed,
                        v,
                        ty: 0,
                        home: 0,
                    });
                    scope.push((name, v));
                }
            }
            Pat::Ctor { name, args } => {
                // What the checker says each payload is, where it said anything monomorphic.
                let payload_tys: Vec<u32> = match self.ctor_of(name) {
                    Some((index, _)) => {
                        let qualified = self.jit.layouts.ctors[index].0.clone();
                        match self.loaded.check.ctors.get(&qualified) {
                            Some(info) => {
                                let fields = info.fields.clone();
                                fields.iter().map(|t| self.jit.ty_of_type(t)).collect()
                            }
                            None => Vec::new(),
                        }
                    }
                    None => Vec::new(),
                };
                let base = self.boxed(value);
                for (i, arg) in args.iter().enumerate() {
                    if !self.binds_any(arg) {
                        continue;
                    }
                    let ty = payload_tys.get(i).copied().unwrap_or(0);
                    let v = self.ctor_arg_inline(base, i, moving);
                    self.bind_pattern(
                        arg,
                        Val {
                            kind: Kind::Boxed,
                            v,
                            ty,
                            home: 0,
                        },
                        true,
                        scope,
                    )?;
                }
            }
            Pat::Record { fields, .. } => {
                let base = self.boxed(value);
                for (name, sub) in fields {
                    if !self.binds_any(sub) {
                        continue;
                    }
                    let ty = self
                        .static_field(value.ty, &name.name)
                        .map_or(0, |(_, ty)| ty);
                    let index = self.field_index(&name.name);
                    let index = self.builder.ins().iconst(types::I64, index);
                    let v = {
                        // 2 moves the field out in place when the record is unshared; 0 reads.
                        let own = self
                            .builder
                            .ins()
                            .iconst(types::I64, if moving { 2 } else { 0 });
                        self.helper(self.jit.helpers.field, &[base, index, own])
                    };
                    self.check();
                    self.bind_pattern(
                        sub,
                        Val {
                            kind: Kind::Boxed,
                            v,
                            ty,
                            home: 0,
                        },
                        true,
                        scope,
                    )?;
                }
            }
        }
        Ok(())
    }

    /// A `match` over `map_get` or `list_at` whose arms only ask whether something was found —
    /// `Some(p)`, `None`, `_` — is a lookup answering the value or nothing, and no constructor
    /// is built: the arms test that word directly.
    fn lookup_match(
        &mut self,
        scrutinee: &Code,
        arms: &[Arm],
        scope: &mut Scope,
    ) -> Result<Option<Val>> {
        let NodeKind::App { func, args, .. } = &scrutinee.kind else {
            return Ok(None);
        };
        let NodeKind::Var { name: q, .. } = &func.kind else {
            return Ok(None);
        };
        if args.len() != 2 || !q.is_bare() || scope.iter().any(|(s, _)| s == q.symbol()) {
            return Ok(None);
        }
        let Denotes::Builtin(index) = self.denotation(q, scope)? else {
            return Ok(None);
        };
        let lookup = match self.jit.builtins[index] {
            Builtin::MapGet => self.jit.helpers.map_lookup,
            Builtin::ListAt => self.jit.helpers.list_lookup,
            _ => return Ok(None),
        };
        let (Some(some), Some(none)) = (self.jit.layouts.some, self.jit.layouts.none) else {
            return Ok(None);
        };
        // Which arms this shape serves: `Some(p)`, `None`, a wildcard, no guards.
        enum Shape<'a> {
            Found(&'a Pat),
            Missing,
            Any,
        }
        let mut shapes = Vec::with_capacity(arms.len());
        for arm in arms {
            if arm.guard.is_some() {
                return Ok(None);
            }
            let shape = match &arm.pat {
                Pat::Wildcard => Shape::Any,
                Pat::Ctor { name, args } => match self.ctor_of(name) {
                    Some((i, 1)) if i as u32 == some && args.len() == 1 => Shape::Found(&args[0]),
                    Some((i, 0)) if i as u32 == none && args.is_empty() => Shape::Missing,
                    _ => return Ok(None),
                },
                Pat::Var { name: id, .. } => match self.ctor_of(&QName::bare(id.clone())) {
                    Some((i, 0)) if i as u32 == none => Shape::Missing,
                    _ => return Ok(None),
                },
                _ => return Ok(None),
            };
            shapes.push(shape);
        }
        let mut handles = Vec::with_capacity(2);
        for a in args.iter() {
            let v = self.consumed(a, scope)?;
            let h = self.boxed(v);
            handles.push(h);
        }
        let found = self.helper(lookup, &[handles[0], handles[1]]);
        self.check();
        let present = self.builder.ins().icmp_imm(IntCC::NotEqual, found, 0);
        let join = self.builder.create_block();
        self.builder.append_block_param(join, types::I64);
        let mut next = self.builder.create_block();
        self.builder.ins().jump(next, &[]);
        let mut ty: Option<u32> = None;
        for (arm, shape) in arms.iter().zip(&shapes) {
            self.builder.switch_to_block(next);
            self.builder.seal_block(next);
            let body_block = self.builder.create_block();
            let after = self.builder.create_block();
            let value = Val {
                kind: Kind::Boxed,
                v: found,
                ty: 0,
                home: 0,
            };
            match shape {
                Shape::Found(p) => {
                    let inner_test = self.builder.create_block();
                    self.builder
                        .ins()
                        .brif(present, inner_test, &[], after, &[]);
                    self.builder.switch_to_block(inner_test);
                    self.builder.seal_block(inner_test);
                    self.test_pattern(p, value, body_block, after)?;
                }
                Shape::Missing => {
                    self.builder
                        .ins()
                        .brif(present, after, &[], body_block, &[]);
                }
                Shape::Any => {
                    self.builder.ins().jump(body_block, &[]);
                }
            }
            self.builder.switch_to_block(body_block);
            self.builder.seal_block(body_block);
            let mut inner = scope.clone();
            let mark = self.homes.len();
            // The value found is held once by this match; a pattern binding the whole of it
            // takes that hold, and any other pattern leaves it to be let go here.
            match shape {
                Shape::Found(p) => {
                    self.bind_pattern(p, value, true, &mut inner)?;
                    let binds_whole = matches!(p, Pat::Var { name: id, .. }
                        if !matches!(self.ctor_of(&QName::bare(id.clone())), Some((_, 0))));
                    if !binds_whole {
                        self.dec_inline(found);
                    }
                }
                Shape::Any => self.dec_inline(found),
                Shape::Missing => {}
            }
            let body = self.consumed(&arm.body, &mut inner)?;
            self.release_homes_from(mark);
            ty = match ty {
                None => Some(body.ty),
                Some(t) if t == body.ty => Some(t),
                Some(_) => Some(0),
            };
            let body = self.boxed(body);
            self.builder.ins().jump(join, &[BlockArg::Value(body)]);
            next = after;
        }
        self.builder.switch_to_block(next);
        self.builder.seal_block(next);
        self.dec_inline(found);
        self.helper_void(self.jit.helpers.no_match, &[]);
        self.builder.ins().jump(self.failure, &[]);
        self.builder.switch_to_block(join);
        self.builder.seal_block(join);
        Ok(Some(Val {
            kind: Kind::Boxed,
            v: self.builder.block_params(join)[0],
            ty: ty.unwrap_or(0),
            home: 0,
        }))
    }

    fn match_expr(&mut self, scrutinee: &Code, arms: &[Arm], scope: &mut Scope) -> Result<Val> {
        if let Some(v) = self.lookup_match(scrutinee, arms, scope)? {
            return Ok(v);
        }
        let value = self.expr(scrutinee, scope)?;
        let join = self.builder.create_block();
        self.builder.append_block_param(join, types::I64);
        let mut next = self.builder.create_block();
        self.builder.ins().jump(next, &[]);
        let mut ty: Option<u32> = None;
        for arm in arms {
            if arm.guard.is_some() {
                return self.refuse("a `match` arm with a guard");
            }
            self.builder.switch_to_block(next);
            self.builder.seal_block(next);
            let body_block = self.builder.create_block();
            let after = self.builder.create_block();
            self.test_pattern(&arm.pat, value, body_block, after)?;
            self.builder.switch_to_block(body_block);
            self.builder.seal_block(body_block);
            let mut inner = scope.clone();
            // A scrutinee that is a local still read afterwards keeps its fields; anything else
            // — a temporary, or a local at its last use — gives them up.
            let moving = !self.is_borrowed_local(scrutinee, scope);
            let mark = self.homes.len();
            self.bind_pattern(&arm.pat, value, moving, &mut inner)?;
            // A temporary scrutinee is held once by this match: a pattern binding the whole
            // of it took that hold into the binding, and any other pattern leaves the shell —
            // its fields taken — to be let go here, before the body runs.
            let binds_whole = matches!(&arm.pat, Pat::Var { name: id, .. }
                if !matches!(self.ctor_of(&QName::bare(id.clone())), Some((_, 0))));
            if !self.is_local(scrutinee, scope) && !binds_whole && value.kind == Kind::Boxed {
                // A record shell of a width this body builds is the next one's memory.
                match self.token_of(value.ty) {
                    Some((slot, _)) => self.reset_inline(value.v, slot, false),
                    None => self.dec_inline(value.v),
                }
            }
            let body = self.consumed(&arm.body, &mut inner)?;
            self.release_homes_from(mark);
            // The arms' type, when every arm agrees.
            ty = match ty {
                None => Some(body.ty),
                Some(t) if t == body.ty => Some(t),
                Some(_) => Some(0),
            };
            let body = self.boxed(body);
            self.builder.ins().jump(join, &[BlockArg::Value(body)]);
            next = after;
        }
        // Nothing matched. The machine raises here; so does this.
        self.builder.switch_to_block(next);
        self.builder.seal_block(next);
        self.helper_void(self.jit.helpers.no_match, &[]);
        self.builder.ins().jump(self.failure, &[]);

        self.builder.switch_to_block(join);
        self.builder.seal_block(join);
        Ok(Val {
            kind: Kind::Boxed,
            v: self.builder.block_params(join)[0],
            ty: ty.unwrap_or(0),
            home: 0,
        })
    }
}

fn pattern_name(p: &Pat) -> &'static str {
    match p {
        Pat::Wildcard => "wildcard",
        Pat::Var { .. } => "binding",
        Pat::Lit(_) => "literal",
        Pat::Ctor { .. } => "constructor",
        Pat::Record { .. } => "record",
        Pat::List { .. } => "list",
    }
}
