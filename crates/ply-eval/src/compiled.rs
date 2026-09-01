//! Where a natively compiled body may be entered in place of evaluating one.

use crate::value::{Closure, ClosureKind, Value};
use ply_core::CheckOutput;
use ply_core::ty::{SECRET, TyVar, Type};
use ply_span::Symbol;
use ply_syntax::ast::Program;
use rustc_hash::FxHashMap;

/// A source of natively compiled bodies for a program's definitions.
pub trait Compiled {
    /// Whether these bodies were compiled from `program`.
    fn describes(&self, program: &Program) -> bool;

    /// Runs `name`'s body over `args`, or declines for any reason at all.
    fn enter(&self, name: &Symbol, args: &[Value], budget: usize) -> Option<Value>;
}

/// What may cross this boundary, in either direction: the two unboxed scalars and [`Value::Bytes`].
pub(crate) fn crossable(value: &Value) -> bool {
    matches!(value, Value::Int(_) | Value::Bool(_) | Value::Bytes(_))
}

/// The `Value` kinds a *carried type* can denote, which is what an argument's discriminant is
/// tested against.
pub(crate) fn crossable_argument_kind(value: &Value) -> bool {
    matches!(
        value,
        Value::Int(_)
            | Value::Bool(_)
            | Value::Bytes(_)
            | Value::List(_)
            | Value::Map(_)
            | Value::Record(_)
            | Value::Ctor { .. }
    )
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
                "Int" | "Bool" | "Bytes" => args.is_empty(),
                "List" | "Map" => args.iter().all(|t| self.carries(t, decl_vars)),
                // the fragment's gaps item 4's three, and the leaf set is deliberately `crossable`'s
                // exactly rather than one kind wider — `Unit` included, which holds nothing and is
                // refused anyway so that the leaf set is the same list in both directions.
                "Float" | "Decimal" | "String" | "Unit" => false,
                // A world handle and a credential are `Type::Con`s like any other.
                "Cell" | ply_core::prelude::TASK_TYPE | SECRET => false,
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

    /// One entry per declared parameter of `name`.
    fn params(&self, name: &Symbol) -> Option<&[Option<Denotes>]> {
        self.sigs.get(name).map(|sig| sig.params.as_slice())
    }

    /// Why [`Gate::ArgumentType`] refused this call: the head of the first thing in the first
    /// offending parameter's declared type that is not carried, or the value's kind when the
    /// parameter *is* carried and the value is not of the kind it denotes.
    pub(crate) fn refusal(
        &self,
        check: Option<&CheckOutput>,
        name: &Symbol,
        args: &[Value],
    ) -> &'static str {
        let Some(flags) = self.params(name) else {
            return "<no signature>";
        };
        if flags.len() != args.len() {
            return "<arity>";
        }
        let declared = check
            .and_then(|c| c.defs.get(name))
            .map(|d| &d.scheme.ty)
            .and_then(|ty| match ty {
                Type::Fn { params, .. } => Some(params.as_slice()),
                _ => None,
            });
        for (i, (denotes, value)) in flags.iter().zip(args).enumerate() {
            if denotes.is_some_and(|d| d.matches(value)) || crossable(value) {
                continue;
            }
            if denotes.is_some() {
                return "<kind mismatch>";
            }
            return declared
                .and_then(|ps| ps.get(i))
                .and_then(|ty| self.blocker(ty, None))
                .unwrap_or("<unknown>");
        }
        "<none>"
    }

    /// The head of the first part of `ty` that is not carried.
    fn blocker(&self, ty: &Type, decl_vars: Option<&[TyVar]>) -> Option<&'static str> {
        match ty {
            Type::Var(v) if decl_vars.is_some_and(|vars| vars.contains(v)) => None,
            Type::Var(_) => Some("Var"),
            Type::Fn { .. } => Some("Fn"),
            Type::Record(fields) => fields.values().find_map(|t| self.blocker(t, decl_vars)),
            Type::Con(name, args) => match name.as_str() {
                "Int" | "Bool" | "Bytes" => None,
                "List" | "Map" => args.iter().find_map(|t| self.blocker(t, decl_vars)),
                "Float" => Some("Float"),
                "Decimal" => Some("Decimal"),
                "String" => Some("String"),
                "Unit" => Some("Unit"),
                "Cell" => Some("Cell"),
                ply_core::prelude::TASK_TYPE => Some("Task"),
                SECRET => Some("Secret"),
                _ => match self.decls.get(name) {
                    None => Some("<undeclared>"),
                    Some(decl) if decl.vars.len() != args.len() => Some("<type arity>"),
                    Some(decl) if !self.safe.get(name).copied().unwrap_or(false) => decl
                        .fields
                        .iter()
                        .find_map(|f| self.blocker(f, Some(&decl.vars)))
                        .or(Some("<declaration>")),
                    Some(_) => args.iter().find_map(|t| self.blocker(t, decl_vars)),
                },
            },
        }
    }

    /// Whether `args` may cross as `name`'s arguments.
    fn args_cross(&self, name: &Symbol, args: &[Value]) -> bool {
        let Some(flags) = self.params(name) else {
            return false;
        };
        flags.len() == args.len()
            && flags.iter().zip(args).all(|(denotes, value)| {
                denotes.is_some_and(|d| d.matches(value)) || crossable(value)
            })
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

/// Which gate refused a call, named rather than collapsed into `None`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Gate {
    /// Not a body this machine lowered: an unlowered closure, a constructor or a builtin.
    NotLoweredCode,
    /// An argument whose *kind* this boundary does not carry — see [`crossable_argument_kind`].
    ArgumentShape,
    /// A declared parameter type that can reach a [`Value::Cell`], [`Value::Task`],
    /// [`Value::Continuation`], [`Value::Closure`] or [`Value::Secret`] — or a definition whose
    /// declared arity is not the one the machine is calling.
    ArgumentType,
    /// Inside a `simulate` region.
    SimulateRegion,
    /// A body with no program-wide name: a lambda.
    Anonymous,
    /// The published effect row is non-empty, or there is no row to read at all.
    PublishedRow,
    /// The published row is empty and the definition performs anyway, under a `handle` of its own
    /// or of something it calls — see [`internally_effectful`].
    InternalEffects,
    /// No nested call left before the machine's own bound.
    Budget,
}

/// Whether entering `name` natively would lose a `perform` the interpreter would have run.
fn internally_effectful(check: Option<&CheckOutput>, name: &Symbol) -> bool {
    check
        .and_then(|check| check.defs.get(name))
        .is_none_or(|def| def.internally_effectful)
}

/// The name a backend may be offered this call under and the budget to offer it with, or the gate
/// that refused the call.
pub(crate) fn admit<'a>(
    closure: &'a Closure,
    args: &[Value],
    in_simulate: bool,
    check: Option<&CheckOutput>,
    types: &CarriedTypes,
    max_calls: usize,
    calls: usize,
) -> Result<(&'a Symbol, usize), Gate> {
    admit_with(
        closure,
        args,
        in_simulate,
        check,
        Some(types),
        max_calls,
        calls,
        crossable_argument_kind,
    )
}

/// [`admit`] with the two argument tests supplied, so a census can ask what a wider argument rung
/// would admit without a second copy of the other six gates.
#[allow(clippy::too_many_arguments)]
pub(crate) fn admit_with<'a>(
    closure: &'a Closure,
    args: &[Value],
    in_simulate: bool,
    check: Option<&CheckOutput>,
    types: Option<&CarriedTypes>,
    max_calls: usize,
    calls: usize,
    carries: impl Fn(&Value) -> bool,
) -> Result<(&'a Symbol, usize), Gate> {
    if !matches!(closure.kind, ClosureKind::Code { .. }) {
        return Err(Gate::NotLoweredCode);
    }
    if !args.iter().all(carries) {
        return Err(Gate::ArgumentShape);
    }
    if in_simulate {
        return Err(Gate::SimulateRegion);
    }
    let name = closure.name.as_ref().ok_or(Gate::Anonymous)?;
    if !crate::memo::pure_by_published_row(check, name) {
        return Err(Gate::PublishedRow);
    }
    if internally_effectful(check, name) {
        return Err(Gate::InternalEffects);
    }
    if let Some(types) = types
        && !types.args_cross(name, args)
    {
        return Err(Gate::ArgumentType);
    }
    let budget = max_calls.checked_sub(calls).ok_or(Gate::Budget)?;
    if budget == 0 {
        return Err(Gate::Budget);
    }
    Ok((name, budget))
}

/// Doubles, because nothing in this workspace implements [`Compiled`].
#[cfg(test)]
mod tests {
    use super::*;
    use crate::argv;
    use crate::build::*;
    use crate::differential::compare_answers;
    use crate::env::Env;
    use crate::limit::DEFAULT_MAX_CALLS;
    use crate::machine::Machine;
    use crate::value::Fields;
    use crate::value::{Closure, ClosureKind};
    use ply_core::{CheckOutput, check_program};
    use ply_span::Diagnostic;
    use ply_syntax::ast::{BinOp, Expr, ExprKind, Item, Program};
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

