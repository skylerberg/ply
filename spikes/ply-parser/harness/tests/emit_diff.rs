//! The eighth comparison: `emit.ply` against `crates/ply-codegen`'s C emitter.
//!
//! The stage the goal names. `lower_diff.rs` says whether a Ply implementation builds the tree a
//! code generator reads; this says whether it writes the C a code generator writes -- byte for
//! byte, placeholders and all.
//!
//! **A nested binary is not in the corpus, and the reason is a finding rather than a gap.** For
//! `a * b + c` the reference emits
//!
//! ```text
//! t1 = unbox(a); t2 = unbox(b); t3 = unbox(c); t4 = mul(t1,t2); t5 = add(t4,t3)
//! ```
//!
//! and this port emits
//!
//! ```text
//! t1 = unbox(a); t2 = unbox(b); t3 = mul(t1,t2); t4 = unbox(c); t5 = add(t3,t4)
//! ```
//!
//! Same arithmetic, same answer, different order: the reference's three unboxes stand at the top
//! of the body, ahead of any arithmetic at all.
//!
//! **It is the function's entry, not the arithmetic.** The reference opens every parameter in
//! declaration order as the body starts, whether the body reads it first, last, or never --
//! `fn unused(a: Int, b: Int) -> Int = b` still binds a dead temporary for `a`, and a `Bool`
//! parameter opens through `rt_unbox_bool_p` rather than the immediate test. Unboxing lazily, at
//! each first read, answers the same and writes the temporaries in the order the *body* happens to
//! reach them. `chained`, `unused` and `onbool` in the corpus below are the three shapes that
//! separate the two, and all three agree.
//!
//! Two earlier notes here were wrong and are gone: the first blamed the emitter's operand
//! sequencing, the second blamed `optimize`'s reordering. `reference_lower_dump` on `chained` is
//! `bbin(add,bbin(mul,ovar(a,0),ovar(b,1)),ovar(c,2))` -- the tree this port lowers -- so the two
//! stages never disagreed about the input, and nothing was reordered.
//!
//! **The corpus here is hand-written and small, and that is deliberate.** The reference optimises
//! before it lowers, so `1 + 2` reaches its emitter as `3` while this port's emitter sees the
//! addition; a shipped corpus would report that difference on every foldable constant and the
//! difference would be `opt.rs`'s rather than the emitter's. `tests/emit.rs` records the coupling.
//! Widening this corpus is what porting `fold_literals` would buy.

use ply_parser_spike_harness::{byte_literal, reference_emit_dump};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .expect("the harness sits at <root>/spikes/ply-parser/harness")
        .to_path_buf()
}

fn spike_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("the harness sits inside the spike")
        .to_path_buf()
}

/// The shipping binary, release first: `examples/desk.ply` is 160 kilobytes and the debug
/// interpreter is several times slower on it.
fn ply_binary() -> PathBuf {
    if let Ok(explicit) = std::env::var("PLY_BIN") {
        let path = PathBuf::from(explicit);
        assert!(
            path.exists(),
            "PLY_BIN names {}, which does not exist",
            path.display()
        );
        return path;
    }
    let root = repo_root();
    for profile in ["release", "debug"] {
        let candidate = root.join("target").join(profile).join("ply");
        if candidate.exists() {
            return candidate;
        }
    }
    panic!(
        "no `ply` binary at {}/target/{{release,debug}}/ply — run \
         `cargo build -p ply-cli --bin ply --release` first, or set PLY_BIN",
        root.display()
    );
}

/// The six modules the parser is, copied into a scratch project.
fn source_dir() -> PathBuf {
    match std::env::var("PLY_PARSER_SRC") {
        Ok(d) => PathBuf::from(d),
        Err(_) => spike_dir(),
    }
}

const MODULES: [&str; 9] = [
    "lexer.ply",
    "spine.ply",
    "types.ply",
    "patterns.ply",
    "exprs.ply",
    "items.ply",
    "rewrite.ply",
    "code.ply",
    "emit.ply",
];

struct Project(PathBuf);

impl Project {
    fn new(label: &str) -> Project {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "ply-parser-spike-{}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed),
            label.replace(['/', '.'], "_")
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a temp directory");
        for m in MODULES {
            std::fs::copy(source_dir().join(m), dir.join(m))
                .unwrap_or_else(|e| panic!("copying {m} from {}: {e}", source_dir().display()));
        }
        Project(dir)
    }

