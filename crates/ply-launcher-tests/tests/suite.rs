//! The environment a launched program reads, end to end: a program performs `env.var`,
//! `env.vars`, `env.terminal` and `env.binary_version` and the launcher answers.

/// The counting allocator is a whole-binary decision, so this test binary installs it too, which
/// is what makes the window tests meaningful here.
#[global_allocator]
static ALLOCATOR: ply_launcher::count::Counting = ply_launcher::count::Counting;

use ply_eval::host::HostRegistry;
use ply_eval::{Analysis, Machine, Provider, Span};
use std::sync::Arc;

/// A program that asks the environment everything it knows to ask.
const ASKER: &str = r#"
nondet effect env {
  read var[e](name: String) -> Option<String>
  read vars[e]() -> List<{ name: String, value: String }>
  read terminal[e](stream: String) -> Bool
  read binary_version[e]() -> String
  read binary_bytes[e]() -> Option<Int>
  read pwd[e]() -> String
  read shipped_digest[e]() -> String
  read builder_digest[e]() -> String
  read reused[e]() -> String
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

/// The program the builder makes of `source`: its front end's answer and its unit.
fn built(source: &str) -> (Analysis, &'static ply_codegen::Unit) {
    let files = ply_machine::builds::module_files(&[("m", source)]);
    let program = ply_machine::builds::checked_program(&files)
        .unwrap_or_else(|d| panic!("the program checks: {d}"));
    let front = program.front.answer;
    let unit =
        ply_codegen::Unit::handed(&front, program.unit).expect("this host has a C toolchain");
    (front, unit)
}

fn ask() -> ply_eval::Value {
    let (front, unit) = built(ASKER);
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

// --- The program this tree's own builder makes --------------------------------

/// A binary enters the `ply` program the committed builder made, so this is where the builder
/// these sources make is seen to build the whole program, and that program to compile a unit and
/// run its test.
#[test]
fn the_builder_these_sources_make_builds_the_program_and_it_runs() {
    let runnable = ply_launcher::shipped::program_by_own_builder()
        .unwrap_or_else(|d| panic!("this tree's builder builds the program: {d}"));
    let project = tempfile::tempdir().expect("a scratch directory");
    std::fs::write(
        project.path().join("m.ply"),
        "test \"a unit is compiled\" {\n  assert(1 + 1 == 2)\n}\n",
    )
    .expect("the project is written");
    let program = ply_launcher::Program {
        runnable,
        shelf: ply_machine::shelf::sources().to_vec(),
        stage: ply_launcher::shipped::own_stage_name(),
        version: env!("CARGO_PKG_VERSION").to_string(),
    };
    let binds = ply_machine::enter::Binds {
        lent: ply_machine::policy::all(),
        ..ply_machine::enter::Binds::default()
    };
    let argv = vec!["test".to_string(), project.path().display().to_string()];
    let (answer, _) = ply_launcher::run(program, project.path(), argv, binds, None).into_parts();
    assert_eq!(answer.unwrap_or_else(|d| panic!("the program ran: {d}")), 0);
}

/// `main` and what it reaches, beside a definition nothing reaches and a comment.
fn entered(answer: &str, aside: &str, comment: &str) -> String {
    format!(
        "// {comment}\nfn answer() -> Int = {answer}\n\npub fn main() -> Int = answer()\n\nfn aside() -> Int = {aside}\n"
    )
}

/// The runnable this tree's builder makes of `text`, which holds the text it was built from.
fn built_by_own(stage: &std::path::Path, name: &str, text: &str) -> Vec<u8> {
    let src = stage.join(format!("{name}.src"));
    std::fs::create_dir_all(&src).expect("the sources' directory");
    std::fs::write(src.join("m.ply"), text).expect("the source is written");
    let out = stage.join(format!("{name}.run"));
    let rows = format!("kept-program-test-{}", std::process::id());
    ply_machine::builds::build_by_own(&src, ".", "m.main", &out, &rows)
        .unwrap_or_else(|d| panic!("this tree's builder builds `{name}`: {d}"));
    std::fs::read(&out).expect("the runnable is read")
}

/// A program is the definition it enters and all that reaches: a text that moved everywhere else
/// is the program already built, and one whose entry reaches something else is built.
#[test]
fn a_text_that_enters_what_a_built_program_does_is_not_built_again() {
    let stage = ply_codegen::c::stage::stage_dir(&format!("kept-program-{}", std::process::id()));
    let first = built_by_own(&stage, "first", &entered("1", "2", "as written"));
    let moved = built_by_own(
        &stage,
        "moved",
        &entered("1", "3", "a comment and `aside` moved"),
    );
    let edited = built_by_own(&stage, "edited", &entered("4", "2", "as written"));
    let _ = std::fs::remove_dir_all(&stage);
    assert!(
        first == moved,
        "a text whose entry hashes as a built program's takes that program, sources and all"
    );
    assert!(
        first != edited,
        "a text whose entry reaches another body is built"
    );
}

/// The value a runnable's `m.main` answers.
fn answer_of(runnable: &[u8]) -> ply_eval::Value {
    let program = ply_machine::runnable::decode(runnable)
        .unwrap_or_else(|why| panic!("the runnable reads: {why}"));
    let front = program.front.answer;
    let unit =
        ply_codegen::Unit::handed(&front, program.unit).expect("this host has a C toolchain");
    Machine::new(&front, unit.attach())
        .expect("the unit was compiled from this program")
        .call("m.main", Vec::new(), Span::DUMMY)
        .into_parts()
        .0
        .expect("the entry ran")
}

/// A build after one body moved reads every module that did not move, and that reaches none that
/// did, through what the build before it kept: here `lib` and the shipped module it imports. The
/// program it makes is the program its sources say, and a file `lib` embeds is one of them.
#[test]
fn a_program_built_through_what_its_last_build_kept_is_the_program() {
    let id = std::process::id();
    let stage = ply_codegen::c::stage::stage_dir(&format!("kept-modules-{id}"));
    let src = stage.join("src");
    std::fs::create_dir_all(&src).expect("the sources' directory");
    std::fs::write(
        src.join("lib.ply"),
        "import std.math (abs)\n\npub fn far(a: Int, b: Int) -> Int = abs(a - b) + bytes_len(embed(\"n.txt\"))\n\ntest \"far\" { assert_eq(far(2, 9), 8) }\n",
    )
    .expect("the library is written");
    std::fs::write(src.join("n.txt"), "x").expect("the embedded file is written");
    let rows = format!("kept-modules-test-{id}");
    let build = |name: &str, more: i64| {
        std::fs::write(
            src.join("m.ply"),
            format!("import lib\n\npub fn main() -> Int = lib::far(2, 9) + {more} + {id} - {id}\n"),
        )
        .expect("the entry's module is written");
        let out = stage.join(name);
        ply_machine::builds::build_by_own(&src, ".", "m.main", &out, &rows)
            .unwrap_or_else(|d| panic!("this tree's builder builds `{name}`: {d}"));
        std::fs::read(&out).expect("the runnable is read")
    };
    let first = build("first.run", 1);
    let second = build("second.run", 2);
    std::fs::write(src.join("n.txt"), "xyz").expect("the embedded file is written again");
    let third = build("third.run", 2);
    let _ = std::fs::remove_dir_all(&stage);
    assert_eq!(answer_of(&first), ply_eval::Value::Int(9));
    assert_eq!(answer_of(&second), ply_eval::Value::Int(10));
    assert_eq!(answer_of(&third), ply_eval::Value::Int(12));
}
