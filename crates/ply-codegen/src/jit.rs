//! The fragment of ADR 0016 §3.2, compiled with Cranelift.

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
use ply_eval::code::{Arm, Stmt};
use ply_eval::{Builtin, Code, NodeKind, Value, lower};
use ply_span::Symbol;
use ply_syntax::ast::{BinOp, Lit, PatternKind, QName, UnOp};
use ply_syntax::resolve::Namespace;
use std::collections::HashMap;
use std::rc::Rc;

/// A compiled function: `extern "C" fn(ctx, args) -> handle`.
pub type Entry = unsafe extern "C" fn(*mut Ctx, *const i64) -> i64;

/// Refused rather than lowered: `<<` discards where the other arithmetic raises, and a shift
/// count outside `0..=63` raises where Cranelift's `ishl` masks it.
const BIT_OPERATORS: &str = "a bitwise operator or shift, which the fragment does not lower";

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
}

struct Helpers {
    box_int: FuncId,
    box_bool: FuncId,
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
    field: FuncId,
    no_fuel: FuncId,
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
    ctors: Vec<(Symbol, usize)>,
    shapes: Vec<Vec<Symbol>>,
    fields: Vec<Symbol>,
    builtins: Vec<Builtin>,
    funcs: HashMap<String, (FuncId, usize, usize)>,
    helpers: Helpers,
    nodes: HashMap<String, usize>,
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
            let (id, _, _) = jit.funcs[name];
            clif.func.signature = jit.entry_signature();
            jit.define(
                &mut clif,
                &mut fctx,
                loaded,
                name,
                params,
                body,
                *module_index,
            )?;
            jit.module.define_function(id, &mut clif)?;
        }
        jit.module.finalize_definitions()?;
        let compile_nanos = started.elapsed().as_nanos();

        let entries = jit
            .funcs
            .iter()
            .map(|(name, (id, arity, _))| (name.clone(), (*id, *arity)))
            .collect();
        Ok(Unit {
            module: jit.module,
            entries,
            tables: Rc::new(Tables {
                consts: jit.consts,
                ctors: jit.ctors,
                shapes: jit.shapes,
                fields: jit.fields,
                builtins: jit.builtins,
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
            clif.func.signature = jit.entry_signature();
            if let Err(e) = jit.define(
                &mut clif,
                &mut fctx,
                loaded,
                name,
                params,
                body,
                *module_index,
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
            box_bool: declare(&mut module, "rt_box_bool", 2, true)?,
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
            ctor_arg: declare(&mut module, "rt_ctor_arg", 3, true)?,
            record: declare(&mut module, "rt_record", 4, true)?,
            field: declare(&mut module, "rt_field", 3, true)?,
            no_fuel: declare(&mut module, "rt_no_fuel", 1, false)?,
        };

        let mut jit = Jit {
            module,
            opts,
            consts: Vec::new(),
            ctors: loaded.ctors(),
            shapes: Vec::new(),
            fields: Vec::new(),
            builtins: Vec::new(),
            funcs: HashMap::new(),
            helpers,
            nodes: HashMap::new(),
        };

        let mut bodies = Vec::new();
        for name in names {
            let (def, module_index) = loaded
                .definition(name)
                .ok_or_else(|| anyhow!("no definition named `{name}`"))?;
            let sig = jit.entry_signature();
            let id = jit
                .module
                .declare_function(&mangle(name), Linkage::Export, &sig)?;
            jit.funcs
                .insert((*name).to_string(), (id, def.params.len(), module_index));
            let params: Vec<Symbol> = def.params.iter().map(|p| p.name.name.clone()).collect();
            bodies.push(((*name).to_string(), params, lower(&def.body), module_index));
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
    ) -> Result<()> {
        let mut builder = FunctionBuilder::new(&mut clif.func, fctx);
        let entry = builder.create_block();
        builder.append_block_params_for_function_params(entry);
        builder.switch_to_block(entry);
        builder.seal_block(entry);
        let ctx_ptr = builder.block_params(entry)[0];
        let args_ptr = builder.block_params(entry)[1];

        let failure = builder.create_block();

        let mut fx = Fx {
            jit: self,
            builder,
            loaded,
            ctx: ctx_ptr,
            failure,
            function: name.to_string(),
            module_index,
        };

        // The prologue `ply_eval::limit` needs and ADR 0019 §5 item 6 records as missing: one
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
        for (i, p) in params.iter().enumerate() {
            let handle =
                fx.builder
                    .ins()
                    .load(types::I64, MemFlags::trusted(), args_ptr, (i * 8) as i32);
            scope.push((
                p.clone(),
                Val {
                    kind: Kind::Boxed,
                    v: handle,
                },
            ));
        }

        let result = fx.expr(body, &mut scope)?;
        let handle = fx.boxed(result);
        // The only path that gives the nested call back.
        let left = fx.load_fuel();
        let restored = fx.builder.ins().iadd_imm(left, 1);
        fx.store_fuel(restored);
        fx.builder.ins().return_(&[handle]);

        fx.builder.switch_to_block(failure);
        let zero = fx.builder.ins().iconst(types::I64, 0);
        fx.builder.ins().return_(&[zero]);

        fx.builder.seal_all_blocks();
        fx.builder.finalize();
        Ok(())
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
        NodeKind::Lit(..) | NodeKind::Var(_) => {}
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
        NodeKind::Field { base, .. } => n += count_nodes(base),
        NodeKind::List { items } => n += items.iter().map(count_nodes).sum::<usize>(),
        NodeKind::Perform { args, .. } => n += args.iter().map(count_nodes).sum::<usize>(),
        NodeKind::Handle { body, .. } => n += count_nodes(body),
        NodeKind::WithCell { init, body, .. } => n += count_nodes(init) + count_nodes(body),
        NodeKind::Simulate { body } => n += count_nodes(body),
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
fn admissible_builtin(b: Builtin) -> Result<(), String> {
    if b.higher_order() {
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

/// What a name denotes, decided at compile time in the order `Machine::lookup` decides it at run
/// time.
enum Denotes {
    Local(Val),
    Compiled(FuncId, usize),
    /// A Ply function this unit did not compile.
    Uncompiled(String),
    Ctor(usize, usize),
    Builtin(usize),
    Constant(i64),
}

struct Fx<'a, 'b> {
    jit: &'a mut Jit,
    builder: FunctionBuilder<'b>,
    loaded: &'static Source,
    ctx: cranelift_codegen::ir::Value,
    failure: cranelift_codegen::ir::Block,
    function: String,
    module_index: usize,
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

    fn intern(&mut self, value: Value) -> i64 {
        self.jit.consts.push(value);
        -(self.jit.consts.len() as i64)
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

    fn boxed(&mut self, val: Val) -> cranelift_codegen::ir::Value {
        match val.kind {
            Kind::Boxed => val.v,
            Kind::Int => self.helper(self.jit.helpers.box_int, &[val.v]),
            Kind::Bool => self.helper(self.jit.helpers.box_bool, &[val.v]),
        }
    }

    fn as_int(&mut self, val: Val) -> cranelift_codegen::ir::Value {
        match val.kind {
            Kind::Int => val.v,
            _ => {
                let handle = self.boxed(val);
                let v = self.helper(self.jit.helpers.unbox_int, &[handle]);
                self.check();
                v
            }
        }
    }

    fn as_bool(&mut self, val: Val) -> cranelift_codegen::ir::Value {
        match val.kind {
            Kind::Bool => val.v,
            _ => {
                let handle = self.boxed(val);
                let v = self.helper(self.jit.helpers.unbox_bool, &[handle]);
                self.check();
                v
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
            && let Some((id, arity, _)) = self.jit.funcs.get(name.as_str())
        {
            return Ok(Denotes::Compiled(*id, *arity));
        }
        if let Some(name) = &global
            && self.loaded.definition(name.as_str()).is_some()
        {
            return Ok(Denotes::Uncompiled(name.to_string()));
        }
        let ctor = global.clone().or_else(|| {
            if q.is_bare() && self.jit.ctors.iter().any(|(n, _)| n == q.symbol()) {
                Some(q.symbol().clone())
            } else {
                None
            }
        });
        if let Some(name) = ctor
            && let Some(index) = self.jit.ctors.iter().position(|(n, _)| *n == name)
        {
            let arity = self.jit.ctors[index].1;
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
        Ok(match &code.kind {
            NodeKind::Lit(Lit::Int(_), _) => Kind::Int,
            NodeKind::Lit(Lit::Bool(_), _) => Kind::Bool,
            NodeKind::Lit(..) => Kind::Boxed,
            NodeKind::Var(q) => match self.denotation(q, scope)? {
                Denotes::Local(v) => v.kind,
                _ => Kind::Boxed,
            },
            NodeKind::Unary { op, .. } => match op {
                UnOp::Not => Kind::Bool,
                UnOp::Neg => Kind::Int,
                UnOp::BitNot => return self.refuse(BIT_OPERATORS),
            },
            NodeKind::Binary { op, .. } => match op {
                BinOp::BitAnd
                | BinOp::BitOr
                | BinOp::BitXor
                | BinOp::Shl
                | BinOp::Shr
                | BinOp::Ushr => return self.refuse(BIT_OPERATORS),
                BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Rem => Kind::Int,
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
                        && let PatternKind::Var(name) = &pat.kind
                    {
                        let kind = self.kind_of(value, &inner)?;
                        inner.push((
                            name.name.clone(),
                            Val {
                                kind,
                                v: cranelift_codegen::ir::Value::from_u32(0),
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

            NodeKind::Var(q) => match self.denotation(q, scope)? {
                Denotes::Local(v) => Ok(v),
                Denotes::Constant(handle) => Ok(self.constant(handle)),
                _ => self.refuse(format!(
                    "`{}` is used as a value rather than called",
                    q.symbol()
                )),
            },

            NodeKind::Unary { op, operand } => {
                if matches!(op, UnOp::BitNot) {
                    return self.refuse(BIT_OPERATORS);
                }
                let value = self.expr(operand, scope)?;
                match op {
                    // Unreachable past the guard above, and an arm rather than
                    // a wildcard so that a *new* `UnOp` fails to compile here
                    // instead of being refused by accident.
                    UnOp::BitNot => self.refuse(BIT_OPERATORS),
                    UnOp::Not => {
                        let b = self.as_bool(value);
                        let one = self.builder.ins().iconst(types::I64, 1);
                        let v = self.builder.ins().bxor(b, one);
                        Ok(Val {
                            kind: Kind::Bool,
                            v,
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
                        Ok(Val { kind: Kind::Int, v })
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
                let t = self.expr(then_branch, &mut inner)?;
                let t = self.coerce(t, kind);
                self.builder.ins().jump(join, &[BlockArg::Value(t)]);

                self.builder.switch_to_block(else_block);
                self.builder.seal_block(else_block);
                let mut inner = scope.clone();
                let e = self.expr(else_branch, &mut inner)?;
                let e = self.coerce(e, kind);
                self.builder.ins().jump(join, &[BlockArg::Value(e)]);

                self.builder.switch_to_block(join);
                self.builder.seal_block(join);
                Ok(Val {
                    kind,
                    v: self.builder.block_params(join)[0],
                })
            }

            NodeKind::Block { stmts, tail } => {
                let mut inner = scope.clone();
                for s in stmts.iter() {
                    match s {
                        Stmt::Let { pat, value, .. } => {
                            let v = self.expr(value, &mut inner)?;
                            match &pat.kind {
                                PatternKind::Var(name) => inner.push((name.name.clone(), v)),
                                PatternKind::Wildcard => {}
                                other => {
                                    return self.refuse(format!(
                                        "a `let` binding a {} pattern",
                                        pattern_name(other)
                                    ));
                                }
                            }
                        }
                        Stmt::Expr { code, .. } => {
                            self.expr(code, &mut inner)?;
                        }
                    }
                }
                match tail {
                    Some(t) => self.expr(t, &mut inner),
                    None => {
                        let handle = self.intern(Value::Unit);
                        Ok(self.constant(handle))
                    }
                }
            }

            NodeKind::Match { scrutinee, arms } => self.match_expr(scrutinee, arms, scope),

            NodeKind::App { func, args, .. } => self.app(func, args, scope),

            NodeKind::Record { fields, .. } => {
                let mut names = Vec::with_capacity(fields.len());
                let mut handles = Vec::with_capacity(fields.len());
                for (name, value) in fields.iter() {
                    let v = self.expr(value, scope)?;
                    let h = self.boxed(v);
                    names.push(name.clone());
                    handles.push(h);
                }
                let shape = self.jit.shapes.len();
                self.jit.shapes.push(names);
                let ptr = self.spill(&handles);
                let shape = self.builder.ins().iconst(types::I64, shape as i64);
                let n = self.builder.ins().iconst(types::I64, handles.len() as i64);
                let v = self.helper(self.jit.helpers.record, &[shape, ptr, n]);
                Ok(Val {
                    kind: Kind::Boxed,
                    v,
                })
            }

            NodeKind::Lambda { .. } => self.refuse("a lambda"),

            NodeKind::Field { base, field } => {
                let base = self.expr(base, scope)?;
                let base = self.boxed(base);
                let index = self.field_index(&field.name);
                let index = self.builder.ins().iconst(types::I64, index);
                let v = self.helper(self.jit.helpers.field, &[base, index]);
                self.check();
                Ok(Val {
                    kind: Kind::Boxed,
                    v,
                })
            }

            NodeKind::List { items } => {
                let mut handles = Vec::with_capacity(items.len());
                for item in items.iter() {
                    let v = self.expr(item, scope)?;
                    let h = self.boxed(v);
                    handles.push(h);
                }
                let ptr = self.spill(&handles);
                let n = self.builder.ins().iconst(types::I64, handles.len() as i64);
                let v = self.helper(self.jit.helpers.list, &[ptr, n]);
                Ok(Val {
                    kind: Kind::Boxed,
                    v,
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

    /// A constant handle, as an immediate when folding is on and as a rebuilt allocation when it is
    /// off.
    fn constant(&mut self, handle: i64) -> Val {
        let v = self.builder.ins().iconst(types::I64, handle);
        if self.jit.opts.fold_literals {
            return Val {
                kind: Kind::Boxed,
                v,
            };
        }
        let v = self.helper(self.jit.helpers.lit, &[v]);
        Val {
            kind: Kind::Boxed,
            v,
        }
    }

    /// A literal, or a refusal for the two the fragment has no arithmetic for.
    fn literal(&mut self, lit: &Lit) -> Result<Val> {
        match lit {
            Lit::Int(i) => Ok(Val {
                kind: Kind::Int,
                v: self.builder.ins().iconst(types::I64, *i),
            }),
            Lit::Bool(b) => Ok(Val {
                kind: Kind::Bool,
                v: self.builder.ins().iconst(types::I64, i64::from(*b)),
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
            });
        }

        let l = self.expr(lhs, scope)?;
        let r = self.expr(rhs, scope)?;
        match op {
            BinOp::BitAnd
            | BinOp::BitOr
            | BinOp::BitXor
            | BinOp::Shl
            | BinOp::Shr
            | BinOp::Ushr => self.refuse(BIT_OPERATORS),
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
                Ok(Val { kind: Kind::Int, v })
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
                Ok(Val { kind: Kind::Int, v })
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
                })
            }
            BinOp::Concat => {
                let a = self.boxed(l);
                let b = self.boxed(r);
                let v = self.helper(self.jit.helpers.concat, &[a, b]);
                self.check();
                Ok(Val {
                    kind: Kind::Boxed,
                    v,
                })
            }
            BinOp::And | BinOp::Or => unreachable!("short-circuit handled above"),
        }
    }

    fn app(&mut self, func: &Code, args: &[Code], scope: &mut Scope) -> Result<Val> {
        let NodeKind::Var(q) = &func.kind else {
            return self.refuse("a call whose callee is an expression");
        };
        let denotes = self.denotation(q, scope)?;
        let mut handles = Vec::with_capacity(args.len());
        for a in args {
            let v = self.expr(a, scope)?;
            let h = self.boxed(v);
            handles.push(h);
        }
        let n = self.builder.ins().iconst(types::I64, handles.len() as i64);
        let v = match denotes {
            Denotes::Compiled(id, arity) => {
                if arity != handles.len() {
                    return self.refuse(format!(
                        "`{}` is called with {} arguments and takes {arity}",
                        q.symbol(),
                        handles.len()
                    ));
                }
                let ptr = self.spill(&handles);
                let callee = self.jit.module.declare_func_in_func(id, self.builder.func);
                let call = self.builder.ins().call(callee, &[self.ctx, ptr]);
                let v = self.builder.inst_results(call)[0];
                self.check();
                v
            }
            Denotes::Uncompiled(target) => {
                return self.refuse(format!(
                    "a call to `{target}`, which is not in this compiled unit"
                ));
            }
            Denotes::Builtin(index) => {
                let ptr = self.spill(&handles);
                let index = self.builder.ins().iconst(types::I64, index as i64);
                let v = self.helper(self.jit.helpers.builtin, &[index, ptr, n]);
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
            Denotes::Local(_) => return self.refuse("a call through a local binding"),
            Denotes::Constant(_) => {
                return self.refuse(format!("`{}` is not a function", q.symbol()));
            }
        };
        Ok(Val {
            kind: Kind::Boxed,
            v,
        })
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
            if q.is_bare() && self.jit.ctors.iter().any(|(n, _)| n == q.symbol()) {
                Some(q.symbol().clone())
            } else {
                None
            }
        })?;
        let index = self.jit.ctors.iter().position(|(n, _)| *n == name)?;
        Some((index, self.jit.ctors[index].1))
    }

    /// Whether a sub-pattern binds without being able to fail.
    fn irrefutable(&self, pat: &ply_syntax::ast::Pattern) -> bool {
        match &pat.kind {
            PatternKind::Wildcard => true,
            PatternKind::Var(id) => self
                .ctor_of(&QName::bare(id.clone()))
                .is_none_or(|(_, arity)| arity != 0),
            _ => false,
        }
    }

    /// Whether a pattern binds anything at all, at any depth, so [`Fx::bind_pattern`] can skip
    /// extracting a sub-value nothing will read.
    fn binds_any(&self, pat: &ply_syntax::ast::Pattern) -> bool {
        match &pat.kind {
            PatternKind::Wildcard | PatternKind::Lit(_) => false,
            PatternKind::Var(_) => self.binder(pat).is_some(),
            PatternKind::Ctor { args, .. } => args.iter().any(|a| self.binds_any(a)),
            PatternKind::Record { fields, .. } => fields.iter().any(|(_, p)| self.binds_any(p)),
            PatternKind::List { items, rest } => {
                items.iter().any(|p| self.binds_any(p))
                    || rest.as_ref().is_some_and(|r| self.binds_any(r))
            }
        }
    }

    /// The name a sub-pattern binds, and `None` for one that binds nothing — a wildcard, or a bare
    /// nullary constructor, which is a test rather than a binder ([`Fx::test_pattern`]).
    fn binder(&self, pat: &ply_syntax::ast::Pattern) -> Option<Symbol> {
        let PatternKind::Var(id) = &pat.kind else {
            return None;
        };
        match self.ctor_of(&QName::bare(id.clone())) {
            Some((_, 0)) => None,
            _ => Some(id.name.clone()),
        }
    }

    /// The refutable half of a pattern: leave the current block for `hit` when it matches and
    /// `miss` when it does not.
    fn test_pattern(
        &mut self,
        pat: &ply_syntax::ast::Pattern,
        value: Val,
        hit: Block,
        miss: Block,
    ) -> Result<()> {
        match &pat.kind {
            PatternKind::Wildcard => {
                self.builder.ins().jump(hit, &[]);
            }
            PatternKind::Var(id) => match self.ctor_of(&QName::bare(id.clone())) {
                Some((index, 0)) => self.test_ctor(value, index, hit, miss),
                _ => {
                    self.builder.ins().jump(hit, &[]);
                }
            },
            PatternKind::Lit(lit) => {
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
            PatternKind::List { items, rest } => {
                // A refutable `rest` would need the tail built before it could be tested;
                // the corpus asks for none.
                if let Some(bad) = rest.iter().find(|p| !self.irrefutable(p)) {
                    return self.refuse(format!(
                        "a {} pattern as a list pattern's rest",
                        pattern_name(&bad.kind)
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
            PatternKind::Ctor { name, args } => {
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
                    let sub = self.helper(self.jit.helpers.ctor_arg, &[base, at]);
                    self.check();
                    let next = self.builder.create_block();
                    self.test_pattern(
                        arg,
                        Val {
                            kind: Kind::Boxed,
                            v: sub,
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
            PatternKind::Record { fields, rest } => {
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
                    let field = self.helper(self.jit.helpers.field, &[base, index]);
                    self.check();
                    let next = self.builder.create_block();
                    self.test_pattern(
                        sub,
                        Val {
                            kind: Kind::Boxed,
                            v: field,
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
    fn bind_pattern(
        &mut self,
        pat: &ply_syntax::ast::Pattern,
        value: Val,
        scope: &mut Scope,
    ) -> Result<()> {
        match &pat.kind {
            PatternKind::Wildcard => {}
            PatternKind::Var(id) => {
                if !matches!(self.ctor_of(&QName::bare(id.clone())), Some((_, 0))) {
                    scope.push((id.name.clone(), value));
                }
            }
            PatternKind::Lit(_) => {}
            PatternKind::List { items, rest } => {
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
                        },
                        scope,
                    )?;
                }
                if let Some(rest) = rest
                    && let Some(name) = self.binder(rest)
                {
                    let from = self.builder.ins().iconst(types::I64, items.len() as i64);
                    let v = self.helper(self.jit.helpers.list_rest, &[base, from]);
                    self.check();
                    scope.push((
                        name,
                        Val {
                            kind: Kind::Boxed,
                            v,
                        },
                    ));
                }
            }
            PatternKind::Ctor { args, .. } => {
                let base = self.boxed(value);
                for (i, arg) in args.iter().enumerate() {
                    if !self.binds_any(arg) {
                        continue;
                    }
                    let i = self.builder.ins().iconst(types::I64, i as i64);
                    let v = self.helper(self.jit.helpers.ctor_arg, &[base, i]);
                    self.check();
                    self.bind_pattern(
                        arg,
                        Val {
                            kind: Kind::Boxed,
                            v,
                        },
                        scope,
                    )?;
                }
            }
            PatternKind::Record { fields, .. } => {
                let base = self.boxed(value);
                for (name, sub) in fields {
                    if !self.binds_any(sub) {
                        continue;
                    }
                    let index = self.field_index(&name.name);
                    let index = self.builder.ins().iconst(types::I64, index);
                    let v = self.helper(self.jit.helpers.field, &[base, index]);
                    self.check();
                    self.bind_pattern(
                        sub,
                        Val {
                            kind: Kind::Boxed,
                            v,
                        },
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
            self.bind_pattern(&arm.pat, value, &mut inner)?;
            let body = self.expr(&arm.body, &mut inner)?;
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
        })
    }
}

fn pattern_name(p: &PatternKind) -> &'static str {
    match p {
        PatternKind::Wildcard => "wildcard",
        PatternKind::Var(_) => "binding",
        PatternKind::Lit(_) => "literal",
        PatternKind::Ctor { .. } => "constructor",
        PatternKind::Record { .. } => "record",
        PatternKind::List { .. } => "list",
    }
}
