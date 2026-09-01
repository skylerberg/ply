//! Both engines over the corpora that exist on disk.

use ply_eval::differential::compare_tests;
use ply_eval::{Fixture, Interp, Machine};
use ply_span::SourceMap;
use ply_syntax::ast::{ModuleName, Program};
use ply_syntax::parse_program;
use ply_syntax::resolve::{Resolved, resolve};
use std::path::{Path, PathBuf};

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

/// Every directory that is one program, plus every stray top-level fixture as a program of its own
/// — which is what they are, since a fixture in `tests/fixtures` names nothing in its neighbour.
fn corpora(root: &Path) -> Vec<(String, PathBuf, Vec<PathBuf>)> {
    let mut out = Vec::new();

    let examples = root.join("examples");
    let files = ply_files(&examples);
    if !files.is_empty() {
        out.push(("examples".to_string(), examples, files));
    }

    let fixtures = root.join("tests/fixtures");
    for dir in subdirectories(&fixtures) {
        let files = ply_files(&dir);
        if !files.is_empty() {
            let name = dir.file_name().unwrap().to_string_lossy().to_string();
            out.push((format!("fixtures/{name}"), dir, files));
        }
    }
    for file in ply_files(&fixtures) {
        let name = file.file_name().unwrap().to_string_lossy().to_string();
        out.push((format!("fixtures/{name}"), fixtures.clone(), vec![file]));
    }
    out
}

#[test]
fn the_two_engines_agree_on_every_corpus_on_disk() {
    let root = workspace_root();
    let mut programs = 0;
    let mut skipped = 0;
    let mut tests = 0;
    let mut atoms = 0;

    for (label, dir, files) in corpora(&root) {
        let Some((program, resolved)) = load(&dir, &files) else {
            skipped += 1;
            continue;
        };
        programs += 1;

        let mut treewalk = Interp::for_program(&program, &resolved);
        let mut machine = Machine::for_program(&program, &resolved);
        let report = compare_tests(&mut treewalk, &mut machine, &Fixture::empty());
        tests += report.compared;
        atoms += machine.trace().performs();

        assert!(report.is_clean(), "{label}\n{report}");
        // A green run whose footprint axis never ran is what let two engines performing different
        // atoms pass this audit.
        assert_eq!(
            report.footprints_compared, report.compared,
            "{label}: an engine stopped reporting what it performed\n{report}"
        );
    }

    assert!(programs > 0, "no corpus loaded; {skipped} were skipped");
    assert!(
        tests > 0,
        "{programs} programs loaded but not one of them declares a test"
    );
    assert!(
        atoms > 0,
        "no corpus performed an effect, so agreeing on footprints proved nothing"
    );
}

/// A refusal is not agreement, so the harness must count it apart.
#[test]
fn a_machine_only_fixture_is_counted_apart_from_what_was_compared() {
    let root = workspace_root();
    let dir = root.join("tests/fixtures");
    let file = dir.join("machine_only_clause.ply");
    assert!(
        file.exists(),
        "{} is part of the repository",
        file.display()
    );

    let (program, resolved) = load(&dir, std::slice::from_ref(&file)).expect("the fixture loads");
    let mut treewalk = Interp::for_program(&program, &resolved);
    let mut machine = Machine::for_program(&program, &resolved);

    let report = compare_tests(&mut treewalk, &mut machine, &Fixture::empty());
    assert!(report.is_clean(), "{report}");
    assert_eq!(report.compared, 0, "{report}");
    assert_eq!(report.machine_only, 1, "{report}");

    assert!(
        Machine::for_program(&program, &resolved)
            .eval_test(0)
            .is_ok()
    );
    let refused = Interp::for_program(&program, &resolved)
        .eval_test(0)
        .expect_err("the tree-walker refuses the clause");
    assert_eq!(refused.code, ply_span::codes::MACHINE_ONLY_CLAUSE);
}

