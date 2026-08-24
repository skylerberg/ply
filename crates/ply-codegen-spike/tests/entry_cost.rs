//! What an entry into the backend costs, and which entry pays for it.
//!
//! `CONTRIBUTING.md` §"Things known to be broken" item 12 said every entry costs
//! O(the previous entry's peak arena), and `benches/r5-timing/RESULTS.md` §3
//! carries the table it withdrew a published claim with — the identical hybrid
//! call `mcts.playouts(0,0,0)` at 0.375 µs after a 4-slot predecessor and
//! 68.083 µs after a 19,584-slot one, 181x, about 3.5 ns a retained slot.
//!
//! **That is fixed, and this file is what the fix is read off.** [`Ctx::end`]
//! now clears the arena at the end of the entry that filled it, so an entry pays
//! for its own work; [`Ctx::begin`] pays for nothing at all. The end-to-end
//! re-take is `mcts --carryover mcts.playouts`, which reads **181.667x / 180.888x
//! before and 1.499x / 1.202x after**, two runs each. This isolates the same
//! mechanism at the unit it lives in.
//!
//! > **Corrected in place (2026-08-24), because this file said otherwise.** It
//! > opened: *"This isolates the mechanism rather than re-running the kernel:
//! > `Ctx::begin` is the whole of it, and it is reachable directly."* That was
//! > true of the code it was written against and is not true now: `begin` no
//! > longer touches the arena except to recover from a path that forgot to close
//! > itself, and [`the_cost_is_the_clear_and_not_the_shrink`] below used to step
//! > across `RETAINED_SLOTS` on the assumption that `begin` shrinks whenever
//! > capacity exceeds it. `end` decides per entry against what that entry used,
//! > so that test now arms the shrink with a predecessor the timed entry does
//! > not justify. What survives unchanged is the finding: **the cost is the
//! > clear and not the shrink**, and the slope is what item 12 is about.
//!
//! > **And a second time, for a reason worth stating (2026-08-24).** The version
//! > between those two amortized the shrink over a 64-entry window, and this
//! > file's shrink measurement ran the window out to arm it. That measurement
//! > was then cited as grounds for deleting the amortization — wrongly, because
//! > it times the shrink and not the **regrowth** the shrink forces on the next
//! > entry. Shrinking a cleared `Vec` is an alloc and a free with nothing to
//! > copy; growing it back through a doubling buffer copies 4,096 then 8,192
//! > then 16,384 live `Value`s, and that cost lands outside this timer in both
//! > arms. What the 1.00x below licenses is narrow: *given the buffer is
//! > shrunk*, the shrink itself is free. It says nothing about how often
//! > shrinking is worth doing, which is why [`SLACK`] exists and is measured by
//! > the steady-state arm rather than by this one.
//!
//! **`#[ignore]` on purpose.** It is a measurement, not a gate. Asserting a
//! floor on the ratio would pin the *defect* in place — the day somebody makes
//! the cost constant the assertion goes red and the fix looks like a regression.
//! Asserting nothing and running anyway would be a green over a number nobody
//! read. So it prints, and it runs on request:
//!
//! ```text
//! cargo test -p ply-codegen-spike --release --test entry_cost -- --ignored --nocapture
//! ```
//!
//! Release matters: the thing being timed is a `clear` and a `shrink_to`, and a
//! debug build measures the profile rather than the code.

use ply_codegen_spike::rt::{Ctx, SLACK, Tables};
use ply_eval::Value;
use ply_span::Symbol;
use std::collections::BTreeMap;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;

/// The predecessor sizes `RESULTS.md` §3 used, so the two tables line up row for
/// row. 4096 is `RETAINED_SLOTS`, which is the floor the arena is kept above.
const LADDER: [usize; 7] = [4, 64, 184, 384, 3824, 4084, 19584];

const BEST_OF: usize = 7;

fn tables() -> Rc<Tables> {
    Rc::new(Tables {
        consts: Vec::new(),
        ctors: Vec::new(),
        shapes: Vec::new(),
        builtins: Vec::new(),
    })
}

fn fill(ctx: &mut Ctx, slots: usize, make: &impl Fn(usize) -> Value) {
    for i in 0..slots {
        ctx.push(make(i));
    }
}

/// Best-of-`BEST_OF` nanoseconds for the two halves of an entry over an arena of
/// `slots` values: closing the entry that filled it, and beginning the next one.
///
/// Best-of rather than a mean because the competing explanation for any number
/// here is scheduler noise, and noise only ever adds. Each repeat refills the
/// arena from a shrunk `Vec`, so what is timed is one `end` against a freshly
/// grown buffer — the state a real entry leaves — and never an `end` over an
/// arena a previous `end` already cleared.
fn halves_ns(slots: usize, make: &impl Fn(usize) -> Value) -> (u128, u128) {
    let (mut end_best, mut begin_best) = (u128::MAX, u128::MAX);
    for _ in 0..BEST_OF {
        // A fresh context per repeat, because what is being timed is an entry
        // over a buffer that grew to hold it, and a `Vec` somebody already
        // cleared is a different measurement.
        let mut ctx = Ctx::new(tables());
        fill(&mut ctx, slots, make);

        let t = Instant::now();
        ctx.end();
        end_best = end_best.min(t.elapsed().as_nanos());

        let t = Instant::now();
        ctx.begin(1_000);
        begin_best = begin_best.min(t.elapsed().as_nanos());

        assert_eq!(
            ctx.unclosed_entries(),
            0,
            "the `begin` timed here found slots still in place, so it is timing the recovery \
             path and not the entry path"
        );
    }
    (end_best, begin_best)
}

