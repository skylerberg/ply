use ply_codegen::c::sweep::{ANSWERED, RUNS, STAMP, USED, claim, sweep, sweep_stages, used};
use std::path::Path;
use std::time::{Duration, SystemTime};

/// `n` files of `size` bytes, the first written longest ago.
fn stock(dir: &Path, names: &[&str], size: usize) {
    let base = SystemTime::now() - Duration::from_secs(10_000);
    for (i, name) in names.iter().enumerate() {
        let path = dir.join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, vec![b'x'; size]).unwrap();
        let when = base + Duration::from_secs(i as u64 * 60);
        let f = std::fs::File::options().write(true).open(&path).unwrap();
        f.set_times(std::fs::FileTimes::new().set_modified(when))
            .unwrap();
    }
}

fn present(dir: &Path, names: &[&str]) -> Vec<String> {
    names
        .iter()
        .filter(|n| dir.join(n).exists())
        .map(|n| n.to_string())
        .collect()
}

#[test]
fn a_cache_inside_its_budget_is_left_alone() {
    let dir = tempfile::tempdir().unwrap();
    let names = ["emit/a.body", "emit/b.body", "c.dylib"];
    stock(dir.path(), &names, 100);
    assert_eq!(sweep(dir.path(), 1_000), 0);
    assert_eq!(present(dir.path(), &names).len(), 3);
}

#[test]
fn a_kept_answer_is_swept_with_the_bodies() {
    let dir = tempfile::tempdir().unwrap();
    let names = ["answers/old", "emit/b.body", "answers/new"];
    stock(dir.path(), &names, 100);
    assert_eq!(sweep(dir.path(), 200), 100);
    assert_eq!(present(dir.path(), &names), ["emit/b.body", "answers/new"]);
}

#[test]
fn the_oldest_entries_go_until_the_rest_fits() {
    let dir = tempfile::tempdir().unwrap();
    let names = ["emit/a.body", "emit/b.body", "emit/c.body", "d.dylib"];
    stock(dir.path(), &names, 100);
    // Four hundred bytes, and room for two.
    let freed = sweep(dir.path(), 200);
    assert_eq!(freed, 200, "two of the four should have gone");
    assert_eq!(
        present(dir.path(), &names),
        vec!["emit/c.body".to_string(), "d.dylib".to_string()],
        "the two written longest ago are the two that go"
    );
}

#[test]
fn an_object_is_as_removable_as_a_body() {
    let dir = tempfile::tempdir().unwrap();
    let names = ["old.dylib", "obj/older.o", "emit/new.body"];
    stock(dir.path(), &names, 100);
    sweep(dir.path(), 100);
    assert_eq!(
        present(dir.path(), &names),
        vec!["emit/new.body".to_string()]
    );
}

/// An object every run loads was written once, long ago: its use, not its writing, is its age.
#[test]
fn an_object_used_lately_outlives_one_written_later() {
    let dir = tempfile::tempdir().unwrap();
    let names = ["loaded.dylib", "obj/later.o", "emit/latest.body"];
    stock(dir.path(), &names, 100);
    used(&dir.path().join("loaded.dylib"));
    sweep(dir.path(), 200);
    assert_eq!(
        present(dir.path(), &names),
        vec!["loaded.dylib".to_string(), "emit/latest.body".to_string()]
    );
}

/// The sweep orders entries by hours, so a use minutes after the last one writes nothing.
#[test]
fn a_recent_mark_stands_and_an_old_one_is_renewed() {
    let dir = tempfile::tempdir().unwrap();
    let mark = |name: &str, ago: u64| {
        let path = dir.path().join(name);
        std::fs::write(&path, b"x").unwrap();
        let when = SystemTime::now() - Duration::from_secs(ago);
        let f = std::fs::File::options().write(true).open(&path).unwrap();
        f.set_times(std::fs::FileTimes::new().set_modified(when))
            .unwrap();
        (path, when)
    };
    let modified = |path: &Path| std::fs::metadata(path).unwrap().modified().unwrap();
    let (recent, when) = mark("recent.dylib", 60);
    used(&recent);
    assert_eq!(modified(&recent), when);
    let (old, when) = mark("old.dylib", 7_200);
    used(&old);
    assert!(modified(&old) > when);
}

