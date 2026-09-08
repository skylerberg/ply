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
        // The whole front end: the emitter resolves a program before it emits, so it imports the
        // resolver, and what the resolver imports.
        let entries = std::fs::read_dir(source_dir())
            .unwrap_or_else(|e| panic!("{}: {e}", source_dir().display()));
        for entry in entries.filter_map(|e| e.ok()) {
            let path = entry.path();
            if path.extension().is_some_and(|x| x == "ply") {
                let name = path.file_name().expect("a file name");
                std::fs::copy(&path, dir.join(name))
                    .unwrap_or_else(|e| panic!("copying {}: {e}", path.display()));
            }
        }
        Project(dir)
    }

    /// The same, with each input's module name, so the dump names bodies the way the reference
    /// does and the two can be matched up.
    fn dumps_in(&self, names: &[String], inputs: &[Vec<u8>], ctors: &[String]) -> Vec<String> {
        let mut src = String::from(
            "// Generated by spikes/ply-parser/harness/tests/emit_diff.rs. Not checked in.\n\
             import emit (emit_dump_all)\n",
        );
        src.push_str(&format!(
            "fn builtins() -> List<Bytes> = [{}]\nfn ctors() -> List<Bytes> = [{}]\n",
            ply_parser_spike_harness::reference_builtins()
                .iter()
                .map(|b| byte_literal(b.as_bytes()))
                .collect::<Vec<_>>()
                .join(", "),
            ctors
                .iter()
                .map(|c| byte_literal(c.as_bytes()))
                .collect::<Vec<_>>()
                .join(", ")
        ));
        // One program, every module resolved together: the whole corpus is one dump.
        src.push_str(&format!(
            "fn names() -> List<Bytes> = [{}]\nfn srcs() -> List<Bytes> = [{}]\n",
            names
                .iter()
                .map(|n| byte_literal(n.as_bytes()))
                .collect::<Vec<_>>()
                .join(", "),
            inputs
                .iter()
                .map(|b| byte_literal(b))
                .collect::<Vec<_>>()
                .join(", ")
        ));
        src.push_str(
            "fn main() -> String = string_of_bytes(bytes_concat_all([bytes_of_string(emit_dump_all(names(), srcs(), ctors(), builtins())), b\"~\"]))\n",
        );
        self.run_probe(&src, 1)
    }

    /// One `ply run` for many inputs, each its own one-module program.
    fn dumps(&self, inputs: &[Vec<u8>]) -> Vec<String> {
        let mut src = String::from(
            "// Generated by spikes/ply-parser/harness/tests/emit_diff.rs. Not checked in.\n\
             import emit (emit_dump_in)\n",
        );
        src.push_str(&format!(
            "fn bi() -> List<Bytes> = [{}]\n",
            ply_parser_spike_harness::reference_builtins()
                .iter()
                .map(|b| byte_literal(b.as_bytes()))
                .collect::<Vec<_>>()
                .join(", ")
        ));
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
                "bytes_of_string(emit_dump_in(b\"m\", c{i}(), bi(), s{i}()))"
            ));
            parts.push("b\"~\"".to_string());
        }
        src.push_str(&format!(
            "fn main() -> String = string_of_bytes(bytes_concat_all([{}]))\n",
            parts.join(", ")
        ));
        self.run_probe(&src, inputs.len())
    }

    /// The producer's rendering: every body of the program, framed by length, as one string.
    fn bodies_in(&self, names: &[String], inputs: &[Vec<u8>], ctors: &[String]) -> String {
        let mut src = String::from(
            "// Generated by spikes/ply-parser/harness/tests/emit_diff.rs. Not checked in.\n\
             import emit (emit_bodies_all)\n",
        );
        src.push_str(&format!(
            "fn builtins() -> List<Bytes> = [{}]\nfn ctors() -> List<Bytes> = [{}]\n",
            ply_parser_spike_harness::reference_builtins()
                .iter()
                .map(|b| byte_literal(b.as_bytes()))
                .collect::<Vec<_>>()
                .join(", "),
            ctors
                .iter()
                .map(|c| byte_literal(c.as_bytes()))
                .collect::<Vec<_>>()
                .join(", ")
        ));
        src.push_str(&format!(
            "fn names() -> List<Bytes> = [{}]\nfn srcs() -> List<Bytes> = [{}]\n",
            names
                .iter()
                .map(|n| byte_literal(n.as_bytes()))
                .collect::<Vec<_>>()
                .join(", "),
            inputs
                .iter()
                .map(|b| byte_literal(b))
                .collect::<Vec<_>>()
                .join(", ")
        ));
        src.push_str(
            "fn main() -> String = emit_bodies_all(names(), srcs(), ctors(), builtins())\n",
        );
        std::fs::write(self.0.join("probe.ply"), src).expect("write the probe");
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
        extract_value(&stdout, &self.0)
    }

    /// Every body the port refused over the program, as `<name>\t<reason>` lines.
    fn refusals_in(&self, names: &[String], inputs: &[Vec<u8>], ctors: &[String]) -> String {
        let mut src = String::from(
            "// Generated by spikes/ply-parser/harness/tests/emit_diff.rs. Not checked in.\n\
             import emit (emit_refusals_all)\n",
        );
        src.push_str(&format!(
            "fn builtins() -> List<Bytes> = [{}]\nfn ctors() -> List<Bytes> = [{}]\n",
            ply_parser_spike_harness::reference_builtins()
                .iter()
                .map(|b| byte_literal(b.as_bytes()))
                .collect::<Vec<_>>()
                .join(", "),
            ctors
                .iter()
                .map(|c| byte_literal(c.as_bytes()))
                .collect::<Vec<_>>()
                .join(", ")
        ));
        src.push_str(&format!(
            "fn names() -> List<Bytes> = [{}]\nfn srcs() -> List<Bytes> = [{}]\n",
            names
                .iter()
                .map(|n| byte_literal(n.as_bytes()))
                .collect::<Vec<_>>()
                .join(", "),
            inputs
                .iter()
                .map(|b| byte_literal(b))
                .collect::<Vec<_>>()
                .join(", ")
        ));
        src.push_str(
            "fn main() -> String = emit_refusals_all(names(), srcs(), ctors(), builtins())\n",
        );
        std::fs::write(self.0.join("probe.ply"), src).expect("write the probe");
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
        extract_value(&stdout, &self.0)
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
        // A record built in *both* arms: the join carries the fields, not the
        // word, and a third record is assembled from them after the arms close.
        "fn two(n: Int) -> { z: Int, a: Int } = if n < 0 { { z: n, a: 0 } } else { { z: 0, a: n } }\n",
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
    assert!(reached >= 48, "only {reached} bodies were emitted");
}

