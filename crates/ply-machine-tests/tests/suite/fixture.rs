//! The projects a test loads, and the repository paths it reads them from.

use std::path::{Path, PathBuf};
use tempfile::TempDir;

/// A scratch directory.
pub fn scratch() -> TempDir {
    TempDir::new().expect("a scratch directory")
}

/// A temporary project whose `m.ply` is `source`.
pub fn project(source: &str) -> TempDir {
    let dir = scratch();
    write(dir.path(), "m.ply", source);
    dir
}

/// Writes `text` to `dir/name`, making the directories above it.
pub fn write(dir: &Path, name: &str, text: &str) {
    let path = dir.join(name);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("the file's directory is made");
    }
    std::fs::write(path, text).expect("the fixture is written");
}

/// The front end the CLI would hand a machine: the project walked, the compiler run once over it,
/// and both marshalled into the record the effects take.
///
/// A test that drives an effect directly has no CLI to do this for it, and every effect that reads a
/// program now takes one. The fixture's module mirrors the package's, so what the machine names has
/// to be what this declares -- `replay`'s own check says so for the payload.
pub fn handed(root: &Path) -> ply_eval::Value {
    use ply_codegen::c::producer::{self, Packages};
    producer::ensure_default();
    let mut paths = Vec::new();
    collect(root, &mut paths);
    paths.sort();
    let mut files: Vec<(String, String, String)> = Vec::new();
    for path in paths {
        let relative = path.strip_prefix(root).unwrap_or(&path).to_path_buf();
        let module = ply_ty::ModuleName::from_relative_path(&relative)
            .expect("a fixture's file is a module");
        let text = std::fs::read_to_string(&path).expect("the fixture is read");
        files.push((path.display().to_string(), module.to_string(), text));
    }
    let own: Vec<(String, String)> = files
        .iter()
        .map(|(_, name, text)| (name.clone(), text.clone()))
        .collect();
    let packages = Packages {
        root: root.display().to_string(),
        manifest: None,
        supplied: Vec::new(),
    };
    let pulled = producer::front_pulling_std_with(&own, ply_machine::shelf::sources(), &packages)
        .expect("the front end runs");
    for name in &pulled.modules {
        let module = ply_ty::ModuleName::from_dotted(name);
        if let Some(text) = ply_machine::shelf::source(&module) {
            files.push((
                ply_machine::shelf::pseudo_path(&module)
                    .display()
                    .to_string(),
                name.clone(),
                text.to_string(),
            ));
        }
    }
    let file = |(path, name, text): (String, String, String)| {
        ply_machine::payload::record(vec![
            ("path", ply_eval::Value::str(&path)),
            ("name", ply_eval::Value::str(&name)),
            ("text", ply_eval::Value::bytes(text.as_bytes())),
        ])
    };
    ply_machine::payload::record(vec![
        ("dump", pulled.dump),
        (
            "files",
            ply_eval::Value::list(files.into_iter().map(file).collect()),
        ),
        ("read_ms", ply_eval::Value::Int(0)),
        ("front_ms", ply_eval::Value::Int(0)),
        ("file_ms", ply_eval::Value::Int(0)),
        ("cached", ply_eval::Value::Bool(false)),
    ])
}

/// Every `.ply` file under `root`.
fn collect(root: &Path, out: &mut Vec<PathBuf>) {
    let entries = match std::fs::read_dir(root) {
        Ok(entries) => entries,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect(&path, out);
        } else if path.extension().is_some_and(|e| e == "ply") {
            out.push(path);
        }
    }
}

/// The repository root, canonical so it is comparable with a path the loader resolved.
pub fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("the crate lives two levels below the repository root")
}

