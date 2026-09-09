//! Where a natively compiled body may be entered in place of evaluating one.

use crate::host::{HostBinding, HostRuntime, HostUse};
use crate::region::Record;
use crate::sim::Seed;
use crate::value::Value;
use ply_core::CheckOutput;
use ply_core::Footprint;
use ply_core::ty::{EffectAtom, IntTy, SECRET, TyVar, Type};
use ply_span::{Diagnostic, Symbol};
use ply_syntax::ast::Program;
use rustc_hash::FxHashMap;
use std::rc::Rc;
use std::sync::Arc;

/// A source of natively compiled bodies for a program's definitions.
pub trait Compiled {
    /// Whether these bodies were compiled from `program`.
    fn describes(&self, program: &Program) -> bool;

    /// Runs `name`'s body over `args`, or declines for any reason at all.
    fn enter(&self, name: &Symbol, args: &[Value], budget: usize) -> Option<Value>;

    /// Runs a test's body whole, through the nullary root the backend synthesized for it, and
    /// says whether the body ran and raised — which `enter` folds into a decline — because a
    /// test the backend fails and the machine passes is a disagreement, not a decline.
    fn enter_test(&self, _name: &Symbol, _budget: usize) -> Entered {
        Entered::Declined
    }

    /// A definition entered whole with its answer, refusal or failure told apart: what an engine
    /// with no machine behind it asks, where [`Compiled::enter`] folds a failure into a decline
    /// for the machine to run again.
    fn enter_whole(&self, _name: &Symbol, _args: &[Value], _budget: usize) -> Entered {
        Entered::Declined
    }

    /// The atoms compiled code performed since the last entry, for the machine's trace. A
    /// handled perform is still a perform, and the observed row is a claim the tests make.
    fn take_performed(&self) -> Vec<EffectAtom> {
        Vec::new()
    }

    /// The seed and step budget the next entry's `simulate` regions run under.
    fn set_seed(&self, _seed: Seed, _steps: u32) {}

    /// What the last entry's regions did, for the search, if it opened any.
    fn simulated(&self) -> Option<Record> {
        None
    }

    /// The host binding a `perform` nothing on the stack answers reaches, and the reactor a
    /// pending answer is waited on.
    fn set_host(&self, _binding: Arc<HostBinding>, _runtime: Option<Rc<dyn HostRuntime>>) {}

    fn set_declared(&self, _declared: Option<Footprint>) {}

    fn set_re_executed(&self, _re_executed: bool) {}

    /// What the entries since the last take asked of the host, and how many linear operations.
    fn take_host_use(&self) -> (HostUse, u64) {
        (HostUse::default(), 0)
    }

    /// What the host runtime said when the entries ended.
    fn take_teardown(&self) -> Vec<Diagnostic> {
        Vec::new()
    }

    /// Whether the backend is to be the only engine: a test or an entry it does not hold fails
    /// rather than falling to the machine.
    fn tier_only(&self) -> bool {
        false
    }
}

/// How a test root's entry ended.
#[derive(Debug)]
pub enum Entered {
    /// The body ran to its answer.
    Answered(Value),
    /// The body ran and raised this.
    Raised(Diagnostic),
    /// The backend did not run the body.
    Declined,
}

/// What may cross this boundary, in either direction: the two unboxed scalars, the two byte
/// carriers and unit — every leaf kind that holds no handle.
pub(crate) fn crossable(value: &Value) -> bool {
    matches!(
        value,
        Value::Int(_) | Value::Bool(_) | Value::Bytes(_) | Value::Str(_) | Value::Unit
    )
}

/// Whether `ty` mentions a fixed-width integer anywhere, at any depth.
pub fn mentions_a_width(ty: &Type) -> bool {
    match ty {
        Type::Var(_) => false,
        Type::Con(name, args) => {
            IntTy::from_name(name.as_str()).is_some() || args.iter().any(mentions_a_width)
        }
        Type::Fn { params, ret, .. } => {
            params.iter().any(mentions_a_width) || mentions_a_width(ret)
        }
        Type::Record(fields) => fields.values().any(mentions_a_width),
    }
}

