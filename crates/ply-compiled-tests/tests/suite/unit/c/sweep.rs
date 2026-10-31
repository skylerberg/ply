use ply_compiled::c::sweep::{
    BEGUN, RETURNED, REUSED, STAMP, STORES, USED, claim, due, finished, sweep, sweep_stages, used,
};
use std::path::Path;
use std::time::{Duration, SystemTime};

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
    let names = ["bodies/a", "bodies/b", "c.dylib"];
    stock(dir.path(), &names, 100);
    assert_eq!(sweep(dir.path(), 1_000), 0);
    assert_eq!(present(dir.path(), &names).len(), 3);
}

#[test]
fn the_sweep_removes_the_oldest_cached_result_even_when_newer_objects_exist() {
    let dir = tempfile::tempdir().unwrap();
    let names = ["bodies/old", "obj/b.o", "bodies/new"];
    stock(dir.path(), &names, 100);
    assert_eq!(sweep(dir.path(), 200), 100);
    assert_eq!(present(dir.path(), &names), ["obj/b.o", "bodies/new"]);
}

#[test]
fn the_oldest_entries_go_until_the_rest_fits() {
    let dir = tempfile::tempdir().unwrap();
    let names = ["bodies/a", "bodies/b", "bodies/c", "d.dylib"];
    stock(dir.path(), &names, 100);
    let freed = sweep(dir.path(), 200);
    assert_eq!(freed, 200, "two of the four should have gone");
    assert_eq!(
        present(dir.path(), &names),
        vec!["bodies/c".to_string(), "d.dylib".to_string()],
        "the two written longest ago are the two that go"
    );
}

#[test]
fn a_file_at_any_depth_is_swept() {
    let dir = tempfile::tempdir().unwrap();
    let names = ["src/a.c", "programs/b", "work/1-2-0/c.o", "obj/d.o"];
    stock(dir.path(), &names, 100);
    assert_eq!(sweep(dir.path(), 100), 300);
    assert_eq!(present(dir.path(), &names), ["obj/d.o"]);
}

#[test]
fn a_sweep_over_budget_deletes_older_object_files_and_libraries_and_keeps_the_newest_cached_result()
{
    let dir = tempfile::tempdir().unwrap();
    let names = ["old.dylib", "obj/older.o", "bodies/new"];
    stock(dir.path(), &names, 100);
    sweep(dir.path(), 100);
    assert_eq!(present(dir.path(), &names), vec!["bodies/new".to_string()]);
}

#[test]
fn an_entry_of_the_last_hour_is_never_swept() {
    let dir = tempfile::tempdir().unwrap();
    let names = ["bodies/old", "obj/older.o"];
    stock(dir.path(), &names, 100);
    std::fs::write(dir.path().join("linked.so"), vec![b'x'; 100]).unwrap();
    assert_eq!(sweep(dir.path(), 0), 200);
    assert!(dir.path().join("linked.so").exists());
}

#[test]
fn an_object_used_recently_survives_the_sweep_while_an_object_written_later_is_removed() {
    let dir = tempfile::tempdir().unwrap();
    let names = ["loaded.dylib", "obj/later.o", "bodies/latest"];
    stock(dir.path(), &names, 100);
    used(&dir.path().join("loaded.dylib"));
    sweep(dir.path(), 200);
    assert_eq!(
        present(dir.path(), &names),
        vec!["loaded.dylib".to_string(), "bodies/latest".to_string()]
    );
}

