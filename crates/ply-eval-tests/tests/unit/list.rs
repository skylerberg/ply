use ply_eval::Value;
use ply_eval::list::*;

fn ints(n: usize) -> Vec<Value> {
    (0..n as i64).map(Value::Int).collect()
}

/// Every operation, against a `Vec` model, across the sizes that reach three levels of trie.
#[test]
fn a_list_agrees_with_a_vec_under_every_operation() {
    let mut list = List::default();
    let mut model: Vec<Value> = Vec::new();
    let mut seed = 0x9e37_79b9_u64;
    for step in 0..40_000u64 {
        seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        let roll = seed >> 33;
        if roll.is_multiple_of(97) && !model.is_empty() {
            let k = (roll as usize / 97) % model.len().min(70);
            list = list.skip(k);
            model.drain(..k);
        } else {
            let copied = list.push(Value::Int(step as i64));
            model.push(Value::Int(step as i64));
            assert_eq!(
                copied, None,
                "a uniquely held list copied on push at step {step}"
            );
        }
        if step % 977 == 0 {
            assert_eq!(list.len(), model.len());
            assert!(list.iter().eq(model.iter()), "diverged at step {step}");
            assert!(list.iter().rev().eq(model.iter().rev()));
            for i in [0, model.len() / 2, model.len().saturating_sub(1)] {
                assert_eq!(list.get(i), model.get(i));
            }
            assert_eq!(list.get(model.len()), None);
        }
    }
    assert_eq!(list.to_vec(), model);
    assert_eq!(List::from(model.clone()), list);
}

#[test]
fn a_shared_push_copies_one_leaf_and_the_path_above_it_whatever_the_length() {
    let mut worst = 0;
    for n in [1usize, 31, 32, 33, 1_000, 1_024, 1_025, 40_000, 100_000] {
        let base = List::from(ints(n));
        let mut pushed = base.clone();
        let copied = pushed
            .push(Value::Int(-1))
            .expect("a shared push is a copy");
        assert_eq!(pushed.len(), n + 1);
        assert_eq!(base.len(), n, "the shared base moved");
        assert_eq!(pushed.last(), Some(&Value::Int(-1)));
        worst = worst.max(copied);
        let levels = shift_for(n) / BITS + 1;
        assert!(
            copied <= WIDTH * (levels as usize + 1),
            "pushing onto a shared list of {n} copied {copied} slots"
        );
    }
    assert!(worst > 0, "the instrument saw no copy at all");
}

#[test]
fn a_rest_shares_the_list_and_a_chain_of_rests_holds_one_leaf() {
    let list = List::from(ints(2_000));
    let mut cursor = list.clone();
    let mut seen = 0;
    while let Some(head) = cursor.first() {
        assert_eq!(head, &Value::Int(seen));
        seen += 1;
        cursor = cursor.skip(1);
    }
    assert_eq!(seen, 2_000);
    assert_eq!(list.len(), 2_000, "the original moved");
    let mut late = list.skip(1_990);
    assert_eq!(
        late.identity().1,
        0,
        "a rest past the trie still holds the trie"
    );
    assert_eq!(late.push(Value::Int(7)), None);
    assert_eq!(
        late.to_vec(),
        [ints(2_000)[1_990..].to_vec(), vec![Value::Int(7)]].concat()
    );
}

#[test]
fn a_push_onto_a_shared_empty_list_is_a_copy_of_nothing_and_not_an_in_place_write() {
    let empty = List::default();
    let mut pushed = empty.clone();
    assert_eq!(pushed.push(Value::Int(1)), Some(0));
    assert!(empty.is_empty());
    let mut alone = List::default();
    assert_eq!(alone.push(Value::Int(1)), None);
}

#[test]
fn the_header_fits_the_value_the_refusal_to_widen_pins() {
    assert!(size_of::<List>() <= 24);
}
