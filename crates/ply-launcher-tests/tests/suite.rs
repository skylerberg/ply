//! The environment a launched program reads, end to end: a program performs `env.var`,
//! `env.terminal` and `env.binary_version` and the launcher answers.

/// The counting allocator is a whole-binary decision, so this test binary installs it too, which
/// is what makes the window tests meaningful here.
#[global_allocator]
static ALLOCATOR: ply_launcher::count::Counting = ply_launcher::count::Counting;

use ply_eval::host::HostRegistry;
use ply_eval::{Machine, Provider};
use ply_span::{SourceId, Span};
use ply_ty::Front;
use std::collections::HashMap;
use std::sync::Arc;

/// A program that asks the environment everything it knows to ask.
const ASKER: &str = r#"
nondet effect env {
  read var[e](name: String) -> Option<String>
  read terminal[e](stream: String) -> Bool
  read binary_version[e]() -> String
  read pwd[e]() -> String
  read shipped_digest[e]() -> String
}

fn main() -> String / {env.var[e], env.terminal[e], env.binary_version[e]} = {
  let found = env.var[e]("PLY_LAUNCHER_TEST_MARK");
  let missing = env.var[e]("PLY_LAUNCHER_TEST_ABSENT");
  let term = env.terminal[e]("stdout");
  let version = env.binary_version[e]();
  let mark = match found { Some(v) -> v, None -> "unset" };
  let miss = match missing { Some(_) -> "present", None -> "absent" };
  mark ++ "|" ++ miss ++ "|" ++ (if term { "terminal" } else { "piped" }) ++ "|" ++ version
}
"#;

fn front_of(source: &str) -> Front {
    let named = vec![("m".to_string(), source.to_string())];
    let ids = vec![SourceId(0)];
    ply_codegen::c::producer::ensure_default();
    ply_codegen::c::producer::checked_front(&named, &ids).expect("the program checks")
}

fn ask() -> String {
    let front = front_of(ASKER);
    let texts: HashMap<String, String> =
        [("m".to_string(), ASKER.to_string())].into_iter().collect();
    let unit = ply_codegen::Unit::over_front(&front, texts).expect("this host has a C toolchain");
    let mut machine = Machine::new(&front);
    machine.set_compiled(unit.attach());
    let mut registry = HostRegistry::new();
    for (op, handler) in ply_launcher::env::registrations("9.9.9-test") {
        registry.register(op, handler);
    }
    let binding = registry.bind(&front.check).expect("the env ops bind");
    machine.set_host_binding(Arc::new(binding));
    machine
        .call("m.main", Vec::new(), Span::DUMMY)
        .expect("the entry ran")
        .to_string()
}

#[test]
fn a_program_reads_its_environment() {
    // Safety: the test is alone in its process (nextest), so the variable is its own.
    unsafe { std::env::set_var("PLY_LAUNCHER_TEST_MARK", "here") };
    let answer = ask();
    assert_eq!(answer, "\"here|absent|piped|9.9.9-test\"");
    unsafe { std::env::remove_var("PLY_LAUNCHER_TEST_MARK") };
}

// --- `--count-allocs` ---------------------------------------------------------

/// The launcher's own flag, taken out of the line wherever it is written; the program parses
/// what is left and never sees it.
#[test]
fn the_count_allocations_flag_is_read_from_the_line_and_taken_out() {
    for line in [
        vec!["--count-allocs=out.json", "run", "p.ply"],
        vec!["run", "--count-allocs=out.json", "p.ply"],
        vec!["run", "p.ply", "--count-allocs=out.json"],
    ] {
        let mut argv: Vec<String> = line.iter().map(|s| s.to_string()).collect();
        let asked = ply_launcher::count::flag(&mut argv).unwrap().unwrap();
        assert_eq!(asked.path, std::path::PathBuf::from("out.json"));
        assert!(!asked.sites);
        assert_eq!(
            argv,
            vec!["run".to_string(), "p.ply".to_string()],
            "the flag was left in the line"
        );
    }
}

#[test]
fn the_count_allocations_flag_may_name_its_path_with_a_space() {
    let mut argv: Vec<String> = ["run", "--count-allocs", "out.json", "p.ply"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    let asked = ply_launcher::count::flag(&mut argv).unwrap().unwrap();
    assert_eq!(asked.path, std::path::PathBuf::from("out.json"));
    assert!(!asked.sites);
    assert_eq!(argv, vec!["run".to_string(), "p.ply".to_string()]);
}

#[test]
fn the_count_allocations_flag_requires_a_path() {
    let mut argv: Vec<String> = ["run", "--count-allocs"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    assert!(ply_launcher::count::flag(&mut argv).is_err());
}

#[test]
fn a_line_without_the_flag_keeps_its_words() {
    let mut argv: Vec<String> = ["run", "p.ply"].iter().map(|s| s.to_string()).collect();
    assert!(ply_launcher::count::flag(&mut argv).unwrap().is_none());
    assert_eq!(argv, vec!["run".to_string(), "p.ply".to_string()]);
}

/// One test, because a window is process-global: two tests taking windows at once would clobber
/// each other's sites, which is fine for a launcher run (one entry, one window) and not for a
/// suite that runs its tests in parallel.
#[test]
fn a_window_counts_what_it_ran_and_only_attributes_when_asked() {
    let some =
        ply_launcher::count::window(|| (0..64u64).map(|n| n * 2).collect::<Vec<u64>>(), false).1;
    let (answer, counted, sites) = ply_launcher::count::window(
        || {
            let v: Vec<u64> = (0..64u64).map(|n| n * 2).collect();
            v.iter().sum::<u64>()
        },
        false,
    );
    assert_eq!(answer, (0..64u64).map(|n| n * 2).sum::<u64>());
    assert!(counted.allocations > 0, "a vector allocates");
    assert!(counted.bytes >= 64 * 8);
    assert!(
        sites.is_empty(),
        "no sites were asked for, so none were recorded"
    );
    assert!(
        counted.allocations <= some.allocations,
        "a window that ended must not count the next window's work"
    );

    // The same work, attributed: the sites are a breakdown of the total, minus the walk's own
    // allocations, which are the walker's rather than the program's.
    let (_, counted, sites) = ply_launcher::count::window(
        || {
            let v: Vec<u64> = (0..64u64).map(|n| n * 2).collect();
            v.len()
        },
        true,
    );
    assert!(counted.allocations > 0);
    assert!(!sites.is_empty(), "a window that asked for sites got none");
    let attributed: u64 = sites.values().map(|at| at.allocations).sum();
    assert!(
        attributed > 0 && attributed <= counted.allocations,
        "{attributed} of {} allocations were attributed",
        counted.allocations
    );
    let bytes: u64 = sites.values().map(|at| at.bytes).sum();
    assert!(bytes > 0 && bytes <= counted.bytes);
}
