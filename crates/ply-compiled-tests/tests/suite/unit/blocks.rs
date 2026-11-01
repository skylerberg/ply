use ply_compiled::blocks::*;

const MEBIBYTE: usize = 1 << 20;

#[test]
fn a_block_allocated_for_a_size_is_at_least_that_size_and_at_most_an_eighth_larger() {
    for bytes in (24..=SMALL).step_by(8) {
        let block = class_block_bytes(class_of(bytes));
        assert!(block >= bytes, "{bytes} bytes got a block of {block}");
        assert!(
            block - bytes <= bytes / 8,
            "{bytes} bytes got a block of {block}"
        );
        assert_eq!(class_of(block), class_of(bytes), "{block} is its own class");
    }
}

#[test]
fn a_freed_block_is_reused_by_the_next_allocation_of_the_same_size_class_but_not_of_a_different_class()
 {
    let mut b = Blocks::default();
    let first = b.take(48);
    let other = b.take(64);
    unsafe { b.give(first, 48) };
    assert_ne!(b.take(64), first);
    assert_eq!(b.take(48), first);
    unsafe { b.give(other, 64) };
}

#[test]
fn the_heap_reports_an_address_as_a_block_only_at_the_start_of_a_block_it_allocated_and_not_after_reset()
 {
    let mut b = Blocks::default();
    let block = b.take(48).addr();
    assert!(b.contains(block));
    assert!(!b.contains(block + 8), "the middle of a block");
    assert!(!b.contains(block + 48), "a block not handed out yet");
    assert!(
        !b.contains(block & !(4 * MEBIBYTE - 1)),
        "the segment's own header"
    );
    let large = b.take(SMALL + 8).addr();
    assert!(b.contains(large) && !b.contains(large + 8));
    assert!(!Blocks::default().contains(block), "another heap's block");
    b.reset();
    assert!(!b.contains(block) && !b.contains(large));
}

#[test]
fn a_block_larger_than_a_page_adds_its_exact_size_to_the_bytes_in_use_until_it_is_freed() {
    let mut b = Blocks::default();
    let block = b.take(3 * MEBIBYTE + 8);
    assert_eq!(b.retained_bytes(), 3 * MEBIBYTE + 8);
    unsafe {
        assert!(b.owns(block, 3 * MEBIBYTE + 8));
        b.give(block, 3 * MEBIBYTE + 8);
    }
    assert_eq!(b.retained_bytes(), 0);
}

#[test]
fn pages_emptied_by_blocks_of_one_size_are_reused_for_blocks_of_another_size() {
    let mut b = Blocks::default();
    let small: Vec<*mut u8> = (0..200_000).map(|_| b.take(32)).collect();
    let peak = b.retained_bytes();
    assert!(peak >= 200_000 * 32);
    for block in small {
        unsafe { b.give(block, 32) };
    }
    assert!(
        b.retained_bytes() < MEBIBYTE,
        "{} bytes held with nothing live",
        b.retained_bytes()
    );
    for _ in 0..(peak / 2 / 512) {
        b.take(512);
    }
    assert!(b.retained_bytes() <= peak, "{} bytes held past {peak}", b.retained_bytes());
}

#[test]
fn segments_left_with_no_live_blocks_are_unmapped_and_the_heap_can_still_allocate() {
    let mut b = Blocks::default();
    let blocks: Vec<*mut u8> = (0..64 * MEBIBYTE / 1024).map(|_| b.take(1024)).collect();
    assert!(b.mapped() >= 64 * MEBIBYTE);
    for block in blocks {
        unsafe { b.give(block, 1024) };
    }
    assert!(
        b.mapped() <= 12 * MEBIBYTE,
        "{} bytes mapped with nothing live",
        b.mapped()
    );
    let again = b.take(1024);
    assert!(b.contains(again.addr()));
}

#[test]
fn a_new_heap_reusing_a_segment_from_a_dropped_heap_contains_only_its_own_blocks() {
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
        assert_eq!(next.retained_bytes(), 64 << 10);
    }
}

