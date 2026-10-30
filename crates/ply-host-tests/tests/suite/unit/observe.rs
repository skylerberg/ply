use ply_host::observe::{self, Binary, Read, Recorder, World};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// A run given no configuration.
static UNCONFIGURED: BTreeMap<String, String> = BTreeMap::new();

fn module(name: &str) -> Option<String> {
    match name {
        "std.list" => Some("list digest".to_string()),
        "std/words/list.txt" => Some("words digest".to_string()),
        _ => None,
    }
}

fn world<'a>(roots: &'a [(String, PathBuf)], binding: &str) -> World<'a> {
    configured(roots, binding, &UNCONFIGURED)
}

fn configured<'a>(
    roots: &'a [(String, PathBuf)],
    binding: &str,
    config: &'a BTreeMap<String, String>,
) -> World<'a> {
    World {
        roots,
        binding: binding.to_string(),
        config,
        binary: Binary {
            shipped: &module,
            program: "program digest".to_string(),
        },
    }
}

fn rooted(dir: &Path) -> Vec<(String, PathBuf)> {
    vec![("cwd".to_string(), dir.to_path_buf())]
}

fn stands(trace: &str, world: &World<'_>) -> bool {
    observe::moved(trace, world, None).is_none()
}

fn moved(trace: &str, world: &World<'_>) -> bool {
    observe::moved(trace, world, None).is_some()
}

fn lines(trace: &str) -> Vec<&str> {
    trace
        .lines()
        .map(|line| line.rsplit_once('\t').map_or(line, |(at, _)| at))
        .collect()
}

#[test]
fn a_run_is_traced_by_what_it_read_under_the_root_that_holds_it() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("src")).unwrap();
    std::fs::write(dir.path().join("src/a.ply"), b"one").unwrap();
    let roots = rooted(dir.path());
    let recorder = Recorder::default();
    recorder.read(Read::File, &dir.path().join("src/a.ply"));
    recorder.read(Read::Dir, &dir.path().join("src"));
    recorder.wrote(&dir.path().join("out"));
    recorder.read(Read::File, &dir.path().join("out/b.ply"));
    let trace = observe::finished(&recorder, &world(&roots, "b"), false).unwrap();
    // What it wrote is its own: reading it back is no input.
    assert_eq!(lines(&trace), ["dir\tcwd\tsrc\t", "file\tcwd\tsrc/a.ply\t"]);
    assert!(stands(&trace, &world(&roots, "b")));
    std::fs::write(dir.path().join("src/b.ply"), b"new").unwrap();
    assert!(moved(&trace, &world(&roots, "b")));
    std::fs::remove_file(dir.path().join("src/b.ply")).unwrap();
    std::fs::write(dir.path().join("src/a.ply"), b"two").unwrap();
    assert_eq!(
        observe::moved(&trace, &world(&roots, "b"), None),
        Some("the file `src/a.ply` under `cwd`".to_string())
    );
}

#[test]
fn a_listing_moves_when_a_name_comes_to_name_something_else() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("src")).unwrap();
    std::fs::write(dir.path().join("src/a"), b"one").unwrap();
    let roots = rooted(dir.path());
    let recorder = Recorder::default();
    recorder.read(Read::Dir, &dir.path().join("src"));
    let trace = observe::finished(&recorder, &world(&roots, "b"), false).unwrap();
    assert!(stands(&trace, &world(&roots, "b")));
    std::fs::remove_file(dir.path().join("src/a")).unwrap();
    std::fs::create_dir(dir.path().join("src/a")).unwrap();
    assert!(moved(&trace, &world(&roots, "b")));
}

#[test]
fn a_directory_the_test_wrote_into_is_read_without_what_it_wrote() {
    let dir = tempfile::tempdir().unwrap();
    let pkg = dir.path().join("pkg");
    std::fs::create_dir(&pkg).unwrap();
    std::fs::write(pkg.join("a.ply"), b"a").unwrap();
    std::fs::write(pkg.join("left over"), b"by another test").unwrap();
    let roots = rooted(dir.path());
    let recorder = Recorder::default();
    // Listed, emptied of what was left there, and written into.
    recorder.read(Read::Dir, &pkg);
    recorder.read(Read::Tree, &pkg);
    std::fs::remove_file(pkg.join("left over")).unwrap();
    recorder.wrote(&pkg.join("left over"));
    std::fs::create_dir(pkg.join(".cache")).unwrap();
    std::fs::write(pkg.join(".cache/entry"), b"kept").unwrap();
    recorder.wrote(&pkg.join(".cache/entry"));
    let trace = observe::finished(&recorder, &world(&roots, "b"), false).unwrap();
    assert_eq!(
        lines(&trace),
        [
            "dir\tcwd\tpkg\t.cache\u{1f}left over",
            "tree\tcwd\tpkg\t.cache/entry\u{1f}left over"
        ]
    );
    // Another run leaves something else there, which this one would have removed.
    std::fs::write(pkg.join("left over"), b"by yet another").unwrap();
    std::fs::write(pkg.join(".cache/entry"), b"moved").unwrap();
    assert!(stands(&trace, &world(&roots, "b")));
    // A module added beside the ones it read is not its own.
    std::fs::write(pkg.join("b.ply"), b"b").unwrap();
    assert!(moved(&trace, &world(&roots, "b")));
}

