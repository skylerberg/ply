use ply_eval::differential::compare_tests;
use ply_eval::{Fixture, Machine};
use ply_span::SourceMap;
use ply_syntax::ast::{ModuleName, Program};
use ply_syntax::parse_program;
use ply_syntax::resolve::{Resolved, resolve};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("the crate sits two levels under the workspace root")
        .to_path_buf()
}

fn ply_files(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<PathBuf> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "ply"))
        .collect();
    out.sort();
    out
}

fn subdirectories(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<PathBuf> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    out.sort();
    out
}

fn std_imports(id: ply_span::SourceId, name: &ModuleName, text: &str) -> Vec<ModuleName> {
    let Ok(module) = ply_syntax::parse_module(id, name.clone(), text) else {
        return Vec::new();
    };
    module
        .imports
        .iter()
        .map(|i| i.module_name())
        .filter(ply_std::is_std)
        .collect()
}

/// `None` for a fixture that does not parse or resolve: many are deliberately broken.
fn load(root: &Path, files: &[PathBuf]) -> Option<(Program, Resolved)> {
    let mut map = SourceMap::new();
    let mut loaded = Vec::new();
    for path in files {
        let text = std::fs::read_to_string(path).ok()?;
        let relative = path.strip_prefix(root).unwrap_or(path);
        let name = ModuleName::from_relative_path(relative).ok()?;
        let id = map.add(path, text.clone());
        loaded.push((id, name, text));
    }
    // Demand-driven like `ply`'s loader, so a fixture importing nothing from `std` gets none of it.
    let mut next = 0;
    while next < loaded.len() {
        let (id, name, text) = &loaded[next];
        next += 1;
        let wanted = std_imports(*id, name, text);
        for module in wanted {
            if loaded.iter().any(|(_, n, _)| *n == module) {
                continue;
            }
            let Some(source) = ply_std::source(&module) else {
                continue;
            };
            let id = map.add(ply_std::pseudo_path(&module), source.to_string());
            loaded.push((id, module, source.to_string()));
        }
    }
    let inputs: Vec<_> = loaded
        .iter()
        .map(|(id, name, text)| (*id, name.clone(), text.as_str()))
        .collect();
    let mut program = parse_program(inputs).ok()?;
    // Unexpanded, a `derive`'s generated definitions would silently not exist.
    if !ply_derive::expand_program(&mut program).is_empty() {
        return None;
    }
    let resolved = resolve(&mut program).ok()?;
    Some((program, resolved))
}

const EXAMPLES: &str = "examples";

struct Corpus {
    label: String,
    dir: PathBuf,
    files: Vec<PathBuf>,
}

/// Every one-program directory, plus each stray top-level fixture as a program of its own.
fn corpora(root: &Path) -> Vec<Corpus> {
    let mut out = Vec::new();

    let examples = root.join(EXAMPLES);
    let files = ply_files(&examples);
    if !files.is_empty() {
        out.push(Corpus {
            label: EXAMPLES.to_string(),
            dir: examples,
            files,
        });
    }

    let fixtures = root.join("tests/fixtures");
    for dir in subdirectories(&fixtures) {
        let files = ply_files(&dir);
        if !files.is_empty() {
            let name = dir.file_name().unwrap().to_string_lossy().to_string();
            out.push(Corpus {
                label: format!("fixtures/{name}"),
                dir,
                files,
            });
        }
    }
    for file in ply_files(&fixtures) {
        let name = file.file_name().unwrap().to_string_lossy().to_string();
        out.push(Corpus {
            label: format!("fixtures/{name}"),
            dir: fixtures.clone(),
            files: vec![file],
        });
    }
    out
}

type Loaded = (
    &'static Program,
    &'static Resolved,
    &'static ply_ty::CheckOutput,
);

/// Parsed once per process and leaked, so every backend over a corpus shares one AST.
struct Entry {
    corpus: Corpus,
    loaded: OnceLock<Option<Loaded>>,
}

impl Entry {
    fn label(&self) -> &str {
        &self.corpus.label
    }