/// What a program measures for one obligation, decided by `proof.domain` itself: the package is
/// compiled once and its `finite` and `name_of` are entered with the obligation's binders and the
/// world's declared types, as the CLI measures them. Past the bound, or a product of no points,
/// there is no domain to walk and the obligation is sampled.
pub fn measured(
    prover: &ply_machine::engine::Prover<'_>,
    obligation: &ply_prove::Obligation,
) -> Option<ply_test::obligation::Domain> {
    use ply_eval::Value;
    use ply_machine::payload::{field_of, option_of, record};
    let package = measuring();
    let module = package.module.as_str();
    let binders = Value::list(
        obligation
            .generated()
            .iter()
            .map(|b| sort_value(module, &b.sort))
            .collect(),
    );
    let decls = Value::list(
        prover
            .world()
            .decls()
            .map(|decl| {
                record(vec![
                    ("name", Value::str(decl.name.as_str())),
                    ("params", Value::Int(decl.params as i64)),
                    (
                        "variants",
                        Value::list(
                            decl.variants
                                .iter()
                                .map(|variant| {
                                    record(vec![
                                        ("name", Value::str(variant.name.as_str())),
                                        (
                                            "fields",
                                            Value::list(
                                                variant
                                                    .fields
                                                    .iter()
                                                    .map(|t| sort_value(module, t))
                                                    .collect(),
                                            ),
                                        ),
                                    ])
                                })
                                .collect(),
                        ),
                    ),
                ])
            })
            .collect(),
    );
    let answer = package.enter("finite", vec![binders, decls]);
    let domain = option_of(&answer, "a domain", ply_span::Span::DUMMY)
        .expect("`finite` answers an option")?;
    let shapes = field_of(domain, "shapes", ply_span::Span::DUMMY)
        .expect("a domain has shapes")
        .as_list(ply_span::Span::DUMMY, "the shapes")
        .expect("a list")
        .iter()
        .map(|shape| ply_machine::claims::shape_of(shape, ply_span::Span::DUMMY).expect("a shape"))
        .collect();
    let texts = Value::list(
        obligation
            .generated()
            .iter()
            .map(|b| Value::str(&b.text))
            .collect(),
    );
    let name = package
        .enter("name_of", vec![texts])
        .as_str(ply_span::Span::DUMMY, "a domain's name")
        .expect("`name_of` answers text")
        .to_string();
    Some(ply_test::obligation::Domain { shapes, name })
}

/// The most points `proof.domain` lets a proof walk.
pub fn bound() -> u64 {
    let bound = measuring()
        .enter("bound", Vec::new())
        .as_int(ply_span::Span::DUMMY, "the bound")
        .expect("`bound` answers a number");
    u64::try_from(bound).expect("a bound is a count")
}

/// `proof.domain`, compiled once for every test that measures a domain.
struct Measuring {
    unit: &'static ply_codegen::Unit,
    /// The name `domain.ply` loads under, which every value handed to it is named by.
    module: String,
}

impl Measuring {
    fn enter(&self, name: &str, args: Vec<ply_eval::Value>) -> ply_eval::Value {
        thread_local! {
            static BODIES: std::cell::OnceCell<std::rc::Rc<dyn ply_eval::Compiled>> =
                const { std::cell::OnceCell::new() };
        }
        let qualified = ply_span::Symbol::new(format!("{}.{name}", self.module));
        BODIES.with(|bodies| {
            let compiled = bodies.get_or_init(|| ply_eval::Provider::attach(self.unit));
            match compiled.enter_whole(&qualified, &args, ply_eval::DEFAULT_MAX_CALLS) {
                ply_eval::Entered::Answered(value) => value,
                ply_eval::Entered::Raised(d) => panic!("`{qualified}` raised: {}", d.message),
                ply_eval::Entered::Declined => panic!("the tier declined `{qualified}`"),
            }
        })
    }
}

fn measuring() -> &'static Measuring {
    static PACKAGE: std::sync::OnceLock<Measuring> = std::sync::OnceLock::new();
    PACKAGE.get_or_init(|| {
        // `domain.ply` imports nothing, so it loads alone rather than beside the compiler that
        // `proof.world` reads.
        let dir = scratch();
        std::fs::copy(
            repo().join("crates/ply-prove/ply/domain.ply"),
            dir.path().join("domain.ply"),
        )
        .expect("the prove package keeps `domain.ply`");
        let loaded = ply_machine::load::load(dir.path())
            .unwrap_or_else(|e| panic!("`proof.domain` loads: {:?}", e.diagnostics));
        let module = loaded
            .check
            .defs
            .values()
            .find(|d| d.simple_name.as_str() == "finite" && d.module.as_str().ends_with("domain"))
            .map(|d| d.module.to_string())
            .expect("`proof.domain` measures domains");
        let texts = ply_machine::support::module_texts(&loaded.check, &loaded.sources);
        let unit = ply_codegen::Unit::over_front(&loaded.front, texts)
            .expect("this host has a C compiler");
        Measuring { unit, module }
    })
}