#[test]
fn a_trace_reads_the_same_from_another_directory_under_the_same_root() {
    let one = tempfile::tempdir().unwrap();
    let other = tempfile::tempdir().unwrap();
    for dir in [one.path(), other.path()] {
        std::fs::write(dir.join("a.ply"), b"same").unwrap();
    }
    let recorder = Recorder::default();
    recorder.read(Read::File, &one.path().join("a.ply"));
    let trace = observe::finished(&recorder, &world(&rooted(one.path()), "b"), false).unwrap();
    assert!(stands(&trace, &world(&rooted(other.path()), "b")));
}

#[test]
fn a_run_that_reached_the_host_stands_for_its_binding_alone() {
    let recorder = Recorder::default();
    let trace = observe::finished(&recorder, &world(&[], "lent"), true).unwrap();
    assert_eq!(lines(&trace), ["binding"]);
    assert!(stands(&trace, &world(&[], "lent")));
    assert!(moved(&trace, &world(&[], "lent another root")));
}

fn values(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

#[test]
fn a_configuration_read_stands_while_its_key_answers_as_it_did() {
    let recorder = Recorder::default();
    recorder.configured("SERVICE_REGION", Some("eu"));
    recorder.configured("SERVICE_ABSENT", None);
    let eu = values(&[("SERVICE_REGION", "eu"), ("UNREAD", "one")]);
    let trace = observe::finished(&recorder, &configured(&[], "b", &eu), false).unwrap();
    assert_eq!(
        lines(&trace),
        ["config\tSERVICE_ABSENT", "config\tSERVICE_REGION"]
    );
    // A key the run never read moves nothing.
    let other_unread = values(&[("SERVICE_REGION", "eu"), ("UNREAD", "two")]);
    assert!(stands(&trace, &configured(&[], "b", &other_unread)));
    let us = values(&[("SERVICE_REGION", "us")]);
    assert_eq!(
        observe::moved(&trace, &configured(&[], "b", &us), None),
        Some("the configuration key `SERVICE_REGION`".to_string())
    );
    let supplied = values(&[("SERVICE_REGION", "eu"), ("SERVICE_ABSENT", "now")]);
    assert_eq!(
        observe::moved(&trace, &configured(&[], "b", &supplied), None),
        Some("the configuration key `SERVICE_ABSENT`".to_string())
    );
}

#[test]
fn a_run_that_read_a_secret_files_no_trace() {
    let recorder = Recorder::default();
    recorder.configured("SERVICE_REGION", Some("eu"));
    recorder.read_secret("SERVICE_API_KEY");
    assert_eq!(observe::finished(&recorder, &world(&[], "b"), false), None);
}

#[test]
fn a_nested_run_s_configuration_is_its_caller_s_handing_and_no_read_of_the_caller_s() {
    let inner = Recorder::default();
    inner.configured("SERVICE_REGION", Some("handed"));
    inner.read_secret("SERVICE_API_KEY");
    let outer = Recorder::default();
    outer.absorb(&inner);
    assert_eq!(
        observe::finished(&outer, &world(&[], "b"), false),
        Some(String::new())
    );
}

#[test]
fn shipped_modules_and_the_program_are_held_at_the_binary_s_digests() {
    let recorder = Recorder::default();
    recorder.shipped("std.list");
    recorder.shipped("std.gone");
    recorder.ran_program();
    let trace = observe::finished(&recorder, &world(&[], "b"), false).unwrap();
    assert_eq!(
        trace,
        "program\tprogram digest\nshipped\tstd.gone\tnone\nshipped\tstd.list\tlist digest\n"
    );
    assert!(stands(&trace, &world(&[], "b")));
    let another = World {
        binary: Binary {
            shipped: &module,
            program: "another program".to_string(),
        },
        ..world(&[], "b")
    };
    assert_eq!(
        observe::moved(&trace, &another, None),
        Some("the `ply` program".to_string())
    );
}

/// The only test that lays the shipped modules out: where they lie is the process's to set once.
#[test]
fn a_read_through_a_resolved_root_is_of_the_shipped_modules_laid_out_behind_a_link() {
    let dir = tempfile::tempdir().unwrap();
    let real = dir.path().join("real");
    std::fs::create_dir_all(real.join("shipped")).unwrap();
    std::os::unix::fs::symlink(&real, dir.path().join("link")).unwrap();
    observe::laid_out(&dir.path().join("link/shipped"), Vec::new());
    let real = real.canonicalize().unwrap();
    let recorder = Recorder::default();
    recorder.read(Read::File, &real.join("shipped/std.list.ply"));
    let trace = observe::finished(&recorder, &world(&[], "b"), false).unwrap();
    assert_eq!(trace, "shipped\tstd.list\tlist digest\n");
    // A data file a module embeds lies below them under its name, and no view stands for it.
    recorder.read(Read::File, &real.join("shipped/std/words/list.txt"));
    recorder.reached(VIEW, true);
    let embedding = observe::finished(&recorder, &world(&[], "b"), false).unwrap();
    assert_eq!(
        embedding,
        "shipdef\tstd.list\tstd.list.take\thash\nshipform\tstd.list\tlist digest\tform\nshipped\tstd/words/list.txt\twords digest\n"
    );
    // What lies beside them is the program's.
    recorder.read(Read::File, &real.join("shipped/stamps"));
    let stamped = observe::finished(&recorder, &world(&[], "b"), false).unwrap();
    assert!(
        stamped.starts_with("program\tprogram digest\n"),
        "{stamped}"
    );
}

const VIEW: &str =
    "shipform\tstd.list\tlist digest\tform\nshipdef\tstd.list\tstd.list.take\thash\n";

#[test]
fn a_view_stands_for_one_whole_read_of_the_modules_it_names() {
    let recorder = Recorder::default();
    recorder.shipped("std.list");
    recorder.shipped("std.list");
    recorder.reached(VIEW, true);
    // Never read of the binary: a caller handed the load its own text.
    recorder.reached("shipform\tstd.handed\tt\tf\n", true);
    let trace = observe::finished(&recorder, &world(&[], "b"), false).unwrap();
    // The second read had no view of its own, so it holds the whole module.
    assert_eq!(
        trace,
        "shipdef\tstd.list\tstd.list.take\thash\nshipform\tstd.list\tlist digest\tform\nshipped\tstd.list\tlist digest\n"
    );
    recorder.reached(VIEW, true);
    let viewed = observe::finished(&recorder, &world(&[], "b"), false).unwrap();
    assert!(!viewed.contains("shipped\t"));
}

#[test]
fn a_view_of_a_load_already_read_adds_to_what_its_view_holds() {
    let recorder = Recorder::default();
    recorder.shipped("std.list");
    recorder.reached(VIEW, true);
    recorder.reached(
        "shipform\tstd.list\tlist digest\tform\nshipdef\tstd.list\tstd.list.drop\thash\n",
        false,
    );
    let trace = observe::finished(&recorder, &world(&[], "b"), false).unwrap();
    assert_eq!(
        lines(&trace),
        [
            "shipdef\tstd.list\tstd.list.drop",
            "shipdef\tstd.list\tstd.list.take",
            "shipform\tstd.list\tlist digest",
        ]
    );
}

#[test]
fn a_view_stands_here_while_its_module_s_text_does() {
    assert!(stands(VIEW, &world(&[], "b")));
    let moved_list = VIEW.replace("list digest", "another digest");
    assert_eq!(
        observe::moved(&moved_list, &world(&[], "b"), None),
        Some("the shipped module `std.list`".to_string())
    );
}

#[test]
fn a_ply_it_started_reports_its_views() {
    let recorder = Recorder::default();
    let file = recorder.child_trace();
    std::fs::write(&file, format!("{VIEW}end\n")).unwrap();
    let trace = observe::finished(&recorder, &world(&[], "b"), false).unwrap();
    assert_eq!(
        trace,
        VIEW.lines()
            .rev()
            .map(|l| format!("{l}\n"))
            .collect::<String>()
    );
}

/// The record is the run's: what a host handler of it reads goes there, a run made on the test's
/// behalf shares it, and a run with none of its own records nothing.
#[test]
fn a_run_records_into_the_record_it_was_handed_and_one_handed_none_records_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let roots = rooted(dir.path());
    let host = ply_host::Host::new();
    let (test, nested, other) = (host.runtime(), host.runtime(), host.runtime());
    assert!(
        observe::of(&*test).is_none(),
        "a run observes nothing until told to"
    );
    let recorder = Arc::new(Recorder::default());
    observe::records_into(&*test, &recorder);
    observe::records_into(&*nested, &observe::of(&*test).expect("the test's record"));
    for (runtime, path) in [(&test, "x"), (&nested, "y")] {
        observe::of(&**runtime)
            .expect("observed")
            .read(Read::Kind, &dir.path().join(path));
    }
    assert!(observe::of(&*other).is_none(), "another run kept no record");
    let trace = observe::finished(&recorder, &world(&roots, "b"), false).unwrap();
    assert_eq!(lines(&trace), ["kind\tcwd\tx\t", "kind\tcwd\ty\t"]);
}

