use ply_host::observe::{self, Binary, Read, Recorder, World};
use std::path::{Path, PathBuf};
use std::sync::Arc;

fn module(name: &str) -> Option<String> {
    match name {
        "std.list" => Some("list digest".to_string()),
        "std/words/list.txt" => Some("words digest".to_string()),
        _ => None,
    }
}

fn world<'a>(roots: &'a [(String, PathBuf)], binding: &str) -> World<'a> {
    World {
        roots,
        binding: binding.to_string(),
        binary: Binary {
            shipped: &module,
            program: "program digest".to_string(),
        },
    }
}

fn rooted(dir: &Path) -> Vec<(String, PathBuf)> {
    vec![("cwd".to_string(), dir.to_path_buf())]
}

fn traced(recorder: &Recorder, world: &World<'_>) -> String {
    observe::finished(recorder, world, false).unwrap()
}

fn lines(trace: &str) -> Vec<&str> {
    trace
        .lines()
        .map(|line| line.rsplit_once('\t').map_or(line, |(at, _)| at))
        .collect()
}

#[test]
fn a_run_s_trace_lists_only_what_it_read_under_the_roots_and_ignores_files_it_wrote() {
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
    assert_eq!(lines(&trace), ["dir\tcwd\tsrc\t", "file\tcwd\tsrc/a.ply\t"]);
}

#[test]
fn a_directory_listing_in_a_trace_changes_when_an_entry_is_replaced_by_one_of_another_kind() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("src")).unwrap();
    std::fs::write(dir.path().join("src/a"), b"one").unwrap();
    let roots = rooted(dir.path());
    let recorder = Recorder::default();
    recorder.read(Read::Dir, &dir.path().join("src"));
    let trace = traced(&recorder, &world(&roots, "b"));
    std::fs::remove_file(dir.path().join("src/a")).unwrap();
    std::fs::create_dir(dir.path().join("src/a")).unwrap();
    assert_ne!(traced(&recorder, &world(&roots, "b")), trace);
}

#[test]
fn a_trace_of_a_directory_excludes_entries_that_the_run_itself_wrote() {
    let dir = tempfile::tempdir().unwrap();
    let pkg = dir.path().join("pkg");
    std::fs::create_dir(&pkg).unwrap();
    std::fs::write(pkg.join("a.ply"), b"a").unwrap();
    std::fs::write(pkg.join("left over"), b"by another test").unwrap();
    let roots = rooted(dir.path());
    let recorder = Recorder::default();
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
    std::fs::write(pkg.join("left over"), b"by yet another").unwrap();
    std::fs::write(pkg.join(".cache/entry"), b"changed").unwrap();
    assert_eq!(traced(&recorder, &world(&roots, "b")), trace);
    std::fs::write(pkg.join("b.ply"), b"b").unwrap();
    assert_ne!(traced(&recorder, &world(&roots, "b")), trace);
}

#[test]
fn a_trace_is_the_same_in_another_directory_with_the_same_contents_under_the_same_root_name() {
    let one = tempfile::tempdir().unwrap();
    let other = tempfile::tempdir().unwrap();
    let trace_in = |dir: &Path| {
        std::fs::write(dir.join("a.ply"), b"same").unwrap();
        let recorder = Recorder::default();
        recorder.read(Read::File, &dir.join("a.ply"));
        traced(&recorder, &world(&rooted(dir), "b"))
    };
    assert_eq!(trace_in(one.path()), trace_in(other.path()));
}

#[test]
fn a_trace_of_a_run_that_used_the_host_contains_only_the_binding_line_and_changes_when_the_binding_changes()
 {
    let recorder = Recorder::default();
    let trace = observe::finished(&recorder, &world(&[], "lent"), true).unwrap();
    assert_eq!(lines(&trace), ["binding"]);
    let rebound = observe::finished(&recorder, &world(&[], "lent another root"), true).unwrap();
    assert_ne!(rebound, trace);
}

#[test]
fn a_trace_of_configuration_reads_lists_each_key_read_and_changes_with_a_value_read() {
    let configured = |region: Option<&str>| {
        let recorder = Recorder::default();
        recorder.configured("SERVICE_REGION", region);
        recorder.configured("SERVICE_ABSENT", None);
        traced(&recorder, &world(&[], "b"))
    };
    let trace = configured(Some("eu"));
    assert_eq!(
        lines(&trace),
        ["config\tSERVICE_ABSENT", "config\tSERVICE_REGION"]
    );
    assert_eq!(configured(Some("eu")), trace);
    assert_ne!(configured(Some("us")), trace);
    assert_ne!(configured(None), trace);
}