/// A half-written entry belongs to a run still in progress.
#[test]
fn a_temporary_is_never_swept() {
    let dir = tempfile::tempdir().unwrap();
    let names = [
        "emit/a.body.1234.0.tmp",
        "obj/c.o.1234.1.tmp",
        "emit/b.body",
        "obj/d.o",
    ];
    stock(dir.path(), &names, 100);
    sweep(dir.path(), 0);
    assert_eq!(
        present(dir.path(), &names),
        vec![
            "emit/a.body.1234.0.tmp".to_string(),
            "obj/c.o.1234.1.tmp".to_string()
        ]
    );
}

#[test]
fn one_caller_an_interval_sweeps_and_the_rest_do_not() {
    let dir = tempfile::tempdir().unwrap();
    assert!(
        claim(dir.path(), Duration::from_secs(600)),
        "the first caller, with no stamp, sweeps"
    );
    assert!(
        !claim(dir.path(), Duration::from_secs(600)),
        "the second inside the interval does not"
    );
    assert!(
        claim(dir.path(), Duration::from_secs(0)),
        "and an interval that has passed hands it back"
    );
}

/// So two processes starting together do not both walk.
#[test]
fn the_stamp_is_marked_by_taking_the_claim_not_by_finishing_the_sweep() {
    let dir = tempfile::tempdir().unwrap();
    assert!(claim(dir.path(), Duration::from_secs(600)));
    assert!(
        dir.path().join(STAMP).exists(),
        "the stamp is there before any entry has been removed"
    );
}

/// A sweep that removed it would hand the claim to the next process a second later.
#[test]
fn the_stamp_survives_a_sweep_that_empties_the_cache() {
    let dir = tempfile::tempdir().unwrap();
    let names = ["emit/a.body", "emit/b.body"];
    stock(dir.path(), &names, 100);
    assert!(claim(dir.path(), Duration::from_secs(600)));
    sweep(dir.path(), 0);
    assert!(
        !claim(dir.path(), Duration::from_secs(600)),
        "the stamp is younger than the interval, so the next caller still skips"
    );
}

#[test]
fn a_budget_of_zero_bytes_is_no_bound_rather_than_an_empty_cache() {
    // The env var is process-wide, so this asserts the parse rather than setting it.
    assert_eq!(
        "0".parse::<u64>().ok().filter(|n| *n > 0),
        None,
        "`PLY_C_CACHE_MAX=0` is the unbounded spelling"
    );
}

fn set(path: &Path, when: SystemTime) {
    let f = if path.is_dir() {
        std::fs::File::open(path).unwrap()
    } else {
        std::fs::File::options().write(true).open(path).unwrap()
    };
    f.set_times(std::fs::FileTimes::new().set_modified(when))
        .unwrap();
}

/// A stage directory as a run leaves it: `size` bytes of unit, last used `ago` before `now`.
fn stage(root: &Path, name: &str, size: usize, now: SystemTime, ago: Duration) {
    let dir = root.join(name);
    std::fs::create_dir_all(dir.join("shelf")).unwrap();
    std::fs::write(dir.join("shelf").join("unit.c.gz"), vec![b'x'; size]).unwrap();
    std::fs::write(dir.join(USED), b"").unwrap();
    for path in [
        dir.join("shelf").join("unit.c.gz"),
        dir.join("shelf"),
        dir.join(USED),
    ] {
        set(&path, now - ago);
    }
    set(&dir, now - ago);
}

/// A cached front of `size` bytes under the stage-directory entry `under`, last used `ago` before
/// `now`.
fn filed(root: &Path, under: &str, name: &str, size: usize, now: SystemTime, ago: Duration) {
    let dir = root.join(under);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(name), vec![b'x'; size]).unwrap();
    set(&dir.join(name), now - ago);
}

/// An opened artifact's cached front.
fn front(root: &Path, name: &str, size: usize, now: SystemTime, ago: Duration) {
    filed(root, ANSWERED, name, size, now, ago);
}

const HOUR: Duration = Duration::from_secs(3600);

#[test]
fn a_stage_directory_inside_its_budget_is_left_alone() {
    let root = tempfile::tempdir().unwrap();
    let now = SystemTime::now();
    stage(root.path(), "stage-a", 100, now, 5 * HOUR);
    front(root.path(), "front.a", 100, now, 5 * HOUR);
    assert_eq!(sweep_stages(root.path(), 1_000, now), 0);
    assert!(root.path().join("stage-a").exists());
    assert!(root.path().join(ANSWERED).join("front.a").exists());
}

