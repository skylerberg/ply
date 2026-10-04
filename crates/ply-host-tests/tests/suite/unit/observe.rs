use ply_eval::host::MachineId;
use ply_host::observe::{self, Binary, Read, World};
use std::path::{Path, PathBuf};

fn module(name: &str) -> Option<String> {
    (name == "std.list").then(|| "list digest".to_string())
}

fn world<'a>(roots: &'a [(String, PathBuf)], binding: &str) -> World<'a> {
    World {
        roots: Some(roots),
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
    let m = MachineId::next();
    let recorder = observe::begin(m);
    observe::read(m, Read::File, &dir.path().join("src/a.ply"));
    observe::read(m, Read::Dir, &dir.path().join("src"));
    observe::read(m, Read::Dir, &dir.path().join("work"));
    observe::wrote(m, &dir.path().join("out"));
    observe::read(m, Read::File, &dir.path().join("out/b.ply"));
    observe::wrote(m, &dir.path().join("work/c.ply"));
    let trace = observe::finished(&recorder, &world(&roots, "b"), false).unwrap();
    observe::end(&recorder);
    // What it wrote is its own, and so is a directory it wrote into, whenever it listed it.
    assert_eq!(lines(&trace), ["dir\tcwd\tsrc", "file\tcwd\tsrc/a.ply"]);
    let probe = MachineId::next();
    assert!(observe::unchanged(&trace, &world(&roots, "b"), probe));
    std::fs::write(dir.path().join("src/b.ply"), b"new").unwrap();
    assert!(!observe::unchanged(&trace, &world(&roots, "b"), probe));
    std::fs::remove_file(dir.path().join("src/b.ply")).unwrap();
    std::fs::write(dir.path().join("src/a.ply"), b"two").unwrap();
    assert!(!observe::unchanged(&trace, &world(&roots, "b"), probe));
}

#[test]
fn a_trace_reads_the_same_from_another_directory_under_the_same_root() {
    let one = tempfile::tempdir().unwrap();
    let other = tempfile::tempdir().unwrap();
    for dir in [one.path(), other.path()] {
        std::fs::write(dir.join("a.ply"), b"same").unwrap();
    }
    let m = MachineId::next();
    let recorder = observe::begin(m);
    observe::read(m, Read::File, &one.path().join("a.ply"));
    let trace = observe::finished(&recorder, &world(&rooted(one.path()), "b"), false).unwrap();
    observe::end(&recorder);
    assert!(observe::unchanged(
        &trace,
        &world(&rooted(other.path()), "b"),
        MachineId::next()
    ));
}

#[test]
fn a_run_that_reached_the_host_stands_for_its_binding_alone() {
    let m = MachineId::next();
    let recorder = observe::begin(m);
    let trace = observe::finished(&recorder, &world(&[], "hosted"), true).unwrap();
    observe::end(&recorder);
    assert_eq!(lines(&trace), ["binding"]);
    assert!(observe::unchanged(&trace, &world(&[], "hosted"), m));
    assert!(!observe::unchanged(&trace, &world(&[], "hermetic"), m));
}

#[test]
fn shipped_modules_and_the_program_are_held_at_the_binary_s_digests() {
    let m = MachineId::next();
    let recorder = observe::begin(m);
    observe::shipped(m, "std.list");
    observe::shipped(m, "std.gone");
    observe::program(m);
    let trace = observe::finished(&recorder, &world(&[], "b"), false).unwrap();
    observe::end(&recorder);
    assert_eq!(
        trace,
        "program\tprogram digest\nshipped\tstd.gone\tnone\nshipped\tstd.list\tlist digest\n"
    );
    assert!(observe::unchanged(&trace, &world(&[], "b"), m));
    let moved = World {
        binary: Binary {
            shipped: &module,
            program: "another program".to_string(),
        },
        ..world(&[], "b")
    };
    assert!(!observe::unchanged(&trace, &moved, m));
}

#[test]
fn an_adopted_machine_is_observed_into_its_parent_until_the_record_ends() {
    let dir = tempfile::tempdir().unwrap();
    let roots = rooted(dir.path());
    let parent = MachineId::next();
    let child = MachineId::next();
    let recorder = observe::begin(parent);
    observe::adopt(child, parent);
    observe::read(child, Read::Kind, &dir.path().join("x"));
    observe::end(&recorder);
    observe::read(child, Read::Kind, &dir.path().join("y"));
    let trace = observe::finished(&recorder, &world(&roots, "b"), false).unwrap();
    assert_eq!(lines(&trace), ["kind\tcwd\tx"]);
}

#[test]
fn a_ply_it_started_reports_into_the_record_and_one_that_never_finished_spoils_it() {
    let dir = tempfile::tempdir().unwrap();
    let roots = rooted(dir.path());
    let m = MachineId::next();
    let recorder = observe::begin(m);
    let file = observe::child_trace(m).unwrap();
    std::fs::write(
        &file,
        format!(
            "read\tfile\t{}\nshipped\tstd.list\tits own digest\nend\n",
            dir.path().join("a.ply").display()
        ),
    )
    .unwrap();
    // A program that is not `ply` never begins its report.
    let _other = observe::child_trace(m).unwrap();
    let trace = observe::finished(&recorder, &world(&roots, "b"), false).unwrap();
    assert_eq!(lines(&trace), ["file\tcwd\ta.ply", "shipped\tstd.list"]);
    // The child's binary answered, not ours.
    assert!(trace.contains("shipped\tstd.list\tits own digest\n"));
    let unfinished = observe::child_trace(m).unwrap();
    std::fs::write(&unfinished, "read\tfile\t/x\n").unwrap();
    assert_eq!(
        observe::finished(&recorder, &world(&roots, "b"), false),
        None
    );
    observe::end(&recorder);
}

#[test]
fn checking_a_trace_enters_what_it_read_into_the_asker_s_record() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.ply"), b"one").unwrap();
    let roots = rooted(dir.path());
    let m = MachineId::next();
    let recorder = observe::begin(m);
    observe::read(m, Read::File, &dir.path().join("a.ply"));
    let trace = observe::finished(&recorder, &world(&roots, "b"), false).unwrap();
    observe::end(&recorder);
    let asker = MachineId::next();
    let asking = observe::begin(asker);
    assert!(observe::unchanged(&trace, &world(&roots, "b"), asker));
    let asked = observe::finished(&asking, &world(&roots, "b"), false).unwrap();
    observe::end(&asking);
    assert_eq!(asked, trace);
}

#[test]
fn a_trace_naming_a_file_is_never_answered_where_no_file_may_be_read() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.ply"), b"one").unwrap();
    let roots = rooted(dir.path());
    let m = MachineId::next();
    let recorder = observe::begin(m);
    observe::read(m, Read::File, &dir.path().join("a.ply"));
    let trace = observe::finished(&recorder, &world(&roots, "b"), false).unwrap();
    observe::end(&recorder);
    let hermetic = World {
        roots: None,
        ..world(&roots, "b")
    };
    assert!(!observe::unchanged(&trace, &hermetic, m));
}