    /// The same, with each input's module name, so the dump names bodies the way the reference
    /// does and the two can be matched up.
    fn dumps_in(&self, names: &[String], inputs: &[Vec<u8>], ctors: &[String]) -> Vec<String> {
        let mut src = String::from(
            "// Generated by spikes/ply-parser/harness/tests/emit_diff.rs. Not checked in.\n\
             import emit (emit_dump_in)\n",
        );
        src.push_str(&format!(
            "fn ctors() -> List<Bytes> = [{}]\n",
            ctors
                .iter()
                .map(|c| byte_literal(c.as_bytes()))
                .collect::<Vec<_>>()
                .join(", ")
        ));
        let mut parts: Vec<String> = Vec::new();
        for (i, bytes) in inputs.iter().enumerate() {
            src.push_str(&format!("fn s{i}() -> Bytes = {}\n", byte_literal(bytes)));
            src.push_str(&format!(
                "fn n{i}() -> Bytes = {}\n",
                byte_literal(names[i].as_bytes())
            ));
            parts.push(format!(
                "bytes_of_string(emit_dump_in(n{i}(), ctors(), s{i}()))"
            ));
            parts.push("b\"~\"".to_string());
        }
        src.push_str(&format!(
            "fn main() -> String = string_of_bytes(bytes_concat_all([{}]))\n",
            parts.join(", ")
        ));
        self.run_probe(&src, inputs.len())
    }