/// A stage goes whole and a front goes on its own, least recently used first, and whatever a run
/// used within the hour stays however far over budget the rest is.
#[test]
fn stages_and_fronts_go_least_recently_used_first() {
    let root = tempfile::tempdir().unwrap();
    let now = SystemTime::now();
    front(root.path(), "front.oldest", 100, now, 5 * HOUR);
    stage(root.path(), "stage-old", 100, now, 3 * HOUR);
    stage(root.path(), "stage-mid", 100, now, 2 * HOUR);
    stage(root.path(), "stage-fresh", 100, now, HOUR / 6);
    front(root.path(), "front.fresh", 100, now, HOUR / 2);
    assert_eq!(sweep_stages(root.path(), 250, now), 300);
    let left: Vec<bool> = ["stage-old", "stage-mid", "stage-fresh"]
        .iter()
        .map(|n| root.path().join(n).exists())
        .collect();
    assert_eq!(left, vec![false, false, true]);
    assert!(!root.path().join(ANSWERED).join("front.oldest").exists());
    assert!(root.path().join(ANSWERED).join("front.fresh").exists());
}

/// A stage is written once and used by every run after, so its use, not its writing, is its age.
#[test]
fn a_stage_used_lately_outlives_one_written_later() {
    let root = tempfile::tempdir().unwrap();
    let now = SystemTime::now();
    stage(root.path(), "written-early", 100, now, 9 * HOUR);
    stage(root.path(), "written-late", 100, now, 3 * HOUR);
    set(
        &root.path().join("written-early").join(USED),
        now - 2 * HOUR,
    );
    sweep_stages(root.path(), 100, now);
    assert!(root.path().join("written-early").exists());
    assert!(!root.path().join("written-late").exists());
}

#[test]
fn a_front_being_written_and_a_file_beside_the_stages_are_never_swept() {
    let root = tempfile::tempdir().unwrap();
    let now = SystemTime::now();
    front(root.path(), "front.1234.tmp", 100, now, 5 * HOUR);
    std::fs::write(root.path().join(STAMP), vec![b'x'; 100]).unwrap();
    set(&root.path().join(STAMP), now - 5 * HOUR);
    sweep_stages(root.path(), 0, now);
    assert!(root.path().join(ANSWERED).join("front.1234.tmp").exists());
    assert!(root.path().join(STAMP).exists());
}

/// A closure `ply run` filed goes on its own, like an artifact's front, and never as one stage
/// holding every run: the least recently used go first, and the directory stays.
#[test]
fn a_runs_front_goes_on_its_own_least_recently_used_first() {
    let root = tempfile::tempdir().unwrap();
    let now = SystemTime::now();
    filed(root.path(), RUNS, "old", 100, now, 6 * HOUR);
    front(root.path(), "front.mid", 100, now, 4 * HOUR);
    stage(root.path(), "stage-a", 100, now, 2 * HOUR);
    filed(root.path(), RUNS, "fresh", 100, now, HOUR / 4);
    assert_eq!(sweep_stages(root.path(), 250, now), 200);
    let runs = root.path().join(RUNS);
    assert!(
        !runs.join("old").exists(),
        "the run used longest ago goes first"
    );
    assert!(!root.path().join(ANSWERED).join("front.mid").exists());
    assert!(
        root.path().join("stage-a").exists(),
        "the rest fits once two have gone"
    );
    assert!(
        runs.join("fresh").exists(),
        "a run used within the hour stays"
    );
}

/// A run's front half written belongs to a run still writing it, and the directory is no stage to
/// remove whole however far over budget its files are.
#[test]
fn a_runs_front_being_written_is_never_swept() {
    let root = tempfile::tempdir().unwrap();
    let now = SystemTime::now();
    filed(root.path(), RUNS, "k.1234.0.tmp", 100, now, 5 * HOUR);
    filed(root.path(), RUNS, "k", 100, now, 5 * HOUR);
    assert_eq!(sweep_stages(root.path(), 0, now), 100);
    let runs = root.path().join(RUNS);
    assert!(runs.join("k.1234.0.tmp").exists());
    assert!(!runs.join("k").exists());
    assert!(runs.is_dir());
}
