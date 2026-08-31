//! What fraction of a real program's calls can cross the compiled seam.
//!
//! `Machine::compiled_counts` counts entries and declines *after* `admit` has
//! cleared a call, so its denominator is the admitted set rather than the
//! program. `differential_corpus.rs`'s "calls offered" is the same denominator:
//! a backend's `enter` is reached only for a call every gate already passed.
//! This file supplies the missing denominator — every call to a function body —
//! and the gate that refused each one.
//!
//! The cross-check that makes it non-vacuous: over the same corpus, in the same
//! process, the census's `admitted` must equal a declining backend's `offered`.

use ply_eval::differential::compare_tests;
use ply_eval::{Fixture, Interp, Machine};
use ply_span::{SourceMap, Symbol};
use ply_syntax::ast::{ModuleName, Program};
use ply_syntax::parse_program;
use ply_syntax::resolve::{Resolved, resolve};
use std::cell::Cell;
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

/// Declines everything and counts what it was handed — which is what `admit`
/// cleared, and nothing else.
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

#[test]
fn the_census_denominator_is_the_program_and_its_numerator_is_what_a_backend_is_offered() {
    ply_eval::census::enable();
    let root = workspace_root();
    let mut offered = 0u64;
    let mut compared = 0usize;

    let mut prev = (0u64, 0u64, 0u64);
    for (_label, dir, files) in corpora(&root) {
        let Some((program, resolved)) = load(&dir, &files) else {
            continue;
        };
        let Ok(check) = ply_core::check_program(&program, &resolved) else {
            continue;
        };
        let backend = std::rc::Rc::new(Declining::over(&program));
        let mut treewalk = Interp::new(&program, &resolved, &check);
        let mut machine = Machine::new(&program, &resolved, &check);
        machine.set_compiled(backend.clone());
        let report = compare_tests(&mut treewalk, &mut machine, &Fixture::empty());
        compared += report.compared;
        offered += backend.offered.get();
        let (b, a, sc, _) = ply_eval::census::snapshot();
        println!(
            "PER-CORPUS {:<28} body={:<9} admitted={:<8} scalar={:<7} tests={}",
            _label,
            b - prev.0,
            a - prev.1,
            sc - prev.2,
            report.compared
        );
        prev = (b, a, sc);
    }

    let (body, admitted, scalar, gates) = ply_eval::census::snapshot();
    println!("{}", ply_eval::census::report());
    println!(
        "CROSS-CHECK  body_calls={body}  admitted={admitted}  scalar_sig={scalar}  \
         declining-backend offered={offered}  tests={compared}"
    );
    let refused: u64 = gates.values().sum();
    assert!(compared > 0, "no corpus ran");
    assert!(body > 0, "the census hook never fired");
    assert_eq!(
        admitted, offered,
        "the census counted a different set from the one the backend was handed"
    );
    assert_eq!(body - admitted, refused, "the gate histogram does not sum");

    // The two halves of the argument test, separated over a corpus.
    //
    // `Gate::ArgumentType` decides an argument from its definition's declared
    // parameter type and `Gate::ArgumentShape` decides it from the value's
    // discriminant, and on a program the checker accepted the second refuses
    // nothing the first admits: a value whose declared type is `Int` is a
    // `Value::Int`. That is an argument about the type system, so it is read off
    // a corpus rather than believed — `type_gated_shipping` is the type gate
    // asked with the kind test removed, and any gap between it and `admitted` is
    // defence in depth actually firing on real source, which is a fact worth
    // seeing rather than a failure.
    //
    // Seen to fail, and by the corruption that can move the two apart rather
    // than by the one a reader reaches for first. Deleting `Denotes::matches`
    // from `CarriedTypes::args_cross` — so a declared type licenses a value of
    // **any** kind — leaves this assertion GREEN, because both sides read the
    // same `args_cross` and move together; that was tried first and is recorded
    // so nobody tries it again. What is red is a corruption of the OTHER half:
    //
    //   `Value::Record(_)` removed from `compiled::crossable_argument_kind`
    //     -> 882207 == 859104, "the kind gate refused 23103 call(s) the
    //        declared types admitted"
    //   `CarriedTypes::args_cross` stubbed to `true`
    //     -> 1293678 == 1207996, 85682 apart
    //
    // Both measured on this corpus on 2026-08-31, with `Compiling ply-eval`
    // confirmed in the output and the file restored and digest-checked after.
    // The vacuity assertion below reds under the stub too, one assertion later.
    let type_gated = ply_eval::census::type_gated_shipping();
    assert_eq!(
        type_gated,
        admitted,
        "the type gate and the kind gate disagree over this corpus: the kind gate refused          {} call(s) the declared types admitted",
        type_gated.saturating_sub(admitted)
    );

    // The two ends of the seam's one table, over a corpus.
    //
    // `admitted_carried_sig` reads `CarriedTypes`'s per-definition `Denotes`,
    // computed once when the table was built; `carried_sig_walked` calls
    // `CarriedTypes::carries` on every declared parameter and on the declared
    // return type at the call. They are the same predicate by two routes, and
    // the point of counting both is that a per-definition PRECOMPUTE has a
    // failure mode a per-call walk does not — it can be stale, or built from a
    // different `CheckOutput`, and nothing about a run says so.
    //
    // Seen to fail, by a corruption that touches ONE route: `CarriedTypes::over`
    // filling `Sig::ret` with `Some(Denotes::Int)` instead of
    // `table.denotes(ret)`, so every definition's return looks carried to the
    // precompute and the walk disagrees. Measured 2026-08-31 with `Compiling
    // ply-eval` confirmed and the file restored and digest-checked after:
    //
    //   "the declared-type walk and the per-definition precompute disagree over
    //    this corpus by 200930 call(s)"  left: 681277  right: 882207
    //
    // A corruption of `CarriedTypes::carries` itself would NOT red this: both
    // routes read that function, so they move together. That is the shape of
    // what this assertion can and cannot see, and it is why it is a
    // precompute-versus-walk check rather than a check on the rule.
    let walked = ply_eval::census::carried_sig_walked();
    assert_eq!(
        walked,
        scalar,
        "the declared-type walk and the per-definition precompute disagree over this corpus by \
         {} call(s)",
        walked.abs_diff(scalar)
    );

    // And the gate is not vacuous on this corpus: it refuses something.
    assert!(
        gates.get("ArgumentType").copied().unwrap_or(0) > 0,
        "`Gate::ArgumentType` refused nothing over the whole corpus, so this run says          nothing about it: {gates:?}"
    );
}
