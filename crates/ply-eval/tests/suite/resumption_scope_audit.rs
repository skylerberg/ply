//! What a resumption sees in its **scope**, as against what it sees in the world.
//!
//! `resumption_semantics_audit` covers the world: cells thread across resumptions, which is ADR
//! 0005's rule, and every test there asserts what a cell holds. Nothing asserts the other half — that a resumption
//! re-enters with the *bindings* it captured — because a persistent `Env` gives that for free. A
//! continuation holds an immutable chain, so there is no way to get it wrong and nothing to test.
//!
//! ADR 0034's slot frames remove that guarantee. A machine-owned slot stack reuses indices across
//! activations and empties a slot at a last use, so "the scope a resumption re-enters with" becomes
//! a thing an implementation can be wrong about, silently. These are the programs that would notice.
//!
//! **Every test here passes on the chain today.** That is the point: they are written before the
//! change so that they are a check on it rather than a description of it, and each names the
//! specific way a slot machine would fail it.

use crate::fixture::Compiled;
use ply_eval::{Value, rc};

/// Runs one test of `source` and requires it to pass. The probes assert their own answers, so a
/// wrong scope is a failed `assert_eq` inside the program rather than a comparison out here.
fn passes(source: &str, test: &str) -> Compiled {
    let compiled = Compiled::new(source);
    let index = compiled.index_of(test);
    if let Err(d) = compiled.machine().eval_test(index) {
        panic!("{test:?} must pass: {d:#?}");
    }
    compiled
}

/// Every cell the test held when its region closed, by ascending cell id.
fn cells_after(compiled: &Compiled, test: &str) -> Vec<i64> {
    let index = compiled.index_of(test);
    let mut machine = compiled.machine();
    machine.cells_mut().journal();
    machine
        .eval_test(index)
        .unwrap_or_else(|d| panic!("{test:?} must run: {d:#?}"));
    let mut cells: Vec<(ply_eval::arena::Slot, Value)> = machine.cells().journalled().to_vec();
    cells.sort_by_key(|(slot, _)| *slot);
    cells
        .into_iter()
        .map(|(_, v)| match v {
            Value::Int(i) => i,
            other => panic!("expected Int cells, found {other:?}"),
        })
        .collect()
}

const MOVE_OUT: &str = r#"
effect amb {
  read flip[coin]() -> Bool
}

// `big` is read once, after the `perform`, so its read is the last use of the binding and
// ADR 0034's step 3 would move the value out of its slot. The second resumption re-enters at the
// same point and reads it again: a slot emptied by the first resumption is empty for the second.
test "a binding whose last use follows the capture" {
  let big = [1, 2, 3];
  let out = handle {
    let b = amb.flip[coin]();
    if b { len(big) } else { len(big) * 10 }
  } with {
    amb.flip[coin]() resume k -> k(true) + k(false),
    return x -> x
  };
  assert_eq(out, 33)
}
"#;

/// A last use inside a captured extent is not a last use, because the extent can run twice.
///
/// This is the sharpest test of ADR 0034's step 3, and the one most likely to be got wrong:
/// `Own::Owned` says "nothing after this point reads the binding", which is true of the *code* and
/// false of the *execution* once a continuation is resumed more than once. On the chain it is safe
/// for a reason that disappears with the chain — `take_unique` refuses at a shared link, and a
/// captured continuation is exactly what shares it.
#[test]
fn a_binding_whose_last_use_follows_the_capture_survives_a_second_resumption() {
    let before = rc::stats();
    passes(MOVE_OUT, "a binding whose last use follows the capture");
    let moved = rc::stats().takes_moved - before.takes_moved;
    // Eight, accounted for under ADR 0034's slot frames — the tripwire fired when the frames
    // landed and this is the answer to its "find out which" instruction. Per resumption (×2):
    // `b` at the `if`, `big` at the taken branch's `len`, and the `return` clause's `x` — all
    // three read from the resumption's *own restored copy* of the captured window, which is what
    // makes a move inside a captured extent sound: the capture snapshots the slots, every
    // resumption restores a fresh clone, and the machine's emptied slot is never re-read. Plus
    // `k`'s second application in the clause, and `out` at the assertion. On the chain machine
    // the count was 2 (`b` and `out`) because a pending frame's clone of the scope made every
    // link shared and `take_unique` refused.
    assert_eq!(
        moved, 8,
        "this probe moved {moved} bindings out of their slots rather than the 8 it moves under \
         slot frames. The breakdown above is the pin; if it moves, find out *which* binding \
         changed before deciding the change is right — a last use inside a captured extent is \
         only sound against a restored copy"
    );
}