    /// Requires the check: without the published row the purity hook is inert and passes vacuously.
    fn loaded(&self) -> Option<Loaded> {
        *self.loaded.get_or_init(|| {
            let (program, resolved) = load(&self.corpus.dir, &self.corpus.files)?;
            let program: &'static Program = Box::leak(Box::new(program));
            let resolved: &'static Resolved = Box::leak(Box::new(resolved));
            let check = ply_core::check_program(program, resolved).ok()?;
            Some((program, resolved, Box::leak(Box::new(check))))
        })
    }
}

fn index() -> &'static [Entry] {
    static INDEX: OnceLock<Vec<Entry>> = OnceLock::new();
    INDEX.get_or_init(|| {
        corpora(&workspace_root())
            .into_iter()
            .map(|corpus| Entry {
                corpus,
                loaded: OnceLock::new(),
            })
            .collect()
    })
}

/// Round-robin over the fixtures in their on-disk order, so a run of same-cost siblings spreads.
const BUCKETS: usize = 8;

#[derive(Clone, Copy)]
enum Selection {
    /// The one real program, and the only selection whose counts prove anything.
    Examples,
    Fixtures(usize),
}

impl Selection {
    fn entries(self) -> Vec<&'static Entry> {
        match self {
            Selection::Examples => index()
                .iter()
                .filter(|entry| entry.label() == EXAMPLES)
                .collect(),
            Selection::Fixtures(bucket) => index()
                .iter()
                .filter(|entry| entry.label() != EXAMPLES)
                .enumerate()
                .filter(|(position, _)| position % BUCKETS == bucket)
                .map(|(_, entry)| entry)
                .collect(),
        }
    }

    fn is_examples(self) -> bool {
        matches!(self, Selection::Examples)
    }
}

impl std::fmt::Display for Selection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Selection::Examples => f.write_str(EXAMPLES),
            Selection::Fixtures(bucket) => write!(f, "fixture bucket {bucket}"),
        }
    }
}

#[test]
fn every_fixture_is_in_exactly_one_bucket_and_examples_is_its_own() {
    let index = index();
    assert_eq!(
        Selection::Examples.entries().len(),
        1,
        "examples/ is not a corpus"
    );
    let mut expected: Vec<&str> = index
        .iter()
        .map(Entry::label)
        .filter(|label| *label != EXAMPLES)
        .collect();
    assert!(
        expected.len() > BUCKETS,
        "{} fixtures over {BUCKETS} buckets leaves buckets that sweep nothing",
        expected.len()
    );
    let mut seen: Vec<&str> = (0..BUCKETS)
        .flat_map(|bucket| Selection::Fixtures(bucket).entries())
        .map(Entry::label)
        .collect();
    expected.sort_unstable();
    seen.sort_unstable();
    assert_eq!(
        seen, expected,
        "the buckets are not a partition of the fixtures"
    );
    assert_eq!(BUCKETS, 8, "`over_every_corpus!` names one test per bucket");
}

mod backends {
    use ply_eval::{Compiled, Machine, Value};
    use ply_span::{Span, Symbol};
    use ply_syntax::ast::Program;
    use ply_syntax::resolve::{Resolved, resolve};
    use std::cell::{Cell, RefCell};

    /// Declines every call and counts what it was offered.
    pub struct Declining {
        program: *const Program,
        offered: Cell<u64>,
    }

    impl Declining {
        pub fn over(program: &Program) -> Declining {
            Declining {
                program: std::ptr::from_ref(program),
                offered: Cell::new(0),
            }
        }

        pub fn offered(&self) -> u64 {
            self.offered.get()
        }
    }

    impl Compiled for Declining {
        fn describes(&self, program: &Program) -> bool {
            std::ptr::eq(self.program, std::ptr::from_ref(program))
        }

        fn enter(&self, _: &Symbol, _: &[Value], _: usize) -> Option<Value> {
            self.offered.set(self.offered.get() + 1);
            None
        }
    }

    /// A backend whose "compiled code" is a nested machine with its own program copy and world.
    pub struct Nested {
        program: *const Program,
        inner: RefCell<Machine<'static>>,
    }

