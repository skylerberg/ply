use ply_corpus::rng::Rng;

#[test]
fn a_seed_fixes_the_whole_stream() {
    let mut a = Rng::new(42);
    let mut b = Rng::new(42);
    let left: Vec<u64> = (0..64).map(|_| a.next_u64()).collect();
    let right: Vec<u64> = (0..64).map(|_| b.next_u64()).collect();
    assert_eq!(left, right);
}

#[test]
fn distinct_seeds_diverge_immediately() {
    assert_ne!(Rng::new(1).next_u64(), Rng::new(2).next_u64());
}

#[test]
fn forks_are_keyed_by_tag_and_not_by_call_order() {
    let root = Rng::new(7);
    let mut first = root.fork(3);
    let mut second = root.fork(3);
    assert_eq!(first.next_u64(), second.next_u64());
    assert_ne!(root.fork(3).next_u64(), root.fork(4).next_u64());
}

#[test]
fn below_stays_in_range_and_tolerates_zero() {
    let mut r = Rng::new(9);
    assert_eq!(r.below(0), 0);
    for _ in 0..1000 {
        assert!(r.below(7) < 7);
    }
}

#[test]
fn between_is_inclusive_on_both_ends() {
    let mut r = Rng::new(11);
    let mut saw_lo = false;
    let mut saw_hi = false;
    for _ in 0..2000 {
        let v = r.between(2, 5);
        assert!((2..=5).contains(&v));
        saw_lo |= v == 2;
        saw_hi |= v == 5;
    }
    assert!(saw_lo && saw_hi);
    assert_eq!(r.between(4, 4), 4);
    assert_eq!(r.between(9, 1), 9);
}

#[test]
fn skew_concentrates_on_the_front() {
    let mut r = Rng::new(13);
    let mut front = 0;
    for _ in 0..2000 {
        if r.skewed_below(100, 3) < 25 {
            front += 1;
        }
    }
    // A uniform draw would put ~500 in the front quarter; the minimum of four puts ~1368 there.
    assert!(
        front > 1200,
        "expected a strong front bias, got {front}/2000"
    );
}
