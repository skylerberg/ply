//! Temporary: the compiler's emitter roots against the Rust derivation they replace.

use ply_span::Symbol;
use ply_ty::{Front, LawInfo, Ordinal, SpecKind, Type};
use std::collections::HashMap;

fn signature(ty: &Type) -> (&[Type], &Type) {
    match ty {
        Type::Fn { params, ret, .. } => (params, ret),
        other => (&[], other),
    }
}

fn is_scalar(ty: &Type) -> bool {
    matches!(ty, Type::Con(name, args) if args.is_empty() && matches!(name.as_str(), "Int" | "Bool"))
}

fn qualified(module: &Symbol, name: &Symbol) -> String {
    if module.as_str().is_empty() {
        return name.to_string();
    }
    format!("{module}.{name}")
}

#[derive(Debug, PartialEq, Eq)]
struct Row {
    root: String,
    arity: usize,
    scalar: bool,
}

fn old(front: &Front) -> Vec<Row> {
    let laws: HashMap<&Symbol, &LawInfo> = front.check.laws.iter().map(|l| (&l.key, l)).collect();
    let mut roots: Vec<Row> = Vec::new();
    let mut tests: Vec<Row> = Vec::new();
    let mut specs: Vec<Row> = Vec::new();
    for (module, items) in &front.ordinals {
        let (mut ordinal, mut law_ordinal) = (0, 0);
        for item in items {
            match item {
                Ordinal::Fn(name, kinds) => {
                    let root = name.to_string();
                    roots.push(Row {
                        root: root.clone(),
                        arity: 0,
                        scalar: false,
                    });
                    let Some(def) = front.check.defs.get(name) else {
                        continue;
                    };
                    let (params, ret) = signature(&def.scheme.ty);
                    let scalar_params = params.iter().all(is_scalar);
                    let arity = params.len() + def.scheme.label_vars.len();
                    let got = roots.last_mut().unwrap();
                    *got = Row {
                        root: root.clone(),
                        arity,
                        scalar: scalar_params && is_scalar(ret),
                    };
                    let (mut requires, mut ensures) = (0, 0);
                    for kind in kinds {
                        let (kind, k, arity, scalar) = match kind {
                            SpecKind::Requires => {
                                requires += 1;
                                ("requires", requires - 1, params.len(), scalar_params)
                            }
                            SpecKind::Ensures => {
                                ensures += 1;
                                (
                                    "ensures",
                                    ensures - 1,
                                    params.len() + 1,
                                    scalar_params && is_scalar(ret),
                                )
                            }
                        };
                        specs.push(Row {
                            root: format!("{root}#{kind}#{k}"),
                            arity,
                            scalar,
                        });
                    }
                }
                Ordinal::Test(_key) => {
                    let root = qualified(module, &ply_codegen::test_root_name(ordinal));
                    tests.push(Row {
                        root,
                        arity: 0,
                        scalar: false,
                    });
                    ordinal += 1;
                }
                Ordinal::Law(key) => {
                    let law = laws.get(key);
                    let binders = law.map(|l| l.binders.as_slice()).unwrap_or_default();
                    for part in ["guard", "body"] {
                        if part == "guard" && !law.is_some_and(|l| l.has_guard) {
                            continue;
                        }
                        let root =
                            qualified(module, &ply_codegen::law_root_name(law_ordinal, part));
                        let scalar = binders.iter().all(|b| is_scalar(&b.ty));
                        specs.push(Row {
                            root,
                            arity: binders.len(),
                            scalar,
                        });
                    }
                    law_ordinal += 1;
                }
            }
        }
    }
    roots.extend(tests);
    roots.extend(specs);
    roots
}

const SRC: &str = r#"
pub type Pair = { a: Int, b: Bool }
type Shape = | Dot | Line(Int)
fn positive(x: Int) -> Bool = x > 0
pub fn checked(x: Int) -> Int requires positive(x) ensures x >= 0 = x
pub fn wide(x: Bytes, y: Pair) -> Pair = y
pub fn relay<[l]>(b: Bytes) -> Unit / { log.write[l] } = log.emit[l](b)
pub effect log { write emit[l](Bytes) -> Unit }
fn shared() -> Int = 1
pub fn api() -> Int = shared() + checked(2)
test "adds" { assert_eq(api(), 3) }
law "identity" forall (x: Int) { checked(x) == x }
law "guarded" forall (x: Int) where x > 0 { checked(x) == x }
"#;

#[test]
fn the_compilers_emitter_roots_are_the_ones_rust_derived() {
    let id = ply_span::SourceId(0);
    let front =
        ply_codegen::c::producer::checked_front(&[("m".to_string(), SRC.to_string())], &[id])
            .expect("checks");
    let want = old(&front);
    let got: Vec<Row> = front
        .emitter_roots
        .iter()
        .map(|r| Row {
            root: r.root.to_string(),
            arity: r.arity,
            scalar: r.scalar,
        })
        .collect();
    assert_eq!(got, want);
    assert!(got.iter().any(|r| r.arity > 0));
    assert!(got.iter().any(|r| r.scalar));
}