/// Which definitions' **declared parameter types** cannot reach a world handle, decided once per
/// program rather than once per call.
pub(crate) struct CarriedTypes {
    /// A declared sum type's own parameters and the field types of every one of its constructors,
    /// by program-wide type name.
    decls: FxHashMap<Symbol, Decl>,
    /// The fixpoint over [`CarriedTypes::decls`]: whether a value of that type can reach a world
    /// handle, its type arguments left to each occurrence.
    safe: FxHashMap<Symbol, bool>,
    /// Per definition, its declared signature read as [`Denotes`].
    sigs: FxHashMap<Symbol, Sig>,
}

/// One definition's declared signature, with every position answered once.
struct Sig {
    /// One entry per declared parameter: the `Value` kind that parameter's type denotes when it is
    /// carried, and `None` when it is not.
    params: Vec<Option<Denotes>>,
    /// The same for the declared return type.
    ret: Option<Denotes>,
}

struct Decl {
    vars: Vec<TyVar>,
    fields: Vec<Type>,
}

/// The one `Value` kind a carried type denotes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Denotes {
    Int,
    Bool,
    Bytes,
    Str,
    Unit,
    List,
    Map,
    Record,
    Ctor,
}

impl Denotes {
    fn matches(self, value: &Value) -> bool {
        match self {
            Denotes::Int => matches!(value, Value::Int(_)),
            Denotes::Bool => matches!(value, Value::Bool(_)),
            Denotes::Bytes => matches!(value, Value::Bytes(_)),
            Denotes::Str => matches!(value, Value::Str(_)),
            Denotes::Unit => matches!(value, Value::Unit),
            Denotes::List => matches!(value, Value::List(_)),
            Denotes::Map => matches!(value, Value::Map(_)),
            Denotes::Record => matches!(value, Value::Record(_)),
            Denotes::Ctor => matches!(value, Value::Ctor { .. }),
        }
    }
}

impl CarriedTypes {
    /// The table for `check`, or an empty one — which admits nothing — for a machine built without
    /// a `CheckOutput`, for the reason [`Gate::PublishedRow`] refuses one: a machine that cannot
    /// read the fact has not been told it holds.
    pub(crate) fn over(check: Option<&CheckOutput>) -> CarriedTypes {
        let mut table = CarriedTypes {
            decls: FxHashMap::default(),
            safe: FxHashMap::default(),
            sigs: FxHashMap::default(),
        };
        let Some(check) = check else { return table };
        for ctor in check.ctors.values() {
            let decl = table
                .decls
                .entry(ctor.type_name.clone())
                .or_insert_with(|| Decl {
                    vars: ctor.scheme.ty_vars.clone(),
                    fields: Vec::new(),
                });
            decl.fields.extend(ctor.fields.iter().cloned());
        }
        table.safe = table.decls.keys().map(|n| (n.clone(), true)).collect();
        // Lowering only ever removes, so this settles; the bound is one round per declaration and
        // the loop asserts nothing about how many it took.
        loop {
            let lowered: Vec<Symbol> = table
                .decls
                .iter()
                .filter(|(name, decl)| {
                    table.safe[*name]
                        && !decl
                            .fields
                            .iter()
                            .all(|f| table.carries(f, Some(&decl.vars)))
                })
                .map(|(name, _)| name.clone())
                .collect();
            if lowered.is_empty() {
                break;
            }
            for name in lowered {
                table.safe.insert(name, false);
            }
        }
        let flags: Vec<(Symbol, Sig)> = check
            .defs
            .iter()
            .filter_map(|(name, def)| match &def.scheme.ty {
                Type::Fn { params, ret, .. } => Some((
                    name.clone(),
                    Sig {
                        params: params.iter().map(|t| table.denotes(t)).collect(),
                        ret: table.denotes(ret),
                    },
                )),
                _ => None,
            })
            .collect();
        table.sigs.extend(flags);
        table
    }

