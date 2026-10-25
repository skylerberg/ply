use ply_compiled::blocks::*;

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

/// A heap that takes a segment an ended heap held walks only its own blocks there.
#[test]
fn a_segment_an_ended_heap_held_holds_none_of_its_blocks() {
    let mut first = Blocks::default();
    for class in 1..=64 {
        for _ in 0..1_000 {
            first.take(class * 16);
        }
    }
    drop(first);
    for _ in 0..16 {
        let mut next = Blocks::default();
        let mut taken: Vec<usize> = (0..1_000).map(|_| next.take(48).addr()).collect();
        let mut walked = Vec::new();
        next.each(|block, _| walked.push(block.addr()));
        taken.sort_unstable();
        walked.sort_unstable();
        assert_eq!(walked, taken);
        assert_eq!(next.held(), 64 << 10);
    }
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

/// A block another thread is handing back keeps the heap it read as its owner until it is pushed:
/// an adopter of that heap goes on only once the block is on the list it drains, so neither the
/// heap the push lands in nor the block is lost.
#[test]
fn an_adopter_waits_for_a_block_being_handed_back_to_the_heap_it_adopts() {
    struct Moved(Blocks);
    unsafe impl Send for Moved {}
    let mut branch = Blocks::default();
    let handed = branch.take(48);
    let rest: Vec<*mut u8> = (0..100).map(|_| branch.take(48)).collect();
    let pending = unsafe { Handing::begin(handed, 48) };
    let branch = Moved(branch);
    let (adopted, done) = std::sync::mpsc::channel();
    std::thread::scope(|s| {
        let adopter = s.spawn(move || {
            let moved = branch;
            let mut parent = Blocks::default();
            parent.adopt(moved.0);
            adopted.send(()).expect("the test waits for the adopter");
            Moved(parent)
        });
        assert!(
            done.recv_timeout(std::time::Duration::from_millis(200))
                .is_err(),
            "the adopter went on while a block was being handed back to the heap it adopted"
        );
        unsafe { pending.finish() };
        done.recv().expect("the adopter goes on");
        let mut parent = adopter.join().expect("the adopter").0;
        let again: Vec<*mut u8> = (0..1_000).map(|_| parent.take(48)).collect();
        assert!(again.contains(&handed));
    });
    drop(rest);
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

/// Branches that each emptied all but one page cost their adopter those pages and no more.
#[test]
fn an_adopter_keeps_no_more_emptied_pages_than_of_its_own() {
    let mut parent = Blocks::default();
    let live: Vec<*mut u8> = (0..100)
        .map(|_| {
            let mut branch = Blocks::default();
            let blocks: Vec<*mut u8> = (0..4_000).map(|_| branch.take(1024)).collect();
            for &block in &blocks[1..] {
                unsafe { branch.give(block, 1024) };
            }
            assert!(branch.kept() > 0);
            parent.adopt(branch);
            blocks[0]
        })
        .collect();
    assert!(parent.kept() <= 2 * MIB, "{} bytes kept", parent.kept());
    assert!(
        parent.held() <= live.len() * (64 << 10),
        "{} bytes held",
        parent.held()
    );
    for block in live {
        unsafe { parent.give(block, 1024) };
    }
    assert!(
        parent.mapped() <= 8 * MIB,
        "{} bytes mapped",
        parent.mapped()
    );
}

#[test]
fn an_adopted_segment_with_no_live_block_goes_back() {
    let mut parent = Blocks::default();
    for _ in 0..100 {
        let mut branch = Blocks::default();
        let blocks: Vec<*mut u8> = (0..4_000).map(|_| branch.take(1024)).collect();
        for block in blocks {
            unsafe { branch.give(block, 1024) };
        }
        parent.adopt(branch);
    }
    assert!(
        parent.mapped() <= 4 * MIB,
        "{} bytes mapped",
        parent.mapped()
    );
    assert_eq!(parent.held(), 0);
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
