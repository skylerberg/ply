//! One dump, two implementations: `ply_ty::front` in Rust and `crates/ply-compiler/ply/front.ply`
//! in Ply. Each has to read what the other wrote and write back the same bytes, since the two will
//! be reading each other's cache files for as long as the port is half done.

use ply_cli::commands::common::{build_backend_over, module_texts};
use ply_eval::{BackendKind, BackendSpec, Machine, Value};
use ply_span::frames::Cursor;
use ply_span::{SourceId, Span};
use ply_ty::Front;

const BASE: &str = "pub fn one() -> Int = 1\npub type Coin = | Heads | Tails\n";

/// An effect, an effect set, a spec, a test and a law, so every frame kind is in the dump.
const APP: &str = "import base\n\
                   effect log { write emit(Bytes) -> Unit }\n\
                   effect set Io = {log.write}\n\
                   pub fn say(b: Bytes) -> Unit / {Io} = log.emit(b)\n\
                   pub fn inc(x: Int) -> Int requires x > 0 ensures result > x = x + base::one()\n\
                   test \"inc adds one\" { assert_eq(inc(1), 2) }\n\
                   law \"inc grows\" forall (x: Int) where x > 0 { inc(x) > x }\n";

/// Ply's side: the dump read back and framed again, and the same by way of `split` and `join`.
const PROBE: &str = r#"
import compiler.front (read_front, dump_text, split, join)
import compiler.diag (framed)
import compiler.spine (num)
import compiler.tycore (at, append)

fn refused(what: Bytes, why: Bytes) -> String =
  string_of_bytes(bytes_concat_all([what, b": ", why]))

pub fn round_trip(dump: String) -> String =
  match read_front(bytes_of_string(dump)) {
    Ok(d) -> string_of_bytes(dump_text(d)),
    Err(e) -> refused(b"unread", e),
  }

// Every part of the split, the one no module declares first, each under a `part` frame.
pub fn parts_of(dump: String) -> String =
  match read_front(bytes_of_string(dump)) {
    Err(e) -> refused(b"unread", e),
    Ok(d) ->
      match split(d) {
        Err(e) -> refused(b"unsplit", e),
        Ok(s) -> {
          let one = |i: Int| framed(b"part", num(i), [dump_text(at(s.parts, i))]);
          string_of_bytes(bytes_concat_all(append([framed(b"part", b"_", [dump_text(s.program)])],
                                                  map(range(0, len(s.parts)), one))))
        },
      },
  }

pub fn through_parts(dump: String) -> String =
  match read_front(bytes_of_string(dump)) {
    Err(e) -> refused(b"unread", e),
    Ok(d) ->
      match split(d) {
        Err(e) -> refused(b"unsplit", e),
        Ok(s) ->
          match join(s.program, s.parts) {
            Err(e) -> refused(b"unjoined", e),
            Ok(whole) -> string_of_bytes(dump_text(whole)),
          },
      },
  }
"#;

fn sources() -> Vec<(String, String)> {
    [("base", BASE), ("app", APP)]
        .iter()
        .map(|(name, text)| (name.to_string(), text.to_string()))
        .collect()
}

/// What the shipped module answers for one dump.
fn through_ply(machine: &mut Machine<'_>, entry: &str, dump: &str) -> String {
    let answered = machine
        .call(entry, vec![Value::str(dump)], Span::DUMMY)
        .unwrap_or_else(|d| panic!("`{entry}`: {} [{}]", d.message, d.code));
    let Value::Str(text) = &answered else {
        panic!("`{entry}` answers a String");
    };
    text.to_string()
}

/// Rust's round trip over the same dump: what `read_front` holds, written out again.
fn through_rust(dump: &str, ids: &[SourceId]) -> String {
    let front = ply_ty::read_front(dump, ids).unwrap_or_else(|e| panic!("{e}\n{dump}"));
    ply_ty::write_front(&front, ids).unwrap_or_else(|e| panic!("{e}"))
}

/// The `part` frames a probe answered, as (name, text).
fn parts_from(answer: &str) -> Vec<(String, String)> {
    let mut frames = Cursor::new(answer.as_bytes(), "frame");
    let mut out = Vec::new();
    while !frames.done() {
        let (words, payload) = frames.unit().unwrap_or_else(|e| panic!("{e}\n{answer}"));
        let (kind, name) = match words[..] {
            [kind, name] => (kind, name),
            _ => panic!("`{}` is not `part <name> <length>`", words.join(" ")),
        };
        assert_eq!(kind, "part");
        let text = std::str::from_utf8(payload).expect("a part is UTF-8");
        out.push((name.to_string(), text.to_string()));
    }
    out
}