#[test]
fn a_block_freed_by_another_thread_is_allocated_again_by_the_heap_that_owns_it() {
    let mut b = Blocks::default();
    let first = b.take(48);
    let rest: Vec<*mut u8> = (0..10_000).map(|_| b.take(48)).collect();
    let handed = first.addr();
    std::thread::scope(|s| {
        s.spawn(move || unsafe { free_block(handed as *mut u8, 48) });
    });
    let again: Vec<*mut u8> = (0..100_000).map(|_| b.take(48)).collect();
    assert!(again.contains(&first));
    drop(rest);
}

#[test]
fn large_blocks_freed_by_another_thread_are_released_before_the_next_large_allocation() {
    let mut b = Blocks::default();
    for _ in 0..64 {
        let handed = b.take(MEBIBYTE).addr();
        std::thread::scope(|s| {
            s.spawn(move || unsafe { free_block(handed as *mut u8, MEBIBYTE) });
        });
    }
    b.take(MEBIBYTE);
    assert_eq!(b.retained_bytes(), MEBIBYTE);
}

#[test]
fn adopting_a_heap_waits_for_another_thread_that_is_still_returning_a_block_to_that_heap() {
    struct Moved(Blocks);
    unsafe impl Send for Moved {}
    let mut branch = Blocks::default();
    let handed = branch.take(48);
    let rest: Vec<*mut u8> = (0..100).map(|_| branch.take(48)).collect();
    let pending = unsafe { RemoteFree::begin(handed, 48) };
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
fn a_heap_that_adopts_another_heap_owns_and_can_free_every_block_of_the_adopted_heap() {
    let mut parent = Blocks::default();
    let mine = parent.take(48);
    let mut branch = Blocks::default();
    let theirs: Vec<*mut u8> = (0..5_000).map(|_| branch.take(48)).collect();
    let big = branch.take(SMALL + 8);
    unsafe {
        assert!(!parent.owns(theirs[0], 48));
        parent.adopt(branch);
        assert!(parent.owns(theirs[0], 48) && parent.owns(big, SMALL + 8));
        assert!(parent.contains(theirs[0].addr()) && parent.contains(big.addr()));
        for &block in &theirs {
            parent.give(block, 48);
        }
        parent.give(big, SMALL + 8);
        parent.give(mine, 48);
    }
    assert!(parent.retained_bytes() <= 2 * (64 << 10), "{} held", parent.retained_bytes());
    let next: Vec<*mut u8> = (0..5_000).map(|_| parent.take(48)).collect();
    assert!(next.iter().all(|block| parent.contains(block.addr())));
}

#[test]
fn adopting_many_heaps_with_empty_pages_caps_the_empty_pages_retained_at_two_mebibytes() {
    let mut parent = Blocks::default();
    let live: Vec<*mut u8> = (0..100)
        .map(|_| {
            let mut branch = Blocks::default();
            let blocks: Vec<*mut u8> = (0..4_000).map(|_| branch.take(1024)).collect();
            for &block in &blocks[1..] {
                unsafe { branch.give(block, 1024) };
            }
            assert!(branch.spare_bytes() > 0);
            parent.adopt(branch);
            blocks[0]
        })
        .collect();
    assert!(
        parent.spare_bytes() <= 2 * MEBIBYTE,
        "{} bytes kept",
        parent.spare_bytes()
    );
    assert!(
        parent.retained_bytes() <= live.len() * (64 << 10),
        "{} bytes held",
        parent.retained_bytes()
    );
    for block in live {
        unsafe { parent.give(block, 1024) };
    }
    assert!(
        parent.mapped() <= 8 * MEBIBYTE,
        "{} bytes mapped",
        parent.mapped()
    );
}

#[test]
fn an_adopted_segment_with_no_live_blocks_is_unmapped() {
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
        parent.mapped() <= 4 * MEBIBYTE,
        "{} bytes mapped",
        parent.mapped()
    );
    assert_eq!(parent.retained_bytes(), 0);
}

#[test]
fn a_reset_heap_has_no_bytes_in_use_maps_at_most_four_mebibytes_and_can_allocate_again() {
    let mut b = Blocks::default();
    for _ in 0..100_000 {
        b.take(256);
    }
    b.take(MEBIBYTE);
    b.reset();
    assert_eq!(b.retained_bytes(), 0);
    assert!(b.mapped() <= 4 * MEBIBYTE);
    let block = b.take(256);
    assert!(b.contains(block.addr()));
    assert_eq!(b.retained_bytes(), 64 << 10);
}
