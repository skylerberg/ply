//! Both engines over the corpora that exist on disk.
//!
//! The hand-written unit tests agree on the shapes somebody thought to write
//! down. Real source is where a disagreement is actually found, so this points
//! the harness at `examples/` and `tests/fixtures/` and refuses to pass if it
//! compared nothing — an all-skipped run and a clean run must not look alike.

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

/// The `std.*` modules one source names. A module that does not parse names
/// nothing: the caller's own parse is what reports that.
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

/// A fixture is often a deliberately broken program, so anything that does not
/// parse or resolve is not this test's business and is counted as skipped
/// rather than failed.
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
    // Demand-driven, exactly as `ply`'s own loader is: a corpus that imports
    // nothing from `std` gets nothing, so a one-file fixture stays the program
    // it is rather than acquiring the stdlib's definitions and tests.
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
    // A `derive` declares no name and every walker skips it, so a harness that
    // forgets to expand runs a program whose generated definitions silently do
    // not exist.
    if !ply_derive::expand_program(&mut program).is_empty() {
        return None;
    }
    let resolved = resolve(&mut program).ok()?;
    Some((program, resolved))
}

/// Every directory that is one program, plus every stray top-level fixture as a
/// program of its own — which is what they are, since a fixture in
/// `tests/fixtures` names nothing in its neighbour.
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
        // A green run whose footprint axis never ran is what let two engines
        // performing different atoms pass this audit. Zero compared footprints
        // is therefore a failure, not a footnote.
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

/// A refusal is not agreement, so the harness must count it apart. Pinned on a
/// real fixture: a corpus only one engine can run passing for an audited one is
/// the failure mode `--engine both` exists to prevent.
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

/// The corpus half of `CONTRIBUTING.md` §"Things known to be broken" item 11,
/// pinned on its fixture rather than left to the sweep above.
///
/// The sweep runs every corpus on disk and would keep passing if this fixture
/// were deleted, renamed or quietly made pure — which is the shape of failure
/// this repository produces most. So the coverage is asserted here: the fixture
/// exists, it declares an effect, its two self-handling definitions publish
/// **empty** rows, the backend can answer both of them, and neither is offered.
///
/// The last of those is what makes the test bite. `backends::TreeWalker` runs a
/// definition on its own `Interp` with no handler stack, so it can answer a
/// definition that discharges its own operations and cannot answer one that
/// performs into its caller. Delete the effects gate and this fixture is
/// entered, the machine records neither atom, and `compare_tests` reports
/// `observed footprint — left {..tally.read[log], ..tally.write[log]},
/// right {}`.
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

    // The other effect gate, on the same corpus: `measured` is what performs
    // into its caller, so its row is not empty and `Gate::PublishedRow` is what
    // refuses it. Asserted on the row rather than on the gate because a corpus
    // run counts declines without recording which gate produced one.
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

    // The control is the point: `doubled` *is* entered, so the two refusals
    // above are this gate rather than a backend that answers nothing here.
    let (entered, _) = machine.compiled_counts();
    assert!(
        entered > 0,
        "no call in this fixture was entered at all, so nothing distinguishes the effects gate \
         from a backend that declines"
    );
}

