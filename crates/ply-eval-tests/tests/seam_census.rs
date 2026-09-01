//! What fraction of a real program's calls can cross the compiled seam.

use ply_eval::{Evaluator, Fixture, Machine};
use ply_span::{SourceMap, Symbol};
use ply_syntax::ast::{ModuleName, Program};
use ply_syntax::parse_program;
use ply_syntax::resolve::{Resolved, resolve};
use std::cell::Cell;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

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
    if !ply_derive::expand_program(&mut program).is_empty() {
        return None;
    }
    let resolved = resolve(&mut program).ok()?;
    Some((program, resolved))
}

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

/// Declines everything and counts what it was handed — which is what `admit` cleared, and nothing
/// else.
struct Declining {
    program: usize,
    offered: Cell<u64>,
}

impl Declining {
    fn over(program: &Program) -> Declining {
        Declining {
            program: std::ptr::from_ref(program) as usize,
            offered: Cell::new(0),
        }
    }
}

impl ply_eval::Compiled for Declining {
    fn describes(&self, program: &Program) -> bool {
        std::ptr::from_ref(program) as usize == self.program
    }
    fn enter(&self, _: &Symbol, _: &[ply_eval::Value], _: usize) -> Option<ply_eval::Value> {
        self.offered.set(self.offered.get() + 1);
        None
    }
}

const EXAMPLES: &str = "examples";

/// Round-robin over the fixtures in their on-disk order, so a run of same-cost siblings spreads.
const BUCKETS: usize = 8;

/// The census counters are process-wide, so the tests here take turns and read deltas.
static TURN: Mutex<()> = Mutex::new(());

/// What one test sweeps.
#[derive(Clone, Copy)]
enum Selection {
    /// The one real program, and the only selection whose counts prove anything.
    Examples,
    Fixtures(usize),
}

impl Selection {
    fn corpora(self) -> Vec<(String, PathBuf, Vec<PathBuf>)> {
        let all = corpora(&workspace_root());
        match self {
            Selection::Examples => all.into_iter().filter(|c| c.0 == EXAMPLES).collect(),
            Selection::Fixtures(bucket) => all
                .into_iter()
                .filter(|c| c.0 != EXAMPLES)
                .enumerate()
                .filter(|(position, _)| position % BUCKETS == bucket)
                .map(|(_, c)| c)
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
    assert_eq!(
        Selection::Examples.corpora().len(),
        1,
        "examples/ is not a corpus"
    );
    let mut expected: Vec<String> = corpora(&workspace_root())
        .into_iter()
        .map(|c| c.0)
        .filter(|label| label != EXAMPLES)
        .collect();
    assert!(
        expected.len() > BUCKETS,
        "{} fixtures over {BUCKETS} buckets leaves buckets that sweep nothing",
        expected.len()
    );
    let mut seen: Vec<String> = (0..BUCKETS)
        .flat_map(|bucket| Selection::Fixtures(bucket).corpora())
        .map(|c| c.0)
        .collect();
    expected.sort_unstable();
    seen.sort_unstable();
    assert_eq!(
        seen, expected,
        "the buckets are not a partition of the fixtures"
    );
    assert_eq!(BUCKETS, 8, "`over_every_corpus!` names one test per bucket");
}

fn the_census_denominator_is_the_program_and_its_numerator_is_what_a_backend_is_offered(
    selection: Selection,
) {
    let _turn = TURN.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    ply_eval::census::enable();
    let (body0, admitted0, scalar0, gates0) = ply_eval::census::snapshot();
    let type_gated0 = ply_eval::census::type_gated_shipping();
    let walked0 = ply_eval::census::carried_sig_walked();
    let mut offered = 0u64;
    let mut compared = 0usize;

    let mut prev = (body0, admitted0, scalar0);
    for (label, dir, files) in selection.corpora() {
        let Some((program, resolved)) = load(&dir, &files) else {
            continue;
        };
        let Ok(check) = ply_core::check_program(&program, &resolved) else {
            continue;
        };
        let backend = std::rc::Rc::new(Declining::over(&program));
        // One machine, deliberately. `admitted` is a process-wide counter this
        // crate's `admit` bumps, so a second machine over the same corpus counts
        // every call twice while only the backed one's backend counts an offer,
        // and the cross-check below reads exactly 2x.
        let mut machine = Machine::new(&program, &resolved, &check);
        machine.set_compiled(backend.clone());
        Evaluator::set_fixture(&mut machine, &Fixture::empty());
        let mut ran = 0usize;
        for index in 0..Evaluator::test_count(&machine) {
            let _ = Evaluator::eval_test(&mut machine, index);
            ran += 1;
        }
        compared += ran;
        offered += backend.offered.get();
        let (b, a, sc, _) = ply_eval::census::snapshot();
        println!(
            "PER-CORPUS {:<28} body={:<9} admitted={:<8} scalar={:<7} tests={}",
            label,
            b - prev.0,
            a - prev.1,
            sc - prev.2,
            ran
        );
        prev = (b, a, sc);
    }

    let (body, admitted, scalar, gates) = ply_eval::census::snapshot();
    let body = body - body0;
    let admitted = admitted - admitted0;
    let scalar = scalar - scalar0;
    let refused: u64 = gates
        .iter()
        .map(|(gate, count)| count - gates0.get(gate).copied().unwrap_or(0))
        .sum();
    let argument_type = gates.get("ArgumentType").copied().unwrap_or(0)
        - gates0.get("ArgumentType").copied().unwrap_or(0);
    println!(
        "CROSS-CHECK over {selection}  body_calls={body}  admitted={admitted}  \
         scalar_sig={scalar}  declining-backend offered={offered}  tests={compared}"
    );
    assert_eq!(
        admitted, offered,
        "over {selection}, the census counted a different set from the one the backend was handed"
    );
    assert_eq!(
        body - admitted,
        refused,
        "over {selection}, the gate histogram does not sum"
    );

    // The two halves of the argument test, separated over a corpus.
    let type_gated = ply_eval::census::type_gated_shipping() - type_gated0;
    assert_eq!(
        type_gated,
        admitted,
        "the type gate and the kind gate disagree over {selection}: the kind gate refused {} \
         call(s) the declared types admitted",
        type_gated.saturating_sub(admitted)
    );

    // The two ends of the seam's one table, over a corpus.
    let walked = ply_eval::census::carried_sig_walked() - walked0;
    assert_eq!(
        walked,
        scalar,
        "the declared-type walk and the per-definition precompute disagree over {selection} by \
         {} call(s)",
        walked.abs_diff(scalar)
    );

    if selection.is_examples() {
        // Cumulative for the process, which is exactly this selection under nextest.
        println!("{}", ply_eval::census::report());
        assert!(compared > 0, "no corpus ran");
        assert!(body > 0, "the census hook never fired");
        // And the gate is not vacuous on this corpus: it refuses something.
        assert!(
            argument_type > 0,
            "`Gate::ArgumentType` refused nothing over examples, so this run says nothing about \
             it: {gates:?}"
        );
    }
}

/// One test per selection: `over_examples` carries the liveness assertions, the buckets the
/// cross-checks over one slice of the fixtures.
macro_rules! over_every_corpus {
    ($family:ident) => {
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
    };
}

over_every_corpus!(
    the_census_denominator_is_the_program_and_its_numerator_is_what_a_backend_is_offered
);