    /// The `Value` kind `ty` denotes, when `ty` is carried.
    fn denotes(&self, ty: &Type) -> Option<Denotes> {
        if !self.carries(ty, None) {
            return None;
        }
        match ty {
            Type::Record(_) => Some(Denotes::Record),
            Type::Con(name, _) => Some(match name.as_str() {
                "Int" => Denotes::Int,
                "Bool" => Denotes::Bool,
                "Bytes" => Denotes::Bytes,
                "String" => Denotes::Str,
                "Unit" => Denotes::Unit,
                "List" => Denotes::List,
                "Map" => Denotes::Map,
                // `carries` cleared it and it is none of the builtin heads, so it is a declared sum
                // type and its values are constructors.
                _ => Denotes::Ctor,
            }),
            // `carries` refuses both of these, so this is unreachable rather than conservative — it
            // is spelled out so that a future kind added to `carries` without an entry here is
            // refused rather than silently denoting whatever the arm above it did.
            Type::Var(_) | Type::Fn { .. } => None,
        }
    }

    /// Whether `ty` is carried.
    pub(crate) fn carries(&self, ty: &Type, decl_vars: Option<&[TyVar]>) -> bool {
        match ty {
            Type::Var(v) => decl_vars.is_some_and(|vars| vars.contains(v)),
            Type::Fn { .. } => false,
            Type::Record(fields) => fields.values().all(|t| self.carries(t, decl_vars)),
            Type::Con(name, args) => match name.as_str() {
                // The leaf set is `crossable`'s exactly, so it is the same list in both directions.
                "Int" | "Bool" | "Bytes" | "String" | "Unit" => args.is_empty(),
                "List" | "Map" => args.iter().all(|t| self.carries(t, decl_vars)),
                // The fragment has no path for either literal, so a body over them is refused
                // before this table is asked; keeping them out here keeps the leaf set honest.
                "Float" | "Decimal" => false,
                // A world handle and a credential are `Type::Con`s like any other.
                "Cell" | ply_core::prelude::TASK_TYPE | SECRET => false,
                // The fixed-width integer types, explicitly rather than by falling through to the
                // undeclared arm below. Compiled code holds one as a tagged immediate, which is
                // what an `Int` is held as, so a value crossing back would arrive as an `Int` and
                // be a *wrong* answer rather than a slow one. The bodies still compile and still
                // call each other directly (ADR 0039); it is the crossing that is refused, and
                // this arm is what makes `std.hash`'s `compress` unreachable from the differential
                // while `blake3` itself is entered whole.
                n if IntTy::from_name(n).is_some() => false,
                _ => match self.decls.get(name) {
                    Some(decl) => {
                        decl.vars.len() == args.len()
                            && self.safe.get(name).copied().unwrap_or(false)
                            && args.iter().all(|t| self.carries(t, decl_vars))
                    }
                    None => false,
                },
            },
        }
    }

    /// Whether `value` may cross back as `name`'s answer.
    pub(crate) fn answer_crosses(&self, name: &Symbol, value: &Value) -> bool {
        self.sigs
            .get(name)
            .and_then(|sig| sig.ret)
            .is_some_and(|d| d.matches(value))
            || crossable(value)
    }

    /// Whether every position of `name`'s declared signature is carried — the registry question,
    /// asked of a definition rather than of a call.
    pub(crate) fn signature_carried(&self, name: &Symbol) -> bool {
        self.sigs
            .get(name)
            .is_some_and(|sig| sig.ret.is_some() && sig.params.iter().all(Option::is_some))
    }
}

/// Doubles, because nothing in this workspace implements [`Compiled`].
#[cfg(test)]
mod tests {
    use super::*;
    use crate::build::*;
    use crate::evaluator::Machine;
    use crate::value::{Closure, ClosureKind};
    use ply_core::{CheckOutput, check_program};
    use ply_span::{Diagnostic, codes};
    use ply_syntax::ast::{BinOp, Expr, Item, Program};
    use ply_syntax::resolve::Resolved;
    use std::cell::RefCell;
    use std::collections::BTreeMap;
    use std::rc::Rc;
    use std::sync::Arc;

    type Reply = dyn Fn(&Symbol, &[Value], usize) -> Option<Value>;