/// Rust's own parts, the one no module declares first, named as the probe names them.
fn rust_parts(dump: &str, ids: &[SourceId]) -> Vec<(String, String)> {
    let front = ply_ty::read_front(dump, ids).unwrap_or_else(|e| panic!("{e}"));
    let (program, parts) = front.split().unwrap_or_else(|e| panic!("{e}"));
    let program = ply_ty::write_front(&program, &[]).unwrap_or_else(|e| panic!("{e}"));
    let mut out = vec![("_".to_string(), program)];
    for (i, (part, id)) in parts.iter().zip(ids).enumerate() {
        let text =
            ply_ty::write_front(part, std::slice::from_ref(id)).unwrap_or_else(|e| panic!("{e}"));
        out.push((i.to_string(), text));
    }
    out
}

/// A part written out and read back, the way the driver files one.
fn filed(part: &Front, over: &[SourceId]) -> Front {
    let text = ply_ty::write_front(part, over).unwrap_or_else(|e| panic!("{e}"));
    ply_ty::read_front(&text, over).unwrap_or_else(|e| panic!("{e}\n{text}"))
}

/// Rust's split and join over the same dump, each part filed as the driver files it.
fn rust_through_parts(dump: &str, ids: &[SourceId]) -> String {
    let front = ply_ty::read_front(dump, ids).unwrap_or_else(|e| panic!("{e}"));
    let (program, parts) = front.split().unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(parts.len(), ids.len());
    let program = filed(&program, &[]);
    let parts: Vec<Front> = parts
        .iter()
        .zip(ids)
        .map(|(part, id)| filed(part, std::slice::from_ref(id)))
        .collect();
    let joined = Front::join(program, parts).unwrap_or_else(|e| panic!("{e}"));
    ply_ty::write_front(&joined, ids).unwrap_or_else(|e| panic!("{e}"))
}

#[test]
fn the_two_implementations_read_and_write_one_dump_alike() {
    let sources = sources();
    let ids: Vec<SourceId> = (0..sources.len()).map(|i| SourceId(i as u32)).collect();

    // Ply writes, Rust reads, Rust writes: the two writers have to agree byte for byte.
    let ply_dump = ply_codegen::c::producer::front_dump(&sources)
        .unwrap_or_else(|e| panic!("the front end does not answer: {e:#}"));
    let rust_dump = through_rust(&ply_dump, &ids);
    assert_eq!(rust_dump, ply_dump, "the two writers disagree");

    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("probe.ply"), PROBE).unwrap();
    let loaded = ply_cli::load::load(dir.path()).expect("the probe checks against the shelf");
    let spec = BackendSpec {
        kind: BackendKind::C,
    };
    let texts = module_texts(&loaded.check, &loaded.sources);
    let provider =
        build_backend_over(&spec, &loaded.front, texts).expect("this host has a C compiler");
    let mut machine = Machine::new(&loaded.front);
    machine.set_compiled(provider.attach(&spec));

    // Rust wrote that dump; Ply reads it and answers what Rust's own reader answers.
    let ply_answer = through_ply(&mut machine, "probe.round_trip", &rust_dump);
    assert_eq!(ply_answer, rust_dump, "the two readers disagree");

    // Ply wrote that; `ply_ty::read_front` takes it and comes back to the same bytes.
    assert_eq!(through_rust(&ply_answer, &ids), rust_dump);

    // The same answer through the split and the join, on each side.
    let through = through_ply(&mut machine, "probe.through_parts", &rust_dump);
    assert_eq!(through, rust_dump, "Ply's split and join lose the answer");
    assert_eq!(rust_through_parts(&rust_dump, &ids), rust_dump);

    // A part is the same bytes whichever side split it, so each side reads the other's.
    let theirs = parts_from(&through_ply(&mut machine, "probe.parts_of", &rust_dump));
    let ours = rust_parts(&rust_dump, &ids);
    assert_eq!(
        theirs.len(),
        ours.len(),
        "the two splits differ in part count"
    );
    for ((ply_name, ply_text), (rust_name, rust_text)) in theirs.iter().zip(&ours) {
        assert_eq!(ply_name, rust_name, "the parts come in different orders");
        assert_eq!(
            ply_text, rust_text,
            "part `{rust_name}` is spelled differently"
        );
        let over: Vec<SourceId> = match rust_name.as_str() {
            "_" => Vec::new(),
            n => vec![ids[n.parse::<usize>().expect("a part is numbered")]],
        };
        ply_ty::read_front(rust_text, &over).unwrap_or_else(|e| panic!("{rust_name}: {e}"));
        assert_eq!(
            through_ply(&mut machine, "probe.round_trip", rust_text),
            *rust_text
        );
    }
}
