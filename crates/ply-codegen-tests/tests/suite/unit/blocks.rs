use ply_codegen::blocks::*;

const MIB: usize = 1 << 20;

#[test]
fn a_class_holds_every_size_it_is_given_within_an_eighth() {
    for bytes in (24..=SMALL).step_by(8) {
        let block = block_of(class_of(bytes));
        assert!(block >= bytes, "{bytes} bytes got a block of {block}");
        assert!(
            block - bytes <= bytes / 8,
            "{bytes} bytes got a block of {block}"
        );
        assert_eq!(class_of(block), class_of(bytes), "{block} is its own class");
    }
}

#[test]
fn a_dead_block_is_the_next_of_its_class_and_of_no_other() {
    let mut b = Blocks::default();
    let first = b.take(48);
    let other = b.take(64);
    unsafe { b.give(first, 48) };
    assert_ne!(b.take(64), first);
    assert_eq!(b.take(48), first);
    assert_eq!(b.reused(), 1);
    unsafe { b.give(other, 64) };
}

#[test]
fn only_where_a_block_begins_is_held() {
    let mut b = Blocks::default();
    let block = b.take(48).addr();
    assert!(b.holds(block));
    assert!(!b.holds(block + 8), "the middle of a block");
    assert!(!b.holds(block + 48), "a block not handed out yet");
    assert!(!b.holds(block & !(4 * MIB - 1)), "the segment's own header");
    let large = b.take(SMALL + 8).addr();
    assert!(b.holds(large) && !b.holds(large + 8));
    assert!(!Blocks::default().holds(block), "another heap's block");
    b.reset();
    assert!(!b.holds(block) && !b.holds(large));
}

#[test]
fn a_block_past_a_pages_sizes_is_held_at_its_size_until_it_is_given_back() {
    let mut b = Blocks::default();
    let block = b.take(3 * MIB + 8);
    assert_eq!(b.held(), 3 * MIB + 8);
    unsafe {
        assert!(b.owns(block, 3 * MIB + 8));
        b.give(block, 3 * MIB + 8);
    }
    assert_eq!(b.held(), 0);
}

/// Blocks of one size, all given back, leave their pages to whatever size is asked for next.
#[test]
fn pages_one_size_emptied_serve_another() {
    let mut b = Blocks::default();
    let small: Vec<*mut u8> = (0..200_000).map(|_| b.take(32)).collect();
    let peak = b.held();
    assert!(peak >= 200_000 * 32);
    for block in small {
        unsafe { b.give(block, 32) };
    }
    assert!(b.held() < MIB, "{} bytes held with nothing live", b.held());
    for _ in 0..(peak / 2 / 512) {
        b.take(512);
    }
    assert!(b.held() <= peak, "{} bytes held past {peak}", b.held());
}

#[test]
fn segments_left_with_no_block_go_back() {
    let mut b = Blocks::default();
    let blocks: Vec<*mut u8> = (0..64 * MIB / 1024).map(|_| b.take(1024)).collect();
    assert!(b.mapped() >= 64 * MIB);
    for block in blocks {
        unsafe { b.give(block, 1024) };
    }
    assert!(
        b.mapped() <= 12 * MIB,
        "{} bytes mapped with nothing live",
        b.mapped()
    );
    let again = b.take(1024);
    assert!(b.holds(again.addr()));
}

/// A block handed back off its heap's thread is the heap's to hand out once it looks for one.
#[test]
fn a_block_another_thread_hands_back_is_handed_out_again() {
    let mut b = Blocks::default();
    let first = b.take(48);
    let rest: Vec<*mut u8> = (0..10_000).map(|_| b.take(48)).collect();
    let handed = first.addr();
    std::thread::scope(|s| {
        s.spawn(move || unsafe { hand_back(handed as *mut u8, 48) });
    });
    let again: Vec<*mut u8> = (0..100_000).map(|_| b.take(48)).collect();
    assert!(again.contains(&first));
    drop(rest);
}

/// A heap that takes only blocks past a page's sizes still takes up the ones handed back to it.
#[test]
fn a_large_block_another_thread_hands_back_is_given_up_before_the_next() {
    let mut b = Blocks::default();
    for _ in 0..64 {
        let handed = b.take(MIB).addr();
        std::thread::scope(|s| {
            s.spawn(move || unsafe { hand_back(handed as *mut u8, MIB) });
        });
    }
    b.take(MIB);
    assert_eq!(b.held(), MIB);
}

#[test]
fn an_adopted_heaps_blocks_are_the_adopters() {
    let mut parent = Blocks::default();
    let mine = parent.take(48);
    let mut branch = Blocks::default();
    let theirs: Vec<*mut u8> = (0..5_000).map(|_| branch.take(48)).collect();
    let big = branch.take(SMALL + 8);
    unsafe {
        assert!(!parent.owns(theirs[0], 48));
        parent.adopt(branch);
        assert!(parent.owns(theirs[0], 48) && parent.owns(big, SMALL + 8));
        assert!(parent.holds(theirs[0].addr()) && parent.holds(big.addr()));
        for &block in &theirs {
            parent.give(block, 48);
        }
        parent.give(big, SMALL + 8);
        parent.give(mine, 48);
    }
    assert!(parent.held() <= 2 * (64 << 10), "{} held", parent.held());
    let next: Vec<*mut u8> = (0..5_000).map(|_| parent.take(48)).collect();
    assert!(next.iter().all(|block| parent.holds(block.addr())));
}

#[test]
fn a_reset_heap_holds_nothing_and_hands_out_afresh() {
    let mut b = Blocks::default();
    for _ in 0..100_000 {
        b.take(256);
    }
    b.take(MIB);
    b.reset();
    assert_eq!(b.held(), 0);
    assert!(b.mapped() <= 4 * MIB);
    let block = b.take(256);
    assert!(b.holds(block.addr()));
    assert_eq!(b.held(), 64 << 10);
}