    /// One call the machine offered a backend.
    #[derive(Clone, Debug, PartialEq)]
    struct Offer {
        name: Symbol,
        args: Vec<Value>,
        budget: usize,
    }

    /// A backend that records every offer and answers by a closure the test supplies.
    struct Double {
        /// Never dereferenced.
        program: *const Program,
        reply: Box<Reply>,
        offers: RefCell<Vec<Offer>>,
    }

    impl Double {
        fn over(
            program: &Program,
            reply: impl Fn(&Symbol, &[Value], usize) -> Option<Value> + 'static,
        ) -> Rc<Double> {
            Rc::new(Double {
                program: std::ptr::from_ref(program),
                reply: Box::new(reply),
                offers: RefCell::new(Vec::new()),
            })
        }

        /// Declines everything and remembers what it was offered.
        fn declining(program: &Program) -> Rc<Double> {
            Double::over(program, |_, _, _| None)
        }

        /// Answers `value` for `name` and declines everything else.
        fn answering(program: &Program, name: &str, value: Value) -> Rc<Double> {
            let wanted = Symbol::new(name);
            Double::over(program, move |asked, _, _| {
                (*asked == wanted).then(|| value.clone())
            })
        }

        fn offers(&self) -> Vec<Offer> {
            self.offers.borrow().clone()
        }

        fn names(&self) -> Vec<String> {
            self.offers
                .borrow()
                .iter()
                .map(|o| o.name.as_str().to_string())
                .collect()
        }
    }

    impl Compiled for Double {
        fn describes(&self, program: &Program) -> bool {
            std::ptr::eq(self.program, std::ptr::from_ref(program))
        }

        fn enter(&self, name: &Symbol, args: &[Value], budget: usize) -> Option<Value> {
            self.offers.borrow_mut().push(Offer {
                name: name.clone(),
                args: args.to_vec(),
                budget,
            });
            (self.reply)(name, args, budget)
        }
    }

    /// A program and the check output the purity gate reads.
    struct Checked {
        program: Program,
        resolved: Resolved,
        check: CheckOutput,
    }

    fn checked(items: Vec<Item>) -> Checked {
        let (program, resolved) = standalone(items);
        let check = match check_program(&program, &resolved) {
            Ok(check) => check,
            Err(ds) => panic!("the program under test does not check: {ds:#?}"),
        };
        Checked {
            program,
            resolved,
            check,
        }
    }

    impl Checked {
        fn machine(&self) -> Machine<'_> {
            Machine::new(&self.program, &self.resolved, &self.check)
        }

