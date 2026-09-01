//! The two numbers ADR 0017 §4 is judged on, taken on the source in the repository rather than on
//! shapes chosen to flatter them.

use ply_eval::rc;
use ply_eval::{Machine, TaskRegions};
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
        for module in std_imports(*id, name, text) {
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

/// Counters are per thread and cumulative, so a corpus is measured by taking them before and after
/// rather than by resetting between programs — a reset would discard whatever a neighbouring test
/// on this thread had counted.
fn delta(before: rc::Stats, after: rc::Stats) -> rc::Stats {
    rc::Stats {
        dup_sites: after.dup_sites - before.dup_sites,
        dup_emitted: after.dup_emitted - before.dup_emitted,
        drop_sites: after.drop_sites - before.drop_sites,
        drop_emitted: after.drop_emitted - before.drop_emitted,
        takes_attempted: after.takes_attempted - before.takes_attempted,
        takes_moved: after.takes_moved - before.takes_moved,
        updates: after.updates - before.updates,
        updates_in_place: after.updates_in_place - before.updates_in_place,
        cycles: after.cycles - before.cycles,
    }
}

fn percent(fraction: Option<f64>) -> String {
    match fraction {
        Some(f) => format!("{:.1}%", f * 100.0),
        None => "—".to_string(),
    }
}

#[test]
fn the_elision_and_reuse_this_milestone_claims_are_printed_as_numbers() {
    let root = workspace_root();
    let start = rc::stats();
    let mut programs = 0;
    let mut tests = 0;

    println!(
        "{:<28} {:>9} {:>9} {:>8} {:>9} {:>9} {:>8}",
        "corpus", "naive", "emitted", "elided", "updates", "in place", "reused"
    );
    for (label, dir, files) in corpora(&root) {
        let Some((program, resolved)) = load(&dir, &files) else {
            continue;
        };
        programs += 1;
        let mut machine = Machine::for_program(&program, &resolved);
        machine.set_regions(TaskRegions::new());
        let before = rc::stats();
        for index in 0..machine.test_count() {
            // A fixture is often a deliberately failing program; what it answered is
            // `differential_corpus`'s business and not this one's.
            let _ = machine.eval_test(index);
            tests += 1;
        }
        let counted = delta(before, rc::stats());
        if counted.dup_sites + counted.drop_sites == 0 {
            continue;
        }
        println!(
            "{:<28} {:>9} {:>9} {:>8} {:>9} {:>9} {:>8}",
            label,
            counted.dup_sites + counted.drop_sites,
            counted.dup_emitted + counted.drop_emitted,
            percent(counted.elided()),
            counted.updates,
            counted.updates_in_place,
            percent(counted.in_place()),
        );
    }
    let total = delta(start, rc::stats());

    println!(
        "{:<28} {:>9} {:>9} {:>8} {:>9} {:>9} {:>8}",
        "TOTAL",
        total.dup_sites + total.drop_sites,
        total.dup_emitted + total.drop_emitted,
        percent(total.elided()),
        total.updates,
        total.updates_in_place,
        percent(total.in_place()),
    );
    println!(
        "moves: {} of {} last uses found a scope nothing else held",
        total.takes_moved, total.takes_attempted
    );

    assert!(programs > 0, "no corpus loaded, so nothing was measured");
    assert!(
        tests > 0,
        "{programs} programs loaded and not one of them declares a test"
    );
    assert!(
        total.dup_sites + total.drop_sites > 10_000,
        "only {} operations were counted, which is too few to read a fraction off",
        total.dup_sites + total.drop_sites
    );

    let elided = total.elided().expect("operations were counted");
    assert!(
        elided > 0.70,
        "the pass elided {} of the naive scheme's operations; it was over 70% when this was written, \
         and a fall means it stopped firing rather than that the corpus changed shape",
        percent(Some(elided)),
    );

    assert!(
        total.updates > 100,
        "only {} updates ran, so the reuse fraction is one program's",
        total.updates
    );
    let in_place = total.in_place().expect("updates ran");
    assert!(
        in_place > 0.50,
        "only {} of updates rewrote their argument; reference counting that never reaches \
         one owner costs the counting and buys nothing",
        percent(Some(in_place)),
    );
}