/// The world and the obligations of a loaded program, read off its checked front the way
/// `proof.world` reads the compiler's answer, in the order it builds them: every definition's
/// `ensures` clauses, then every law. An audit here drives the prover with no program to build them.
pub fn world_of(
    loaded: &ply_machine::load::Loaded,
) -> (ply_prove::World, Vec<ply_prove::Obligation>) {
    use ply_prove::world::{Decl, Signature, Variant};
    use ply_prove::{Obligation, ObligationKind, World};
    use ply_span::Symbol;
    use ply_ty::{SpecKind, Type};

    let check = &loaded.check;
    let mut decls: Vec<Decl> = Vec::new();
    for ctor in check.ctors.values() {
        let answer = match &ctor.scheme.ty {
            Type::Fn { ret, .. } => ret.as_ref(),
            other => other,
        };
        let params: Vec<ply_ty::TyVar> = match answer {
            Type::Con(_, args) => args
                .iter()
                .map(|a| match a {
                    Type::Var(v) => *v,
                    _ => ply_ty::TyVar(u32::MAX),
                })
                .collect(),
            _ => Vec::new(),
        };
        let variant = Variant {
            name: ctor.name.clone(),
            index: ctor.index,
            fields: ctor.fields.iter().map(|f| sort_of(f, &params)).collect(),
            depth: None,
        };
        match decls.iter_mut().find(|d| d.name == ctor.type_name) {
            Some(decl) => decl.variants.push(variant),
            None => decls.push(Decl {
                name: ctor.type_name.clone(),
                params: params.len(),
                variants: vec![variant],
                depth: None,
            }),
        }
    }
    for decl in &mut decls {
        decl.variants.sort_by_key(|v| v.index);
    }
    let signatures: Vec<Signature> = check
        .defs
        .values()
        .map(|def| Signature {
            name: def.name.clone(),
            sort: sort_of(&def.scheme.ty, &met(&[&def.scheme.ty])),
            pure: def.footprint.is_empty(),
        })
        .collect();

    let mut obligations = Vec::new();
    for (name, info) in &check.defs {
        if !info.spec.iter().any(|s| s.kind == SpecKind::Ensures) {
            continue;
        }
        let (params, ret) = match &info.scheme.ty {
            Type::Fn { params, ret, .. } => (params.as_slice(), ret.as_ref()),
            other => (&[][..], other),
        };
        let written = loaded
            .front
            .defs_written
            .get(name)
            .map(|w| w.params.iter().map(|p| p.name.clone()).collect::<Vec<_>>())
            .unwrap_or_default();
        let mut named: Vec<(Symbol, &Type)> = written.into_iter().zip(params).collect();
        named.push((Symbol::new("result"), ret));
        let binders = binders_of(&named);
        let guarded = info.spec.iter().any(|s| s.kind == SpecKind::Requires);
        let keys = loaded.hashes.specs.get(name);
        for (ordinal, clause) in info
            .spec
            .iter()
            .filter(|c| c.kind == SpecKind::Ensures)
            .enumerate()
        {
            let key = *keys
                .and_then(|keys| keys.get(clause.index))
                .unwrap_or_else(|| panic!("`{name}`'s clause {} was not hashed", clause.index));
            obligations.push(Obligation {
                key,
                owner: name.clone(),
                kind: ObligationKind::Ensures { index: ordinal },
                span: clause.span,
                binders: binders.clone(),
                guarded,
                host: false,
                footprint: row(&clause.footprint),
            });
        }
    }
    for law in &check.laws {
        let key = *loaded
            .hashes
            .laws
            .get(law.index)
            .unwrap_or_else(|| panic!("the law `{}` was not hashed", law.key));
        let named: Vec<(Symbol, &Type)> = law
            .binders
            .iter()
            .map(|b| (b.name.clone(), &b.ty))
            .collect();
        obligations.push(Obligation {
            key,
            owner: law.key.clone(),
            kind: ObligationKind::Law,
            span: law.span,
            binders: binders_of(&named),
            guarded: law.has_guard,
            host: law.host,
            footprint: row(&law.footprint),
        });
    }
    (World::new(decls, signatures), obligations)
}

fn row(footprint: &ply_ty::Footprint) -> Option<String> {
    (!footprint.is_empty()).then(|| footprint.to_string())
}