#[test]
fn a_run_that_read_a_secret_produces_no_trace() {
    let recorder = Recorder::default();
    recorder.configured("SERVICE_REGION", Some("eu"));
    recorder.read_secret("SERVICE_API_KEY");
    assert_eq!(observe::finished(&recorder, &world(&[], "b"), false), None);
}

#[test]
fn a_nested_run_s_configuration_and_secret_reads_are_not_added_to_the_caller_s_trace() {
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
fn a_trace_records_the_digests_of_the_program_and_the_standard_library_modules_it_used() {
    let recorder = Recorder::default();
    recorder.shipped("std.list");
    recorder.shipped("std.gone");
    recorder.ran_program();
    let trace = observe::finished(&recorder, &world(&[], "b"), false).unwrap();
    assert_eq!(
        trace,
        "program\tprogram digest\nshipped\tstd.gone\tnone\nshipped\tstd.list\tlist digest\n"
    );
    let another = World {
        binary: Binary {
            shipped: &module,
            program: "another program".to_string(),
        },
        ..world(&[], "b")
    };
    assert_ne!(traced(&recorder, &another), trace);
}

#[test]
fn reading_the_standard_library_modules_through_a_symbolic_link_to_their_directory_is_traced_as_library_reads()
 {
    let dir = tempfile::tempdir().unwrap();
    let real = dir.path().join("real");
    std::fs::create_dir_all(real.join("shipped")).unwrap();
    std::os::unix::fs::symlink(&real, dir.path().join("link")).unwrap();
    observe::set_shipped_and_cache_dirs(&dir.path().join("link/shipped"), Vec::new());
    let real = real.canonicalize().unwrap();
    let recorder = Recorder::default();
    recorder.read(Read::File, &real.join("shipped/std.list.ply"));
    let trace = observe::finished(&recorder, &world(&[], "b"), false).unwrap();
    assert_eq!(trace, "shipped\tstd.list\tlist digest\n");
    recorder.read(Read::File, &real.join("shipped/std/words/list.txt"));
    let embedding = observe::finished(&recorder, &world(&[], "b"), false).unwrap();
    assert_eq!(
        embedding,
        "shipped\tstd.list\tlist digest\nshipped\tstd/words/list.txt\twords digest\n"
    );
    recorder.read(Read::File, &real.join("shipped/stamps"));
    let stamped = observe::finished(&recorder, &world(&[], "b"), false).unwrap();
    assert!(
        stamped.starts_with("program\tprogram digest\n"),
        "{stamped}"
    );
}

#[test]
fn a_run_records_reads_into_the_recorder_it_is_given_and_a_run_with_no_recorder_records_no_reads() {
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
fn a_recorder_that_absorbs_another_recorder_includes_the_reads_of_the_other_recorder_in_its_trace()
{
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
fn a_finished_child_ply_trace_is_merged_into_the_recorder_trace_and_an_unfinished_child_trace_makes_the_trace_invalid()
 {
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
    let _other = recorder.child_trace();
    let trace = observe::finished(&recorder, &world(&roots, "b"), false).unwrap();
    assert_eq!(lines(&trace), ["file\tcwd\ta.ply\t", "shipped\tstd.list"]);
    assert!(trace.contains("shipped\tstd.list\tits own digest\n"));
    let unfinished = recorder.child_trace();
    std::fs::write(&unfinished, "read\tfile\t/x\n").unwrap();
    assert_eq!(
        observe::finished(&recorder, &world(&roots, "b"), false),
        None
    );
}

#[test]
fn the_cache_directory_of_a_ply_front_end_is_not_recorded_as_read_in_a_trace() {
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
    std::fs::create_dir(pkg.join(".ply-cache")).unwrap();
    std::fs::write(pkg.join(".ply-cache/index"), b"filed").unwrap();
    assert_eq!(traced(&recorder, &world(&roots, "b")), trace);
}

#[test]
fn reads_of_git_metadata_in_a_checkout_are_not_recorded_in_a_trace() {
    let dir = tempfile::tempdir().unwrap();
    let recorder = Recorder::default();
    recorder.read(Read::File, &dir.path().join(".git/HEAD"));
    recorder.read(Read::Kind, &dir.path().join("pkg/.git"));
    let trace = observe::finished(&recorder, &world(&rooted(dir.path()), "b"), false).unwrap();
    assert_eq!(trace, "");
}