        fn types(&self) -> CarriedTypes {
            CarriedTypes::over(Some(&self.check))
        }
    }

    /// The same thing from source, because the argument gate is now a question about *declared
    /// types* and `crate::build`'s `fn_def` cannot write one.
    fn checked_source(source: &str) -> Checked {
        let mut program = ply_syntax::parse_program(vec![(
            ply_span::SourceId(0),
            ply_syntax::ast::ModuleName::anonymous(),
            source,
        )])
        .expect("the fixture must parse");
        let resolved =
            ply_syntax::resolve::resolve(&mut program).expect("the fixture must resolve");
        let check = match check_program(&program, &resolved) {
            Ok(check) => check,
            Err(ds) => panic!("the program under test does not check: {ds:#?}"),
        };
        Checked {
            program,
            resolved,
            check,
        }
    }

    /// `Diagnostic` has no `PartialEq`, so a failing outcome cannot be compared with `assert_eq!`
    #[track_caller]
    fn ok(outcome: Result<Value, Diagnostic>) -> Value {
        match outcome {
            Ok(value) => value,
            Err(d) => panic!("expected a value, got {}: {}", d.code, d.message),
        }
    }

    fn double_def() -> Item {
        fn_def_sig(
            "double",
            &[("x", tcon("Int"))],
            tcon("Int"),
            bin(BinOp::Mul, var("x"), int(2)),
        )
    }

    #[test]
    fn a_machine_with_no_backend_never_asks_and_never_counts() {
        let c = checked(vec![double_def()]);
        let mut machine = c.machine();
        assert_eq!(
            ok(machine.eval_expr_for_test(&callv("double", vec![int(21)]))),
            Value::Int(42)
        );
        assert_eq!(machine.compiled_counts(), (0, 0));
        assert_eq!(machine.compiled_refusals(), 0);
    }

    /// `hoist_staleness_audit.rs`'s hazard: a bisection builds a program whose definitions carry
    /// the names of the ones they replace.
    #[test]
    fn a_backend_built_over_another_program_is_ignored() {
        let elsewhere = checked(vec![fn_def_sig(
            "double",
            &[("x", tcon("Int"))],
            tcon("Int"),
            int(1000),
        )]);
        let backend = Double::answering(&elsewhere.program, "double", Value::Int(84));

        let c = checked(vec![double_def()]);
        let mut machine = c.machine();
        machine.set_compiled(backend.clone());
        assert_eq!(
            ok(machine.eval_expr_for_test(&callv("double", vec![int(21)]))),
            Value::Int(42)
        );
        assert_eq!(machine.compiled_counts(), (0, 0));
        assert!(backend.offers().is_empty());
    }

    /// A `Code` closure built by hand, so [`admit`] can be asked about a body the machine would not
    /// otherwise hand it: an anonymous one, or one under a name no definition publishes.
    fn code_closure(name: Option<&str>, params: &[&str], body: Expr) -> Closure {
        let params: Vec<Symbol> = params.iter().copied().map(Symbol::new).collect();
        let lowered = crate::code::lower_fn(&params, &body);
        Closure {
            name: name.map(Symbol::new),
            kind: ClosureKind::Code {
                params: Rc::new(params),
                size: lowered.size,
                body: lowered.code,
                captures: crate::code::no_captures(),
                captured: Rc::from(Vec::new()),
                module: 0,
            },
        }
    }

    /// A program whose `handled` performs and discharges its own operation, and whose `wrapper`
    /// does nothing but call it.
    fn self_handled() -> Checked {
        checked(vec![
            effect_def("state", &[("get", ply_syntax::ast::Mode::Read, false)]),
            fn_def_sig(
                "touch",
                &[("x", tcon("Int"))],
                tcon("Int"),
                perform("state", "get", None, vec![var("x")]),
            ),
            fn_def_sig(
                "handled",
                &[("x", tcon("Int"))],
                tcon("Int"),
                handle(
                    callv("touch", vec![var("x")]),
                    vec![clause(
                        "state",
                        "get",
                        None,
                        &["n"],
                        bin(BinOp::Add, var("n"), int(1)),
                    )],
                ),
            ),
            fn_def_sig(
                "wrapper",
                &[("x", tcon("Int"))],
                tcon("Int"),
                callv("handled", vec![var("x")]),
            ),
            fn_def_sig(
                "bump",
                &[("x", tcon("Int"))],
                tcon("Int"),
                bin(BinOp::Add, var("x"), int(0)),
            ),
        ])
    }

    /// The purity gate reads the published row, so a machine driven without a type-check pass has
    /// nothing to clear a definition with and the hook is inert.
    #[test]
    fn a_machine_with_no_check_output_offers_nothing() {
        let (program, resolved) = standalone(vec![double_def()]);
        let backend = Double::declining(&program);
        let mut machine = Machine::for_program(&program, &resolved);
        machine.set_compiled(backend.clone());
        assert_eq!(
            ok(machine.eval_expr_for_test(&callv("double", vec![int(21)]))),
            Value::Int(42)
        );
        assert!(backend.offers().is_empty());
        assert_eq!(machine.compiled_counts(), (0, 0));
    }

    #[test]
    fn crossable_admits_every_leaf_kind_that_holds_no_handle_and_nothing_else() {
        assert!(crossable(&Value::Int(0)));
        assert!(crossable(&Value::Bool(false)));
        assert!(crossable(&Value::bytes(b"GET /orders HTTP/1.1")));
        assert!(
            crossable(&Value::bytes(b"")),
            "an empty `Bytes` is a `Bytes`"
        );
        assert!(crossable(&Value::str("s")));
        assert!(
            crossable(&Value::str("")),
            "an empty `String` is a `String`"
        );
        assert!(crossable(&Value::Unit));
        for refused in [
            Value::Float(0.0),
            Value::Decimal(Default::default()),
            Value::List(Default::default()),
            Value::Secret(Arc::new(Value::Int(1))),
            Value::Secret(Arc::new(Value::bytes(b"hunter2"))),
        ] {
            assert!(!crossable(&refused), "{refused:?} crossed the boundary");
        }
    }

    /// `Cell`, `Task` and `Secret` are `Type::Con`s with a name and arguments, exactly as `Option`
    /// is, so a rule that read "any nominal type is a record or a constructor" would carry all
    /// three — and the third is a credential while the first two are handles into this run's world.
    #[test]
    fn a_world_handle_typed_parameter_is_refused_though_it_is_a_nominal_type() {
        let c = checked_source(
            "fn holds_cell(c: Cell<Int>) -> Int = 1\n\
             fn holds_secret(s: Secret<Int>) -> Int = 1\n\
             fn holds_fn(g: (Int) -> Int) -> Int = g(1)\n\
             fn holds_int(n: Int) -> Int = n\n",
        );
        let types = c.types();
        for name in ["holds_cell", "holds_secret", "holds_fn"] {
            let ty = &c.check.defs[&Symbol::new(name)].scheme.ty;
            let ply_core::ty::Type::Fn { params, .. } = ty else {
                panic!("{name} publishes no function type");
            };
            assert!(
                !types.carries(&params[0], None),
                "{name}'s declared parameter type {:?} is carried",
                params[0]
            );
        }
        let ply_core::ty::Type::Fn { params, .. } =
            &c.check.defs[&Symbol::new("holds_int")].scheme.ty
        else {
            panic!("holds_int publishes no function type");
        };
        assert!(
            types.carries(&params[0], None),
            "the control failed: `Int` is not carried, so the loop above says nothing"
        );
    }

    // 2026-08-31.

    /// A carried declared return type licenses one `Value` kind, not any.
    #[test]
    fn an_answer_whose_kind_is_not_its_declared_returns_is_refused_unless_it_is_childless() {
        let c = checked_source(
            "type Scan = { at: Int, tok: Bytes }\n\
             fn scan(i: Int) -> Scan = { at: i, tok: b\"x\" }\n",
        );
        let holding = Value::list(vec![Value::Closure(Arc::new(code_closure(
            None,
            &["y"],
            var("y"),
        )))]);
        let types = c.types();
        let scan = Symbol::new("scan");
        assert!(
            !types.answer_crosses(&scan, &holding),
            "a `List` holding a `Closure` came back under a declared `-> Scan`"
        );
        assert!(
            types.answer_crosses(
                &scan,
                &record_value(&[("at", Value::Int(0)), ("tok", Value::bytes(b""))])
            ),
            "the record the declaration denotes was refused, so the widening bought nothing"
        );
        assert!(
            types.answer_crosses(&scan, &Value::Int(0)),
            "the childless clause was lost: `Mutation::WrongType` and `Mutation::Answers` both \
             answer an `Int` for a definition that returns something else, and refusing it here \
             would police a wrong answer with a kind test"
        );
    }

    /// A declared return type that can hold code is not answered for at all.
    #[test]
    fn a_closure_bearing_record_return_is_refused_however_ordinary_the_record_looks() {
        let c = checked_source(
            "type Box = { run: (Int) -> Int, tag: Int }\n\
             type Plain = { tag: Int }\n\
             fn make_box(n: Int) -> Box = { run: |y: Int| y, tag: n }\n\
             fn make_plain(n: Int) -> Plain = { tag: n }\n",
        );
        let types = c.types();
        // A record with no closure in it, under a declared type that can hold one.
        let innocent = record_value(&[("tag", Value::Int(1))]);
        assert!(
            !types.answer_crosses(&Symbol::new("make_box"), &innocent),
            "a record came back under a declared return type that can hold a `Closure`"
        );
        assert!(
            types.answer_crosses(&Symbol::new("make_plain"), &innocent),
            "the control failed: a record of `Int` was refused too"
        );
        assert!(
            !types.signature_carried(&Symbol::new("make_box")),
            "a backend's registry would hold a definition the machine will not hear from"
        );
        assert!(types.signature_carried(&Symbol::new("make_plain")));
    }

    /// Entering a call now hides its whole subtree, and the effects gate has to hold over the
    /// subtree rather than over the entry.
    #[test]
    fn an_entered_subtree_is_refused_for_an_effect_two_hops_down_that_it_would_hide() {
        let c = self_handled();
        // `wrapper` calls `handled`, which discharges `state.get` under its own handler.
        let mut machine = c.machine();
        assert_eq!(
            ok(machine.eval_expr_for_test(&callv("wrapper", vec![int(1)]))),
            Value::Int(2)
        );
        assert_eq!(
            machine.trace().performs(),
            1,
            "the fixture is wrong: nothing was performed, so hiding the subtree would cost \
             nothing"
        );
        drop(machine);

        // And the machine offers it to nobody, so the subtree is never hidden.
        let backend = Double::declining(&c.program);
        let mut machine = c.machine();
        machine.set_compiled(backend.clone());
        assert_eq!(
            ok(machine.eval_expr_for_test(&callv("wrapper", vec![int(1)]))),
            Value::Int(2)
        );
        assert!(
            !backend.names().iter().any(|n| n == "wrapper"),
            "a definition whose subtree performs was offered: {:?}",
            backend.names()
        );
        assert_eq!(
            machine.trace().performs(),
            1,
            "the atoms the interpreter records were lost"
        );
    }

    /// A record `Value` from a list of fields, which no helper in [`crate::build`] answers because
    /// that module builds `Expr`s.
    /// A backend that holds only test roots, and ends every entry the one way it is told to.
    struct Roots {
        /// Never dereferenced.
        program: *const Program,
        entered: Box<dyn Fn() -> Entered>,
    }

    impl Compiled for Roots {
        fn describes(&self, program: &Program) -> bool {
            std::ptr::eq(self.program, std::ptr::from_ref(program))
        }

        fn enter(&self, _: &Symbol, _: &[Value], _: usize) -> Option<Value> {
            None
        }

        fn enter_test(&self, _: &Symbol, _: usize) -> Entered {
            (self.entered)()
        }
    }

    fn first_test_under(
        c: &Checked,
        entered: impl Fn() -> Entered + 'static,
    ) -> (Result<(), Diagnostic>, (u64, u64)) {
        let mut machine = c.machine();
        machine.set_compiled(Rc::new(Roots {
            program: &c.program,
            entered: Box::new(entered),
        }));
        let outcome = machine.eval_test_in(c.program.modules[0].name.as_symbol(), 0);
        (outcome, machine.compiled_counts())
    }

    fn double_doubles(expected: i64) -> Vec<Item> {
        vec![
            double_def(),
            test_def(
                "double doubles",
                callv(
                    "assert_eq",
                    vec![callv("double", vec![int(21)]), int(expected)],
                ),
            ),
        ]
    }

    fn assertion_raised() -> Entered {
        Entered::Raised(Diagnostic::error(codes::RUNTIME_ERROR, "assertion failed"))
    }

    /// The same raise where the machine raises too is the machine's diagnostic, as before.
    #[test]
    fn a_test_root_the_backend_raised_in_keeps_the_machines_diagnostic_when_it_raises_too() {
        let c = checked(double_doubles(43));
        let (outcome, _) = first_test_under(&c, assertion_raised);
        let d = outcome.expect_err("the assertion fails in the machine");
        assert_ne!(d.code, codes::ENGINE_DIVERGENCE, "{}", d.message);
    }

    fn record_value(fields: &[(&str, Value)]) -> Value {
        let mut map = BTreeMap::new();
        for (name, value) in fields {
            map.insert(Symbol::new(name), value.clone());
        }
        Value::Record(Arc::new(map.into_iter().collect()))
    }
}