/// The corpus half of `CONTRIBUTING.md` §"Things known to be broken" item 11, pinned on its fixture
/// rather than left to the sweep above.
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
            let name = ply_span::Symbol::new(format!("self_handled_effect.{simple}"));
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
        .get(&ply_span::Symbol::new("self_handled_effect.measured"))
        .expect("the fixture declares `measured`");
    assert!(
        !measured.footprint.is_empty(),
        "`measured` stopped publishing a row, so this corpus no longer reaches the row gate"
    );

    let backend = std::rc::Rc::new(backends::TreeWalker::over(&program));
    let mut treewalk = Interp::new(&program, &resolved, &check);
    let mut machine = Machine::new(&program, &resolved, &check);
    machine.set_compiled(backend);

    let report = compare_tests(&mut treewalk, &mut machine, &Fixture::empty());
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

/// `examples/` is the corpus the milestone's exit criterion names, so it gets its own assertion
/// rather than being one entry in a loop that would still pass if it silently stopped loading.
#[test]
fn the_two_engines_agree_on_examples() {
    let root = workspace_root();
    let dir = root.join("examples");
    let files = ply_files(&dir);
    assert!(!files.is_empty(), "examples/ holds no .ply files");

    let (program, resolved) = load(&dir, &files).expect("examples/ parses and resolves");
    let mut treewalk = Interp::for_program(&program, &resolved);
    let mut machine = Machine::for_program(&program, &resolved);

    let report = compare_tests(&mut treewalk, &mut machine, &Fixture::empty());
    assert!(report.compared >= files.len(), "{report}");
    assert!(report.is_clean(), "{report}");
}

/// The same corpora, with a backend attached.
mod backends {
    use ply_eval::{Compiled, Interp, Value};
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

    /// A backend whose "compiled code" is the tree-walker, over its own copy of the program with
    /// its own world.
    pub struct TreeWalker {
        program: *const Program,
        inner: RefCell<Interp<'static>>,
    }

