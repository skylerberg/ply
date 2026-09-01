//! What an entry into the backend costs, and which entry pays for it.

use ply_codegen_spike::rt::{Ctx, Tables};
use ply_eval::Value;
use ply_span::Symbol;
use std::collections::BTreeMap;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;

/// The predecessor sizes `RESULTS.md` §3 used, so the two tables line up row for row.
const LADDER: [usize; 7] = [4, 64, 184, 384, 3824, 4084, 19584];

const BEST_OF: usize = 7;

fn tables() -> Rc<Tables> {
    Rc::new(Tables {
        consts: Vec::new(),
        ctors: Vec::new(),
        shapes: Vec::new(),
        fields: Vec::new(),
        builtins: Vec::new(),
    })
}

fn fill(ctx: &mut Ctx, slots: usize, make: &impl Fn(usize) -> Value) {
    for i in 0..slots {
        ctx.push(make(i));
    }
}

/// Best-of-`BEST_OF` nanoseconds for the two halves of an entry over an arena of `slots` values:
/// closing the entry that filled it, and beginning the next one.
fn halves_ns(slots: usize, make: &impl Fn(usize) -> Value) -> (u128, u128) {
    let (mut end_best, mut begin_best) = (u128::MAX, u128::MAX);
    for _ in 0..BEST_OF {
        // A fresh context per repeat, because what is being timed is an entry over a buffer that
        // grew to hold it, and a `Vec` somebody already cleared is a different measurement.
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
    // Once through, discarded: the first allocation of the process is not the steady state, and it
    // lands on whichever row runs first.
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

    // `Instant` on this platform resolves to about 40 ns, and both halves after a 4-slot entry are
    // under that — they time as 0 or as one tick, depending on the run.
    let biggest = LADDER[LADDER.len() - 1];
    let (end, begin) = timed[LADDER.len() - 1];
    println!("\n  at {biggest} slots: end {end} ns, begin {begin} ns");
    println!("  end slope: {:.3} ns/slot", end as f64 / biggest as f64);
    println!(
        "  begin, which is the half the *next* entry pays: {begin} ns, \
         {:.4} ns/slot\n",
        begin as f64 / biggest as f64
    );

    // The one thing asserted: the run happened.
    assert_eq!(timed.len(), LADDER.len());
}

#[test]
#[ignore = "a measurement, not a gate — see this file's header"]
fn what_an_entry_pays_to_hand_back_what_it_did_not_earn() {
    const N: usize = 19584;
    let int = |i: usize| Value::Int(i as i64);

    /// One entry of `slots`, preceded by one of `slots * before`, timed at its close.
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
        // Deliberately not divided by the slots released: 4N and 8N release twice as much for about
        // the same money, so the cost is a transition and not a rate, and a per-slot column would
        // invite the reader to extrapolate a line through it.
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

    // Worth saying out loud for the next person who benchmarks an arena: the `Int` ladder was not
    // wrong, it was the cheapest case, and the way that was found out was by changing the element
    // type rather than by re-reading the code.
    assert!(int > 0 && string > 0 && record > 0);
}