/// The obligations as the program hands them over with `configure`, in a world of no declarations
/// or signatures: what a test driving the claims effect directly configures a run with.
pub fn world_value(obligations: &[ply_prove::Obligation]) -> ply_eval::Value {
    use ply_eval::Value;
    use ply_machine::payload::{ctor, option, record};
    use ply_prove::ObligationKind;
    let obligation = |o: &ply_prove::Obligation| {
        record(vec![
            ("key", Value::str(o.key.to_hex())),
            ("owner", Value::str(o.owner.as_str())),
            (
                "kind",
                match o.kind {
                    ObligationKind::Ensures { index } => ctor(
                        "proof.obligation",
                        "Ensures",
                        vec![Value::Int(index as i64)],
                    ),
                    ObligationKind::Law => ctor("proof.obligation", "Law", vec![option(None)]),
                },
            ),
            (
                "at",
                record(vec![
                    ("module", Value::Int(i64::from(o.span.source.0))),
                    ("start", Value::Int(i64::from(o.span.start))),
                    ("end", Value::Int(i64::from(o.span.end))),
                ]),
            ),
            (
                "binders",
                Value::list(
                    o.binders
                        .iter()
                        .map(|b| {
                            record(vec![
                                ("name", Value::str(b.name.as_str())),
                                ("ty", sort_value("proof.domain", &b.sort)),
                                ("text", Value::str(&b.text)),
                            ])
                        })
                        .collect(),
                ),
            ),
            ("guarded", Value::Bool(o.guarded)),
            ("host", Value::Bool(o.host)),
            ("footprint", option(o.footprint.as_deref().map(Value::str))),
            ("frame", ctor("proof.obligation", "Pure", Vec::new())),
        ])
    };
    record(vec![
        ("decls", Value::list(Vec::new())),
        ("signatures", Value::list(Vec::new())),
        (
            "obligations",
            Value::list(obligations.iter().map(obligation).collect()),
        ),
    ])
}

/// A `proof.domain.Ty`, named by the module `proof.domain` loaded under.
fn sort_value(module: &str, sort: &ply_prove::Sort) -> ply_eval::Value {
    use ply_eval::Value;
    use ply_machine::payload::{ctor, record};
    use ply_prove::Sort;
    let each = |sorts: &[Sort]| Value::list(sorts.iter().map(|s| sort_value(module, s)).collect());
    match sort {
        Sort::Var(v) => ctor(module, "Var", vec![Value::Int(i64::from(*v))]),
        Sort::Con(name, args) => ctor(module, "Con", vec![Value::str(name.as_str()), each(args)]),
        Sort::Fn { params, ret, pure } => ctor(
            module,
            "Fn",
            vec![each(params), sort_value(module, ret), Value::Bool(*pure)],
        ),
        Sort::Record(fields) => ctor(
            module,
            "Record",
            vec![Value::list(
                fields
                    .iter()
                    .map(|(name, field)| {
                        record(vec![
                            ("name", Value::str(name.as_str())),
                            ("ty", sort_value(module, field)),
                        ])
                    })
                    .collect(),
            )],
        ),
    }
}

/// Each variable of `types` once, where it first appears.
fn met(types: &[&ply_ty::Type]) -> Vec<ply_ty::TyVar> {
    fn walk(ty: &ply_ty::Type, seen: &mut Vec<ply_ty::TyVar>) {
        match ty {
            ply_ty::Type::Var(v) => {
                if !seen.contains(v) {
                    seen.push(*v);
                }
            }
            ply_ty::Type::Con(_, args) => args.iter().for_each(|a| walk(a, seen)),
            ply_ty::Type::Fn { params, ret, .. } => {
                params.iter().for_each(|p| walk(p, seen));
                walk(ret, seen);
            }
            ply_ty::Type::Record(fields) => fields.values().for_each(|f| walk(f, seen)),
        }
    }
    let mut seen = Vec::new();
    for ty in types {
        walk(ty, &mut seen);
    }
    seen
}

fn sort_of(ty: &ply_ty::Type, vars: &[ply_ty::TyVar]) -> ply_prove::Sort {
    use ply_prove::Sort;
    match ty {
        ply_ty::Type::Var(v) => {
            Sort::Var(vars.iter().position(|x| x == v).unwrap_or(vars.len()) as u32)
        }
        ply_ty::Type::Con(name, args) => Sort::Con(
            name.clone(),
            args.iter().map(|a| sort_of(a, vars)).collect(),
        ),
        ply_ty::Type::Fn {
            params,
            ret,
            effects,
        } => Sort::func(
            params.iter().map(|p| sort_of(p, vars)).collect(),
            sort_of(ret, vars),
            effects.is_pure(),
        ),
        ply_ty::Type::Record(fields) => Sort::record(
            fields
                .iter()
                .map(|(name, f)| (name.clone(), sort_of(f, vars))),
        ),
    }
}

/// One claim's binders, numbered together, each printed with its variables' letters.
fn binders_of(named: &[(ply_span::Symbol, &ply_ty::Type)]) -> Vec<ply_prove::Binder> {
    let types: Vec<&ply_ty::Type> = named.iter().map(|(_, ty)| *ty).collect();
    let vars = met(&types);
    named
        .iter()
        .map(|(name, ty)| {
            let sort = sort_of(ty, &vars);
            ply_prove::Binder {
                name: name.clone(),
                text: sort.to_string(),
                sort,
            }
        })
        .collect()
}