    /// One `ply run` for many inputs, each its own one-module program.
    fn dumps(&self, inputs: &[Vec<u8>]) -> Vec<String> {
        let mut src = String::from(
            "// Generated by spikes/ply-parser/harness/tests/emit_diff.rs. Not checked in.\n\
             import emit (emit_dump_in)\n",
        );
        let mut parts: Vec<String> = Vec::new();
        for (i, bytes) in inputs.iter().enumerate() {
            src.push_str(&format!("fn s{i}() -> Bytes = {}\n", byte_literal(bytes)));
            let ctors = ply_parser_spike_harness::reference_ctors(&[(
                "m".to_string(),
                String::from_utf8_lossy(bytes).to_string(),
            )]);
            src.push_str(&format!(
                "fn c{i}() -> List<Bytes> = [{}]\n",
                ctors
                    .iter()
                    .map(|c| byte_literal(c.as_bytes()))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
            parts.push(format!(
                "bytes_of_string(emit_dump_in(b\"m\", c{i}(), s{i}()))"
            ));
            parts.push("b\"~\"".to_string());
        }
        src.push_str(&format!(
            "fn main() -> String = string_of_bytes(bytes_concat_all([{}]))\n",
            parts.join(", ")
        ));
        self.run_probe(&src, inputs.len())
    }

    /// Writes the probe, runs it, and splits the dumps back out.
    fn run_probe(&self, src: &str, n: usize) -> Vec<String> {
        std::fs::write(self.0.join("probe.ply"), src).expect("write the probe");
        // The cache keys on the project's modules, and `probe.ply` changes every call, so this is
        // belt and braces — but a stale hit here would be a green comparison against a dump of some
        // other input, which is the one failure this harness must not have.
        let _ = std::fs::remove_dir_all(self.0.join(".ply-cache"));

        let out = Command::new(ply_binary())
            .arg("run")
            .args(backend_args())
            .arg(&self.0)
            .arg("--json")
            .output()
            .expect("run ply");
        let stdout = String::from_utf8_lossy(&out.stdout).to_string();
        let stderr = String::from_utf8_lossy(&out.stderr).to_string();
        assert!(
            out.status.success(),
            "`ply run {}` failed ({}):\n{stdout}\n{stderr}",
            self.0.display(),
            out.status
        );
        let joined = extract_value(&stdout, &self.0);
        let mut parts: Vec<String> = joined.split('~').map(str::to_string).collect();
        let last = parts.pop().expect("split yields at least one part");
        assert!(
            last.is_empty() && parts.len() == n,
            "the probe answered {} dumps for {} inputs",
            parts.len(),
            n
        );
        parts
    }
}

impl Drop for Project {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// The `value` field of `ply run --json`, unwrapped.
fn extract_value(stdout: &str, dir: &Path) -> String {
    let needle = "\n  \"value\": \"";
    let occurrences = stdout.matches(needle).count();
    assert_eq!(
        occurrences,
        1,
        "`ply run {}` printed {occurrences} top-level `value` fields:\n{stdout}",
        dir.display()
    );
    let after = &stdout[stdout.find(needle).expect("checked above") + needle.len()..];
    let quote = "\\\"";
    assert!(
        after.starts_with(quote),
        "`ply run {}` answered with something that is not a rendered `String`:\n{stdout}",
        dir.display()
    );
    let rest = &after[quote.len()..];
    let end = rest
        .find(quote)
        .unwrap_or_else(|| panic!("the rendered `String` is never closed in:\n{stdout}"));
    let body = &rest[..end];
    // Emitted C has newlines in it, which `--json` escapes. The other differentials refuse an
    // escape because their dumps are printable ASCII by construction; this one decodes, and a
    // decoder written by hand rather than by `replace` because the escaping is layered and a
    // single pass leaves a stray backslash that reads as a difference in the C.
    // Twice, because the escaping is layered: `ply run --json` escapes the newline and the shell
    // capture escapes the escape, so one pass leaves the `n` behind and it reads as a difference
    // in the C rather than in the transport.
    let once = unescape(body);
    unescape(&once)
}

fn unescape(body: &str) -> String {
    let mut out = String::with_capacity(body.len());
    let mut it = body.chars().peekable();
    while let Some(c) = it.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match it.peek() {
            Some('n') => {
                it.next();
                out.push('\n');
            }
            Some('\\') => {
                it.next();
                out.push('\\');
            }
            _ => out.push(c),
        }
    }
    out
}

/// The bodies an emitter produced, by the name each is for.
fn by_name(dump: &str) -> std::collections::BTreeMap<String, String> {
    let mut out = std::collections::BTreeMap::new();
    for record in dump.split(";f:").skip(1) {
        let Some((name, body)) = record.split_once(';') else {
            continue;
        };
        out.insert(name.to_string(), body.trim_end_matches(';').to_string());
    }
    out
}

/// Every body the port emitted is the body the reference emits.
fn compare(label: &str, inputs: &[(String, Vec<u8>)]) -> (usize, usize) {
    let project = Project::new(label);
    let texts: Vec<Vec<u8>> = inputs.iter().map(|(_, t)| t.clone()).collect();
    let got = project.dumps(&texts);
    let mut failures: Vec<String> = Vec::new();
    let (mut reached, mut available) = (0usize, 0usize);
    for ((name, text), actual) in inputs.iter().zip(&got) {
        let source = String::from_utf8_lossy(text).to_string();
        let expected = by_name(&reference_emit_dump(&[("m".to_string(), source)]));
        let mine = by_name(actual);
        available += expected.len();
        reached += mine.len();
        for (fname, body) in &mine {
            match expected.get(fname) {
                None => failures.push(format!(
                    "{label}: `{fname}` in {name} was emitted by the port and not by the reference"
                )),
                Some(want) if want != body => failures.push(format!(
                    "{label}: the two emitters disagree on `{fname}` in {name}:\n--- reference\n{want}\n--- port\n{body}"
                )),
                Some(_) => {}
            }
        }
    }
    println!("  {label}: {reached} of {available} body/bodies emitted, all agreeing");
    assert!(
        failures.is_empty(),
        "{} disagreement(s)\n\n{}",
        failures.len(),
        failures.join("\n")
    );
    (reached, available)
}

/// Every body the port emits is the body the reference emits, and it emits some.
#[test]
fn the_emitter_agrees_with_ply_codegen_wherever_the_port_reaches() {
    // Integer arithmetic over parameters: what the port covers, written out rather than mined, so
    // that widening the port means widening this list and nothing else.
    let programs: &[&str] = &[
        "fn g(a: Int, b: Int) -> Int = a * b\n",
        "fn h(a: Int, b: Int) -> Int = a + b\n",
        "fn k(a: Int, b: Int) -> Int = a - b\n",
        "fn d(a: Int, b: Int) -> Int = a / b\n",
        "fn r(a: Int, b: Int) -> Int = a % b\n",
        "fn lt(a: Int, b: Int) -> Bool = a < b\n",
        "fn le(a: Int, b: Int) -> Bool = a <= b\n",
        "fn gt(a: Int, b: Int) -> Bool = a > b\n",
        "fn ge(a: Int, b: Int) -> Bool = a >= b\n",
        "fn eq(a: Int, b: Int) -> Bool = a == b\n",
        "fn ne(a: Int, b: Int) -> Bool = a != b\n",
        // The shape that showed the port unboxing per read where the reference binds once.
        "fn nested(a: Int, b: Int) -> Int = (a + b) * (a - b)\n",
        "fn mx(a: Int, b: Int) -> Int = if a < b { b } else { a }\n",
        "fn mn(a: Int, b: Int) -> Int = if a < b { a } else { b }\n",
        // Nested, so the join of the inner `if` is a branch value of the outer.
        "fn clamp3(a: Int, b: Int) -> Int = if a < b { if a < 0 { 0 } else { a } } else { b }\n",
        // A branch that unboxes a slot the other branch never reads: the
        // temporary is block-scoped and must not be reused after the brace.
        "fn pick(a: Int, b: Int, c: Int) -> Int = (if c < 0 { a } else { b }) + a\n",
        // The entry prologue: every parameter is opened, in order, whether the
        // body reads it first, last or not at all, and a `Bool` opens through
        // its own helper.
        "fn chained(a: Int, b: Int, c: Int) -> Int = a * b + c\n",
        "fn unused(a: Int, b: Int) -> Int = b\n",
        "fn onbool(a: Int, b: Bool) -> Int = if b { a } else { 0 }\n",
        // Blocks: a binding is renamed even when its value is already a
        // temporary, and the block binds its own tail on the way out.
        "fn onelet(a: Int, b: Int) -> Int = {\n  let c = a + b;\n  c - a\n}\n",
        "fn twolet(a: Int, b: Int) -> Int = {\n  let c = a + b;\n  let d = c * c;\n  d - b\n}\n",
        "fn letif(a: Int, b: Int) -> Int = {\n  let c = if a < b { b } else { a };\n  c + 1\n}\n",
        // A binding inside a branch, whose slot the other branch never fills.
        "fn iflet(a: Int, b: Int) -> Int = if a < b {\n  let c = a * 2;\n  c + b\n} else { b }\n",
        // Lists: each item boxed into a `Word` local, then the array, then the helper.
        "fn lst(a: Int) -> List<Int> = [a, 1]\n",
        "fn lst1(a: Int, b: Int) -> List<Int> = [a + b]\n",
        "fn lst0() -> List<Int> = []\n",
        // The unary pair, and the literal that is not an `Int`.
        "fn tru() -> Bool = true\n",
        "fn fls() -> Bool = false\n",
        "fn neg(a: Int) -> Int = -a\n",
        "fn nt(b: Bool) -> Bool = !b\n",
        "fn ntc(a: Int, b: Int) -> Bool = !(a < b)\n",
        "fn negif(a: Int, b: Int) -> Int = if a < b { -a } else { -b }\n",
        // A field is read at its place among the record's names *sorted*, which
        // is the checker's order rather than the declaration's: `.z` below is 1.
        "fn fz(r: { z: Int, a: Int }) -> Int = r.z\n",
        "fn fa(r: { z: Int, a: Int }) -> Int = r.a\n",
        "fn f2(r: { z: Int, a: Int }) -> Int = r.z + r.a\n",
        // A record is built in the order its fields are written and assembled in
        // the order the shape holds them, which is by name.
        "fn mk(n: Int) -> { z: Int, a: Int } = { z: n, a: 1 }\n",
        "fn mk1(n: Int) -> { only: Int } = { only: n + 1 }\n",
        // Built here and read here: the read is answered from the register the
        // value is already in, and the record is still built because something
        // else might ask for its word.
        "fn mkf(n: Int) -> Int = { z: n, a: 1 }.a\n",
        // A `match` is arms over one flag, and a body that falls off the end raises.
        "fn sign(n: Int) -> Int = match n { 0 -> 0, _ -> if n < 0 { n } else { 1 } }\n",
        "fn bindm(n: Int) -> Int = match n { 1 -> 10, k -> k + 1 }\n",
        "fn three(n: Int) -> Int = match n { 0 -> 1, 1 -> 2, 2 -> 3, _ -> 0 }\n",
        // Constructors: a tagged object, and the test that reads its tag back.
        "type T = | A(Int) | B\nfn mkc(n: Int) -> T = A(n)\n",
        "type T = | A(Int) | B\nfn nb() -> T = B\n",
        "type T = | A(Int) | B\nfn rd(t: T) -> Int = match t { A(x) -> x, B -> 0 }\n",
        "type P = | P2(Int, Int)\nfn both(a: Int, b: Int) -> Int = match P2(a, b) { P2(x, y) -> x + y }\n",
        // A lambda is a closure *and* a function written after the one that
        // builds it, and calling one through a name is `rt_call_p`. Neither
        // fuses: `apply` takes the closure as a value.
        "fn apply(f: (Int) -> Int, n: Int) -> Int = f(n)\nfn mkl(k: Int) -> Int = apply(|x: Int| x + k, k)\n",
        // Deliberately absent, and worth saying why: a record built in both arms
        // of an `if` is *deferred* by the reference -- neither arm builds one,
        // and the join carries the fields as separate temporaries. A record of
        // immediates whose every read is answered from the built table is never
        // looked at, so building it is an allocation and a row of stores nothing
        // observes. That is `deferred` and `record_locals` in `c/emit.rs`, and
        // this port does not reach them:
        //   fn two(n: Int) -> { z: Int, a: Int } =
        //     if n < 0 { { z: n, a: 0 } } else { { z: 0, a: n } }
    ];
    let inputs: Vec<(String, Vec<u8>)> = programs
        .iter()
        .enumerate()
        .map(|(i, p)| (format!("program {i}"), p.as_bytes().to_vec()))
        .collect();
    let (reached, available) = compare("arithmetic", &inputs);
    // A port that emitted nothing would agree with the reference on every body it produced.
    assert_eq!(
        reached, available,
        "the port emitted {reached} of the {available} bodies the reference did"
    );
    assert!(reached >= 47, "only {reached} bodies were emitted");
}

/// `--backend` for every `ply` this differential runs.
fn backend_args() -> Vec<String> {
    match std::env::var("PLY_BACKEND").as_deref() {
        Ok("none") => Vec::new(),
        Ok(other) => vec!["--backend".to_string(), other.to_string()],
        Err(_) => vec!["--backend".to_string(), "cranelift".to_string()],
    }
}

/// The same comparison over the **shipped** corpus rather than over shapes chosen to exercise one
/// node.
///
/// The hand-written corpus above says a form is right. This says how much of the language the port
/// has, which is the number that decides where to work next -- and it is the number that will be
/// wrong if a form only looks right on an input written to make it look right. Every body the port
/// emits must be the body the reference emits, byte for byte, and the count may only rise.
#[test]
fn the_port_agrees_with_the_reference_over_the_shipped_corpus() {
    let inputs = shipped();
    let expected = by_name(&reference_emit_dump(&inputs));
    assert!(
        expected.len() > 500,
        "the reference emitted only {} bodies, so this proves little",
        expected.len()
    );
    let project = Project::new("shipped");
    let texts: Vec<Vec<u8>> = inputs.iter().map(|(_, t)| t.as_bytes().to_vec()).collect();
    let names: Vec<String> = inputs.iter().map(|(n, _)| n.clone()).collect();
    let ctors = ply_parser_spike_harness::reference_ctors(&inputs);
    let dumps = project.dumps_in(&names, &texts, &ctors);
    let (mut reached, mut differ) = (0usize, Vec::new());
    for dump in &dumps {
        for (name, body) in by_name(dump) {
            let Some(want) = expected.get(&name) else {
                // The reference refused it; there is nothing to compare against.
                continue;
            };
            reached += 1;
            if want != &body {
                if differ.is_empty() {
                    println!("--- {name}, reference\n{want}\n--- {name}, port\n{body}");
                }
                differ.push(name);
            }
        }
    }
    println!(
        "  the shipped corpus: the port emits {reached} of the {} bodies the reference does",
        expected.len()
    );
    // Named, not tolerated. Each of these needs a *deferred* record: the reference declares the
    // local the record will land in at the top of the body, so that materialising it inside a
    // branch still names something the whole body can see, and a record whose every read is
    // answered from the built table is never materialised at all. The port does not reach it.
    //
    // Asserted as an equality rather than a subset, so a new disagreement fails here and fixing
    // `deferred` shrinks this list rather than leaving it to rot.
    let expected_gaps = ["std.hash.padded_words"];
    assert_eq!(
        differ, expected_gaps,
        "the disagreements are not the ones this test knows about"
    );
    assert!(
        reached >= 279,
        "the port emitted {reached} shipped bodies -- raise this when it grows, never lower it"
    );
}

/// The shipped standard library and examples, named as the reference names them.
fn shipped() -> Vec<(String, String)> {
    let root = source_dir()
        .parent()
        .and_then(std::path::Path::parent)
        .expect("the spike sits two levels under the repository root")
        .to_path_buf();
    let mut out = Vec::new();
    for dir in ["crates/ply-std/ply", "examples"] {
        let Ok(entries) = std::fs::read_dir(root.join(dir)) else {
            continue;
        };
        let mut paths: Vec<std::path::PathBuf> = entries
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|e| e == "ply"))
            .collect();
        paths.sort();
        for p in paths {
            let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or("m");
            let name = if dir.ends_with("ply-std/ply") {
                format!("std.{stem}")
            } else {
                stem.to_string()
            };
            if let Ok(text) = std::fs::read_to_string(&p) {
                out.push((name, text));
            }
        }
    }
    out
}
