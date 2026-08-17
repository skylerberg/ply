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
    let resolved = resolve(&program).ok()?;
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