        fn forget(&self) {
            self.offers.borrow_mut().clear();
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

    /// A `Code` closure standing for `name` at its declared arity, so [`admit`] can be asked about
    /// a definition the fixture declares.
    fn named(c: &Checked, name: &str) -> Closure {
        let params: Vec<&str> = match &c.check.defs[&Symbol::new(name)].scheme.ty {
            ply_core::ty::Type::Fn { params, .. } => (0..params.len()).map(|_| "p").collect(),
            other => panic!("{name} publishes {other:?} rather than a function type"),
        };
        code_closure(Some(name), &params, int(0))
    }

    /// `Result<Value, Diagnostic>` has no `PartialEq`, and the comparison this wants is the one
    /// `differential` makes: the code, the message, every label with its span, and every note.
    fn rendered(outcome: &Result<Value, Diagnostic>) -> String {
        format!("{outcome:?}")
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

    /// The property every other claim rests on: with a backend that answers nothing, the machine is
    /// the machine.
    #[test]
    fn a_backend_that_declines_everything_changes_nothing() {
        let items = vec![
            double_def(),
            fn_def_sig(
                "half",
                &[("x", tcon("Int"))],
                tcon("Int"),
                bin(BinOp::Div, var("x"), int(2)),
            ),
            fn_def_sig(
                "boom",
                &[("x", tcon("Int"))],
                tcon("Int"),
                bin(BinOp::Div, var("x"), int(0)),
            ),
            fn_def_sig(
                "table",
                &[],
                tapp("List", vec![tcon("Int")]),
                list(vec![int(1), int(2), int(3)]),
            ),
        ];
        let subjects = [
            callv("double", vec![int(21)]),
            bin(
                BinOp::Add,
                callv("double", vec![int(1)]),
                callv("half", vec![int(8)]),
            ),
            callv("boom", vec![int(1)]),
            callv("double", vec![string("not a number")]),
            bin(BinOp::Add, callv("table", vec![]), callv("table", vec![])),
        ];

        let c = checked(items);
        let baseline: Vec<String> = {
            let mut machine = c.machine();
            subjects
                .iter()
                .map(|e| rendered(&machine.eval_expr_for_test(e)))
                .collect()
        };

        let backend = Double::declining(&c.program);
        let mut machine = c.machine();
        machine.set_compiled(backend.clone());
        for (e, want) in subjects.iter().zip(&baseline) {
            assert_eq!(&rendered(&machine.eval_expr_for_test(e)), want);
        }
        let (entries, declines) = machine.compiled_counts();
        assert_eq!(entries, 0);
        assert!(declines > 0, "the backend was never offered a call at all");
        assert_eq!(machine.compiled_refusals(), 0);
        assert!(
            backend.offers().iter().any(|o| o.name.as_str() == "double"),
            "the backend was never offered `double`: {:?}",
            backend.offers()
        );
    }

    #[test]
    fn an_accepted_call_gets_its_name_its_arguments_and_a_budget_and_its_answer_is_used() {
        let c = checked(vec![double_def()]);
        let backend = Double::answering(&c.program, "double", Value::Int(84));
        let mut machine = c.machine();
        machine.set_compiled(backend.clone());

        // 84 rather than 42: the compiled answer was used and the body was not evaluated.
        assert_eq!(
            ok(machine.eval_expr_for_test(&callv("double", vec![int(21)]))),
            Value::Int(84)
        );
        assert_eq!(
            backend.offers(),
            vec![Offer {
                name: Symbol::new("double"),
                args: vec![Value::Int(21)],
                budget: DEFAULT_MAX_CALLS,
            }]
        );
        assert_eq!(machine.compiled_counts(), (1, 0));
    }

    #[test]
    fn a_bool_crosses_in_both_directions_and_a_float_crosses_in_neither() {
        let c = checked(vec![
            fn_def_sig(
                "not",
                &[("b", tcon("Bool"))],
                tcon("Bool"),
                un(ply_syntax::ast::UnOp::Not, var("b")),
            ),
            fn_def_sig(
                "twice",
                &[("f", tcon("Float"))],
                tcon("Float"),
                bin(BinOp::Add, var("f"), var("f")),
            ),
        ]);

        let backend = Double::answering(&c.program, "not", Value::Bool(true));
        let mut machine = c.machine();
        machine.set_compiled(backend.clone());
        assert_eq!(
            ok(machine.eval_expr_for_test(&callv("not", vec![boolean(true)]))),
            Value::Bool(true)
        );
        assert_eq!(backend.offers().len(), 1);
        assert_eq!(backend.offers()[0].args, vec![Value::Bool(true)]);
        drop(machine);

        // the fragment's gaps item 4: the spike's fragment accepts `Float` arithmetic and fails on it at
        // run time.
        let backend = Double::declining(&c.program);
        let mut machine = c.machine();
        machine.set_compiled(backend.clone());
        assert_eq!(
            ok(machine.eval_expr_for_test(&callv("twice", vec![float(1.5)]))),
            Value::Float(3.0)
        );
        assert!(backend.offers().is_empty(), "a `Float` reached a backend");
        // Control: the same definition with an `Int` is offered.
        assert_eq!(
            ok(machine.eval_expr_for_test(&callv("twice", vec![int(2)]))),
            Value::Int(4)
        );
        assert_eq!(backend.names(), vec!["twice"]);
    }

    /// The boundary checks the *kind* of what comes back, in every profile.
    #[test]
    fn an_answer_this_boundary_refuses_is_declined_and_the_body_is_evaluated() {
        let c = checked(vec![double_def()]);
        for refused in [
            Value::str("a string"),
            Value::Float(1.0),
            Value::Unit,
            Value::List(Default::default()),
        ] {
            let backend = Double::answering(&c.program, "double", refused.clone());
            let mut machine = c.machine();
            machine.set_compiled(backend.clone());
            assert_eq!(
                ok(machine.eval_expr_for_test(&callv("double", vec![int(21)]))),
                Value::Int(42),
                "a backend answering {refused:?} was believed"
            );
            assert_eq!(machine.compiled_counts(), (0, 1));
            assert_eq!(machine.compiled_refusals(), 1);
            assert_eq!(backend.offers().len(), 1);
        }
    }

    /// Stated as a limitation, not a guarantee: the seam checks a kind and never a value.
    #[test]
    fn a_wrong_int_passes_the_seam_and_is_caught_only_by_the_other_engine() {
        let c = checked(vec![double_def()]);
        let backend = Double::answering(&c.program, "double", Value::Int(99));
        let mut machine = c.machine();
        machine.set_compiled(backend);
        let subject = callv("double", vec![int(21)]);
        let from_machine = machine.eval_expr_for_test(&subject);

        assert_eq!(from_machine.as_ref().ok(), Some(&Value::Int(99)));
        assert_eq!(machine.compiled_counts(), (1, 0));
        assert_eq!(
            machine.compiled_refusals(),
            0,
            "the boundary reported a violation it cannot actually see"
        );

        let mut plain = Machine::new(&c.program, &c.resolved, &c.check);
        let from_plain = plain.eval_expr_for_test(&subject);
        assert_eq!(from_plain.as_ref().ok(), Some(&Value::Int(42)));
        assert!(
            compare_answers(
                &plain,
                &machine,
                "the expression under test",
                &from_plain,
                &from_machine,
            )
            .is_some(),
            "the backend audit did not report a backend that answered 99 for 42"
        );
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

    /// `interp.rs` mints a closure per top-level `fn` carrying the program-wide name, and one
    /// handed into a machine reaches `enter_code` through the `ClosureKind::Fn` arm.
    #[test]
    fn an_unlowered_closure_with_a_program_wide_name_is_never_offered() {
        let body = bin(BinOp::Mul, var("x"), int(2));
        let call_it = callv("f", vec![int(21)]);
        let c = checked(vec![double_def()]);

        let unlowered = Value::Closure(Arc::new(Closure {
            name: Some(Symbol::new("double")),
            kind: ClosureKind::Fn {
                params: vec![Symbol::new("x")],
                body: Arc::new(body),
                env: Env::empty(),
                module: 0,
            },
        }));

        let backend = Double::declining(&c.program);
        let mut machine = c.machine();
        machine.set_compiled(backend.clone());
        let got = machine.eval_expr_in(&call_it, 0, &[(Symbol::new("f"), unlowered)]);
        assert_eq!(ok(got), Value::Int(42));
        assert!(
            backend.offers().is_empty(),
            "an unlowered closure was routed into a backend: {:?}",
            backend.offers()
        );
        assert_eq!(machine.compiled_counts(), (0, 0));

        // Control: the machine's own `double`, under the same name the closure above carries, is
        // offered.
        assert_eq!(
            ok(machine.eval_expr_for_test(&callv("double", vec![int(21)]))),
            Value::Int(42)
        );
        assert_eq!(backend.names(), vec!["double"]);
    }

    /// A lambda is `ClosureKind::Code` with no name, and nothing anonymous reaches a backend — a
    /// backend is keyed by program-wide name and has nothing to answer for an anonymous body.
    #[test]
    fn an_anonymous_closure_is_never_offered() {
        let c = checked(vec![double_def()]);
        let backend = Double::declining(&c.program);
        let mut machine = c.machine();
        machine.set_compiled(backend.clone());
        let e = bin(
            BinOp::Add,
            call(
                lam(&["x"], bin(BinOp::Mul, var("x"), int(2))),
                vec![int(21)],
            ),
            callv("double", vec![int(0)]),
        );
        assert_eq!(ok(machine.eval_expr_for_test(&e)), Value::Int(42));
        assert_eq!(
            backend.names(),
            vec!["double"],
            "an anonymous closure was offered to a backend"
        );
    }

    /// A `Code` closure built by hand, so [`admit`] can be asked about a body the machine would not
    /// otherwise hand it: an anonymous one, or one under a name no definition publishes.
    fn code_closure(name: Option<&str>, params: &[&str], body: Expr) -> Closure {
        Closure {
            name: name.map(Symbol::new),
            kind: ClosureKind::Code {
                params: Rc::new(params.iter().copied().map(Symbol::new).collect()),
                body: crate::code::lower(&body),
                env: Env::empty(),
                module: 0,
            },
        }
    }

    fn double_closure() -> Closure {
        code_closure(Some("double"), &["x"], bin(BinOp::Mul, var("x"), int(2)))
    }

    /// The gate chain over `c`'s program, outside a region and at full budget.
    fn gate(c: &Checked, closure: &Closure, args: &[Value]) -> Result<(String, usize), Gate> {
        admit(
            closure,
            args,
            false,
            Some(&c.check),
            &CarriedTypes::over(Some(&c.check)),
            DEFAULT_MAX_CALLS,
            0,
        )
        .map(|(name, budget)| (name.as_str().to_string(), budget))
    }

    /// What every gate test below reads its refusal against: this call, on this program, clears all
    /// of them.
    fn admitted() -> Result<(String, usize), Gate> {
        Ok(("double".to_string(), DEFAULT_MAX_CALLS))
    }

    /// An unlowered closure carries a program-wide name over a body that is a deep clone rather
    /// than a node of the program.
    #[test]
    fn a_body_this_machine_did_not_lower_is_refused_by_the_kind_gate() {
        let c = checked(vec![double_def()]);
        let unlowered = Closure {
            name: Some(Symbol::new("double")),
            kind: ClosureKind::Fn {
                params: vec![Symbol::new("x")],
                body: Arc::new(bin(BinOp::Mul, var("x"), int(2))),
                env: Env::empty(),
                module: 0,
            },
        };
        assert_eq!(
            gate(&c, &unlowered, &[Value::Int(21)]),
            Err(Gate::NotLoweredCode)
        );
        assert_eq!(
            gate(&c, &double_closure(), &[Value::Int(21)]),
            admitted(),
            "the same name over a lowered body is refused too, so the test above says nothing"
        );
    }

    /// The kinds this boundary carries, asked of the gate rather than of a run.
    #[test]
    fn an_argument_this_boundary_does_not_carry_is_refused_by_the_shape_gate() {
        let c = checked(vec![double_def()]);
        let subject = double_closure();
        for refused in [
            Value::Float(1.0),
            Value::str("21"),
            Value::Unit,
            Value::Secret(Arc::new(Value::Int(21))),
        ] {
            assert_eq!(
                gate(&c, &subject, std::slice::from_ref(&refused)),
                Err(Gate::ArgumentShape),
                "{refused:?} was carried across the boundary"
            );
        }
        assert_eq!(
            gate(&c, &subject, &[Value::List(Default::default())]),
            Err(Gate::ArgumentType),
            "a `List` where `Int` is declared crossed the boundary"
        );
        assert_eq!(gate(&c, &subject, &[Value::Int(21)]), admitted());
        assert_eq!(gate(&c, &subject, &[Value::Bool(true)]), admitted());
        assert_eq!(
            gate(&c, &subject, &[Value::bytes(b"GET / HTTP/1.1\r\n")]),
            admitted(),
            "a `Bytes` argument is refused, and a lexer has no other kind"
        );
    }

    /// The `Bytes` widening, end to end through the machine rather than through [`admit`]: in as an
    /// argument, and out as an answer.
    #[test]
    fn a_bytes_crosses_in_as_an_argument_and_out_as_an_answer() {
        // `head` takes whatever it is given and hands it straight back, so the machine's own answer
        // is a `Bytes` too and the comparison below is between two answers of the same kind.
        let c = checked(vec![fn_def_poly(
            "head",
            &["a"],
            &[("b", tvar("a"))],
            tvar("a"),
            var("b"),
        )]);
        let call = callv("head", vec![bytes(b"GET /orders HTTP/1.1")]);

        // The control: no backend, and the machine's own answer.
        assert_eq!(
            ok(c.machine().eval_expr_for_test(&call)),
            Value::bytes(b"GET /orders HTTP/1.1")
        );

        // In.
        let backend = Double::declining(&c.program);
        let mut machine = c.machine();
        machine.set_compiled(backend.clone());
        assert_eq!(
            ok(machine.eval_expr_for_test(&call)),
            Value::bytes(b"GET /orders HTTP/1.1")
        );
        assert_eq!(
            backend.offers()[0].args,
            vec![Value::bytes(b"GET /orders HTTP/1.1")],
            "the `Bytes` did not reach the backend"
        );
        assert_eq!(machine.compiled_counts(), (0, 1));
        drop(machine);

        // Out.
        let backend = Double::answering(&c.program, "head", Value::bytes(b"HTTP/1.1 200"));
        let mut machine = c.machine();
        machine.set_compiled(backend.clone());
        assert_eq!(
            ok(machine.eval_expr_for_test(&call)),
            Value::bytes(b"HTTP/1.1 200")
        );
        assert_eq!(machine.compiled_counts(), (1, 0));
        assert_eq!(machine.compiled_refusals(), 0);
    }

    /// Inside a `simulate` region every cell touch and every allocation is an `Access` the search
    /// prunes on, and a body the machine did not run records none of them.
    #[test]
    fn a_call_inside_a_simulate_region_is_refused_by_the_region_gate() {
        let c = checked(vec![double_def()]);
        let subject = double_closure();
        let args = [Value::Int(21)];
        assert_eq!(
            admit(
                &subject,
                &args,
                true,
                Some(&c.check),
                &CarriedTypes::over(Some(&c.check)),
                DEFAULT_MAX_CALLS,
                0
            ),
            Err(Gate::SimulateRegion)
        );
        assert_eq!(gate(&c, &subject, &args), admitted());
    }

    /// The gate this block exists to arm, and the one hole `CONTRIBUTING.md` §"Things known to be
    /// broken" item 13 named.
    #[test]
    fn an_anonymous_body_is_refused_by_the_name_gate_rather_than_by_the_row_gate() {
        let c = checked(vec![double_def()]);
        let anonymous = code_closure(None, &["x"], bin(BinOp::Mul, var("x"), int(2)));
        assert_eq!(
            gate(&c, &anonymous, &[Value::Int(21)]),
            Err(Gate::Anonymous)
        );
        assert_eq!(
            gate(&c, &double_closure(), &[Value::Int(21)]),
            admitted(),
            "the same body under a published name is refused too, so the refusal above is not \
             the name"
        );
        let fabricated = code_closure(Some(""), &["x"], bin(BinOp::Mul, var("x"), int(2)));
        assert_eq!(
            gate(&c, &fabricated, &[Value::Int(21)]),
            Err(Gate::PublishedRow),
            "a name the program does not publish cleared the row gate, and the substitution \
             above would now be visible to the behavioural test after all"
        );
    }

    /// The published row is the reviewable artifact, and "no row at all" is the same refusal as "a
    /// row that is not empty" — a machine built without a `CheckOutput` enters nothing, which is
    /// most of this crate's own tests.
    #[test]
    fn a_row_that_is_not_empty_and_a_row_that_is_missing_are_both_refused_by_the_row_gate() {
        let c = checked(vec![
            double_def(),
            effect_def("state", &[("get", ply_syntax::ast::Mode::Read, false)]),
            fn_def_sig(
                "touch",
                &[("x", tcon("Int"))],
                tcon("Int"),
                perform("state", "get", None, vec![var("x")]),
            ),
        ]);
        assert!(
            !c.check.defs[&Symbol::new("touch")].footprint.is_empty(),
            "the fixture is wrong: `touch` publishes an empty row"
        );
        let effectful = code_closure(Some("touch"), &["x"], var("x"));
        assert_eq!(
            gate(&c, &effectful, &[Value::Int(1)]),
            Err(Gate::PublishedRow)
        );
        let unknown = code_closure(Some("never.declared"), &["x"], var("x"));
        assert_eq!(
            gate(&c, &unknown, &[Value::Int(1)]),
            Err(Gate::PublishedRow)
        );
        assert_eq!(
            admit(
                &double_closure(),
                &[Value::Int(21)],
                false,
                None,
                &CarriedTypes::over(None),
                DEFAULT_MAX_CALLS,
                0
            ),
            Err(Gate::PublishedRow),
            "a machine with no `CheckOutput` cleared a definition it has no row for"
        );
        assert_eq!(gate(&c, &double_closure(), &[Value::Int(21)]), admitted());
    }

    /// `budget` is the machine's remaining nested calls, so the last one belongs to the machine:
    /// the interpreted path raises the bound at the machine's own span.
    #[test]
    fn the_last_nested_call_is_refused_by_the_budget_gate() {
        let c = checked(vec![double_def()]);
        let subject = double_closure();
        let args = [Value::Int(21)];
        let types = CarriedTypes::over(Some(&c.check));
        let at = |max: usize, calls: usize| {
            admit(&subject, &args, false, Some(&c.check), &types, max, calls)
                .map(|(_, budget)| budget)
        };
        assert_eq!(at(8, 8), Err(Gate::Budget));
        // Not evidence for `checked_sub` over `saturating_sub`: both answer `Err` here, one via
        // `None` and one via the `budget == 0` refusal.
        assert_eq!(at(8, 9), Err(Gate::Budget));
        assert_eq!(at(8, 7), Ok(1));
        assert_eq!(at(8, 0), Ok(8));
    }

    /// The ordering is a cost claim — a call taking a record, a list or a string is refused on one
    /// discriminant test per argument and never hashes a `Symbol` into `CheckOutput::defs` — and a
    /// cost claim nothing asserts is a comment.
    #[test]
    fn the_shape_gate_is_reached_before_the_row_is_looked_up() {
        let c = checked(vec![double_def()]);
        let unknown = code_closure(Some("never.declared"), &["x"], var("x"));
        assert_eq!(
            gate(&c, &unknown, &[Value::str("21")]),
            Err(Gate::ArgumentShape),
            "the row was looked up for a call the argument shape had already refused"
        );
        let anonymous = code_closure(None, &["x"], var("x"));
        assert_eq!(
            gate(&c, &anonymous, &[Value::str("21")]),
            Err(Gate::ArgumentShape)
        );
        // Re-taken for the type gate (the fragment census registered this debt): a `Record` argument is
        // NOT in the lookup-free half any more.
        let record = Value::Record(Arc::new(Fields::default()));
        assert_eq!(
            gate(&c, &unknown, std::slice::from_ref(&record)),
            Err(Gate::PublishedRow),
            "a `Record` argument is still refused before the row is looked up, so              the cost claim this test re-takes did not actually change"
        );
        assert_eq!(
            gate(&c, &anonymous, &[record]),
            Err(Gate::Anonymous),
            "a `Record` argument under an anonymous body is still refused before              the name gate"
        );
    }

    /// The published row is the reviewable artifact, and it is what both the constant memo and this
    /// boundary read.
    #[test]
    fn a_definition_whose_published_row_is_not_empty_is_never_offered() {
        let c = checked(vec![
            effect_def("state", &[("get", ply_syntax::ast::Mode::Read, false)]),
            fn_def_sig(
                "touch",
                &[("x", tcon("Int"))],
                tcon("Int"),
                perform("state", "get", None, vec![var("x")]),
            ),
            fn_def_sig(
                "bump",
                &[("x", tcon("Int"))],
                tcon("Int"),
                bin(BinOp::Add, var("x"), int(0)),
            ),
        ]);
        assert!(
            !c.check.defs[&Symbol::new("touch")].footprint.is_empty(),
            "the fixture is wrong: `touch` publishes an empty row"
        );
        assert_eq!(
            gate(
                &c,
                &code_closure(Some("touch"), &["x"], var("x")),
                &[Value::Int(1)]
            ),
            Err(Gate::PublishedRow),
            "the row gate is not what refused a definition whose row is not empty"
        );

        let backend = Double::declining(&c.program);
        let mut machine = c.machine();
        machine.set_compiled(backend.clone());
        // `bump` is the control: same shape, same arguments, empty row, and it sits inside the same
        // `handle` so the hook is demonstrably live there.
        let e = handle(
            bin(
                BinOp::Add,
                callv("touch", vec![int(1)]),
                callv("bump", vec![int(0)]),
            ),
            vec![clause(
                "state",
                "get",
                None,
                &["n"],
                bin(BinOp::Add, var("n"), int(1)),
            )],
        );
        assert_eq!(ok(machine.eval_expr_for_test(&e)), Value::Int(2));
        assert_eq!(
            backend.names(),
            vec!["bump"],
            "a definition that can `perform` was offered to a backend"
        );
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

    /// The gate this whole change exists for (`CONTRIBUTING.md` §"Things known to be broken" item
    /// 11).
    #[test]
    fn a_definition_that_discharges_its_own_effects_is_refused_by_the_internal_effects_gate() {
        let c = self_handled();
        let handled = &c.check.defs[&Symbol::new("handled")];
        assert!(
            handled.footprint.is_empty() && handled.performed.is_empty(),
            "the fixture is wrong: `handled` publishes {:?} and performed {:?}, so the row gate \
             would refuse it and this test would prove nothing",
            handled.footprint,
            handled.performed
        );
        assert!(
            crate::memo::pure_by_published_row(Some(&c.check), &Symbol::new("handled")),
            "the row gate refused `handled`, so nothing below is about the effects gate"
        );

        let subject = code_closure(Some("handled"), &["x"], var("x"));
        assert_eq!(
            gate(&c, &subject, &[Value::Int(1)]),
            Err(Gate::InternalEffects)
        );

        let control = code_closure(Some("bump"), &["x"], var("x"));
        assert_eq!(
            gate(&c, &control, &[Value::Int(1)]),
            Ok(("bump".to_string(), DEFAULT_MAX_CALLS)),
            "a genuinely pure definition in the same program was refused too, so the refusal \
             above is not about this program"
        );
    }

    /// The half a per-body fact cannot reach, and the reason `DefInfo::internally_effectful` is
    /// transitive.
    #[test]
    fn a_definition_that_only_calls_one_that_discharges_its_own_effects_is_refused_too() {
        let c = self_handled();
        let wrapper = &c.check.defs[&Symbol::new("wrapper")];
        assert!(
            wrapper.footprint.is_empty() && wrapper.performed.is_empty(),
            "the fixture is wrong: `wrapper` publishes a row, so the row gate would refuse it"
        );

        let subject = code_closure(Some("wrapper"), &["x"], var("x"));
        assert_eq!(
            gate(&c, &subject, &[Value::Int(1)]),
            Err(Gate::InternalEffects)
        );

        // The atoms this refusal is protecting, measured rather than asserted from the row: running
        // `wrapper` performs, and the published row says it does not.
        let mut machine = c.machine();
        assert_eq!(
            ok(machine.eval_expr_for_test(&callv("wrapper", vec![int(1)]))),
            Value::Int(2)
        );
        assert_eq!(machine.trace().performs(), 1);
        assert_eq!(
            machine
                .trace()
                .footprint()
                .atoms()
                .map(|a| a.to_string())
                .collect::<Vec<_>>(),
            vec!["state.read".to_string()],
            "the engine recorded no atom, so entering `wrapper` would lose nothing and this \
             gate would be pointless"
        );
    }

    /// The same thing said about a run rather than about the gate: with a backend attached, neither
    /// the definition that handles its own operation nor the one that merely calls it is ever
    /// offered, and the atoms both of them perform are still recorded.
    #[test]
    fn nothing_that_performs_under_its_own_handler_is_offered_to_a_backend() {
        let c = self_handled();
        let backend = Double::declining(&c.program);
        let mut machine = c.machine();
        machine.set_compiled(backend.clone());

        let e = bin(
            BinOp::Add,
            bin(
                BinOp::Add,
                callv("handled", vec![int(1)]),
                callv("wrapper", vec![int(10)]),
            ),
            callv("bump", vec![int(100)]),
        );
        assert_eq!(ok(machine.eval_expr_for_test(&e)), Value::Int(113));
        assert_eq!(
            backend.names(),
            vec!["bump"],
            "a definition that performs under its own handler was offered to a backend"
        );
        assert_eq!(machine.trace().performs(), 2);
    }

    /// The whole partial-order story, and the reason it is one gate: inside a region every cell
    /// touch and every allocation is an `Access` the search prunes on, and a body the machine did
    /// not run records none of them.
    #[test]
    fn nothing_is_offered_inside_a_simulate_region() {
        let c = checked(vec![double_def()]);
        let backend = Double::declining(&c.program);
        let mut machine = c.machine();
        machine.set_compiled(backend.clone());
        // The control is the second half of the same expression, on the same machine and the same
        // definition: `double(1)` outside the region is offered, so the silence inside it is a gate
        // firing rather than a fixture that never reached the hook.
        let e = bin(
            BinOp::Add,
            ex(ExprKind::Simulate {
                body: Box::new(callv("double", vec![int(21)])),
            }),
            callv("double", vec![int(1)]),
        );
        assert_eq!(ok(machine.eval_expr_for_test(&e)), Value::Int(44));
        assert_eq!(
            backend.offers(),
            vec![Offer {
                name: Symbol::new("double"),
                args: vec![Value::Int(1)],
                budget: DEFAULT_MAX_CALLS,
            }],
            "a call inside a `simulate` region reached the backend"
        );
    }

    /// The read side of the constant memo stays ahead of the hook, and the write side still goes
    /// through `Frame::Call { memo }`.
    #[test]
    fn a_nullary_constant_is_entered_once_and_memoized_afterwards() {
        let c = checked(vec![fn_def_sig("answer", &[], tcon("Int"), int(1))]);
        let backend = Double::answering(&c.program, "answer", Value::Int(7));
        let mut machine = c.machine();
        machine.set_compiled(backend.clone());
        let e = bin(
            BinOp::Add,
            callv("answer", vec![]),
            bin(BinOp::Add, callv("answer", vec![]), callv("answer", vec![])),
        );
        assert_eq!(ok(machine.eval_expr_for_test(&e)), Value::Int(21));
        assert_eq!(
            backend.offers().len(),
            1,
            "the memo did not take over after the first compiled entry: {:?}",
            backend.offers()
        );
        assert_eq!(machine.compiled_counts(), (1, 0));
    }

    /// `limit.rs` exists so a runaway recursion is a diagnostic.
    #[test]
    fn the_budget_is_the_machines_remaining_depth_and_never_reaches_zero() {
        let c = checked(vec![fn_def_sig(
            "down",
            &[("n", tcon("Int"))],
            tcon("Int"),
            if_(
                bin(BinOp::Eq, var("n"), int(0)),
                int(0),
                callv("down", vec![bin(BinOp::Sub, var("n"), int(1))]),
            ),
        )]);
        let subject = callv("down", vec![int(1_000)]);

        let baseline = rendered(&c.machine().with_max_calls(8).eval_expr_for_test(&subject));
        assert!(
            baseline.contains("recursion limit of 8 nested calls exceeded"),
            "the fixture never reached the bound: {baseline}"
        );

        let backend = Double::declining(&c.program);
        let mut machine = c.machine().with_max_calls(8);
        machine.set_compiled(backend.clone());
        assert_eq!(rendered(&machine.eval_expr_for_test(&subject)), baseline);

        let budgets: Vec<usize> = backend.offers().iter().map(|o| o.budget).collect();
        assert_eq!(
            budgets,
            vec![8, 7, 6, 5, 4, 3, 2, 1],
            "a backend was handed a depth the machine did not have left"
        );
    }

    /// The free list serves an entered call's argument vector too.
    #[test]
    fn an_entered_call_returns_its_argument_vector_to_the_free_list() {
        let c = checked(vec![double_def()]);
        let subject = callv("double", vec![int(21)]);

        argv::drain_the_free_list();
        let mut interpreted = c.machine();
        assert_eq!(ok(interpreted.eval_expr_for_test(&subject)), Value::Int(42));
        let after_interpreted = argv::kept();

        argv::drain_the_free_list();
        let backend = Double::answering(&c.program, "double", Value::Int(84));
        let mut machine = c.machine();
        machine.set_compiled(backend);
        assert_eq!(ok(machine.eval_expr_for_test(&subject)), Value::Int(84));
        let after_entered = argv::kept();

        assert!(
            after_interpreted[0] > 0,
            "the fixture never used a pooled buffer at all"
        );
        assert_eq!(
            after_entered, after_interpreted,
            "the entered path left the free list in a different state than the interpreted one"
        );
    }

    /// The gates are ordered so that the argument shape is tested before the name is looked up.
    #[test]
    fn a_call_taking_a_non_scalar_is_never_offered() {
        let c = checked(vec![
            double_def(),
            fn_def_poly(
                "head",
                &["a"],
                &[("xs", tapp("List", vec![tvar("a")]))],
                tcon("Int"),
                callv("len", vec![var("xs")]),
            ),
            fn_def_poly(
                "width",
                &["a"],
                &[("s", tapp("List", vec![tvar("a")]))],
                tcon("Int"),
                callv("len", vec![var("s")]),
            ),
        ]);
        let backend = Double::declining(&c.program);
        let mut machine = c.machine();
        machine.set_compiled(backend.clone());
        assert_eq!(
            ok(machine.eval_expr_for_test(&callv("head", vec![list(vec![int(1), int(2)])]))),
            Value::Int(2)
        );
        assert_eq!(
            ok(machine.eval_expr_for_test(&callv("width", vec![string("abcd")]))),
            Value::Int(4)
        );
        assert!(
            backend.offers().is_empty(),
            "a non-scalar argument reached a backend: {:?}",
            backend.offers()
        );
        assert_eq!(machine.compiled_counts(), (0, 0));

        // Control: the hook is live on this machine.
        assert_eq!(
            ok(machine.eval_expr_for_test(&callv("double", vec![int(21)]))),
            Value::Int(42)
        );
        assert_eq!(backend.names(), vec!["double"]);
    }

    /// A `Secret` may not cross in either direction: `value.rs` redacts it on render and
    /// `escape.rs` walks its payload deliberately, and a backend builds messages the machine never
    /// sees.
    #[test]
    fn a_secret_is_never_offered_and_never_accepted() {
        let c = checked(vec![double_def()]);
        let backend = Double::declining(&c.program);
        let mut machine = c.machine();
        machine.set_compiled(backend.clone());
        // Reaching `enter_code` with a `Secret` argument goes through `call`, which is the only
        // route that can carry a value the program did not build.
        let outcome = machine.call(
            "double",
            vec![Value::Secret(Arc::new(Value::str("hunter2")))],
            sp(),
        );
        let rendered = rendered(&outcome);
        assert!(
            rendered.contains("E0502") && rendered.contains("arithmetic expects Int"),
            "the fixture stopped reaching the hook: {rendered}"
        );
        assert!(!rendered.contains("hunter2"), "a credential was printed");
        assert!(
            backend.offers().is_empty(),
            "a `Secret` was handed to a backend"
        );
        backend.forget();
        // Control: the same definition with an `Int` is offered.
        assert_eq!(
            ok(machine.call("double", vec![Value::Int(21)], sp())),
            Value::Int(42)
        );
        assert_eq!(backend.names(), vec!["double"]);
        drop(machine);

        let answering =
            Double::answering(&c.program, "double", Value::Secret(Arc::new(Value::Int(1))));
        let mut machine = c.machine();
        machine.set_compiled(answering);
        assert_eq!(
            ok(machine.eval_expr_for_test(&callv("double", vec![int(21)]))),
            Value::Int(42)
        );
        assert_eq!(machine.compiled_refusals(), 1);
    }

    /// The `--audit-backend` comparison, taken between a machine with a backend and one without: the
    /// rendered value, the outcome field by field, the footprint, and the cell arena slot by slot.
    #[track_caller]
    fn agree_on(c: &Checked, backend: Rc<Double>, e: &Expr) {
        let mut plain = c.machine();
        let mut entered = c.machine();
        entered.set_compiled(backend);
        let left = plain.eval_expr_for_test(e);
        let right = entered.eval_expr_for_test(e);
        if let Some(d) =
            compare_answers(&plain, &entered, "the expression under test", &left, &right)
        {
            panic!("a backend changed what the machine did — {d}");
        }
    }

    /// A continuation cannot be captured beneath a native activation, because nothing runs in the
    /// machine while a body runs and the body has returned before its `Frame::Call` is even pushed.
    #[test]
    fn a_multi_shot_resume_over_an_entered_call_answers_what_the_machine_answers() {
        let c = checked(vec![
            effect_def("state", &[("get", ply_syntax::ast::Mode::Read, false)]),
            fn_def_sig(
                "triple",
                &[("x", tcon("Int"))],
                tcon("Int"),
                bin(BinOp::Mul, var("x"), int(3)),
            ),
        ]);
        let e = handle(
            bin(
                BinOp::Add,
                perform("state", "get", None, vec![]),
                callv("triple", vec![int(2)]),
            ),
            vec![general_clause(
                "state",
                "get",
                None,
                &[],
                "k",
                bin(
                    BinOp::Add,
                    callv("k", vec![int(1)]),
                    callv("k", vec![int(10)]),
                ),
            )],
        );

        // The backend answers exactly what the body computes, so an identical result is evidence
        // about the control flow rather than about the value.
        agree_on(
            &c,
            Double::answering(&c.program, "triple", Value::Int(6)),
            &e,
        );

        let backend = Double::answering(&c.program, "triple", Value::Int(6));
        let mut machine = c.machine();
        machine.set_compiled(backend.clone());
        // `k(1)` gives `1 + 6`, `k(10)` gives `10 + 6`.
        assert_eq!(ok(machine.eval_expr_for_test(&e)), Value::Int(23));
        assert_eq!(
            backend.names(),
            vec!["triple", "triple"],
            "the fixture did not enter compiled code once per resumption"
        );
        assert_eq!(machine.compiled_counts(), (2, 0));
    }

    /// The other half of the same invariant: a clause that never resumes leaves the delimiter with
    /// its own value, and an entered call that already finished is not parked waiting for anything.
    #[test]
    fn a_discarded_continuation_over_an_entered_call_halts_with_the_handlers_value() {
        let c = checked(vec![
            effect_def("state", &[("get", ply_syntax::ast::Mode::Read, false)]),
            fn_def_sig(
                "triple",
                &[("x", tcon("Int"))],
                tcon("Int"),
                bin(BinOp::Mul, var("x"), int(3)),
            ),
        ]);
        let e = handle(
            bin(
                BinOp::Add,
                callv("triple", vec![int(2)]),
                perform("state", "get", None, vec![]),
            ),
            vec![general_clause("state", "get", None, &[], "k", int(99))],
        );

        agree_on(
            &c,
            Double::answering(&c.program, "triple", Value::Int(6)),
            &e,
        );

        let backend = Double::answering(&c.program, "triple", Value::Int(6));
        let mut machine = c.machine();
        machine.set_compiled(backend.clone());
        assert_eq!(ok(machine.eval_expr_for_test(&e)), Value::Int(99));
        assert_eq!(
            backend.names(),
            vec!["triple"],
            "the fixture never entered compiled code before the clause discarded"
        );
    }

    /// `differential::audit_state` compares the final arena as the ordered `(Slot, rendered value)`
    /// sequence, and an entered call must leave it alone.
    #[test]
    fn a_cell_touching_caller_agrees_slot_for_slot_with_an_entered_callee() {
        let c = checked(vec![fn_def_sig(
            "bump",
            &[("x", tcon("Int"))],
            tcon("Int"),
            bin(BinOp::Add, var("x"), int(1)),
        )]);
        let e = with_cell(
            "s",
            int(1),
            "c",
            block(
                vec![discard(callv(
                    "cell_set",
                    vec![var("c"), callv("bump", vec![int(8)])],
                ))],
                Some(callv("cell_get", vec![var("c")])),
            ),
        );
        agree_on(&c, Double::answering(&c.program, "bump", Value::Int(9)), &e);

        let backend = Double::answering(&c.program, "bump", Value::Int(9));
        let mut machine = c.machine();
        machine.set_compiled(backend.clone());
        assert_eq!(ok(machine.eval_expr_for_test(&e)), Value::Int(9));
        assert_eq!(machine.compiled_counts(), (1, 0));
    }

    /// The one difference this boundary knowingly makes, asserted rather than assumed.
    #[test]
    fn an_entered_definition_that_opens_its_own_region_skips_an_allocation() {
        let c = checked(vec![fn_def_poly(
            "boxed",
            &["a"],
            &[("n", tvar("a"))],
            tvar("a"),
            with_cell("s", var("n"), "c", callv("cell_get", vec![var("c")])),
        )]);
        assert!(
            c.check.defs[&Symbol::new("boxed")].footprint.is_empty(),
            "the fixture is wrong: `boxed` does not publish an empty row"
        );
        let e = callv("boxed", vec![int(5)]);

        // Program-visible state is identical, arena included.
        agree_on(
            &c,
            Double::answering(&c.program, "boxed", Value::Int(5)),
            &e,
        );

        let mut plain = c.machine();
        assert_eq!(ok(plain.eval_expr_for_test(&e)), Value::Int(5));
        let interpreted_allocations = plain.cells().stats().allocations;

        let mut entered = c.machine();
        entered.set_compiled(Double::answering(&c.program, "boxed", Value::Int(5)));
        assert_eq!(ok(entered.eval_expr_for_test(&e)), Value::Int(5));
        assert_eq!(entered.compiled_counts(), (1, 0));

        assert!(interpreted_allocations > 0);
        assert_eq!(
            entered.cells().stats().allocations,
            interpreted_allocations - 1,
            "the accounting this test exists to pin down moved"
        );
    }

    /// A backend cannot raise — `enter` answers a `Value` or nothing — so every diagnostic a run
    /// produces is the machine's own, at the machine's own span, with the machine's own labels and
    /// notes.
    #[test]
    fn a_failure_after_an_accepted_call_is_the_machines_own_diagnostic() {
        let c = checked(vec![
            fn_def_sig(
                "safe",
                &[("x", tcon("Int"))],
                tcon("Int"),
                bin(BinOp::Add, var("x"), int(1)),
            ),
            fn_def_sig(
                "risky",
                &[("x", tcon("Int"))],
                tcon("Int"),
                bin(BinOp::Div, int(10), var("x")),
            ),
        ]);
        let subjects = [
            bin(BinOp::Div, callv("safe", vec![int(1)]), int(0)),
            callv("risky", vec![callv("safe", vec![int(-1)])]),
            bin(
                BinOp::Add,
                callv("safe", vec![int(i64::MAX - 1)]),
                int(i64::MAX),
            ),
        ];

        let baseline: Vec<String> = {
            let mut plain = c.machine();
            subjects
                .iter()
                .map(|e| rendered(&plain.eval_expr_for_test(e)))
                .collect()
        };
        assert!(
            baseline.iter().all(|r| r.starts_with("Err(")),
            "the fixture stopped failing: {baseline:?}"
        );

        // Faithful rather than constant: what is under test is where the diagnostic comes from, and
        // a backend answering the wrong number would be testing that instead.
        let backend = Double::over(&c.program, |name, args, _| match (name.as_str(), args) {
            ("safe", [Value::Int(x)]) => x.checked_add(1).map(Value::Int),
            _ => None,
        });
        let mut machine = c.machine();
        machine.set_compiled(backend.clone());
        for (e, want) in subjects.iter().zip(&baseline) {
            assert_eq!(&rendered(&machine.eval_expr_for_test(e)), want);
        }
        let (entries, _) = machine.compiled_counts();
        assert_eq!(
            entries,
            subjects.len() as u64,
            "the backend was never entered, so this proves nothing about failures under it"
        );
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

    /// A `simulate` in a *definition's* body is a different case from a call made inside a live
    /// region, and it is refused by a different gate: the row gains `sim.read`, so the purity gate
    /// takes it.
    #[test]
    fn a_definition_that_opens_its_own_simulate_region_is_never_offered() {
        let c = checked(vec![
            double_def(),
            fn_def_sig(
                "searched",
                &[("n", tcon("Int"))],
                tcon("Int"),
                ex(ExprKind::Simulate {
                    body: Box::new(bin(BinOp::Add, var("n"), int(1))),
                }),
            ),
        ]);
        assert!(
            !c.check.defs[&Symbol::new("searched")].footprint.is_empty(),
            "the fixture is wrong: a `simulate` body published an empty row"
        );

        let backend = Double::declining(&c.program);
        let mut machine = c.machine();
        machine.set_compiled(backend.clone());
        assert_eq!(
            ok(machine.eval_expr_for_test(&bin(
                BinOp::Add,
                callv("searched", vec![int(1)]),
                callv("double", vec![int(0)]),
            ))),
            Value::Int(2)
        );
        assert_eq!(
            backend.names(),
            vec!["double"],
            "a definition that opens a `simulate` region was offered to a backend"
        );
    }

    #[test]
    fn crossable_admits_the_two_scalars_and_bytes_and_nothing_else() {
        assert!(crossable(&Value::Int(0)));
        assert!(crossable(&Value::Bool(false)));
        assert!(crossable(&Value::bytes(b"GET /orders HTTP/1.1")));
        assert!(
            crossable(&Value::bytes(b"")),
            "an empty `Bytes` is a `Bytes`"
        );
        for refused in [
            Value::Float(0.0),
            Value::str("s"),
            Value::Unit,
            Value::List(Default::default()),
            Value::Secret(Arc::new(Value::Int(1))),
            Value::Secret(Arc::new(Value::bytes(b"hunter2"))),
        ] {
            assert!(!crossable(&refused), "{refused:?} crossed the boundary");
        }
    }

    /// The property that makes [`crossable`]'s shallow test a sound one, asked of the containers
    /// rather than of the scalars.
    #[test]
    fn a_closure_bearing_record_is_refused_on_its_declared_type() {
        let c = checked_source(
            "type Box = { run: (Int) -> Int, tag: Int }\n\
             type Plain = { tag: Int }\n\
             fn use_box(b: Box) -> Int = b.tag\n\
             fn use_plain(p: Plain) -> Int = p.tag\n",
        );
        let mut fields = BTreeMap::new();
        fields.insert(
            Symbol::new("run"),
            Value::Closure(Arc::new(code_closure(
                None,
                &["y"],
                bin(BinOp::Mul, var("y"), int(2)),
            ))),
        );
        fields.insert(Symbol::new("tag"), Value::Int(1));
        let holding = Value::Record(Arc::new(fields.into_iter().collect()));

        assert_eq!(
            gate(&c, &named(&c, "use_box"), &[holding]),
            Err(Gate::ArgumentType),
            "a record whose declared type holds a `Closure` crossed the boundary, so \
             `internally_effectful`'s argument now has a hole one field deep"
        );

        // The same record *shape* under a declared type that cannot hold code: `tag` alone.
        let mut plain = BTreeMap::new();
        plain.insert(Symbol::new("tag"), Value::Int(1));
        assert_eq!(
            gate(
                &c,
                &named(&c, "use_plain"),
                &[Value::Record(Arc::new(plain.into_iter().collect()))]
            ),
            Ok(("use_plain".to_string(), DEFAULT_MAX_CALLS)),
            "a record of `Int` did not cross, so the widening bought nothing"
        );

        // And the empty one, which is where a value walk would have differed: an empty `Box` holds
        // no closure and is refused anyway, because the question asked is about the type.
        assert_eq!(
            gate(
                &c,
                &named(&c, "use_box"),
                &[Value::Record(Arc::new(Fields::default()))]
            ),
            Err(Gate::ArgumentType),
            "an empty record under a closure-bearing declared type crossed"
        );
    }

    /// A declared sum type is decided from its constructors' field types, and a type that mentions
    /// itself must not make that decision recurse.
    #[test]
    fn a_recursive_type_is_decided_rather_than_walked_into_itself() {
        let c = checked_source(
            "type Tree = | Leaf | Node(Tree, Int)\n\
             type Even = | EZero | ESucc(Odd)\n\
             type Odd = | OSucc(Even)\n\
             fn use_tree(t: Tree) -> Int = 0\n\
             fn use_even(e: Even) -> Int = 0\n",
        );
        let leaf = Value::Ctor {
            name: Symbol::new("Leaf"),
            args: Arc::new(Vec::new()),
        };
        assert_eq!(
            gate(&c, &named(&c, "use_tree"), &[leaf]),
            Ok(("use_tree".to_string(), DEFAULT_MAX_CALLS)),
            "a recursive type of carried fields was refused"
        );
        let zero = Value::Ctor {
            name: Symbol::new("EZero"),
            args: Arc::new(Vec::new()),
        };
        assert_eq!(
            gate(&c, &named(&c, "use_even"), &[zero]),
            Ok(("use_even".to_string(), DEFAULT_MAX_CALLS)),
            "a mutually recursive pair of carried types was refused"
        );
    }

    /// The other side of it: recursion must not make a type that *does* reach a closure look
    /// carried.
    #[test]
    fn a_recursive_type_that_reaches_a_closure_is_refused() {
        let c = checked_source(
            "type Bad = | BLeaf((Int) -> Int) | BNode(Bad)\n\
             type Ping = | PNil | PCons(Pong)\n\
             type Pong = | QNil | QCons(Ping, (Int) -> Int)\n\
             fn use_bad(b: Bad) -> Int = 0\n\
             fn use_ping(p: Ping) -> Int = 0\n",
        );
        for (name, ctor) in [("use_bad", "BNode"), ("use_ping", "PNil")] {
            let value = Value::Ctor {
                name: Symbol::new(ctor),
                args: Arc::new(Vec::new()),
            };
            assert_eq!(
                gate(&c, &named(&c, name), &[value]),
                Err(Gate::ArgumentType),
                "{name} took a value whose declared type reaches a closure"
            );
        }
    }

    /// Generics: refused on the type, and rescued by the value when the value is childless.
    #[test]
    fn a_type_variable_parameter_is_refused_unless_the_value_is_childless() {
        let c = checked_source(
            "fn poly<a>(x: a, n: Int) -> Int = n\n\
             fn ints(xs: List<Int>) -> Int = len(xs)\n",
        );
        // The decision itself, asked of the type rather than of a call, because the two assertions
        // below are both satisfied by a rule that admits `Type::Var` and is rescued by the kind
        // comparison.
        let types = c.types();
        let ply_core::ty::Type::Fn { params, .. } = &c.check.defs[&Symbol::new("poly")].scheme.ty
        else {
            panic!("poly publishes no function type");
        };
        assert!(
            matches!(params[0], ply_core::ty::Type::Var(_)),
            "the fixture stopped being generic: {:?}",
            params[0]
        );
        assert!(
            !types.carries(&params[0], None),
            "a `Type::Var` is carried, so a closure passed at that position would cross"
        );
        assert!(
            types.carries(&params[1], None),
            "the control failed: `Int` is not carried"
        );

        let poly = named(&c, "poly");
        assert_eq!(
            gate(&c, &poly, &[Value::Int(1), Value::Int(2)]),
            Ok(("poly".to_string(), DEFAULT_MAX_CALLS)),
            "a generic definition called at a scalar stopped being admitted, so \
             the type gate is a trade and not a widening"
        );
        assert_eq!(
            gate(
                &c,
                &poly,
                &[Value::List(Arc::new(vec![Value::Int(1)])), Value::Int(2)]
            ),
            Err(Gate::ArgumentType),
            "a container crossed under a `Type::Var`, which can be a closure"
        );
        // And the same container under a declared `List<Int>` does cross, so the refusal above is
        // the variable's and not the list's.
        assert_eq!(
            gate(
                &c,
                &named(&c, "ints"),
                &[Value::List(Arc::new(vec![Value::Int(1)]))]
            ),
            Ok(("ints".to_string(), DEFAULT_MAX_CALLS))
        );
    }

    /// A declared type licenses a value only when the value is of the kind that type denotes.
    #[test]
    fn a_value_whose_kind_is_not_its_declared_types_is_refused() {
        let c = checked_source("fn twice(x: Int) -> Int = x * 2\n");
        let holding = Value::List(Arc::new(vec![Value::Closure(Arc::new(code_closure(
            None,
            &["y"],
            var("y"),
        )))]));
        assert_eq!(
            gate(&c, &named(&c, "twice"), &[holding]),
            Err(Gate::ArgumentType),
            "a `List` holding a `Closure` crossed under a declared `Int`"
        );
        assert_eq!(
            gate(&c, &named(&c, "twice"), &[Value::Int(21)]),
            Ok(("twice".to_string(), DEFAULT_MAX_CALLS))
        );
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

    /// A record argument reaching a real backend through a real machine, which is what every gate
    /// assertion above is a proxy for.
    #[test]
    fn a_record_argument_reaches_a_backend_through_the_machine() {
        let c = checked_source(
            "type Pair = { a: Int, b: Bytes }\n\
             fn first(p: Pair) -> Int = p.a\n\
             test \"t\" { assert(first({a: 7, b: b\"x\"}) == 7) }\n",
        );
        let backend = Double::declining(&c.program);
        let mut machine = c.machine();
        machine.set_compiled(backend.clone());
        let call = callv(
            "first",
            vec![record(vec![("a", int(7)), ("b", bytes(b"x"))])],
        );
        assert_eq!(ok(machine.eval_expr_for_test(&call)), Value::Int(7));
        assert_eq!(backend.names(), vec!["first"]);
        assert_eq!(
            backend.offers()[0].args,
            vec![Value::Record(Arc::new(Fields::from_iter([
                (Symbol::new("a"), Value::Int(7)),
                (Symbol::new("b"), Value::bytes(b"x")),
            ])))],
            "the record the machine offered is not the one the call built"
        );
    }

    /// An arity mismatch is the machine's diagnostic, phrased from `closure.describe()`, and it
    /// stays ahead of the hook.
    #[test]
    fn an_arity_mismatch_is_the_machines_diagnostic_and_no_backend_sees_it() {
        let c = checked(vec![double_def()]);
        let subject = callv("double", vec![int(1), int(2)]);
        let baseline = rendered(&c.machine().eval_expr_for_test(&subject));

        let backend = Double::declining(&c.program);
        let mut machine = c.machine();
        machine.set_compiled(backend.clone());
        assert_eq!(rendered(&machine.eval_expr_for_test(&subject)), baseline);
        assert!(
            backend.offers().is_empty(),
            "a call whose arity does not match was offered to a backend"
        );
        // Control: at the right arity the same call is offered.
        assert_eq!(
            ok(machine.eval_expr_for_test(&callv("double", vec![int(21)]))),
            Value::Int(42)
        );
        assert_eq!(backend.names(), vec!["double"]);
    }

    /// One hop is `wrapper`.
    #[test]
    fn the_effects_gate_follows_a_call_chain_to_a_fixpoint_rather_than_one_hop() {
        let c = checked(vec![
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
                "hop1",
                &[("x", tcon("Int"))],
                tcon("Int"),
                callv("handled", vec![var("x")]),
            ),
            fn_def_sig(
                "hop2",
                &[("x", tcon("Int"))],
                tcon("Int"),
                callv("hop1", vec![var("x")]),
            ),
            fn_def_sig(
                "hop3",
                &[("x", tcon("Int"))],
                tcon("Int"),
                callv("hop2", vec![var("x")]),
            ),
            fn_def_sig(
                "hop4",
                &[("x", tcon("Int"))],
                tcon("Int"),
                callv("hop3", vec![var("x")]),
            ),
            // Only `ping` can reach the handler; `pong` reaches it through the recursion, which a
            // propagation that stopped at a cycle would miss.
            fn_def_sig(
                "ping",
                &[("x", tcon("Int"))],
                tcon("Int"),
                if_(
                    bin(BinOp::Lt, var("x"), int(1)),
                    callv("handled", vec![var("x")]),
                    callv("pong", vec![bin(BinOp::Sub, var("x"), int(1))]),
                ),
            ),
            fn_def_sig(
                "pong",
                &[("x", tcon("Int"))],
                tcon("Int"),
                callv("ping", vec![var("x")]),
            ),
            fn_def_sig(
                "via_lambda",
                &[("x", tcon("Int"))],
                tcon("Int"),
                block(
                    vec![letv("f", lam(&["y"], callv("handled", vec![var("y")])))],
                    Some(call(var("f"), vec![var("x")])),
                ),
            ),
            fn_def_sig(
                "clean1",
                &[("x", tcon("Int"))],
                tcon("Int"),
                bin(BinOp::Add, var("x"), int(1)),
            ),
            fn_def_sig(
                "clean2",
                &[("x", tcon("Int"))],
                tcon("Int"),
                callv("clean1", vec![var("x")]),
            ),
            fn_def_sig(
                "clean3",
                &[("x", tcon("Int"))],
                tcon("Int"),
                callv("clean2", vec![var("x")]),
            ),
            fn_def_sig(
                "clean4",
                &[("x", tcon("Int"))],
                tcon("Int"),
                callv("clean3", vec![var("x")]),
            ),
        ]);

        let refused = ["hop1", "hop2", "hop3", "hop4", "ping", "pong", "via_lambda"];
        for name in refused {
            let info = &c.check.defs[&Symbol::new(name)];
            assert!(
                info.footprint.is_empty() && info.performed.is_empty(),
                "the fixture is wrong: `{name}` publishes a row, so the row gate refuses it and \
                 this says nothing about the effects gate"
            );
            let subject = code_closure(Some(name), &["x"], var("x"));
            assert_eq!(
                gate(&c, &subject, &[Value::Int(1)]),
                Err(Gate::InternalEffects),
                "`{name}` was admitted, so the propagation stopped short of it"
            );
        }
        for name in ["clean1", "clean2", "clean3", "clean4"] {
            let subject = code_closure(Some(name), &["x"], var("x"));
            assert_eq!(
                gate(&c, &subject, &[Value::Int(1)]),
                Ok((name.to_string(), DEFAULT_MAX_CALLS)),
                "`{name}` is pure at every hop and was refused anyway"
            );
        }
    }

    // 2026-08-31.

    /// A definition answering a record is entered and its answer is used.
    #[test]
    fn a_record_answer_crosses_back_under_its_declared_return_type() {
        let c = checked_source(
            "type Scan = { at: Int, tok: Bytes }\n\
             fn scan(i: Int) -> Scan = { at: i, tok: b\"x\" }\n\
             fn count(i: Int) -> Int = i\n",
        );
        let answer = record_value(&[("at", Value::Int(7)), ("tok", Value::bytes(b"x"))]);

        let backend = Double::answering(&c.program, "scan", answer.clone());
        let mut machine = c.machine();
        machine.set_compiled(backend.clone());
        assert_eq!(
            ok(machine.eval_expr_for_test(&callv("scan", vec![int(1)]))),
            answer,
            "a record answer was refused under a declared return type that carries it"
        );
        assert_eq!(machine.compiled_counts(), (1, 0));
        assert_eq!(machine.compiled_refusals(), 0);
        drop(machine);

        let backend = Double::answering(&c.program, "count", answer.clone());
        let mut machine = c.machine();
        machine.set_compiled(backend.clone());
        assert_eq!(
            ok(machine.eval_expr_for_test(&callv("count", vec![int(3)]))),
            Value::Int(3),
            "a record answer was believed for a definition declared `-> Int`"
        );
        assert_eq!(machine.compiled_counts(), (0, 1));
        assert_eq!(machine.compiled_refusals(), 1);
    }

    /// A carried declared return type licenses one `Value` kind, not any.
    #[test]
    fn an_answer_whose_kind_is_not_its_declared_returns_is_refused_unless_it_is_childless() {
        let c = checked_source(
            "type Scan = { at: Int, tok: Bytes }\n\
             fn scan(i: Int) -> Scan = { at: i, tok: b\"x\" }\n",
        );
        let holding = Value::List(Arc::new(vec![Value::Closure(Arc::new(code_closure(
            None,
            &["y"],
            var("y"),
        )))]));
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

    /// The same claim for the deterministic scheduler, and this one is a gate away from where a
    /// reader looks for it.
    #[test]
    fn a_definition_that_calls_one_that_opens_a_simulate_region_is_never_offered() {
        let c = checked(vec![
            double_def(),
            fn_def_sig(
                "searched",
                &[("n", tcon("Int"))],
                tcon("Int"),
                ex(ExprKind::Simulate {
                    body: Box::new(bin(BinOp::Add, var("n"), int(1))),
                }),
            ),
            fn_def_sig(
                "outer",
                &[("n", tcon("Int"))],
                tcon("Int"),
                callv("searched", vec![var("n")]),
            ),
        ]);
        assert!(
            !c.check.defs[&Symbol::new("outer")].footprint.is_empty(),
            "the fixture is wrong: a definition two hops from a `simulate` published an empty \
             row, so the row gate would clear it and the subtree would be hidden"
        );

        let backend = Double::declining(&c.program);
        let mut machine = c.machine();
        machine.set_compiled(backend.clone());
        assert_eq!(
            ok(machine.eval_expr_for_test(&bin(
                BinOp::Add,
                callv("outer", vec![int(1)]),
                callv("double", vec![int(0)]),
            ))),
            Value::Int(2)
        );
        assert_eq!(
            backend.names(),
            vec!["double"],
            "a definition that reaches a `simulate` region two hops down was offered"
        );
    }

    /// The budget bounds the whole entered subtree, not the entry.
    #[test]
    fn an_entered_subtree_is_bounded_by_the_budget_it_was_handed_and_not_by_its_entry() {
        let c = checked_source(
            "fn down(n: Int) -> Int = if n <= 0 { 0 } else { down(n - 1) + 1 }\n\
             fn top(n: Int) -> Int = down(n)\n",
        );
        let call = callv("top", vec![int(400)]);

        let bare = Machine::new(&c.program, &c.resolved, &c.check).with_max_calls(50);
        let mut bare = bare;
        let without = bare.eval_expr_for_test(&call);
        assert!(
            rendered(&without).contains("recursion limit of 50 nested calls exceeded"),
            "the fixture is wrong: the machine did not reach its own bound: {}",
            rendered(&without)
        );
        drop(bare);

        let fragment = crate::backend::Fragment::over(&c.program, &c.resolved, &c.check);
        assert!(
            fragment.holds(&Symbol::new("top")) && fragment.holds(&Symbol::new("down")),
            "the fixture is wrong: the backend has no body for the recursion under test"
        );
        let mut backed = Machine::new(&c.program, &c.resolved, &c.check).with_max_calls(50);
        backed.set_compiled(fragment.attach(&crate::backend::Spec::honest()));
        let with = backed.eval_expr_for_test(&call);
        assert_eq!(
            rendered(&with),
            rendered(&without),
            "an entered subtree outran the machine's bound and answered where the machine raises"
        );
        assert_eq!(
            backed.compiled_counts().0,
            0,
            "the backend answered a call whose subtree cannot fit the budget"
        );

        // The control: the same program under a budget the recursion fits, so the refusal above is
        // the bound's and not the fixture's.
        let mut ok_run = Machine::new(&c.program, &c.resolved, &c.check);
        ok_run.set_compiled(fragment.attach(&crate::backend::Spec::honest()));
        assert_eq!(
            ok(ok_run.eval_expr_for_test(&call)),
            Value::Int(400),
            "the recursion does not fit the default bound either, so nothing above is about the \
             budget"
        );
        assert!(ok_run.compiled_counts().0 > 0);
    }

    /// What a collapse actually is, at unit scale: the machine offers the entry and never sees
    /// anything under it.
    #[test]
    fn an_entered_call_hides_its_subtree_and_the_machine_offers_none_of_it() {
        let c = checked_source(
            "type Scan = { at: Int }\n\
             fn leaf(i: Int) -> Int = i + 1\n\
             fn middle(i: Int) -> Int = leaf(i) + leaf(i)\n\
             fn outer(i: Int) -> Scan = { at: middle(i) }\n",
        );
        // Declining, so the machine evaluates everything itself: this is the set of calls the seam
        // is offered when nothing is entered.
        let declining = Double::declining(&c.program);
        let mut machine = c.machine();
        machine.set_compiled(declining.clone());
        assert_eq!(
            ok(machine.eval_expr_for_test(&callv("outer", vec![int(1)]))),
            record_value(&[("at", Value::Int(4))])
        );
        assert_eq!(
            declining.names(),
            vec!["outer", "middle", "leaf", "leaf"],
            "the fixture is wrong: the subtree this entry would hide is not offered without it"
        );
        drop(machine);

        // Answering, and the same expression offers exactly one call.
        let answering =
            Double::answering(&c.program, "outer", record_value(&[("at", Value::Int(4))]));
        let mut machine = c.machine();
        machine.set_compiled(answering.clone());
        assert_eq!(
            ok(machine.eval_expr_for_test(&callv("outer", vec![int(1)]))),
            record_value(&[("at", Value::Int(4))])
        );
        assert_eq!(
            answering.names(),
            vec!["outer"],
            "the entry did not swallow its subtree"
        );
        assert_eq!(machine.compiled_counts(), (1, 0));
    }

    /// A record `Value` from a list of fields, which no helper in [`crate::build`] answers because
    /// that module builds `Expr`s.
    fn record_value(fields: &[(&str, Value)]) -> Value {
        let mut map = BTreeMap::new();
        for (name, value) in fields {
            map.insert(Symbol::new(name), value.clone());
        }
        Value::Record(Arc::new(map.into_iter().collect()))
    }

    /// A `Float` in flight is what `crossable` refuses; this is the same statement about the answer
    /// rather than the argument, and it is separate because the two are separate gates.
    fn float(f: f64) -> Expr {
        ex(ExprKind::Lit(ply_syntax::ast::Lit::Float(f)))
    }
}
