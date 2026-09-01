//! What fraction of a real program's calls can cross the compiled seam.

use ply_eval::differential::compare_tests;
use ply_eval::{Fixture, Machine};
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
        let mut plain = Machine::new(&program, &resolved, &check);
        let mut machine = Machine::new(&program, &resolved, &check);
        machine.set_compiled(backend.clone());
        let report = compare_tests(&mut plain, &mut machine, &Fixture::empty());
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
    let type_gated = ply_eval::census::type_gated_shipping();
    assert_eq!(
        type_gated,
        admitted,
        "the type gate and the kind gate disagree over this corpus: the kind gate refused          {} call(s) the declared types admitted",
        type_gated.saturating_sub(admitted)
    );

    // The two ends of the seam's one table, over a corpus.
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