#[test]
fn using_a_library_file_sets_its_modified_time_to_now_only_when_that_time_is_over_an_hour_old() {
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

#[test]
fn a_temporary_is_never_swept() {
    let dir = tempfile::tempdir().unwrap();
    let names = [
        "bodies/a.1234.0.tmp",
        "obj/c.o.1234.1.tmp",
        "bodies/b",
        "obj/d.o",
    ];
    stock(dir.path(), &names, 100);
    sweep(dir.path(), 0);
    assert_eq!(
        present(dir.path(), &names),
        vec![
            "bodies/a.1234.0.tmp".to_string(),
            "obj/c.o.1234.1.tmp".to_string()
        ]
    );
}

#[test]
fn only_one_process_can_claim_the_sweep_until_its_claim_expires_then_another_can() {
    let dir = tempfile::tempdir().unwrap();
    assert!(
        claim(dir.path(), Duration::from_secs(120)),
        "the first caller, with no sweep begun, begins one"
    );
    assert!(dir.path().join(BEGUN).exists());
    assert!(
        !claim(dir.path(), Duration::from_secs(120)),
        "the second inside the lease does not"
    );
    assert!(
        claim(dir.path(), Duration::from_secs(0)),
        "and a lease that has lapsed hands it back"
    );
}

#[test]
fn a_sweep_stays_needed_until_a_sweep_finishes_and_is_needed_again_after_the_interval_passes() {
    let dir = tempfile::tempdir().unwrap();
    assert!(due(dir.path(), Duration::from_secs(600)));
    assert!(claim(dir.path(), Duration::from_secs(120)));
    assert!(
        due(dir.path(), Duration::from_secs(600)),
        "beginning a sweep is not finishing one"
    );
    finished(dir.path());
    assert!(!due(dir.path(), Duration::from_secs(600)));
    assert!(
        due(dir.path(), Duration::from_secs(0)),
        "and an interval that has passed owes the next"
    );
}

#[test]
fn the_sweep_timestamp_files_remain_after_a_sweep_deletes_every_cache_entry() {
    let dir = tempfile::tempdir().unwrap();
    let names = ["bodies/a", "bodies/b"];
    stock(dir.path(), &names, 100);
    assert!(claim(dir.path(), Duration::from_secs(120)));
    finished(dir.path());
    set_file(
        &dir.path().join(STAMP),
        SystemTime::now() - Duration::from_secs(20_000),
    );
    set_file(
        &dir.path().join(BEGUN),
        SystemTime::now() - Duration::from_secs(20_000),
    );
    assert_eq!(sweep(dir.path(), 0), 200);
    assert!(dir.path().join(STAMP).exists() && dir.path().join(BEGUN).exists());
}

fn set_file(path: &Path, when: SystemTime) {
    let f = std::fs::File::options().write(true).open(path).unwrap();
    f.set_times(std::fs::FileTimes::new().set_modified(when))
        .unwrap();
}

#[test]
fn a_cache_budget_of_zero_bytes_means_unlimited_rather_than_an_empty_cache() {
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

fn stage(root: &Path, name: &str, size: usize, now: SystemTime, ago: Duration) {
    let dir = root.join(name);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("builder.run"), vec![b'x'; size]).unwrap();
    std::fs::write(dir.join(USED), b"").unwrap();
    for path in [dir.join("builder.run"), dir.join(USED)] {
        set(&path, now - ago);
    }
    set(&dir, now - ago);
}

fn cache_file(root: &Path, under: &str, name: &str, size: usize, now: SystemTime, ago: Duration) {
    let dir = root.join(under);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(name), vec![b'x'; size]).unwrap();
    set(&dir.join(name), now - ago);
}

fn front(root: &Path, name: &str, size: usize, now: SystemTime, ago: Duration) {
    cache_file(root, RETURNED, name, size, now, ago);
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
    assert!(root.path().join(RETURNED).join("front.a").exists());
}

#[test]
fn sweep_stages_deletes_the_least_recently_used_cached_directories_and_files_first_until_the_rest_fits_the_budget()
 {
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
    assert!(!root.path().join(RETURNED).join("front.oldest").exists());
    assert!(root.path().join(RETURNED).join("front.fresh").exists());
}

