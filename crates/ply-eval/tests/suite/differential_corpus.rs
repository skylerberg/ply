//! Both engines over the corpora that exist on disk.
//!
//! `examples/` is the one real program and carries every liveness assertion; the fixtures are
//! mostly deliberately broken programs, swept in round-robin buckets so that no single test walks
//! the whole tree.

use ply_eval::differential::compare_tests;
use ply_eval::{BackendSpec, Fixture, Fragment, Machine, Mutation, Offers};
use ply_span::{SourceMap, Symbol};
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

/// The `std.*` modules one source names.
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

/// A fixture is often a deliberately broken program, so anything that does not parse or resolve is
/// not this test's business and is counted as skipped rather than failed.
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
    // Demand-driven, exactly as `ply`'s own loader is: a corpus that imports nothing from `std`
    // gets nothing, so a one-file fixture stays the program it is rather than acquiring the
    // stdlib's definitions and tests.
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
    // A `derive` declares no name and every walker skips it, so a harness that forgets to expand
    // runs a program whose generated definitions silently do not exist.
    if !ply_derive::expand_program(&mut program).is_empty() {
        return None;
    }
    let resolved = resolve(&mut program).ok()?;
    Some((program, resolved))
}

const EXAMPLES: &str = "examples";

/// One program on disk.
struct Corpus {
    label: String,
    dir: PathBuf,
    files: Vec<PathBuf>,
}

/// Every directory that is one program, plus every stray top-level fixture as a program of its own
/// — which is what they are, since a fixture in `tests/fixtures` names nothing in its neighbour.
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
    &'static ply_core::CheckOutput,
);

/// A corpus and its program, parsed once per process and leaked, so that every backend over one
/// corpus costs one AST.
struct Entry {
    corpus: Corpus,
    loaded: OnceLock<Option<Loaded>>,
}

impl Entry {
    fn label(&self) -> &str {
        &self.corpus.label
    }

    /// `None` for a fixture that does not parse, resolve or check. The check is required rather
    /// than optional: the purity gate reads the published row, and a machine built without one has
    /// an inert hook, which would be green over a seam it never reached.
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

/// The corpora in their on-disk order.
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

/// What one test sweeps.
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

/// The corpus half of `CONTRIBUTING.md` §"Things known to be broken" item 11,
/// pinned on its fixture rather than left to the sweep above.
#[test]
fn a_definition_that_discharges_its_own_effects_is_in_the_corpus_and_is_never_entered() {
    let root = workspace_root();
    let dir = root.join("tests/fixtures");
    let file = dir.join("self_handled_effect.ply");
    assert!(
        file.exists(),
        "{} is part of the repository, and it is the only corpus in the tree that declares an \
         effect and discharges it",
        file.display()
    );

    let (program, resolved) = load(&dir, std::slice::from_ref(&file)).expect("the fixture loads");
    let check = ply_core::check_program(&program, &resolved).expect("the fixture checks");

    let empty: Vec<&str> = ["handled", "wrapper", "doubled"]
        .iter()
        .filter(|simple| {
            let name = Symbol::new(format!("self_handled_effect.{simple}"));
            let def = check
                .defs
                .get(&name)
                .unwrap_or_else(|| panic!("the fixture declares `{name}`"));
            def.footprint.is_empty() && def.performed.is_empty()
        })
        .copied()
        .collect();
    assert_eq!(
        empty,
        vec!["handled", "wrapper", "doubled"],
        "the fixture stopped publishing empty rows, so the row gate now refuses these and the \
         effects gate is unexercised again"
    );

    // The other effect gate, on the same corpus: `measured` is what performs into its caller, so
    // its row is not empty and `Gate::PublishedRow` is what refuses it.
    let measured = check
        .defs
        .get(&Symbol::new("self_handled_effect.measured"))
        .expect("the fixture declares `measured`");
    assert!(
        !measured.footprint.is_empty(),
        "`measured` stopped publishing a row, so this corpus no longer reaches the row gate"
    );

    let backend = std::rc::Rc::new(backends::Nested::over(&program));
    let mut plain = Machine::new(&program, &resolved, &check);
    let mut machine = Machine::new(&program, &resolved, &check);
    machine.set_compiled(backend);

    let report = compare_tests(&mut plain, &mut machine, &Fixture::empty());
    assert!(report.is_clean(), "{report}");
    assert_eq!(report.compared, 1, "{report}");
    assert_eq!(report.footprints_compared, 1, "{report}");
    assert!(
        machine.trace().performs() > 0,
        "the fixture performed nothing, so agreeing on its footprint proves nothing"
    );

    // The control is the point: `doubled` *is* entered, so the two refusals above are this gate
    // rather than a backend that answers nothing here.
    let (entered, _) = machine.compiled_counts();
    assert!(
        entered > 0,
        "no call in this fixture was entered at all, so nothing distinguishes the effects gate \
         from a backend that declines"
    );
}

/// The same corpora, with a backend attached.
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

