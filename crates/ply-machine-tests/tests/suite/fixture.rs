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
    let pulled =
        producer::front_pulling_std_with(&own, ply_machine::shelf::sources(), &[], &[], &packages)
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
        ("dump", ply_eval::Value::bytes(pulled.dump.as_bytes())),
        (
            "files",
            ply_eval::Value::list(files.into_iter().map(file).collect()),
        ),
        ("packages", ply_eval::Value::list(Vec::new())),
        ("read_ms", ply_eval::Value::Int(0)),
        ("front_ms", ply_eval::Value::Int(0)),
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
/// prover's declared types, as `prover.typed` hands them to the CLI. Past the bound, or a product of
/// no points, there is no domain to walk and the obligation is sampled.
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
            .map(|b| ty_value(module, &b.ty))
            .collect(),
    );
    let decls = Value::list(
        prover
            .world()
            .declared()
            .map(|(name, decl)| {
                record(vec![
                    ("name", Value::str(name.as_str())),
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
                                                    .map(|t| ty_value(module, t))
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
            .map(|b| Value::str(b.ty.to_string()))
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

/// The prove package, compiled once for every test that measures a domain.
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
        let loaded = ply_machine::load::load(&repo().join("crates/ply-prove/ply"))
            .unwrap_or_else(|e| panic!("the prove package loads: {:?}", e.diagnostics));
        let module = loaded
            .check
            .defs
            .values()
            .find(|d| d.simple_name.as_str() == "finite" && d.module.as_str().ends_with("domain"))
            .map(|d| d.module.to_string())
            .expect("the prove package measures domains");
        let texts = ply_machine::support::module_texts(&loaded.check, &loaded.sources);
        let unit = ply_codegen::Unit::over_front(&loaded.front, texts)
            .expect("this host has a C compiler");
        Measuring { unit, module }
    })
}

/// A type as `proof.domain` reads one, named by the module it loaded under.
fn ty_value(module: &str, ty: &ply_ty::Type) -> ply_eval::Value {
    use ply_eval::Value;
    use ply_machine::payload::{ctor, record};
    match ty {
        ply_ty::Type::Var(var) => ctor(module, "Var", vec![Value::Int(i64::from(var.0))]),
        ply_ty::Type::Fn { .. } => ctor(module, "Fn", Vec::new()),
        ply_ty::Type::Record(fields) => ctor(
            module,
            "Record",
            vec![Value::list(
                fields
                    .iter()
                    .map(|(name, field)| {
                        record(vec![
                            ("name", Value::str(name.as_str())),
                            ("ty", ty_value(module, field)),
                        ])
                    })
                    .collect(),
            )],
        ),
        ply_ty::Type::Con(name, args) => ctor(
            module,
            "Con",
            vec![
                Value::str(name.as_str()),
                Value::list(args.iter().map(|a| ty_value(module, a)).collect()),
            ],
        ),
    }
}