const SLOT_REUSE: &str = r#"
effect amb {
  read flip[coin]() -> Bool
}

fn deep(n: Int) -> Int =
  if n == 0 { 0 } else { deep(n - 1) + n }

// Between the two resumptions the clause runs `deep`, which pushes and pops many activations. On a
// machine-owned slot stack those reuse the indices the captured frames named, so a resumption that
// reads its window rather than a snapshot of it reads whatever `deep` left there.
test "a binding read across intervening activations" {
  let a = 7;
  let b = 11;
  let out = handle {
    let f = amb.flip[coin]();
    if f { a } else { b }
  } with {
    amb.flip[coin]() resume k -> {
      let first = k(true);
      let noise = deep(60);
      let second = k(false);
      first + second + noise - noise
    },
    return x -> x
  };
  assert_eq(out, 18)
}
"#;

/// Sixty activations run between the two resumptions, over the slots the captured frames named.
#[test]
fn a_binding_survives_the_activations_that_run_between_two_resumptions() {
    passes(SLOT_REUSE, "a binding read across intervening activations");
}

const BOUNDARY: &str = r#"
effect amb {
  read flip[coin]() -> Bool
}

// The two halves of ADR 0005's rule in one program: `base` is a *binding* and must read the same on
// both resumptions, while the cell is *state* and must accumulate across them. An implementation
// that restores too much fails the cell assertion; one that restores too little fails the sum.
test "a binding is restored while a cell threads" {
  with_cell[s](0) { c -> {
    let base = 100;
    let out = handle {
      let f = amb.flip[coin]();
      cell_set(c, cell_get(c) + 1);
      base + cell_get(c)
    } with {
      amb.flip[coin]() resume k -> k(true) + k(false),
      return x -> x
    };
    assert_eq(out, 203);
    assert_eq(cell_get(c), 2)
  } }
}
"#;

/// The boundary itself, and it is two-sided on purpose.
///
/// `base + cell` is 101 on the first resumption and 102 on the second: the binding is the same both
/// times and the cell is not. Restoring the cell along with the scope gives 202 and reddens the
/// second assertion; restoring neither gives a wrong sum. Only the split ADR 0005 describes
/// passes both, which is what makes this the test to run first.
#[test]
fn a_binding_is_restored_while_a_cell_threads() {
    let compiled = passes(BOUNDARY, "a binding is restored while a cell threads");
    assert_eq!(
        cells_after(&compiled, "a binding is restored while a cell threads"),
        vec![2],
        "the cell threads across both resumptions even though the binding does not"
    );
}

const NESTED: &str = r#"
effect amb {
  read flip[coin]() -> Bool
}

effect pick {
  read one[p]() -> Bool
}

// A capture inside the extent of another capture. Each continuation has to restore its own window,
// and the windows overlap: `outer` is in scope at both capture points and must read 1000 in all
// four leaves. The inner handler resumes twice for 1001 + 1002 = 2003, and the outer resumes that
// whole computation twice for 4006 — so any leaf that loses `outer` moves the total by a thousand.
test "nested captures each restore their own scope" {
  let outer = 1000;
  let out = handle {
    let x = amb.flip[coin]();
    handle {
      let y = pick.one[p]();
      if y { outer + 1 } else { outer + 2 }
    } with {
      pick.one[p]() resume j -> j(true) + j(false),
      return v -> v
    }
  } with {
    amb.flip[coin]() resume k -> k(true) + k(false),
    return v -> v
  };
  assert_eq(out, 4006)
}
"#;

/// Two live continuations at once, over overlapping windows.
#[test]
fn nested_captures_each_restore_their_own_scope() {
    passes(NESTED, "nested captures each restore their own scope");
}