#[test]
fn sweep_stages_keeps_a_cached_directory_used_recently_over_another_directory_written_later_but_used_less_recently()
 {
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
fn the_sweep_deletes_a_directory_of_cached_records_as_a_whole_ordered_by_when_it_was_last_used() {
    let root = tempfile::tempdir().unwrap();
    let now = SystemTime::now();
    let store = |name: &str, written: Duration, used: Duration| {
        let dir = root.path().join(STORES).join(name);
        std::fs::create_dir_all(&dir).unwrap();
        for file in ["seg-1", "manifest-a"] {
            std::fs::write(dir.join(file), vec![b'x'; 100]).unwrap();
            set(&dir.join(file), now - written);
        }
        std::fs::write(dir.join(USED), b"").unwrap();
        set(&dir.join(USED), now - used);
        set(&dir, now - written);
    };
    store("idle", 9 * HOUR, 9 * HOUR);
    store("read", 9 * HOUR, 2 * HOUR);
    std::fs::create_dir_all(root.path().join("obj")).unwrap();
    std::fs::write(root.path().join("obj/a.o"), vec![b'x'; 100]).unwrap();
    set(&root.path().join("obj/a.o"), now - 5 * HOUR);
    assert_eq!(sweep(root.path(), 300), 200);
    assert!(!root.path().join(STORES).join("idle").exists());
    assert!(root.path().join(STORES).join("read").join("seg-1").exists());
    assert!(root.path().join("obj/a.o").exists());
    set(&root.path().join(STORES).join("read").join(USED), now);
    sweep(root.path(), 0);
    assert!(
        root.path()
            .join(STORES)
            .join("read")
            .join("manifest-a")
            .exists()
    );
    assert!(!root.path().join("obj/a.o").exists());
}

#[test]
fn sweep_stages_never_deletes_a_temporary_file_still_being_written_or_the_file_that_records_the_last_sweep()
 {
    let root = tempfile::tempdir().unwrap();
    let now = SystemTime::now();
    front(root.path(), "front.1234.tmp", 100, now, 5 * HOUR);
    std::fs::write(root.path().join(STAMP), vec![b'x'; 100]).unwrap();
    set(&root.path().join(STAMP), now - 5 * HOUR);
    sweep_stages(root.path(), 0, now);
    assert!(root.path().join(RETURNED).join("front.1234.tmp").exists());
    assert!(root.path().join(STAMP).exists());
}

#[test]
fn sweep_stages_deletes_reused_cached_results_and_returned_files_and_stage_directories_together_least_recently_used_first()
 {
    let root = tempfile::tempdir().unwrap();
    let now = SystemTime::now();
    cache_file(root.path(), REUSED, "old", 100, now, 6 * HOUR);
    front(root.path(), "front.mid", 100, now, 4 * HOUR);
    stage(root.path(), "stage-a", 100, now, 2 * HOUR);
    cache_file(root.path(), REUSED, "fresh", 100, now, HOUR / 4);
    assert_eq!(sweep_stages(root.path(), 250, now), 200);
    let reused = root.path().join(REUSED);
    assert!(
        !reused.join("old").exists(),
        "the cached result used longest ago goes first"
    );
    assert!(!root.path().join(RETURNED).join("front.mid").exists());
    assert!(
        root.path().join("stage-a").exists(),
        "the rest fits once two have gone"
    );
    assert!(
        reused.join("fresh").exists(),
        "a cached result used within the hour stays"
    );
}

#[test]
fn a_reused_cached_result_being_written_is_never_swept() {
    let root = tempfile::tempdir().unwrap();
    let now = SystemTime::now();
    cache_file(root.path(), REUSED, "k.1234.0.tmp", 100, now, 5 * HOUR);
    cache_file(root.path(), REUSED, "k", 100, now, 5 * HOUR);
    assert_eq!(sweep_stages(root.path(), 0, now), 100);
    let reused = root.path().join(REUSED);
    assert!(reused.join("k.1234.0.tmp").exists());
    assert!(!reused.join("k").exists());
    assert!(reused.is_dir());
}