    /// A backend whose "compiled code" is a nested machine, over its own copy of the program with
    /// its own world.
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

// --- The eight wrong backends, at corpus scale ------------------------------

/// What one corruption did over one selection.
struct Sweep {
    compared: usize,
    /// Tests where the machine with the backend and the machine without it answered differently.
    diverged: usize,
    entered: u64,
    offers: Offers,
    /// The first disagreement, whole, because a count nobody can read is a count nobody checks.
    first: Option<String>,
}

/// Every corpus in the selection, twice: once on a machine with no backend and once on a machine
/// with `spec` attached, compared against each other.
fn sweep(spec: BackendSpec, selection: Selection) -> Sweep {
    let mut out = Sweep {
        compared: 0,
        diverged: 0,
        entered: 0,
        offers: Offers::default(),
        first: None,
    };
    for entry in selection.entries() {
        let Some((program, resolved, check)) = entry.loaded() else {
            continue;
        };
        let fragment = Fragment::over_static(program, resolved, check);
        let mut plain = Machine::new(program, resolved, check);
        let mut backed = Machine::new(program, resolved, check);
        backed.set_compiled(fragment.attach(&spec));

        let report = compare_tests(&mut plain, &mut backed, &Fixture::empty());
        out.compared += report.compared;
        out.diverged += report.divergences.len();
        if out.first.is_none()
            && let Some(d) = report.divergences.first()
        {
            out.first = Some(format!("{}: {d}", entry.label()));
        }
        out.entered += backed.compiled_counts().0;
        let seen = fragment.offers();
        out.offers.offered += seen.offered;
        out.offers.offered_target += seen.offered_target;
        out.offers.fired += seen.fired;
        out.offers.bytes_in += seen.bytes_in;
        out.offers.bytes_out += seen.bytes_out;
        out.offers.containers_out += seen.containers_out;
    }
    out
}

/// A divergence is only ever reported where the corruption changed an answer. That the corruption
/// is *caught* is `examples`'s to prove: a fixture's tests can fail for their own reasons before a
/// changed answer is observable, so a fired corruption with no divergence is not a finding there.
#[track_caller]
fn fires_and_is_caught(name: &str, mutation: Mutation, selection: Selection) {
    let spec = BackendSpec {
        mutation,
        target: None,
        ..BackendSpec::honest()
    };
    let sweep = sweep(spec, selection);
    if sweep.diverged > 0 {
        assert!(
            sweep.offers.fired > 0,
            "{name} over {selection}: {} divergences were reported and the corruption changed no \
             answer, so the wrapper itself changed one: {}",
            sweep.diverged,
            sweep.first.as_deref().unwrap_or("-")
        );
    }
    if selection.is_examples() {
        assert!(
            sweep.compared > 0,
            "{name}: no corpus ran, so this proves nothing"
        );
        assert!(
            sweep.offers.fired > 0,
            "{name}: the corruption never changed an answer over {} tests, so this run says \
             nothing about the corpus that did not catch it ({} offers, {} entered)",
            sweep.compared,
            sweep.offers.offered,
            sweep.entered
        );
        assert!(
            sweep.diverged > 0,
            "{name}: {} answers were changed over {} tests and nothing reported one",
            sweep.offers.fired,
            sweep.compared
        );
    }
    println!(
        "{name} over {selection}: {} of {} tests reported it · {} answers changed · {} offers · \
         {} entered\n  first: {}",
        sweep.diverged,
        sweep.compared,
        sweep.offers.fired,
        sweep.offers.offered,
        sweep.entered,
        sweep.first.as_deref().unwrap_or("-")
    );
}

/// The control every other test here is read against: the wrapper is the backend, and the backend
/// is honest.
fn honest(selection: Selection) {
    let sweep = sweep(BackendSpec::honest(), selection);
    assert_eq!(
        sweep.offers.fired, 0,
        "the honest backend changed an answer over {selection}"
    );
    assert_eq!(
        sweep.diverged, 0,
        "the honest backend disagreed with the machine over {selection}: {:?}",
        sweep.first
    );
    if selection.is_examples() {
        assert!(sweep.compared > 0);
        assert!(
            sweep.entered > 0,
            "{} calls were offered over {} tests and none was entered, so every mutation below \
             would be corrupting a seam nobody reaches",
            sweep.offers.offered,
            sweep.compared
        );
        // The `Bytes` and container widenings of the seam, asserted rather than assumed: a value
        // kind nothing on disk crosses leaves every green in this file saying nothing about it.
        assert!(
            sweep.offers.bytes_in > 0,
            "no call carrying a `Bytes` argument was offered over {} tests, so the `Bytes` \
             widening of `compiled::crossable` is inert on this corpus",
            sweep.compared
        );
        assert!(
            sweep.offers.bytes_out > 0,
            "{} calls carried a `Bytes` in and not one answered a `Bytes`, so the widening bought \
             arguments and no returns",
            sweep.offers.bytes_in
        );
        assert!(
            sweep.offers.containers_out > 0,
            "not one entered call answered a `List`, `Map`, `Record` or `Ctor` over {} tests, so \
             the container widening of `Machine::compiled_answer` is inert on this corpus",
            sweep.compared
        );
    }
    println!(
        "honest backend over {selection}: {} entered of {} offered, over {} tests · {} offers \
         carried a `Bytes` argument · {} answered a `Bytes` · {} answered a container",
        sweep.entered,
        sweep.offers.offered,
        sweep.compared,
        sweep.offers.bytes_in,
        sweep.offers.bytes_out,
        sweep.offers.containers_out
    );
}

/// The ninth wrong backend, and the one the container widening made possible: a container answer
/// of exactly the right kind with a [`ply_eval::Value::Cell`] in its first position.
fn handle(selection: Selection) {
    fires_and_is_caught("handle", Mutation::Handle, selection);
}

fn off_by_one(selection: Selection) {
    fires_and_is_caught("off-by-one", Mutation::OffByOne, selection);
}

fn inverted(selection: Selection) {
    fires_and_is_caught("inverted", Mutation::Inverted, selection);
}

/// The one corruption that is invisible to a single call: every answer it gives was a correct
/// answer to *some* call, so only a corpus that varies its arguments can see it.
fn stale(selection: Selection) {
    fires_and_is_caught("stale", Mutation::Stale, selection);
}

fn wrong_type(selection: Selection) {
    fires_and_is_caught("wrong-type", Mutation::WrongType, selection);
}

/// A backend answering for a definition it has no body for.
fn unoffered(selection: Selection) {
    fires_and_is_caught("unoffered", Mutation::Unoffered, selection);
}

/// Accepting a call the machine must never offer.
fn performs_never_offered(selection: Selection) {
    let sweep = sweep(
        BackendSpec {
            mutation: Mutation::Answers(7),
            target: Some(Symbol::new("self_handled_effect.handled")),
            ..BackendSpec::honest()
        },
        selection,
    );
    assert_eq!(
        sweep.offers.offered_target, 0,
        "a definition that discharges its own effects was offered to a backend over {selection}"
    );
    assert_eq!(sweep.offers.fired, 0, "{selection}");
    assert_eq!(sweep.diverged, 0, "{selection}: {:?}", sweep.first);
    if selection.is_examples() {
        assert!(
            sweep.offers.offered > 0 && sweep.entered > 0,
            "the seam was never reached at all, so the zero above proves nothing"
        );
    }
}

/// Running past the machine's call budget instead of declining.
fn budget(selection: Selection) {
    let sweep = sweep(
        BackendSpec {
            mutation: Mutation::ExceedsBudget(Some(4)),
            target: None,
            ..BackendSpec::honest()
        },
        selection,
    );
    assert_eq!(
        sweep.offers.fired, 0,
        "a corpus in {selection} now recurses past the machine's call bound, which is good news: \
         the budget corruption fires here after all and this test's note is what needs updating"
    );
    if selection.is_examples() {
        assert!(
            sweep.offers.offered > 0 && sweep.entered > 0,
            "the seam was never reached at all"
        );
    }
}

/// One test per family per selection: `<family>::over_examples` carries the family's liveness
/// assertions, `<family>::over_fixture_bucket_<k>` the safety ones over one slice of the fixtures.
macro_rules! over_every_corpus {
    ($($family:ident),* $(,)?) => {$(
        mod $family {
            use super::Selection;

            #[test]
            fn over_examples() {
                super::$family(Selection::Examples);
            }
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

over_every_corpus!(
    declining,
    answering,
    honest,
    handle,
    off_by_one,
    inverted,
    stale,
    wrong_type,
    unoffered,
    performs_never_offered,
    budget,
);