/// `examples/` is the corpus the milestone's exit criterion names, so it gets
/// its own assertion rather than being one entry in a loop that would still
/// pass if it silently stopped loading.
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
///
/// `crates/ply-eval/src/compiled.rs` holds hand-built doubles over hand-built
/// programs. This is the same seam over real source at corpus scale, which is
/// where ADR 0018 §0's 1,344 green cases would have been the wrong instrument:
/// the counters below fail the test if the seam was never reached.
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

    /// A backend whose "compiled code" is the tree-walker, over its own copy of
    /// the program with its own world.
    ///
    /// It is not a JIT and does not pretend to be one. What it is, is a backend
    /// that answers the right value for every call this boundary admits, which
    /// makes the *accept* path testable against real source: every gate, the
    /// `Frame::Call` push, the constant memo and the argument-vector hand-back
    /// run for thousands of calls instead of the dozen a hand-built double
    /// reaches. A wrong answer here is a defect in the seam.
    ///
    /// The program is leaked because a backend may not borrow — see the
    /// `compiled` field on `Machine`. A test binary that leaks one AST is a
    /// better trade than a seam nobody entered.
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
        // `Machine::for_program` carries no `CheckOutput`, and the purity gate
        // reads the published row: without one the hook is inert and this test
        // would be green over a seam it never reached.
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
//
// ADR 0026 §6 item 3, and the condition §4.7 puts on deleting
// `crates/ply-codegen-spike`: *"the eight wrong backends of `tests/mutations.rs`,
// reproduced over the `Compiled` doubles in `crates/ply-eval/tests/`, running
// under `cargo test --workspace`, with a corpus that has been seen to fail."*
//
// The doubles above are what made the *accept* path testable on real source.
// These are what make it testable that a wrong answer on real source would be
// **noticed** — which is a different claim, and the one `CONTRIBUTING.md` §"The
// one rule" says this project keeps failing to check. Every test here follows
// `mutations.rs`'s three steps and asserts the middle one first: the corruption
// fired, and only then that something reported it.
//
// The backend is `ply_eval::Reference` rather than a hand-built double, because
// `Mutation::Unoffered` needs a backend that can *miss* — one that answers
// everything has no registry gap to corrupt — and because it is the same
// backend `ply test --backend` installs, so a corruption caught here is caught
// by a command a user can run.
//
// # Measured sensitivity
//
// Printed by every test below rather than asserted as a magic number, because
// §4.7's condition names measured sensitivity and a count that is asserted is a
// count nobody re-takes. What is asserted is that each corruption fired and that
// at least one test reported it.

use ply_eval::{BackendSpec, Fragment, Mutation, Offers};
use std::sync::OnceLock;

/// The corpora, loaded once and leaked, so that eight backends over one corpus
/// cost one AST rather than eight.
///
/// `Fragment::over_static` is what makes that possible: a backend may not borrow
/// (see `Machine`'s `compiled` field), and a caller that already holds a leaked
/// program has paid for that once.
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
    /// Tests where the machine with the backend and the machine without it
    /// answered differently. This is the number that says the corpus can see.
    diverged: usize,
    entered: u64,
    offers: Offers,
    /// The first disagreement, whole, because a count nobody can read is a count
    /// nobody checks.
    first: Option<String>,
}

/// Every corpus, twice: once on a machine with no backend and once on a machine
/// with `spec` attached, compared against each other.
///
/// Against the plain **machine** rather than against the tree-walker, and that
/// is the whole of the care this needs: a divergence reported here is the
/// backend's and nothing else's, which is the same arrangement
/// `ply test --engine both --backend ..` runs.
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
    }
    out
}

