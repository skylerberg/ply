use ply_core::{EffectAtom, Resource, Row, RowVar, Scheme, TyVar, Type};
use ply_span::Symbol;
use ply_store::{CachedCtor, CachedOp, DeclBody, canonicalize_decl_body, canonicalize_scheme};
use ply_syntax::ast::Mode;
use std::collections::BTreeMap;

fn var(n: u32) -> Type {
    Type::Var(TyVar(n))
}

/// `fn f<a, b, e>(a, b) -> a / e`, quantified over whatever numbers the run happened to hand
/// out.
fn identity_pair(a: u32, b: u32, e: u32) -> Scheme {
    Scheme {
        ty_vars: vec![TyVar(a), TyVar(b)],
        row_vars: vec![RowVar(e)],
        ty: Type::Fn {
            params: vec![var(a), var(b)],
            ret: Box::new(var(a)),
            effects: Row::open(RowVar(e)),
        },
    }
}

#[test]
fn two_runs_of_the_same_definition_canonicalize_to_the_same_bytes() {
    let cold = identity_pair(0, 1, 0);
    let warm = identity_pair(412, 87, 9);
    assert_ne!(cold, warm, "the counters really did differ");
    assert_eq!(canonicalize_scheme(&cold), canonicalize_scheme(&warm));
    assert_eq!(
        serde_json::to_string(&canonicalize_scheme(&cold)).unwrap(),
        serde_json::to_string(&canonicalize_scheme(&warm)).unwrap(),
        "byte-identical, which is what the equivalence test compares"
    );
}

#[test]
fn canonicalizing_twice_changes_nothing() {
    let once = canonicalize_scheme(&identity_pair(7, 3, 5));
    assert_eq!(canonicalize_scheme(&once), once);
}

#[test]
fn variables_are_numbered_by_first_use_not_by_the_quantifier_list() {
    let listed_backwards = Scheme {
        ty_vars: vec![TyVar(9), TyVar(4)],
        row_vars: vec![],
        ty: Type::Fn {
            params: vec![var(4)],
            ret: Box::new(var(9)),
            effects: Row::empty(),
        },
    };
    let canonical = canonicalize_scheme(&listed_backwards);
    assert_eq!(
        canonical.ty,
        Type::Fn {
            params: vec![var(0)],
            ret: Box::new(var(1)),
            effects: Row::empty(),
        }
    );
    assert_eq!(
        canonical.ty_vars,
        vec![TyVar(0), TyVar(1)],
        "the quantifier list is sorted, so its source order cannot leak in"
    );
}

#[test]
fn a_quantified_variable_the_body_never_mentions_still_gets_a_number() {
    let phantom = Scheme {
        ty_vars: vec![TyVar(2), TyVar(8)],
        row_vars: vec![],
        ty: var(8),
    };
    let reordered = Scheme {
        ty_vars: vec![TyVar(8), TyVar(2)],
        row_vars: vec![],
        ty: var(8),
    };
    assert_eq!(canonicalize_scheme(&phantom).ty, var(0));
    assert_eq!(
        canonicalize_scheme(&phantom).ty_vars,
        vec![TyVar(0), TyVar(1)]
    );
    assert_eq!(
        canonicalize_scheme(&phantom),
        canonicalize_scheme(&reordered)
    );
}

#[test]
fn a_difference_that_is_not_a_renaming_survives() {
    let shared = Scheme {
        ty_vars: vec![TyVar(0)],
        row_vars: vec![],
        ty: Type::Fn {
            params: vec![var(0)],
            ret: Box::new(var(0)),
            effects: Row::empty(),
        },
    };
    let distinct = Scheme {
        ty_vars: vec![TyVar(0), TyVar(1)],
        row_vars: vec![],
        ty: Type::Fn {
            params: vec![var(0)],
            ret: Box::new(var(1)),
            effects: Row::empty(),
        },
    };
    assert_ne!(
        canonicalize_scheme(&shared),
        canonicalize_scheme(&distinct),
        "`(a) -> a` and `(a) -> b` are not alpha-equivalent"
    );
}