/// `--backend` for every `ply` this differential runs.
fn backend_args() -> Vec<String> {
    match std::env::var("PLY_BACKEND").as_deref() {
        Ok("none") => Vec::new(),
        Ok(other) => vec!["--backend".to_string(), other.to_string()],
        Err(_) => vec!["--backend".to_string(), "c".to_string()],
    }
}

/// The same comparison over the **shipped** corpus rather than over shapes chosen to exercise one
/// node, on what the C tier reads when this emitter is its producer: the text **and the tables**
/// it names by its own positions, in the cache's encoding.
///
/// The hand-written corpus above says a form is right. This says how much of the language the port
/// has, which is the number that decides where to work next -- and it is the number that will be
/// wrong if a form only looks right on an input written to make it look right. Two bodies agree
/// when they resolve to the same C: the same text over tables numbered differently is one body,
/// and the same text over different constants is two.
///
/// A body the port emits differently from the reference is not wrong by that alone -- it runs
/// under the tier's audit like every body the reference emits, and that audit is the oracle for
/// what the port writes. It is slower, or it is the reference's rule the port has not taken yet,
/// and the census below says which. So this holds two floors and one ceiling rather than a list of
/// names: the bodies reached and the bodies agreeing may only rise, and the bodies disagreeing may
/// only fall.
#[test]
fn the_port_resolves_to_the_references_c_over_the_shipped_corpus() {
    let inputs = shipped();
    let expected = ply_parser_spike_harness::reference_emit_encoded(&inputs);
    assert!(
        expected.len() > 500,
        "the reference emitted only {} bodies, so this proves little",
        expected.len()
    );
    let project = Project::new("shipped");
    let texts: Vec<Vec<u8>> = inputs.iter().map(|(_, t)| t.as_bytes().to_vec()).collect();
    let names: Vec<String> = inputs.iter().map(|(n, _)| n.clone()).collect();
    let ctors = ply_parser_spike_harness::reference_ctors(&inputs);
    let mine = framed(&project.bodies_in(&names, &texts, &ctors));
    let (mut reached, mut agreeing, mut differ) = (0usize, 0usize, Vec::new());
    for (name, enc) in &mine {
        let Some(want) = expected.get(name) else {
            // The reference refused it; there is nothing to compare against.
            continue;
        };
        reached += 1;
        let (mine, theirs) = (resolved(enc), resolved(want));
        // `PLY_EMIT_DIFF_SHOW=a.b,c.d` prints the named bodies from both sides, agreeing or
        // not; without it the first disagreement is printed.
        let show =
            std::env::var("PLY_EMIT_DIFF_SHOW").is_ok_and(|s| s.split(',').any(|n| n == name));
        if show || (mine != theirs && differ.is_empty()) {
            println!("--- {name}, reference\n{theirs}\n--- {name}, port\n{mine}");
        }
        if mine == theirs {
            agreeing += 1;
        } else {
            differ.push(name.clone());
        }
    }
    println!(
        "  the shipped corpus: the port emits {reached} of the {} bodies the reference does, \
         {agreeing} resolving to the reference's C",
        expected.len()
    );
    // `PLY_EMIT_DIFF_LIST=1` prints every body the port reached, one per line and marked where it
    // disagrees, so two runs can be diffed for what one change reached, lost, or opened.
    if std::env::var("PLY_EMIT_DIFF_LIST").is_ok() {
        for n in mine.keys().filter(|n| expected.contains_key(*n)) {
            let mark = if differ.contains(n) {
                "differs"
            } else {
                "agrees"
            };
            println!("reached {n} {mark}");
        }
    }
    println!("  disagreeing: {}", differ.join(" "));
    // Lowered once, on purpose, from 279 to 278, when the `if` join stopped guessing: where the
    // two arms answer different kinds the reference reads the checker's type and this port cannot,
    // so it refuses rather than write the wrong conversion into both. A correct refusal is worth
    // more than a body. It has since risen well past that.
    assert!(
        reached >= 1213,
        "the port emitted {reached} shipped bodies -- raise this when it grows, and lower it only \
         for a refusal that is more correct than what it replaces"
    );
    assert!(
        agreeing >= 1207,
        "{agreeing} shipped bodies resolve to the reference's C -- raise this when it grows"
    );
    // What disagrees is six `std.hash` bodies. Four want the *deferred record local*, the half
    // of deferring this port does not do: a record whose every read is answered from the built
    // table is never materialised, and the local it would land in is declared at the top of the
    // body. `std.hash.round` and `std.hash.blake3` read declared `U32` fields at their width
    // where the port reads words; the port carries one width and the reference six.
    assert!(
        differ.len() <= 6,
        "{} shipped bodies disagree with the reference -- lower this when they close, and raise \
         it only for a body that is right by the audit and slower on purpose",
        differ.len()
    );
}