    impl TreeWalker {
        pub fn over(program: &Program) -> TreeWalker {
            let copy: &'static mut Program = Box::leak(Box::new(program.clone()));
            let resolved: &'static Resolved = Box::leak(Box::new(
                resolve(copy).expect("the corpus resolved once already"),
            ));
            let copy: &'static Program = copy;
            TreeWalker {
                program: std::ptr::from_ref(program),
                inner: RefCell::new(Interp::for_program(copy, resolved)),
            }
        }
    }

    impl Compiled for TreeWalker {
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

#[test]
fn a_backend_that_declines_everything_changes_nothing_over_every_corpus_on_disk() {
    let root = workspace_root();
    let mut offered = 0;
    let mut compared = 0;

    for (label, dir, files) in corpora(&root) {
        let Some((program, resolved)) = load(&dir, &files) else {
            continue;
        };
        // `Machine::for_program` carries no `CheckOutput`, and the purity gate reads the published
        // row: without one the hook is inert and this test would be green over a seam it never
        // reached.
        let Ok(check) = ply_core::check_program(&program, &resolved) else {
            continue;
        };
        let backend = std::rc::Rc::new(backends::Declining::over(&program));
        let mut treewalk = Interp::new(&program, &resolved, &check);
        let mut machine = Machine::new(&program, &resolved, &check);
        machine.set_compiled(backend.clone());

        let report = compare_tests(&mut treewalk, &mut machine, &Fixture::empty());
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

    assert!(compared > 0, "no corpus ran");
    assert!(
        offered > 0,
        "{compared} tests ran and the seam was never reached, so this proves nothing"
    );
    println!("declining backend: {offered} calls offered over {compared} tests");
}

#[test]
fn a_backend_that_answers_correctly_agrees_over_every_corpus_on_disk() {
    let root = workspace_root();
    let mut entered = 0;
    let mut declined = 0;
    let mut compared = 0;

    for (label, dir, files) in corpora(&root) {
        let Some((program, resolved)) = load(&dir, &files) else {
            continue;
        };
        let Ok(check) = ply_core::check_program(&program, &resolved) else {
            continue;
        };
        let mut treewalk = Interp::new(&program, &resolved, &check);
        let mut machine = Machine::new(&program, &resolved, &check);
        machine.set_compiled(std::rc::Rc::new(backends::TreeWalker::over(&program)));

        let report = compare_tests(&mut treewalk, &mut machine, &Fixture::empty());
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

    assert!(compared > 0, "no corpus ran");
    assert!(
        entered > 0,
        "{compared} tests ran and no call was ever entered, so the accept path is unexercised"
    );
    println!("answering backend: {entered} entered, {declined} declined, over {compared} tests");
}

// --- The eight wrong backends, at corpus scale ------------------------------

use ply_eval::{BackendSpec, Fragment, Mutation, Offers};
use std::sync::OnceLock;

/// The corpora, loaded once and leaked, so that eight backends over one corpus cost one AST rather
/// than eight.
type Loaded = (
    String,
    &'static Program,
    &'static Resolved,
    &'static ply_core::CheckOutput,
);

fn corpus() -> &'static [Loaded] {
    static CORPUS: OnceLock<Vec<Loaded>> = OnceLock::new();
    CORPUS.get_or_init(|| {
        let root = workspace_root();
        let mut out = Vec::new();
        for (label, dir, files) in corpora(&root) {
            let Some((program, resolved)) = load(&dir, &files) else {
                continue;
            };
            let program: &'static Program = Box::leak(Box::new(program));
            let resolved: &'static Resolved = Box::leak(Box::new(resolved));
            let Ok(check) = ply_core::check_program(program, resolved) else {
                continue;
            };
            out.push((label, program, resolved, Box::leak(Box::new(check)) as &_));
        }
        out
    })
}

/// What one corruption did over every corpus on disk.
struct Sweep {
    compared: usize,
    /// Tests where the machine with the backend and the machine without it answered differently.
    diverged: usize,
    entered: u64,
    offers: Offers,
    /// The first disagreement, whole, because a count nobody can read is a count nobody checks.
    first: Option<String>,
}

/// Every corpus, twice: once on a machine with no backend and once on a machine with `spec`
/// attached, compared against each other.
fn sweep(spec: BackendSpec) -> Sweep {
    let mut out = Sweep {
        compared: 0,
        diverged: 0,
        entered: 0,
        offers: Offers::default(),
        first: None,
    };
    for (label, program, resolved, check) in corpus() {
        let fragment = Fragment::over_static(program, resolved, check);
        let mut plain = Machine::new(program, resolved, check);
        let mut backed = Machine::new(program, resolved, check);
        backed.set_compiled(fragment.attach(&spec));

        let report =
            ply_eval::differential::compare_tests(&mut plain, &mut backed, &Fixture::empty());
        out.compared += report.compared;
        out.diverged += report.divergences.len();
        if out.first.is_none()
            && let Some(d) = report.divergences.first()
        {
            out.first = Some(format!("{label}: {d}"));
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

#[track_caller]
fn fires_and_is_caught(name: &str, mutation: Mutation, target: Option<&str>) -> Sweep {
    let spec = BackendSpec {
        mutation,
        target: target.map(ply_span::Symbol::new),
        ..BackendSpec::honest()
    };
    let sweep = sweep(spec);
    assert!(
        sweep.compared > 0,
        "{name}: no corpus ran, so this proves nothing"
    );
    assert!(
        sweep.offers.fired > 0,
        "{name}: the corruption never changed an answer over {} tests, so this run says nothing \
         about the corpus that did not catch it ({} offers, {} entered)",
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
    println!(
        "{name}: {} of {} tests reported it · {} answers changed · {} offers · {} entered\n  first: {}",
        sweep.diverged,
        sweep.compared,
        sweep.offers.fired,
        sweep.offers.offered,
        sweep.entered,
        sweep.first.as_deref().unwrap_or("-")
    );
    sweep
}

/// The control every other test here is read against: the wrapper is the backend, and the backend
/// is honest.
#[test]
fn the_honest_backend_changes_no_answer_over_every_corpus_on_disk() {
    let sweep = sweep(BackendSpec::honest());
    assert_eq!(
        sweep.offers.fired, 0,
        "the honest backend changed an answer"
    );
    assert_eq!(
        sweep.diverged, 0,
        "the honest backend disagreed with the machine: {:?}",
        sweep.first
    );
    assert!(sweep.compared > 0);
    assert!(
        sweep.entered > 0,
        "{} calls were offered over {} tests and none was entered, so every test below would be \
         corrupting a seam nobody reaches",
        sweep.offers.offered,
        sweep.compared
    );
    // The `Bytes` widening of `compiled::crossable` (2026-08-30), asserted rather than assumed.
    assert!(
        sweep.offers.bytes_in > 0,
        "no call carrying a `Bytes` argument was offered over {} tests, so the widening of \
         `compiled::crossable` is inert on every corpus on disk and this file's greens say \
         nothing about it",
        sweep.compared
    );
    assert!(
        sweep.offers.bytes_out > 0,
        "{} calls carried a `Bytes` in and not one answered a `Bytes`, so the widening bought \
         arguments and no returns",
        sweep.offers.bytes_in
    );
    // The 2026-08-31 widening of the ANSWER test, asserted for the reason the two above are: a
    // return type nothing on disk returns leaves every green in this file a green over a seam the
    // change did not reach.
    assert!(
        sweep.offers.containers_out > 0,
        "not one entered call answered a `List`, `Map`, `Record` or `Ctor` over {} tests, so \
         the widening of `Machine::compiled_answer`'s answer test is inert on every corpus on \
         disk and this file's greens say nothing about it",
        sweep.compared
    );
    println!(
        "honest backend: {} entered of {} offered, over {} tests · {} offers carried a `Bytes` \
         argument · {} answered a `Bytes` · {} answered a container",
        sweep.entered,
        sweep.offers.offered,
        sweep.compared,
        sweep.offers.bytes_in,
        sweep.offers.bytes_out,
        sweep.offers.containers_out
    );
}

/// The ninth wrong backend, and the one the 2026-08-31 answer widening made possible: a container
/// answer of exactly the right kind with a [`ply_eval::Value::Cell`] in its first position.
#[test]
fn a_handle_forged_into_a_container_answer_is_caught_over_the_corpus() {
    fires_and_is_caught("handle", Mutation::Handle, None);
}

#[test]
fn an_off_by_one_compiled_answer_is_caught_over_the_corpus() {
    fires_and_is_caught("off-by-one", Mutation::OffByOne, None);
}

#[test]
fn an_inverted_compiled_comparison_is_caught_over_the_corpus() {
    fires_and_is_caught("inverted", Mutation::Inverted, None);
}

/// The one corruption that is invisible to a single call: every answer it gives was a correct
/// answer to *some* call, so only a corpus that varies its arguments can see it.
#[test]
fn a_stale_compiled_answer_is_caught_over_the_corpus() {
    fires_and_is_caught("stale", Mutation::Stale, None);
}

#[test]
fn a_wrong_kinded_compiled_answer_is_caught_over_the_corpus() {
    fires_and_is_caught("wrong-type", Mutation::WrongType, None);
}

/// A backend answering for a definition it has no body for.
#[test]
fn an_answer_for_a_definition_with_no_body_is_caught_over_the_corpus() {
    fires_and_is_caught("unoffered", Mutation::Unoffered, None);
}

/// Accepting a call the machine must never offer.
#[test]
fn a_definition_that_performs_is_never_offered_to_a_wrong_backend() {
    let sweep = sweep(BackendSpec {
        mutation: Mutation::Answers(7),
        target: Some(ply_span::Symbol::new("self_handled_effect.handled")),
        ..BackendSpec::honest()
    });
    assert_eq!(
        sweep.offers.offered_target, 0,
        "a definition that discharges its own effects was offered to a backend"
    );
    assert_eq!(sweep.offers.fired, 0);
    assert_eq!(sweep.diverged, 0, "{:?}", sweep.first);
    assert!(
        sweep.offers.offered > 0 && sweep.entered > 0,
        "the seam was never reached at all, so the zero above proves nothing"
    );
}

/// Running past the machine's call budget instead of declining.
#[test]
fn the_budget_corruption_has_nothing_to_bite_on_this_corpus_and_says_so() {
    let sweep = sweep(BackendSpec {
        mutation: Mutation::ExceedsBudget(Some(4)),
        target: None,
        ..BackendSpec::honest()
    });
    assert_eq!(
        sweep.offers.fired, 0,
        "a corpus in this tree now recurses past the machine's call bound, which is good news: \
         the budget corruption fires here after all and this test's note is what needs updating"
    );
    assert!(
        sweep.offers.offered > 0 && sweep.entered > 0,
        "the seam was never reached at all"
    );
}