#[track_caller]
fn fires_and_is_caught(name: &str, mutation: Mutation, target: Option<&str>) -> Sweep {
    let spec = BackendSpec {
        mutation,
        target: target.map(ply_span::Symbol::new),
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

/// The control every other test here is read against: the wrapper is the
/// backend, and the backend is honest.
///
/// Without this a red result below could be the *presence* of a backend rather
/// than the corruption. The offer and entry counts are asserted for the reason
/// `mutations.rs` asserts them: a green result over a seam nobody reached is the
/// exact shape of vacuous pass this project produces most.
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
    // The `Bytes` widening of `compiled::crossable` (2026-08-30), asserted
    // rather than assumed. A widening nothing on disk exercises leaves every
    // test in this file green over a seam it did not reach, which is the
    // vacuous pass `CONTRIBUTING.md` §"The one rule" names — and it is exactly
    // the shape this widening could take, since `Bytes` is a kind a corpus
    // might never pass to a named function at all.
    //
    // Both directions, because they are two mechanisms: `admit`'s `crossable`
    // test on the arguments, and `Machine::compiled_answer`'s on the answer.
    // Before the widening both of these were 0 by construction.
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
    println!(
        "honest backend: {} entered of {} offered, over {} tests · {} offers carried a `Bytes` \
         argument · {} answered a `Bytes`",
        sweep.entered,
        sweep.offers.offered,
        sweep.compared,
        sweep.offers.bytes_in,
        sweep.offers.bytes_out
    );
}

#[test]
fn an_off_by_one_compiled_answer_is_caught_over_the_corpus() {
    fires_and_is_caught("off-by-one", Mutation::OffByOne, None);
}

#[test]
fn an_inverted_compiled_comparison_is_caught_over_the_corpus() {
    fires_and_is_caught("inverted", Mutation::Inverted, None);
}

/// The one corruption that is invisible to a single call: every answer it gives
/// was a correct answer to *some* call, so only a corpus that varies its
/// arguments can see it.
///
/// ADR 0026 §7 named this as the mutation most likely not to survive the move
/// out of the spike, because the spike's corpus *generates* cases and this one
/// runs real programs. It survives, and the number it survives by is printed.
#[test]
fn a_stale_compiled_answer_is_caught_over_the_corpus() {
    fires_and_is_caught("stale", Mutation::Stale, None);
}

#[test]
fn a_wrong_kinded_compiled_answer_is_caught_over_the_corpus() {
    fires_and_is_caught("wrong-type", Mutation::WrongType, None);
}

/// A backend answering for a definition it has no body for.
///
/// The gap this lives in is real on this corpus rather than constructed:
/// `compiled::admit` gates on the shape of the **arguments** and never on the
/// return type, so every `Int -> List<..>`, `Int -> String` and `Int -> Record`
/// in `examples/` is offered and must be declined.
///
/// > **Narrowed, not closed, by the `Bytes` widening (2026-08-30).** The gap
/// > used to hold every `Int -> Bytes` too, and `std.router.hex_char` alone was
/// > 65,560 calls of it; those are now inside the fragment and answered. What
/// > is left is `String`, `Float`, `Decimal`, `List`, `Map`, `Record`, `Ctor`
/// > and every polymorphic return, which is still most of `examples/`. This
/// > test is what fails the day the fragment grows to cover the whole of what
/// > the seam offers — at which point `Unoffered` has nothing to invent an
/// > answer for and stops being a corruption, and something else has to police
/// > the registry.
#[test]
fn an_answer_for_a_definition_with_no_body_is_caught_over_the_corpus() {
    fires_and_is_caught("unoffered", Mutation::Unoffered, None);
}

/// Accepting a call the machine must never offer.
///
/// **Not caught, and that is the finding rather than a gap** — the same one
/// `crates/ply-codegen-spike/tests/mutations.rs` records. `handled` and
/// `wrapper` in `tests/fixtures/self_handled_effect.ply` perform under a
/// `handle` of their own, publish empty rows, and are refused by
/// `Gate::InternalEffects`; a backend standing ready to answer one is never
/// asked. What stands is the offer count, which is the fact the gate makes true,
/// and the control beside it — the seam *was* reached — is what stops the zero
/// being a backend nobody consulted.
#[test]
fn a_definition_that_performs_is_never_offered_to_a_wrong_backend() {
    let sweep = sweep(BackendSpec {
        mutation: Mutation::Answers(7),
        target: Some(ply_span::Symbol::new("self_handled_effect.handled")),
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
///
/// **Not fired on this corpus, and the reason is worth writing down**: nothing
/// in `examples/` or `tests/fixtures/` recurses past `DEFAULT_MAX_CALLS`, so the
/// honest backend never has to decline for want of budget and there is no
/// decline for the corruption to replace. It is checked from the shipping
/// command instead, on a corpus built to outrun the bound —
/// `crates/ply-cli/tests/backend.rs`'s `DEEP` — and that is the only place in
/// this workspace where this corruption fires.
///
/// Asserted rather than skipped, because "the corpus cannot exercise this" and
/// "the corpus stopped exercising this" must not look alike: if a corpus ever
/// does outrun the bound, this test fails and says so.
#[test]
fn the_budget_corruption_has_nothing_to_bite_on_this_corpus_and_says_so() {
    let sweep = sweep(BackendSpec {
        mutation: Mutation::ExceedsBudget(Some(4)),
        target: None,
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