#[test]
#[ignore = "a measurement, not a gate — see this file's header"]
fn an_entry_pays_for_its_own_arena_and_the_next_one_pays_for_nothing() {
    let int = |i: usize| Value::Int(i as i64);
    // Once through, discarded: the first allocation of the process is not the
    // steady state, and it lands on whichever row runs first.
    halves_ns(1024, &int);

    let timed: Vec<(u128, u128)> = LADDER.iter().map(|&n| halves_ns(n, &int)).collect();

    println!("\nOne entry over N slots, best of {BEST_OF}, release:\n");
    println!(
        "  {:>10}  {:>10}  {:>12}  {:>10}",
        "slots", "end ns", "end ns/slot", "begin ns"
    );
    for (&slots, &(end, begin)) in LADDER.iter().zip(&timed) {
        println!(
            "  {:>10}  {:>10}  {:>12.3}  {:>10}",
            slots,
            end,
            end as f64 / slots as f64,
            begin
        );
    }

    // `Instant` on this platform resolves to about 40 ns, and both halves after
    // a 4-slot entry are under that — they time as 0 or as one tick, depending
    // on the run. A ratio taken against them is a measurement of the clock, so
    // the headline is the slope, read off the largest row where the resolution
    // is three orders of magnitude away. The slope is the claim item 12 actually
    // makes ("about 3.5 ns a retained slot"); the ratio was only ever the slope
    // times the ladder's span.
    let biggest = LADDER[LADDER.len() - 1];
    let (end, begin) = timed[LADDER.len() - 1];
    println!("\n  at {biggest} slots: end {end} ns, begin {begin} ns");
    println!("  end slope: {:.3} ns/slot", end as f64 / biggest as f64);
    println!(
        "  begin, which is the half the *next* entry pays: {begin} ns, \
         {:.4} ns/slot\n",
        begin as f64 / biggest as f64
    );

    // The one thing asserted: the run happened. Every claim above is in the
    // printed table, which is what the reader is here for.
    assert_eq!(timed.len(), LADDER.len());
}