/// `body <name> <n>\n` and then `n` bytes, repeated: the framing the C tier's producer reads.
fn framed(dump: &str) -> std::collections::BTreeMap<String, String> {
    let mut out = std::collections::BTreeMap::new();
    let bytes = dump.as_bytes();
    let mut at = 0usize;
    while at < bytes.len() {
        let line_end = at + dump[at..].find('\n').expect("a frame header ends");
        let header = &dump[at..line_end];
        let mut parts = header.split(' ');
        assert_eq!(parts.next(), Some("body"), "a frame header: {header:?}");
        let name = parts.next().expect("a frame names its body").to_string();
        let n: usize = parts
            .next()
            .expect("a frame has a length")
            .parse()
            .expect("a frame's length is a number");
        let start = line_end + 1;
        out.insert(name, dump[start..start + n].to_string());
        at = start + n;
    }
    out
}

/// A body with every placeholder replaced by the entry it names, so two bodies compare on what
/// their C *means* rather than on how their tables are numbered. An entry nothing names -- the
/// reference pools a constant it then does not use, in two bodies -- is not a difference.
fn resolved(enc: &str) -> String {
    // The tables are read by their counts, not by searching for the `text` line: a field or a
    // call can be named `text` too.
    let mut entries: std::collections::HashMap<char, Vec<String>> =
        std::collections::HashMap::new();
    let mut rest = enc;
    loop {
        let Some(nl) = rest.find('\n') else {
            break;
        };
        let head = &rest[..nl];
        rest = &rest[nl + 1..];
        if head == "text" {
            break;
        }
        let Some((what, n)) = head.split_once(' ') else {
            panic!("a table header, not {head:?}");
        };
        let n: usize = n.parse().expect("a table header counts its entries");
        let kind = match what {
            "consts" => 'c',
            "builtins" => 'b',
            "fields" => 'f',
            "shapes" => 's',
            "lambdas" => 'l',
            "calls" => 'x',
            other => panic!("a table this encoding does not have: {other}"),
        };
        let mut taken = Vec::with_capacity(n);
        for _ in 0..n {
            let end = rest.find('\n').expect("a table entry ends");
            taken.push(rest[..end].to_string());
            rest = &rest[end + 1..];
        }
        entries.insert(kind, taken);
    }
    let text = rest;
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find("@@") {
        out.push_str(&rest[..at]);
        let body = &rest[at + 2..];
        let end = body.find("@@").expect("a placeholder closes");
        let (kind, digits) = body[..end].split_at(1);
        let i: usize = digits.parse().expect("a placeholder is numbered");
        let entry = entries
            .get(&kind.chars().next().expect("a kind"))
            .and_then(|es| es.get(i))
            .cloned()
            .unwrap_or_else(|| format!("<missing {kind}{i}>"));
        out.push_str(&format!("<{kind}:{entry}>"));
        rest = &body[end + 2..];
    }
    out.push_str(rest);
    // The calls are not named by the text and gate a cached body's reuse, so they are part of
    // what has to agree.
    if let Some(calls) = entries.get(&'x') {
        out.push_str("\ncalls: ");
        out.push_str(&calls.join(","));
    }
    out
}