    impl Nested {
        pub fn over(program: &Program) -> Nested {
            let copy: &'static mut Program = Box::leak(Box::new(program.clone()));
            let resolved: &'static Resolved = Box::leak(Box::new(
                resolve(copy).expect("the corpus resolved once already"),
            ));
            let copy: &'static Program = copy;
            Nested {
                program: std::ptr::from_ref(program),
                inner: RefCell::new(Machine::for_program(copy, resolved)),
            }
        }
    }

    impl Compiled for Nested {
        fn describes(&self, program: &Program) -> bool {
            std::ptr::eq(self.program, std::ptr::from_ref(program))
        }

        fn enter(&self, name: &Symbol, args: &[Value], _: usize) -> Option<Value> {
            let mut inner = self.inner.try_borrow_mut().ok()?;
            match inner.call(name.as_str(), args.to_vec(), Span::DUMMY) {
                Ok(v @ (Value::Int(_) | Value::Bool(_))) => Some(v),
                _ => None,
            }
        }
    }
}

fn declining(selection: Selection) {
    let mut offered = 0;
    let mut compared = 0;

    for entry in selection.entries() {
        let Some((program, resolved, check)) = entry.loaded() else {
            continue;
        };
        let label = entry.label();
        let backend = std::rc::Rc::new(backends::Declining::over(program));
        let mut plain = Machine::new(program, resolved, check);
        let mut machine = Machine::new(program, resolved, check);
        machine.set_compiled(backend.clone());

        let report = compare_tests(&mut plain, &mut machine, &Fixture::empty());
        assert!(report.is_clean(), "{label}\n{report}");
        assert_eq!(
            report.footprints_compared, report.compared,
            "{label}\n{report}"
        );
        assert_eq!(
            machine.compiled_counts().0,
            0,
            "{label}: a declining backend was entered"
        );
        assert_eq!(machine.compiled_refusals(), 0, "{label}");
        offered += backend.offered();
        compared += report.compared;
    }

    if selection.is_examples() {
        assert!(compared > 0, "no corpus ran");
        assert!(
            offered > 0,
            "{compared} tests ran and the seam was never reached, so this proves nothing"
        );
    }
    println!("declining backend over {selection}: {offered} calls offered over {compared} tests");
}

fn answering(selection: Selection) {
    let mut entered = 0;
    let mut declined = 0;
    let mut compared = 0;

    for entry in selection.entries() {
        let Some((program, resolved, check)) = entry.loaded() else {
            continue;
        };
        let label = entry.label();
        let mut plain = Machine::new(program, resolved, check);
        let mut machine = Machine::new(program, resolved, check);
        machine.set_compiled(std::rc::Rc::new(backends::Nested::over(program)));

        let report = compare_tests(&mut plain, &mut machine, &Fixture::empty());
        assert!(report.is_clean(), "{label}\n{report}");
        assert_eq!(
            report.footprints_compared, report.compared,
            "{label}\n{report}"
        );
        assert_eq!(
            machine.compiled_refusals(),
            0,
            "{label}: the boundary refused an answer it should never have been offered"
        );
        let (e, d) = machine.compiled_counts();
        entered += e;
        declined += d;
        compared += report.compared;
    }

    if selection.is_examples() {
        assert!(compared > 0, "no corpus ran");
        assert!(
            entered > 0,
            "{compared} tests ran and no call was ever entered, so the accept path is unexercised"
        );
    }
    println!(
        "answering backend over {selection}: {entered} entered, {declined} declined, over \
         {compared} tests"
    );
}

macro_rules! over_every_corpus {
    ($($family:ident),* $(,)?) => {$(
        mod $family {
            use super::Selection;

            #[test]
            fn over_fixture_bucket_0() {
                super::$family(Selection::Fixtures(0));
            }
            #[test]
            fn over_fixture_bucket_1() {
                super::$family(Selection::Fixtures(1));
            }
            #[test]
            fn over_fixture_bucket_2() {
                super::$family(Selection::Fixtures(2));
            }
            #[test]
            fn over_fixture_bucket_3() {
                super::$family(Selection::Fixtures(3));
            }
            #[test]
            fn over_fixture_bucket_4() {
                super::$family(Selection::Fixtures(4));
            }
            #[test]
            fn over_fixture_bucket_5() {
                super::$family(Selection::Fixtures(5));
            }
            #[test]
            fn over_fixture_bucket_6() {
                super::$family(Selection::Fixtures(6));
            }
            #[test]
            fn over_fixture_bucket_7() {
                super::$family(Selection::Fixtures(7));
            }
        }
    )*};
}

over_every_corpus!(declining, answering);
