mod facts;

use ply_test::bisect::Skipped;

#[test]
fn every_skip_reason_explains_itself_distinctly() {
    let all = [
        Skipped::NotRequested,
        Skipped::NeverPassed,
        Skipped::Host,
        Skipped::Nondet,
        Skipped::Panicked,
        Skipped::NoChanges,
        Skipped::NoBodies,
        Skipped::NoHybrids,
    ];
    let mut described: Vec<&str> = all.iter().map(|s| s.describe()).collect();
    described.sort_unstable();
    described.dedup();
    assert_eq!(described.len(), all.len());

    let mut codes: Vec<&str> = all.iter().map(|s| s.as_str()).collect();
    codes.sort_unstable();
    codes.dedup();
    assert_eq!(codes.len(), all.len());
}
