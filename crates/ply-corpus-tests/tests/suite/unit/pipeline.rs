use ply_corpus::pipeline::{Phase, Timings, discover, front};
use std::time::Duration;

#[test]
fn timings_accumulate_rather_than_replace() {
    let mut t = Timings::default();
    t.record(Phase::Read, Duration::from_millis(3));
    t.record(Phase::Read, Duration::from_millis(4));
    t.record(Phase::Compile, Duration::from_millis(1));
    assert_eq!(t.get(Phase::Read), Duration::from_millis(7));
    assert_eq!(t.total(), Duration::from_millis(8));
    assert_eq!(t.get(Phase::Execute), Duration::ZERO);
}

#[test]
fn discovery_skips_hidden_directories_and_non_ply_files() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join(".ply-cache")).unwrap();
    std::fs::create_dir_all(dir.path().join("pkg")).unwrap();
    std::fs::write(dir.path().join("a.ply"), "").unwrap();
    std::fs::write(dir.path().join("pkg/b.ply"), "").unwrap();
    std::fs::write(dir.path().join("corpus.json"), "{}").unwrap();
    std::fs::write(dir.path().join(".ply-cache/c.ply"), "").unwrap();

    let found = discover(dir.path()).unwrap();
    assert_eq!(found.len(), 2);
    assert!(found.iter().all(|p| p.extension().unwrap() == "ply"));
}

#[test]
fn an_empty_root_is_an_error_rather_than_an_empty_program() {
    let dir = tempfile::tempdir().unwrap();
    let err = front(dir.path()).unwrap_err();
    assert!(err.to_string().contains("no `.ply` files"));
}
