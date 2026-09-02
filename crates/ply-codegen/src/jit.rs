//! The fragment of the spike's fragment, compiled with Cranelift.

use crate::heap::{HEADER, Heap, KIND_BYTES, KIND_LIST, Layouts, Word};
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
use ply_syntax::ast::{BinOp, Lit, QName, UnOp};
use ply_syntax::resolve::Namespace;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
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
enum Kind {
    Int,
    Bool,
    Boxed,
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
    no_match: FuncId,
    lit: FuncId,
    equal: FuncId,
    concat: FuncId,
    record_fits: FuncId,
    record_has: FuncId,
    builtin: FuncId,
    ctor: FuncId,
    list: FuncId,
    list_fits: FuncId,
    list_at: FuncId,
    list_rest: FuncId,
    ctor_is: FuncId,
    ctor_arg: FuncId,
    record: FuncId,
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
    shift_count: FuncId,
    dup: FuncId,
    dec: FuncId,
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
}

/// What the code generator knows of a value's type at compile time: enough to read a record's
/// field at its offset rather than by name. A record type is its fields sorted, each with its own
/// type, which is the order the shape lays them out in.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum Ty {
    Unknown,
    Int,
    Bool,
    Record(Vec<(Symbol, u32)>),
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

fn kind_of_type(t: &ply_core::ty::Type) -> Kind {
    match t {
        ply_core::ty::Type::Con(name, args) if args.is_empty() => match name.as_str() {
            "Int" => Kind::Int,
            "Bool" => Kind::Bool,
            _ => Kind::Boxed,
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
        },
        ty if arity == 0 => Sig {
            params: Vec::new(),
            ret: kind_of_type(ty),
            param_tys: Vec::new(),
            ret_ty: jit.ty_of_type(ty),
        },
        _ => boxed,
    }
}

