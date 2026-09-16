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
//! difference would be `opt.rs`'s rather than the emitter's. `emit.rs` beside this records the coupling.
//! Widening this corpus is what porting `fold_literals` would buy.
//!
//! The port is entered in-process through `port`: the bundle the binary carries is the emitter
//! under test, and `PLY_C_EMITTER=ply:<dir>` enters a working copy `stage` has bootstrapped.

use ply_compiler_diff::port::{self, bytes_list};
use ply_compiler_diff::reference_emit_dump;
use ply_eval::Value;
use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("this crate sits at <root>/crates/ply-compiler-diff")
        .to_path_buf()
}

/// The four lists every whole-program entry of `emit.ply` takes: the modules' names and sources,
/// the unit's constructor table and the builtins.
fn program_args(inputs: &[(String, String)]) -> [Value; 4] {
    let names: Vec<String> = inputs.iter().map(|(n, _)| n.clone()).collect();
    let srcs: Vec<String> = inputs.iter().map(|(_, s)| s.clone()).collect();
    [
        bytes_list(&names),
        bytes_list(&srcs),
        bytes_list(&ply_compiler_diff::reference_ctors(inputs)),
        bytes_list(&ply_compiler_diff::reference_builtins()),
    ]
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

/// The port under test is the bootstrap bundle, and a bundle emitted from other sources than the
/// tree's is the *old* emitter: a disagreement it reports says nothing about `emit.ply`. A working
/// copy named by `PLY_C_EMITTER`, or an emitter the reference builds, is never stale.
fn the_bundle_is_the_sources() {
    if std::env::var_os("PLY_C_EMITTER").is_some()
        || std::env::var("PLY_C_BOOTSTRAP").as_deref() == Ok("off")
    {
        return;
    }
    ply_codegen::c::producer::ensure_default();
    assert_eq!(
        ply_compiler::bootstrap::SOURCES.trim(),
        ply_codegen::c::producer::identity(),
        "the bootstrap bundle was emitted from other sources than crates/ply-compiler/ply, so the \
         port under test is the old emitter; refresh it first (PLY_C_BOOTSTRAP_REFRESH=1 on \
         ply-codegen-tests' bootstrap test, or CI's `bootstrap-bundle` artifact)"
    );
}

/// Every body the port emitted is the body the reference emits.
fn compare(label: &str, inputs: &[(String, Vec<u8>)]) -> (usize, usize) {
    the_bundle_is_the_sources();
    let builtins = bytes_list(&ply_compiler_diff::reference_builtins());
    let mut failures: Vec<String> = Vec::new();
    let (mut reached, mut available) = (0usize, 0usize);
    for (name, text) in inputs {
        let source = String::from_utf8_lossy(text).to_string();
        let program = [("m".to_string(), source)];
        let ctors = bytes_list(&ply_compiler_diff::reference_ctors(&program));
        let actual = port::call(
            "emit.emit_dump_in",
            &[
                Value::bytes(b"m"),
                ctors,
                builtins.clone(),
                Value::bytes(text),
            ],
        );
        let expected = by_name(&reference_emit_dump(&program));
        let mine = by_name(&actual);
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
        // A `match` over `list_at` or `map_get` that unwraps at once is one lookup helper and
        // no `Some`: in either arm order, binding a name or nothing, and not when the name is
        // this module's own definition rather than the builtin.
        "fn at(xs: List<Int>, i: Int) -> Int = match list_at(xs, i) { Some(x) -> x, None -> 0 }\n",
        "fn has(xs: List<Int>, i: Int) -> Bool = match list_at(xs, i) { None -> false, Some(_) -> true }\n",
        "fn get(m: Map<Int, Int>, k: Int) -> Int = match map_get(m, k) { Some(v) -> v + 1, None -> 0 }\n",
        "fn list_at(xs: List<Int>, i: Int) -> Option<Int> = None\nfn own(xs: List<Int>, i: Int) -> Int = match list_at(xs, i) { Some(x) -> x, None -> 0 }\n",
        // `bytes_at` whose index reads the buffer again: the buffer is read first and bound
        // after the index, and under release the index's read is the buffer's last use.
        "fn last(b: Bytes) -> Int = bytes_at(b, bytes_len(b) - 1)\n",
        "fn trailing_zeros(b: Bytes) -> Int =\n  fold(range(0, bytes_len(b)), 0, |acc: Int, i: Int|\n    if bytes_at(b, bytes_len(b) - 1 - i) == 48 && acc == i { acc + 1 } else { acc })\n",
        // A lambda is a closure *and* a function written after the one that
        // builds it, and calling one through a name is `rt_call_p`. Neither
        // fuses: `apply` takes the closure as a value.
        "fn apply(f: (Int) -> Int, n: Int) -> Int = f(n)\nfn mkl(k: Int) -> Int = apply(|x: Int| x + k, k)\n",
        // `iterate` is the loop, and a step whose every exit is a written `Stop` or `Continue`
        // is written into the loop's own variables with no constructor built: through a `match`
        // on the state, through an `if` whose arm is a `match` at a block's tail, and through
        // the lookup peephole, where the arms test the element the helper answered. A step with
        // an exit that is not written -- a call through a closure -- is built and then peeled.
        // A guard refuses the whole body in both emitters, so it is not here.
        "type S = | Go(Int) | Halt(Int)\nfn cnt(n: Int) -> Int = iterate(Go(0), n, |s: S| match s { Go(k) -> if k < 3 { Continue(Go(k + 1)) } else { Stop(k) }, Halt(k) -> Stop(k) })\n",
        "type S = | Go(Int) | Halt(Int)\nfn blk(n: Int) -> Int = iterate(Go(0), n, |s: S| {\n  let d = n - 1;\n  if d < 0 { Stop(0) } else { match s { Go(k) -> Continue(Halt(k + d)), Halt(k) -> Stop(k) } }\n})\n",
        "fn sum(xs: List<Int>) -> Int = iterate({ i: 0, acc: 0 }, len(xs) + 1, |s: { i: Int, acc: Int }| match list_at(xs, s.i) { Some(x) -> Continue({ i: s.i + 1, acc: s.acc + x }), None -> Stop(s.acc) })\n",
        "type S = | Go(Int) | Halt(Int)\nfn thru(n: Int, f: (Int) -> Iter<S, Int>) -> Int = iterate(Go(0), n, |s: S| match s { Go(k) -> Continue(Halt(k + 1)), Halt(k) -> f(k) })\n",
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
    let (reached, agreeing, differ) = resolved_against_reference("shipped", shipped());
    // Lowered once, on purpose, from 279 to 278, when the `if` join stopped guessing: where the
    // two arms answer different kinds the reference reads the checker's type and this port cannot,
    // so it refuses rather than write the wrong conversion into both. A correct refusal is worth
    // more than a body. It has since risen well past that.
    assert!(
        reached >= 1284,
        "the port emitted {reached} shipped bodies -- raise this when it grows, and lower it only \
         for a refusal that is more correct than what it replaces"
    );
    assert!(
        agreeing >= 1284,
        "{agreeing} shipped bodies resolve to the reference's C -- raise this when it grows"
    );
    // Nothing disagrees. Raise this only for a body that is right by the audit and slower on
    // purpose, and name the reason beside it.
    assert!(
        differ.is_empty(),
        "{} shipped bodies disagree with the reference -- lower this when they close, and raise \
         it only for a body that is right by the audit and slower on purpose",
        differ.len()
    );
}

/// The emitter's own sources, ratcheted on their own: the bootstrap runs what the port emits
/// for them, and the audit of the emitter's tests under the tier is what says the differences
/// still open are benign, which the floor below records rather than the emptiness the shipped
/// corpus reached.
#[test]
fn the_port_resolves_its_own_sources_to_the_references_c() {
    let sources = emitter_sources();
    let lines: usize = sources.iter().map(|(_, text)| text.lines().count()).sum();
    ply_codegen::c::producer::reset_census();
    let (reached, agreeing, differ) = resolved_against_reference("emitter", sources);
    // What emitting its own sources cost the compiled compiler, held to the census file: the
    // value model's measure, read here where the work is already done (ADR 0051 §2).
    if let Err(report) = ply_compiler_diff::census::hold("emitter-over-own-sources", lines) {
        panic!("{report}");
    }
    assert!(
        reached >= 2525,
        "the port emitted {reached} of its own bodies -- raise this when it grows, and lower it \
         only by the bodies a change deletes"
    );
    assert!(
        agreeing >= 2285,
        "{agreeing} of the port's own bodies resolve to the reference's C -- raise this when it \
         grows, and lower it only by the bodies a change deletes"
    );
    println!("  {} of the port's own bodies still differ", differ.len());
}

fn resolved_against_reference(
    label: &str,
    inputs: Vec<(String, String)>,
) -> (usize, usize, Vec<String>) {
    the_bundle_is_the_sources();
    let expected = ply_compiler_diff::reference_emit_encoded(&inputs);
    assert!(
        expected.len() > 500,
        "the reference emitted only {} bodies, so this proves little",
        expected.len()
    );
    let mine = framed(&port::call(
        "emit.emit_bodies_reference",
        &program_args(&inputs),
    ));
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
        "  {label}: the port emits {reached} of the {} bodies the reference does, \
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
    (reached, agreeing, differ)
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
            "performs" => 'p',
            "handles" => 'h',
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
    for (kind, label) in [('p', "performs"), ('h', "handles")] {
        if let Some(es) = entries.get(&kind) {
            out.push_str(&format!("\n{label}: "));
            out.push_str(&es.join(","));
        }
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
    let args = program_args(&inputs);
    let reached: std::collections::BTreeSet<String> =
        by_name(&port::call("emit.emit_dump_all", &args))
            .into_keys()
            .collect();
    // Each body's lowered form, from the oracle for the stage before the emitter.
    let lowered = ply_compiler_diff::reference_lower_dump(&inputs);
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
    let refusals = port::call("emit.emit_refusals_all", &args);
    for line in refusals.lines() {
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
        for line in refusals.lines() {
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
    // The census has something to say, or there is nothing left for it to say: the port reaches
    // every body the reference emits, and may reach more.
    assert!(unreached + test_roots > 0 || expected.keys().all(|n| reached.contains(n)));
}

/// The shipped standard library and examples, named as the reference names them.
fn shipped() -> Vec<(String, String)> {
    corpus(&["crates/ply-std/ply", "examples"])
}

/// The emitter's own sources: the bootstrap builds the emitter from what it emits for them, so
/// a body of its own the port emits wrongly is a bug the bootstrap runs.
fn emitter_sources() -> Vec<(String, String)> {
    corpus(&["crates/ply-std/ply", "crates/ply-compiler/ply"])
}

fn corpus(dirs: &[&str]) -> Vec<(String, String)> {
    let root = repo_root();
    let mut out = Vec::new();
    for dir in dirs {
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
