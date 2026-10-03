//! The environment a launched program reads, end to end: a program performs `env.var`,
//! `env.vars`, `env.terminal` and `env.binary_version` and the launcher answers.

/// The counting allocator is a whole-binary decision, so this test binary installs it too, which
/// is what makes the window tests meaningful here.
#[global_allocator]
static ALLOCATOR: ply_launcher::count::Counting = ply_launcher::count::Counting;

use ply_eval::host::HostRegistry;
use ply_eval::{Analysis, Machine, Provider, SourceId, Span};
use std::collections::HashMap;
use std::sync::Arc;

/// A program that asks the environment everything it knows to ask.
const ASKER: &str = r#"
nondet effect env {
  read var[e](name: String) -> Option<String>
  read vars[e]() -> List<{ name: String, value: String }>
  read terminal[e](stream: String) -> Bool
  read binary_version[e]() -> String
  read pwd[e]() -> String
  read shipped_digest[e]() -> String
  read builder_digest[e]() -> String
  read fronts[e]() -> String
  read bodies[e]() -> String
}

fn main() -> String / {env.var[e], env.vars[e], env.terminal[e], env.binary_version[e]} = {
  let found = env.var[e]("PLY_LAUNCHER_TEST_MARK");
  let listed = filter(env.vars[e](), |v: { name: String, value: String }|
    v.name == "PLY_LAUNCHER_TEST_MARK" && v.value == "here");
  let missing = env.var[e]("PLY_LAUNCHER_TEST_ABSENT");
  let term = env.terminal[e]("stdout");
  let version = env.binary_version[e]();
  let mark = match found { Some(v) -> v, None -> "unset" };
  let miss = match missing { Some(_) -> "present", None -> "absent" };
  mark ++ "|" ++ miss ++ "|" ++ (if len(listed) == 1 { "listed" } else { "unlisted" }) ++ "|"
    ++ (if term { "terminal" } else { "piped" }) ++ "|" ++ version
}
"#;

fn front_of(source: &str) -> Analysis {
    let named = vec![("m".to_string(), source.to_string())];
    let ids = vec![SourceId(0)];
    ply_codegen::c::producer::ensure_default();
    ply_codegen::c::producer::checked_analysis(&named, &ids).expect("the program checks")
}

fn ask() -> ply_eval::Value {
    let front = front_of(ASKER);
    let texts: HashMap<String, String> =
        [("m".to_string(), ASKER.to_string())].into_iter().collect();
    let unit = ply_codegen::Unit::over_front(&front, texts).expect("this host has a C toolchain");
    let mut machine =
        Machine::new(&front, unit.attach()).expect("the unit was compiled from this program");
    let mut registry = HostRegistry::new();
    for (op, handler) in ply_launcher::env::registrations("9.9.9-test") {
        registry.register(op, handler);
    }
    let binding = registry.bind(&front.check).expect("the env ops bind");
    machine.set_host_binding(Arc::new(binding));
    machine
        .call("m.main", Vec::new(), Span::DUMMY)
        .into_parts()
        .0
        .expect("the entry ran")
}