#[test]
fn effect_atoms_and_type_constructors_are_left_alone() {
    let named = Scheme::mono(Type::Fn {
        params: vec![Type::Con(Symbol::new("Order"), vec![var(3)])],
        ret: Box::new(Type::int()),
        effects: Row::closed([EffectAtom::new(
            "store.db",
            Resource::Named(Symbol::new("users")),
            Mode::Write,
        )]),
    });
    let canonical = canonicalize_scheme(&named);
    let Type::Fn {
        params, effects, ..
    } = &canonical.ty
    else {
        panic!("shape must be preserved");
    };
    assert_eq!(params[0], Type::Con(Symbol::new("Order"), vec![var(0)]));
    assert_eq!(
        effects.atoms.iter().next().unwrap().effect,
        Symbol::new("store.db"),
        "renaming a label would make the cache answer for a different effect"
    );
}

#[test]
fn record_fields_are_walked_in_key_order() {
    let mut fields = BTreeMap::new();
    fields.insert(Symbol::new("b"), var(7));
    fields.insert(Symbol::new("a"), var(9));
    let canonical = canonicalize_scheme(&Scheme::mono(Type::Record(fields)));
    let Type::Record(out) = &canonical.ty else {
        panic!("shape must be preserved");
    };
    assert_eq!(out[&Symbol::new("a")], var(0));
    assert_eq!(out[&Symbol::new("b")], var(1));
}

#[test]
fn one_numbering_spans_a_declaration_so_its_constructors_stay_related() {
    // `type Pair<a, b> = | P(a, b) | Q(b)` — `Q`'s field is the *second* parameter, and
    // numbering each constructor on its own would erase that.
    let pair = |a: u32, b: u32| DeclBody::Type {
        arity: 2,
        ctors: vec![
            CachedCtor {
                fields: vec![var(a), var(b)],
                scheme: Scheme {
                    ty_vars: vec![TyVar(a), TyVar(b)],
                    row_vars: vec![],
                    ty: Type::Fn {
                        params: vec![var(a), var(b)],
                        ret: Box::new(Type::Con(Symbol::new("Pair"), vec![var(a), var(b)])),
                        effects: Row::empty(),
                    },
                },
            },
            CachedCtor {
                fields: vec![var(b)],
                scheme: Scheme {
                    ty_vars: vec![TyVar(a), TyVar(b)],
                    row_vars: vec![],
                    ty: Type::Fn {
                        params: vec![var(b)],
                        ret: Box::new(Type::Con(Symbol::new("Pair"), vec![var(a), var(b)])),
                        effects: Row::empty(),
                    },
                },
            },
        ],
    };

    let canonical = canonicalize_decl_body(&pair(6, 2));
    assert_eq!(
        canonical,
        canonicalize_decl_body(&pair(41, 40)),
        "two runs must agree"
    );

    let DeclBody::Type { ctors, .. } = &canonical else {
        panic!("shape must be preserved");
    };
    assert_eq!(ctors[0].fields, vec![var(0), var(1)]);
    assert_eq!(
        ctors[1].fields,
        vec![var(1)],
        "`Q` still names the second parameter"
    );
}

#[test]
fn an_effects_operations_share_one_numbering_too() {
    let body = |a: u32| DeclBody::Effect {
        nondet: true,
        ops: vec![
            CachedOp {
                name: Symbol::new("get"),
                mode: Mode::Read,
                resource_param: true,
                params: vec![Type::int()],
                ret: var(a),
            },
            CachedOp {
                name: Symbol::new("put"),
                mode: Mode::Write,
                resource_param: false,
                params: vec![var(a)],
                ret: Type::unit(),
            },
        ],
    };
    let canonical = canonicalize_decl_body(&body(31));
    assert_eq!(canonical, canonicalize_decl_body(&body(4)));
    let DeclBody::Effect { ops, nondet } = &canonical else {
        panic!("shape must be preserved");
    };
    assert!(nondet, "`nondet` decides whether a test may cache at all");
    assert_eq!(ops[0].ret, var(0));
    assert_eq!(ops[1].params, vec![var(0)]);
}