/// One compiled program, and the tables its runtime context needs.
pub struct Unit {
    module: JITModule,
    entries: HashMap<String, (FuncId, usize)>,
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
            jit.module.define_function(func.typed, &mut clif)?;
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
        Ok(Unit {
            module: jit.module,
            entries,
            tables: Rc::new(Tables {
                consts: jit.consts,
                const_words: jit.const_words,
                layouts: jit.layouts,
                fields: jit.fields,
                builtins: jit.builtins,
                functions,
                memo: RefCell::new(Vec::new()),
                immortals: RefCell::new(jit.immortals),
            }),
            nodes: jit.nodes,
            compile_nanos,
        })
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
                _ => Ty::Unknown,
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
            no_match: declare(&mut module, "rt_no_match", 1, false)?,
            lit: declare(&mut module, "rt_lit", 2, true)?,
            equal: declare(&mut module, "rt_equal", 3, true)?,
            concat: declare(&mut module, "rt_concat", 3, true)?,
            record_fits: declare(&mut module, "rt_record_fits", 4, true)?,
            record_has: declare(&mut module, "rt_record_has", 3, true)?,
            builtin: declare(&mut module, "rt_builtin", 4, true)?,
            ctor: declare(&mut module, "rt_ctor", 4, true)?,
            list: declare(&mut module, "rt_list", 3, true)?,
            list_fits: declare(&mut module, "rt_list_fits", 4, true)?,
            list_at: declare(&mut module, "rt_list_at", 3, true)?,
            list_rest: declare(&mut module, "rt_list_rest", 3, true)?,
            ctor_is: declare(&mut module, "rt_ctor_is", 3, true)?,
            ctor_arg: declare(&mut module, "rt_ctor_arg", 4, true)?,
            record: declare(&mut module, "rt_record", 4, true)?,
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
            shift_count: declare(&mut module, "rt_shift_count", 2, false)?,
            dup: declare(&mut module, "rt_dup", 2, true)?,
            dec: declare(&mut module, "rt_dec", 2, false)?,
            constant: declare(&mut module, "rt_constant", 2, true)?,
        };

        let mut jit = Jit {
            module,
            opts,
            consts: Vec::new(),
            const_words: Vec::new(),
            layouts: Layouts::new(loaded.ctors()),
            immortals: Heap::persistent(),
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
            let body = crate::opt::optimize(loaded, module_index, def);
            let code = lower_fn(&params, &body).code;
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
            kinds: HashMap::new(),
        };

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
                    let v = fx.home(Val {
                        kind: *kind,
                        v: block_params[i + 1],
                        ty: sig.param_tys[i],
                        home: 0,
                    });
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
        let answer = fx.coerce(result, sig.map_or(Kind::Boxed, |s| s.ret));
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
            kinds: HashMap::new(),
        };
        let mut args = vec![fx.ctx];
        for (i, kind) in func.sig.params.iter().enumerate() {
            let handle =
                fx.builder
                    .ins()
                    .load(types::I64, MemFlags::trusted(), args_ptr, (i * 8) as i32);
            let v = fx.coerce(
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
        let handle = fx.boxed(Val {
            kind: func.sig.ret,
            v: r,
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
    )
}

fn admissible_builtin(b: Builtin) -> Result<(), String> {
    if b.higher_order() && !lowered_callback(b) {
        return Err(format!("`{}`, a builtin that calls user code", b.name()));
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
    Lambda {
        id: FuncId,
        env: Vec<cranelift_codegen::ir::Value>,
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

    fn is_local(&self, code: &Code, scope: &Scope) -> bool {
        matches!(&code.kind, NodeKind::Var { name: q, .. }
            if q.is_bare() && scope.iter().any(|(s, _)| s == q.symbol()))
    }

    fn is_borrowed_local(&self, code: &Code, scope: &Scope) -> bool {
        code.own != Own::Owned && self.is_local(code, scope)
    }

    /// Whether `arg` is a local whose read here is the binding's last.
    fn last_use_of_local(&self, arg: &Code, scope: &Scope) -> bool {
        arg.own == Own::Owned
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
                        Kind::Int
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
                            _ => Kind::Boxed,
                        },
                        None => Kind::Boxed,
                    },
                    _ => Kind::Boxed,
                },
                _ => Kind::Boxed,
            },
            NodeKind::Unary { op, .. } => match op {
                UnOp::Not => Kind::Bool,
                UnOp::Neg | UnOp::BitNot => Kind::Int,
            },
            NodeKind::Binary { op, .. } => match op {
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
                | BinOp::Rem => Kind::Int,
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

    fn coerce(&mut self, val: Val, to: Kind) -> cranelift_codegen::ir::Value {
        match to {
            Kind::Boxed => self.boxed(val),
            Kind::Int => self.as_int(val),
            Kind::Bool => self.as_bool(val),
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
                self.builder.append_block_param(join, types::I64);
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
                for s in stmts.iter() {
                    match s {
                        Stmt::Let { pat, value, .. } => {
                            let v = self.consumed(value, &mut inner)?;
                            match pat {
                                Pat::Var { name, .. } => {
                                    let v = self.home(v);
                                    inner.push((name.name.clone(), v));
                                }
                                Pat::Wildcard => {}
                                other if self.binds_without_test(other) => {
                                    self.bind_pattern(other, v, true, &mut inner)?;
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
                            let v = self.expr(code, &mut inner)?;
                            if v.kind == Kind::Boxed && !self.is_local(code, &inner) {
                                self.dec_inline(v.v);
                            }
                        }
                    }
                }
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
                let ptr = self.spill(&ordered);
                let shape = self.builder.ins().iconst(types::I64, i64::from(shape));
                let n = self.builder.ins().iconst(types::I64, ordered.len() as i64);
                let v = self.helper(self.jit.helpers.record, &[shape, ptr, n]);
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
                    let val = self.captured(*own, *val);
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
                } else if code.own == Own::OwnedField && self.is_local(b, scope) {
                    2
                } else if self.is_local(b, scope) {
                    0
                } else {
                    3
                };
                let base = self.expr(b, scope)?;
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
                    if own == 1 || own == 3 {
                        self.dec_inline(base);
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
                let a = self.as_int(l);
                let n = self.as_int(r);
                // The interpreter refuses a count outside `0..64` rather than masking it.
                let bad = self
                    .builder
                    .ins()
                    .icmp_imm(IntCC::UnsignedGreaterThanOrEqual, n, 64);
                let refused = self.builder.create_block();
                let ok = self.builder.create_block();
                self.builder.ins().brif(bad, refused, &[], ok, &[]);
                self.builder.switch_to_block(refused);
                self.builder.seal_block(refused);
                self.helper_void(self.jit.helpers.shift_count, &[n]);
                self.builder.ins().jump(self.failure, &[]);
                self.builder.switch_to_block(ok);
                self.builder.seal_block(ok);
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
                let a = self.as_int(l);
                let b = self.as_int(r);
                let cc = match op {
                    BinOp::Lt => IntCC::SignedLessThan,
                    BinOp::Le => IntCC::SignedLessThanOrEqual,
                    BinOp::Gt => IntCC::SignedGreaterThan,
                    _ => IntCC::SignedGreaterThanOrEqual,
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
                let native = (l.kind == Kind::Int && r.kind == Kind::Int)
                    || (l.kind == Kind::Bool && r.kind == Kind::Bool);
                let v = if native {
                    let cc = if matches!(op, BinOp::Eq) {
                        IntCC::Equal
                    } else {
                        IntCC::NotEqual
                    };
                    let a = if l.kind == Kind::Int {
                        self.as_int(l)
                    } else {
                        self.as_bool(l)
                    };
                    let b = if r.kind == Kind::Int {
                        self.as_int(r)
                    } else {
                        self.as_bool(r)
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
                let mut env = Vec::with_capacity(captures.len());
                for (name, own) in captures.names.iter().zip(&captures.owns) {
                    let Some((_, val)) = scope.iter().rev().find(|(s, _)| s == name) else {
                        return self.refuse(format!(
                            "a lambda capturing `{name}`, which is not a local of its body"
                        ));
                    };
                    let val = self.captured(*own, *val);
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
                Ok(Some(Step::Lambda { id, env }))
            }
            _ => Ok(None),
        }
    }

    /// One call of a fused loop's step, which owns its arguments and answers an owned value.
    fn call_step(&mut self, step: &Step, args: &[Val]) -> Val {
        match step {
            Step::Typed(f) => {
                let mut vals = Vec::with_capacity(args.len() + 1);
                vals.push(self.ctx);
                for (a, kind) in args.iter().zip(&f.sig.params) {
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
                Val {
                    kind: f.sig.ret,
                    v,
                    ty: f.sig.ret_ty,
                    home: 0,
                }
            }
            Step::Lambda { id, env } => {
                let mut handles = Vec::with_capacity(env.len() + args.len());
                for w in env {
                    self.inc_inline(*w);
                    handles.push(*w);
                }
                for a in args {
                    let h = self.boxed(*a);
                    handles.push(h);
                }
                let ptr = self.spill(&handles);
                let callee = self.jit.module.declare_func_in_func(*id, self.builder.func);
                let call = self.builder.ins().call(callee, &[self.ctx, ptr]);
                let v = self.builder.inst_results(call)[0];
                self.check();
                Val {
                    kind: Kind::Boxed,
                    v,
                    ty: 0,
                    home: 0,
                }
            }
        }
    }

    /// What a fused loop held for its step, let go once the loop is done.
    fn release_step(&mut self, step: Step) {
        if let Step::Lambda { env, .. } = step {
            for w in env {
                self.dec_inline(w);
            }
        }
    }

    /// `fold` over a `range` and `iterate`, as loops in this body calling their step directly
    /// when `step_of` admits it, rather than a list built and a closure called back through the
    /// runtime's loop. The interpreter's limits and refusals hold: a range past its limit and
    /// an `iterate` that runs out or answers the wrong constructor raise, and so decline.
    fn fused_loop(&mut self, b: Builtin, args: &[Code], scope: &mut Scope) -> Result<Option<Val>> {
        match b {
            Builtin::Fold if args.len() == 3 => {
                let NodeKind::App {
                    func: range,
                    args: bounds,
                } = &args[0].kind
                else {
                    return Ok(None);
                };
                let NodeKind::Var { name: q, .. } = &range.kind else {
                    return Ok(None);
                };
                if bounds.len() != 2
                    || !matches!(self.denotation(q, scope)?, Denotes::Builtin(i) if self.jit.builtins[i] == Builtin::Range)
                    || !matches!(
                        &args[2].kind,
                        NodeKind::Var { .. } | NodeKind::Lambda { .. }
                    )
                {
                    return Ok(None);
                }
                // Left to right, as the interpreter evaluates them: the bounds, the seed, then
                // the step — whose captures are read when the lambda is.
                let lo = self.consumed(&bounds[0], scope)?;
                let lo = self.as_int(lo);
                let hi = self.consumed(&bounds[1], scope)?;
                let hi = self.as_int(hi);
                let init = self.consumed(&args[1], scope)?;
                let Some(step) = self.step_of(&args[2], 2, scope)? else {
                    // Nothing is emitted for a step this cannot call; the bounds and the seed are
                    // already evaluated, so the generic path must not evaluate them again.
                    return self.refuse(
                        "a `fold` over a range whose step is not a call this body can make",
                    );
                };
                let acc_kind = match &step {
                    Step::Typed(f) => f.sig.params[0],
                    Step::Lambda { .. } => Kind::Boxed,
                };
                let acc = self.coerce(init, acc_kind);

                let span = self.builder.ins().isub(hi, lo);
                let too_big =
                    self.builder
                        .ins()
                        .icmp_imm(IntCC::SignedGreaterThan, span, rt::RANGE_LIMIT);
                let refuse = self.builder.create_block();
                let header = self.builder.create_block();
                self.builder.ins().brif(
                    too_big,
                    refuse,
                    &[],
                    header,
                    &[BlockArg::Value(lo), BlockArg::Value(acc)],
                );
                self.builder.switch_to_block(refuse);
                self.builder.seal_block(refuse);
                self.helper_void(self.jit.helpers.bad_range, &[lo, hi]);
                self.check();
                self.builder
                    .ins()
                    .jump(header, &[BlockArg::Value(lo), BlockArg::Value(acc)]);

                self.builder.append_block_param(header, types::I64);
                self.builder.append_block_param(header, types::I64);
                let body = self.builder.create_block();
                let exit = self.builder.create_block();
                self.builder.append_block_param(exit, types::I64);
                self.builder.switch_to_block(header);
                let i = self.builder.block_params(header)[0];
                let acc = self.builder.block_params(header)[1];
                let more = self.builder.ins().icmp(IntCC::SignedLessThan, i, hi);
                self.builder
                    .ins()
                    .brif(more, body, &[], exit, &[BlockArg::Value(acc)]);
                self.builder.switch_to_block(body);
                self.builder.seal_block(body);
                let acc_val = Val {
                    kind: acc_kind,
                    v: acc,
                    ty: 0,
                    home: 0,
                };
                let x = Val {
                    kind: Kind::Int,
                    v: i,
                    ty: 0,
                    home: 0,
                };
                let next = self.call_step(&step, &[acc_val, x]);
                let next = self.coerce(next, acc_kind);
                let i1 = self.builder.ins().iadd_imm(i, 1);
                self.builder
                    .ins()
                    .jump(header, &[BlockArg::Value(i1), BlockArg::Value(next)]);
                self.builder.seal_block(header);
                self.builder.switch_to_block(exit);
                self.builder.seal_block(exit);
                let v = self.builder.block_params(exit)[0];
                self.release_step(step);
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
                if !matches!(
                    &args[2].kind,
                    NodeKind::Var { .. } | NodeKind::Lambda { .. }
                ) {
                    return Ok(None);
                }
                let seed = self.consumed(&args[0], scope)?;
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
                let r = self.call_step(
                    &step,
                    &[Val {
                        kind: Kind::Boxed,
                        v: state,
                        ty: 0,
                        home: 0,
                    }],
                );
                let r = self.boxed(r);
                // `Continue(x)` or `Stop(x)`, unwrapped in place: the payload is held once more
                // and the constructor let go.
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
                let is_ctor = self.builder.ins().icmp_imm(
                    IntCC::Equal,
                    kind,
                    i64::from(crate::heap::KIND_CTOR),
                );
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
                let payload =
                    self.builder
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
                self.builder.seal_block(header);
                self.builder.switch_to_block(exit);
                self.builder.seal_block(exit);
                let v = self.builder.block_params(exit)[0];
                self.release_step(step);
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
            let callee = self.captured(func.own, v);
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
            for (a, kind) in args.iter().zip(&f.sig.params) {
                let v = self.consumed(a, scope)?;
                let v = self.coerce(v, *kind);
                vals.push(v);
            }
            let callee = self
                .jit
                .module
                .declare_func_in_func(f.typed, self.builder.func);
            let call = self.builder.ins().call(callee, &vals);
            let v = self.builder.inst_results(call)[0];
            self.check();
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
                let v = match b {
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
                let ptr = self.spill(&handles);
                let index = self.builder.ins().iconst(types::I64, index as i64);
                self.helper(self.jit.helpers.ctor, &[index, ptr, n])
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
                Some((index, 0)) => self.test_ctor(value, index, hit, miss),
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
                self.test_ctor(boxed, index, cur, miss);
                for (i, arg) in args.iter().enumerate() {
                    if self.irrefutable(arg) {
                        continue;
                    }
                    self.builder.switch_to_block(cur);
                    self.builder.seal_block(cur);
                    let at = self.builder.ins().iconst(types::I64, i as i64);
                    let read = self.builder.ins().iconst(types::I64, 0);
                    let sub = self.helper(self.jit.helpers.ctor_arg, &[base, at, read]);
                    self.check();
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

    fn test_ctor(&mut self, value: Val, index: usize, hit: Block, miss: Block) {
        let v = self.boxed(value);
        let index = self.builder.ins().iconst(types::I64, index as i64);
        let is = self.helper(self.jit.helpers.ctor_is, &[v, index]);
        let is = self.builder.ins().icmp_imm(IntCC::NotEqual, is, 0);
        self.builder.ins().brif(is, hit, &[], miss, &[]);
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
                    let i = self.builder.ins().iconst(types::I64, i as i64);
                    let take = self.builder.ins().iconst(types::I64, i64::from(moving));
                    let v = self.helper(self.jit.helpers.ctor_arg, &[base, i, take]);
                    self.check();
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

    fn match_expr(&mut self, scrutinee: &Code, arms: &[Arm], scope: &mut Scope) -> Result<Val> {
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