/// What keeps the port out of the bodies the reference emits and it does not reach: for every
/// such body, the node tags in its lowered form, aggregated. An instrument, printed rather than
/// asserted, so the next increment on the port goes where the bodies are.
#[test]
fn the_census_of_what_keeps_the_port_out() {
    let inputs = shipped();
    let expected = by_name(&reference_emit_dump(&inputs));
    let project = Project::new("census");
    let texts: Vec<Vec<u8>> = inputs.iter().map(|(_, t)| t.as_bytes().to_vec()).collect();
    let names: Vec<String> = inputs.iter().map(|(n, _)| n.clone()).collect();
    let ctors = ply_parser_spike_harness::reference_ctors(&inputs);
    let reached: std::collections::BTreeSet<String> = project
        .dumps_in(&names, &texts, &ctors)
        .iter()
        .flat_map(|d| by_name(d).into_keys())
        .collect();
    // Each body's lowered form, from the oracle for the stage before the emitter.
    let lowered = ply_parser_spike_harness::reference_lower_dump(&inputs);
    let mut forms: std::collections::BTreeMap<String, String> = std::collections::BTreeMap::new();
    for record in lowered.split(";f:").skip(1) {
        let Some((head, body)) = record.split_once(';') else {
            continue;
        };
        let name = head.split(':').next().unwrap_or_default().to_string();
        forms.insert(name, body.to_string());
    }
    /// The node tags of one lowered form, each stripped of the ownership letter in front of it.
    fn tags_of(form: &str) -> std::collections::BTreeSet<&'static str> {
        const TAGS: [&str; 21] = [
            "lit", "var", "un", "bin", "app", "if", "block", "let", "do", "list", "rec", "match",
            "arm", "perform", "cell", "region", "sim", "handle", "lam", "fld", "upd",
        ];
        let mut out = std::collections::BTreeSet::new();
        for tag in TAGS {
            for own in ["b", "o", "f", ""] {
                if form.contains(&format!("{own}{tag}(")) {
                    out.insert(tag);
                }
            }
        }
        out
    }
    let mut in_unreached: std::collections::BTreeMap<&str, usize> = Default::default();
    let mut in_reached: std::collections::BTreeMap<&str, usize> = Default::default();
    let (mut unreached, mut test_roots, mut reached_n) = (0usize, 0usize, 0usize);
    for name in expected.keys() {
        let Some(form) = forms.get(name) else {
            // A test root: the reference synthesizes a body per test, and the port emits none.
            if !reached.contains(name) {
                test_roots += 1;
            }
            continue;
        };
        let counts = if reached.contains(name) {
            reached_n += 1;
            &mut in_reached
        } else {
            unreached += 1;
            &mut in_unreached
        };
        for t in tags_of(form) {
            *counts.entry(t).or_default() += 1;
        }
    }
    let mut rows: Vec<(usize, usize, &str)> = in_unreached
        .iter()
        .map(|(t, n)| (*n, in_reached.get(t).copied().unwrap_or(0), *t))
        .collect();
    rows.sort_by(|a, b| b.cmp(a));
    println!(
        "  {test_roots} test roots the port does not emit; {unreached} functions it does not reach \
         against {reached_n} it does; node tags, in unreached against reached bodies:"
    );
    for (u, r, t) in rows {
        println!("    {u:5} / {r:<5} {t}");
    }
    // The constructs the port refuses outright, and how many unreached bodies hold none of them:
    // those are the ones a shape rule keeps out rather than a missing construct.
    let outright: [(&str, &[&str]); 7] = [
        ("a lambda", &["lam("]),
        ("a record update", &["upd("]),
        (
            "a bitwise or shift operator",
            &[
                "bin(bitand,",
                "bin(bitor,",
                "bin(bitxor,",
                "bin(shl,",
                "bin(shr,",
                "bin(ushr,",
            ],
        ),
        ("a Float or Decimal literal", &["lit(f", "lit(d"]),
        ("a fixed-width literal", &["lit(x"]),
        (
            "an effect construct",
            &["perform(", "handle(", "sim(", "cell(", "region("],
        ),
        ("a match guard", &["guard("]),
    ];
    let mut with: std::collections::BTreeMap<&str, usize> = Default::default();
    let mut none = 0usize;
    let mut none_names: Vec<&String> = Vec::new();
    for name in expected.keys() {
        if reached.contains(name) {
            continue;
        }
        let Some(form) = forms.get(name) else {
            continue;
        };
        let mut any = false;
        for (what, needles) in &outright {
            if needles.iter().any(|n| form.contains(n)) {
                *with.entry(what).or_default() += 1;
                any = true;
            }
        }
        if !any {
            none += 1;
            none_names.push(name);
        }
    }
    // The port's own reasons, aggregated by their head -- the text before the first `: ` -- over
    // the bodies the reference emits and the port refused.
    let mut by_reason: std::collections::BTreeMap<String, Vec<String>> = Default::default();
    for line in project.refusals_in(&names, &texts, &ctors).lines() {
        let Some((name, why)) = line.split_once(" :: ") else {
            continue;
        };
        if !expected.contains_key(name) {
            continue;
        }
        let head = why.split_once(": ").map_or(why, |(h, _)| h).to_string();
        by_reason.entry(head).or_default().push(name.to_string());
    }
    let mut reasons: Vec<(usize, &String, &Vec<String>)> =
        by_reason.iter().map(|(r, ns)| (ns.len(), r, ns)).collect();
    reasons.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(b.1)));
    println!("  the port's own reasons, over the bodies the reference emits:");
    for (n, reason, names) in &reasons {
        let sample: Vec<&str> = names.iter().take(3).map(String::as_str).collect();
        println!("    {n:5}  {reason}  e.g. {}", sample.join(", "));
    }
    // `PLY_EMIT_DIFF_DETAIL=<head>` prints what follows the `: ` of every reason with that head,
    // counted: which operator, which field, which name.
    if let Ok(head) = std::env::var("PLY_EMIT_DIFF_DETAIL") {
        let mut detail: std::collections::BTreeMap<String, Vec<String>> = Default::default();
        for line in project.refusals_in(&names, &texts, &ctors).lines() {
            let Some((name, why)) = line.split_once(" :: ") else {
                continue;
            };
            if !expected.contains_key(name) {
                continue;
            }
            if let Some((h, rest)) = why.split_once(": ")
                && h == head
            {
                detail
                    .entry(rest.to_string())
                    .or_default()
                    .push(name.to_string());
            }
        }
        let mut rows: Vec<(usize, String, Vec<String>)> = detail
            .into_iter()
            .map(|(d, ns)| (ns.len(), d, ns))
            .collect();
        rows.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        println!("  `{head}`, by what follows it:");
        for (n, d, ns) in rows.iter().take(25) {
            let sample: Vec<&str> = ns.iter().take(3).map(String::as_str).collect();
            println!("    {n:5}  {d}  e.g. {}", sample.join(", "));
        }
    }
    println!("  of the unreached functions, those holding:");
    for (what, n) in &with {
        println!("    {n:5}  {what}");
    }
    none_names.sort_by_key(|n| forms.get(*n).map_or(0, String::len));
    println!("    {none:5}  none of those -- kept out by a shape rule; the smallest:");
    for n in none_names.iter().take(12) {
        println!(
            "           {n}  ({} bytes of lowered form)",
            forms.get(*n).map_or(0, String::len)
        );
    }
    assert!(unreached + test_roots > 0 || expected.len() == reached.len());
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