#[test]
fn a_test_a_test_ran_is_read_by_the_outer_one_too() {
    let dir = tempfile::tempdir().unwrap();
    let roots = rooted(dir.path());
    let (outer, inner) = (Recorder::default(), Recorder::default());
    inner.read(Read::Kind, &dir.path().join("inner"));
    outer.read(Read::Kind, &dir.path().join("outer"));
    outer.absorb(&inner);
    outer.absorb(&outer);
    let trace = observe::finished(&outer, &world(&roots, "b"), false).unwrap();
    assert_eq!(lines(&trace), ["kind\tcwd\tinner\t", "kind\tcwd\touter\t"]);
}

#[test]
fn a_ply_it_started_reports_into_the_record_and_one_that_never_finished_spoils_it() {
    let dir = tempfile::tempdir().unwrap();
    let roots = rooted(dir.path());
    let recorder = Recorder::default();
    let file = recorder.child_trace();
    std::fs::write(
        &file,
        format!(
            "read\tfile\t{}\nshipped\tstd.list\tits own digest\nend\n",
            dir.path().join("a.ply").display()
        ),
    )
    .unwrap();
    // A program that is not `ply` never begins its report.
    let _other = recorder.child_trace();
    let trace = observe::finished(&recorder, &world(&roots, "b"), false).unwrap();
    assert_eq!(lines(&trace), ["file\tcwd\ta.ply\t", "shipped\tstd.list"]);
    // The child's binary responded, not ours.
    assert!(trace.contains("shipped\tstd.list\tits own digest\n"));
    let unfinished = recorder.child_trace();
    std::fs::write(&unfinished, "read\tfile\t/x\n").unwrap();
    assert_eq!(
        observe::finished(&recorder, &world(&roots, "b"), false),
        None
    );
}