#[test]
fn a_program_reads_its_environment() {
    // Safety: the test is alone in its process (nextest), so the variable is its own.
    unsafe { std::env::set_var("PLY_LAUNCHER_TEST_MARK", "here") };
    let answer = ask();
    assert_eq!(
        answer,
        ply_eval::Value::str("here|absent|listed|piped|9.9.9-test")
    );
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
        assert!(!asked.exact);
        assert_eq!(asked.every(), 0, "no sites, so nothing is walked");
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
    // The same work, attributed. Every allocation the window counted is attributed to a site, and
    // the walk's own allocations are neither: they are the instrument's, not the program's. That
    // makes the rows a *complete* breakdown of the total — an allocation counted but not
    // attributed is exactly the bug worth catching (the walker's `Vec` growth used to be counted
    // in the total and left out of every row).
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
    assert_eq!(
        attributed, counted.allocations,
        "the rows are what the window counted, in full"
    );
    let bytes: u64 = sites.values().map(|at| at.bytes).sum();
    assert_eq!(
        bytes, counted.bytes,
        "the rows' bytes are the window's bytes"
    );
    // And the same work again at the run's default sample: every allocation is still counted, one
    // in `SAMPLED` is walked, and the rows come back scaled up so they read in the totals' units.
    // Their sum is near the total rather than equal to it, which is the trade the report states.
    let (answer, counted, sites) = ply_launcher::count::window_sampled(
        || {
            let mut v: Vec<Vec<u64>> = Vec::new();
            for n in 0..4096u64 {
                v.push(vec![n; 4]);
            }
            v.len()
        },
        ply_launcher::count::SAMPLED,
    );
    assert_eq!(answer, 4096);
    assert!(
        counted.allocations >= 4096,
        "4096 vectors allocate at least once each: {}",
        counted.allocations
    );
    assert!(!sites.is_empty(), "a long window sampled some sites");
    let walked: u64 = sites.values().map(|at| at.allocations).sum();
    assert!(
        walked < counted.allocations,
        "the sample is smaller than the window: {walked} of {}",
        counted.allocations
    );
    let scaled = walked * u64::from(ply_launcher::count::SAMPLED);
    let low = counted.allocations * 3 / 4;
    let high = counted.allocations * 5 / 4;
    assert!(
        (low..=high).contains(&scaled),
        "the scaled rows are near the total: {scaled} against {}",
        counted.allocations
    );
}

/// The flags that ask for sites: plain ones sample — the walk is the whole cost of a site census —
/// and `-exact` walks every allocation. The stronger request wins wherever it stands, and asking
/// for sites at all still means sites.
#[test]
fn the_sites_flags_choose_between_every_allocation_and_a_sample() {
    for (line, exact) in [
        (vec!["run", "--count-alloc-sites=out.json"], false),
        (vec!["run", "--count-alloc-sites-exact=out.json"], true),
        (
            vec![
                "run",
                "--count-alloc-sites=out.json",
                "--count-alloc-sites-exact=e.json",
            ],
            true,
        ),
        (
            vec![
                "run",
                "--count-alloc-sites-exact=e.json",
                "--count-alloc-sites=out.json",
            ],
            true,
        ),
    ] {
        let mut argv: Vec<String> = line.iter().map(|s| s.to_string()).collect();
        let asked = ply_launcher::count::flag(&mut argv).unwrap().unwrap();
        assert!(asked.sites, "sites were asked for: {line:?}");
        assert_eq!(asked.exact, exact, "{line:?}");
        assert_eq!(
            asked.every(),
            if exact {
                1
            } else {
                ply_launcher::count::SAMPLED
            },
            "{line:?}"
        );
    }
}

/// A sample of every `n`th allocation would alias with a program that allocates exactly `n` times
/// around an iteration — the same site every time, which is the one thing a site census must not
/// do. The sample is spread instead.
#[test]
fn the_sample_does_not_fall_on_one_period_of_the_window() {
    let every = ply_launcher::count::SAMPLED;
    let hits: Vec<u64> = (0..(1u64 << 20))
        .filter(|n| ply_launcher::count::sampled_nth(*n, every))
        .collect();
    let expected = (1usize << 20) / every as usize;
    assert!(
        hits.len() > expected / 2 && hits.len() < expected * 2,
        "{} sampled of an expected {expected}",
        hits.len()
    );
    assert!(
        hits.windows(2)
            .any(|pair| pair[1] - pair[0] != u64::from(every)),
        "every gap is {every}: the sample is periodic after all"
    );
    assert!(ply_launcher::count::sampled_nth(0, 1), "exact walks all");
    assert!(
        ply_launcher::count::sampled_nth(7, 0),
        "no sites walks none"
    );
}
