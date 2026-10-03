use ply_codegen::c::{answers, producer};
use ply_eval::Value;

/// Bytes no earlier run asked about, so the first question is always worked out.
fn fresh() -> Value {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    Value::bytes(format!("{}-{nanos}", std::process::id()).as_bytes())
}

#[test]
fn an_answer_is_kept_under_its_emitter_its_entry_and_its_arguments() {
    let args = [fresh()];
    let key = |emitter: &str, entry: &str, args: &[Value]| {
        answers::key(emitter, entry, args).expect("bytes encode")
    };
    let base = key("e1", "hash.hex", &args);
    assert_eq!(base, key("e1", "hash.hex", &args));
    assert_ne!(base, key("e2", "hash.hex", &args));
    assert_ne!(base, key("e1", "hash.other", &args));
    assert_ne!(base, key("e1", "hash.hex", &[fresh()]));
    // A boundary between arguments is part of the key, so moving bytes across one is another key.
    assert_ne!(
        key("e1", "e", &[Value::bytes(b"ab"), Value::bytes(b"c")]),
        key("e1", "e", &[Value::bytes(b"a"), Value::bytes(b"bc")]),
    );
}

#[test]
fn a_kept_answer_reads_back_as_it_was_written() {
    let key = answers::key("e1", "entry", &[fresh()]).expect("bytes encode");
    assert!(
        answers::read(&key).is_none(),
        "nothing was kept under a fresh key"
    );
    let answer = Value::list(vec![Value::bytes(b"one"), Value::Int(2), Value::Bool(true)]);
    answers::write(&key, &answer);
    assert_eq!(answers::read(&key), Some(answer));
}

/// A census counts the emitter's entries, so the second of two equal questions is visible as one
/// that entered nothing.
#[test]
fn an_answer_asked_again_of_one_emitter_is_read_back_rather_than_worked_out() {
    std::thread::spawn(|| {
        producer::ensure_default();
        let args = [fresh()];
        let entries = || producer::census().entries;
        let before = entries();
        let first = producer::call("hash.hex", &args).expect("the entry answers");
        let worked = entries();
        let second = producer::call("hash.hex", &args).expect("the entry answers");
        assert_eq!(first, second);
        assert!(worked > before, "the first question entered the emitter");
        assert_eq!(entries(), worked, "the second was read back");
    })
    .join()
    .unwrap();
}

#[test]
fn a_thread_taking_a_census_works_out_every_answer() {
    std::thread::spawn(|| {
        producer::ensure_default();
        producer::call("hash.hex", &[fresh()]).expect("the emitter builds and answers");
        producer::reset_census();
        let args = [fresh()];
        producer::call("hash.hex", &args).expect("the entry answers");
        producer::call("hash.hex", &args).expect("the entry answers");
        assert_eq!(producer::census().entries, 2);
    })
    .join()
    .unwrap();
}