/// What an entry pays to hand back a buffer it did not earn.
///
/// Item 12 named two costs — *"the clear drops that many `Value`s, and the
/// shrink reallocates"* — and the ladder above cannot separate them, because it
/// grows the arena and the shrink together. This holds the timed entry fixed at
/// 19,584 slots and varies **only what ran before it**, which is what decides
/// whether its `end` shrinks: a predecessor the same size leaves a buffer this
/// entry justifies, and larger predecessors leave one it does not. Every row
/// clears the same 19,584 slots, so the difference between them is the
/// reallocation and nothing else.
///
/// > **This is the third time this measurement has been taken and the second
/// > time it has overturned its own predecessor, so read the number and not the
/// > sentence.** It was first taken against a `begin` that shrank on every
/// > entry, and reported the shrink as no part of the cost. It was taken again
/// > against a 64-entry window, holding the arena at 19,584 slots and forcing
/// > the shrink, and reported **1.00x** — 81,667 ns against 81,708 ns — which
/// > was then cited as grounds for deleting the window. That comparison shrank
/// > 32,768 slots to 19,584, a buffer already close to its target, and
/// > generalised from it to every shrink. It does not generalise: the cost is in
/// > **releasing the buffer**, so it scales with how much is given back, and a
/// > predecessor four times the size makes the same shrink cost real money. The
/// > surviving claim from all three takes is narrow and unchanged — the 181x of
/// > item 12 was the clear, at ~4.17 ns a slot, not the reallocation — and the
/// > table below is what the design trade-off is actually read off.
///
/// **What three runs of it said, at load 27-48**, against a steady-state row of
/// 75-82 µs: a 2N predecessor cost +7.3, +0.8 and -6.4 µs, which straddles zero
/// and is inter-arm noise; 4N cost +67, +29 and +58 µs; 8N cost +91, +41 and
/// +48 µs. So the sign and the order of magnitude are stable and the magnitude
/// is not — releasing a multi-megabyte buffer costs **tens of microseconds**,
/// the same order as clearing the 19,584 slots themselves, and a single run of
/// this test should not be quoted to more than one significant figure. The
/// negative row is the useful one to keep in view: it is what the noise floor
/// between two arms looks like, and it is why the 2N row is reported as "no
/// measurable cost" rather than as a speed-up.
#[test]
#[ignore = "a measurement, not a gate — see this file's header"]
fn what_an_entry_pays_to_hand_back_what_it_did_not_earn() {
    const N: usize = 19584;
    let int = |i: usize| Value::Int(i as i64);

    /// One entry of `slots`, preceded by one of `slots * before`, timed at its
    /// close. `before == 1` is the steady state, where nothing is handed back.
    fn end_ns(slots: usize, before: usize, make: &impl Fn(usize) -> Value) -> (u128, usize) {
        let mut best = u128::MAX;
        let mut held = 0;
        for _ in 0..BEST_OF {
            let mut ctx = Ctx::new(tables());
            fill(&mut ctx, slots * before, make);
            ctx.end();
            fill(&mut ctx, slots, make);
            held = ctx.slots.capacity();
            let t = Instant::now();
            ctx.end();
            best = best.min(t.elapsed().as_nanos());
        }
        (best, held)
    }

    end_ns(1024, 1, &int);
    let rows: Vec<(usize, u128, usize)> = [1usize, 2, 4, 8]
        .iter()
        .map(|&b| {
            let (ns, held) = end_ns(N, b, &int);
            (b, ns, held)
        })
        .collect();

    let (_, baseline, _) = rows[0];
    println!("\nCtx::end over {N} slots, by what the entry before it left:\n");
    println!(
        "  {:>12}  {:>14}  {:>10}  {:>14}",
        "predecessor", "capacity held", "end ns", "over steady"
    );
    for &(before, ns, held) in &rows {
        // Deliberately not divided by the slots released: 4N and 8N release
        // twice as much for about the same money, so the cost is a transition
        // and not a rate, and a per-slot column would invite the reader to
        // extrapolate a line through it.
        println!(
            "  {:>11}N  {:>14}  {:>10}  {:>+14}",
            before,
            held,
            ns,
            ns as i128 - baseline as i128
        );
    }
    println!(
        "\n  The steady state hands nothing back and pays nothing for it, which is what\n  \
         `SLACK` is for. Every other row is one `free` of a buffer this entry did not\n  \
         allocate, charged to it because it is the entry that found the buffer\n  \
         unearned. Tens of microseconds, once, at a downward transition -- against\n  \
         item 12, which charged every entry 4.17 ns for each of its predecessor's\n  \
         slots, for ever.\n"
    );

    assert!(rows.iter().all(|&(_, ns, _)| ns > 0));
}

/// What widening the fragment would do to the cost above.
///
/// ADR 0018 §0's census and the lexer measurement agree that the next thing to
/// compile is record construction and field access, and Map and List builtins.
/// The ladder above fills the arena with [`Value::Int`], which is the cheapest
/// thing a clear can walk: no refcount, no destructor, nothing to touch but the
/// discriminant. A fragment that builds records puts `Arc<BTreeMap<..>>` in
/// those slots instead, and the clear then drops a refcount per slot and frees a
/// map whenever it was the last handle.
///
/// So this prices the same close over the values a widened fragment would
/// actually leave behind. It is the reason item 12 was worth fixing *before* the
/// widening rather than after: the widening multiplies both terms — more slots
/// per entry, and a more expensive drop for each one — and a cost that is
/// charged to the call that caused it survives that multiplication far better
/// than one charged to whatever ran next.
#[test]
#[ignore = "a measurement, not a gate — see this file's header"]
fn the_slope_is_a_function_of_what_the_slots_hold() {
    const N: usize = 19584;

    let int = halves_ns(N, &|i| Value::Int(i as i64)).0;
    let string = halves_ns(N, &|i| Value::Str(Arc::from(format!("tok{i}").as_str()))).0;
    let record = halves_ns(N, &|i| {
        let mut fields = BTreeMap::new();
        fields.insert(Symbol::new("kind"), Value::Int(i as i64));
        fields.insert(Symbol::new("text"), Value::Str(Arc::from("ident")));
        Value::Record(Arc::new(fields))
    })
    .0;

    println!("\nCtx::end over {N} slots, by what the slots hold:\n");
    println!(
        "  {:>28}  {:>10}  {:>10}",
        "slot contents", "end ns", "ns/slot"
    );
    for (what, t) in [
        ("Int (the ladder above)", int),
        ("Str — one Arc drop", string),
        ("Record — Arc<BTreeMap> drop", record),
    ] {
        println!("  {:>28}  {:>10}  {:>10.3}", what, t, t as f64 / N as f64);
    }
    println!(
        "\n  a record-shaped arena costs {:.1}x an Int-shaped one to clear\n",
        record as f64 / int.max(1) as f64
    );

    // Worth saying out loud for the next person who benchmarks an arena: the
    // `Int` ladder was not wrong, it was the cheapest case, and the way that was
    // found out was by changing the element type rather than by re-reading the
    // code. `Int` is what anybody reaches for first, and it is the one element
    // type whose drop is free.
    assert!(int > 0 && string > 0 && record > 0);
}