#[test]
fn checking_a_trace_enters_what_it_read_into_the_asker_s_record() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.ply"), b"one").unwrap();
    let roots = rooted(dir.path());
    let recorder = Recorder::default();
    recorder.read(Read::File, &dir.path().join("a.ply"));
    let trace = observe::finished(&recorder, &world(&roots, "b"), false).unwrap();
    let asking = Recorder::default();
    assert!(observe::moved(&trace, &world(&roots, "b"), Some(&asking)).is_none());
    let asked = observe::finished(&asking, &world(&roots, "b"), false).unwrap();
    assert_eq!(asked, trace);
}

#[test]
fn a_trace_naming_a_root_the_run_is_not_lent_is_never_answered() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.ply"), b"one").unwrap();
    let roots = rooted(dir.path());
    let recorder = Recorder::default();
    recorder.read(Read::File, &dir.path().join("a.ply"));
    let trace = observe::finished(&recorder, &world(&roots, "b"), false).unwrap();
    assert!(moved(&trace, &world(&[], "b")));
}

#[test]
fn a_front_end_s_store_is_neither_read_nor_listed() {
    let dir = tempfile::tempdir().unwrap();
    let pkg = dir.path().join("pkg");
    std::fs::create_dir(&pkg).unwrap();
    std::fs::write(pkg.join("a.ply"), b"a").unwrap();
    let roots = rooted(dir.path());
    let recorder = Recorder::default();
    recorder.read(Read::Dir, &pkg);
    recorder.read(Read::Tree, &pkg);
    recorder.read(Read::File, &pkg.join(".ply-cache/index"));
    let trace = observe::finished(&recorder, &world(&roots, "b"), false).unwrap();
    assert_eq!(lines(&trace), ["dir\tcwd\tpkg\t", "tree\tcwd\tpkg\t"]);
    // Another test's store appearing beside the sources moves nothing.
    std::fs::create_dir(pkg.join(".ply-cache")).unwrap();
    std::fs::write(pkg.join(".ply-cache/index"), b"filed").unwrap();
    assert!(stands(&trace, &world(&roots, "b")));
}

#[test]
fn a_checkout_s_version_control_is_no_input() {
    let dir = tempfile::tempdir().unwrap();
    let recorder = Recorder::default();
    recorder.read(Read::File, &dir.path().join(".git/HEAD"));
    recorder.read(Read::Kind, &dir.path().join("pkg/.git"));
    let trace = observe::finished(&recorder, &world(&rooted(dir.path()), "b"), false).unwrap();
    assert_eq!(trace, "");
}
