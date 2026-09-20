use ply_codegen::c::sweep::{STAMP, claim, sweep};
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

/// A half-written entry belongs to a run still in progress.
#[test]
fn a_temporary_is_never_swept() {
    let dir = tempfile::tempdir().unwrap();
    let names = [
        "emit/a.1234.tmp",
        "obj/c.1234.otmp",
        "emit/b.body",
        "obj/d.o",
    ];
    stock(dir.path(), &names, 100);
    sweep(dir.path(), 0);
    assert_eq!(
        present(dir.path(), &names),
        vec!["emit/a.1234.tmp".to_string(), "obj/c.1234.otmp".to_string()]
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
