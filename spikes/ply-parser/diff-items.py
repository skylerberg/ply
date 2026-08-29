"""The Items area's differential against the shipping parser.

For each fixture that reaches a parser error path, this compares
`crates/ply-syntax`'s diagnostics against `items.ply`'s on **code, every
label's span, every label's primary flag, and the note count** -- the widened
signature `/tmp/ply-parser-spike/PREREGISTRATION.md` M7 registers, because 45 of
the reference's diagnostic sites are `E0001 UNEXPECTED_TOKEN` and code plus span
alone is a weak signature for a parser.

    python3 spikes/ply-parser/diff-items.py [project-dir]

The reference side is `ply check --json`, which reports byte offsets. That works
only for inputs whose diagnostics all come from parsing -- a fixture that parses
clean would go on to resolve and typecheck and report things this parser cannot.
Every fixture below is therefore a syntax error, which is also what this area
needs: no real `.ply` file in the tree reaches one line of the recovery half.

**This comparison is blind to the tree.** `arm-items.sh` measures how blind:
three of its fifteen mutations pass this check and are caught only by
`items.ply`'s own `test` blocks. See `GAPS-items.md` §P7.
"""

import json, os, re, shutil, subprocess, sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))
PLY = ROOT + "/target/debug/ply"
if not os.path.exists(PLY):
    PLY = ROOT + "/target/release/ply"
PROJ = sys.argv[1] if len(sys.argv) > 1 else os.environ.get("PLY_PARSER_WORKDIR", "/tmp/pall")

def ref_diags(src):
    d = os.path.join(PROJ, "..", "ply-parser-diff-ref")
    shutil.rmtree(d, ignore_errors=True); os.makedirs(d)
    open(d + "/m.ply", "w").write(src)
    out = subprocess.run([PLY, "check", d, "--json"], capture_output=True, text=True).stdout
    j = json.loads(out)
    got = []
    for x in j.get("diagnostics", []):
        labels = [(l["start"]["offset"], l["end"]["offset"], l["primary"]) for l in x["labels"]]
        got.append((x["code"], len(labels), len(x["notes"]), labels))
    return got

def byte_literal(b):
    out = []
    for ch in b:
        if ch == 0x22: out.append('\\"')
        elif ch == 0x5c: out.append('\\\\')
        elif 0x20 <= ch < 0x7f: out.append(chr(ch))
        elif ch == 0x0a: out.append('\\n')
        elif ch == 0x09: out.append('\\t')
        else: out.append('\\x%02x' % ch)
    return 'b"' + ''.join(out) + '"'

def ply_dump(src):
    open(PROJ + "/probe.ply", "w").write(
        "import items (dump)\nfn source() -> Bytes = %s\nfn main() -> String = dump(source())\n"
        % byte_literal(src.encode()))
    r = subprocess.run([PLY, "run", PROJ, "--json"], capture_output=True, text=True)
    j = json.loads(r.stdout)
    if j.get("exit_code"):
        raise SystemExit("ply run failed: %s" % [x.get("message") for x in j.get("diagnostics", [])][:3])
    v = j["value"]
    # `ply run --json` renders a `String` value with its own quotes, so the
    # field holds a quoted literal. The dump is ASCII with no `"` and no `\\`
    # by construction, so stripping the quotes is the whole unescaping.
    assert v.startswith('"') and v.endswith('"'), v[:40]
    return v[1:-1]

# `!CODE:S:E:L:N;` then L labels: code, primary span, label count, note count.
DIAG = re.compile(r'!([A-Z]\d+):(\d+):(\d+):(\d+):(\d+);')
LBL  = re.compile(r'=(\d+):(\d+):([01]);')

def read_diags(tail, k):
    """Exactly k diagnostics, or None if the tail is not that."""
    out, pos = [], 0
    for _ in range(k):
        d = DIAG.match(tail, pos)
        if not d: return None
        pos = d.end()
        labels = []
        for _ in range(int(d.group(4))):
            l = LBL.match(tail, pos)
            if not l: return None
            pos = l.end()
            labels.append((int(l.group(1)), int(l.group(2)), l.group(3) == "1"))
        out.append((d.group(1), int(d.group(4)), int(d.group(5)), labels))
    return out if pos == len(tail) else None

def ply_diags(dump):
    """The dump ends with `#K;` and then exactly K diagnostics. Take the last
    `#K;` whose tail parses as exactly that and nothing else."""
    for m in reversed(list(re.finditer(r'#(\d+);', dump))):
        got = read_diags(dump[m.end():], int(m.group(1)))
        if got is not None:
            return got
    raise SystemExit("no diagnostic block in dump: %r" % dump[-80:])

FIXTURES = {
 "record type with a hole":            'type T = { , }\nfn g() = 2\n',
 "unclosed effect block":              'effect E {\nfn h() = 3\n',
 "a number where a fn name goes":      'fn 9() = 1\nfn ok() = 2\n',
 "law with no quoted label":           'law/host bad { 1 }\n',
 "unknown deriver":                    'derive frobnicate for Order\n',
 "a pub test":                         'pub test "t" { 1 }\n',
 "a pub law":                          'pub law "l" { 1 }\n',
 "a pub derive":                       'pub derive json for T\n',
 "a pub effect set":                   'pub effect set S = {a.read}\n',
 "forall that binds nothing":          'law "l" forall () { 1 }\n',
 "import renames and selects":         'import a as b (c)\n',
 "import selects nothing":             'import a ()\n',
 "effect set with a row variable":     'effect set S = {a.read | e}\n',
 "import after a definition":          'fn f() = 1\nimport a\n',
 "two colons twice":                   'fn f() = a::b::c\n',
 "unclosed parameter list":            'fn f(a: Int\n',
 "type parameter list unclosed":       'type T<a = Int\n',
 "op with no mode":                    'effect E { gen() -> Int }\n',
 "op with no arrow":                   'effect E { read gen() }\n',
 "derive with no for":                 'derive json Order\n',
 "where with a bad constraint":        'fn f() -> Int where nope(a) = 1\n',
 "where with an unknown deriver":      'fn f() -> Int where derivable(zz, a) = 1\n',
 "variant list that stops short":      'type T = | A | \n',
 "effect set unclosed":                'effect set S = {a.read\n',
 "no body at all":                     'fn f()\n',
 "empty input":                        '',
 "only a pub":                         'pub\n',
 "garbage between two items":          'fn a() = 1\n$$$\nfn b() = 2\n',
 # Added after arming: each of these exists because a mutation survived
 # without it, and the mutation it kills is named.
 "bracket depth carries recovery past an item keyword":
                                      'type T = 9 ( fn ) \nfn g() = 2\n',
 "`law` not followed by a label is not an item":
                                      'law x { 1 }\n',
 "an import that selects and then renames":
                                      'import a (b) as c\n',
 "`derive` needs a name after it to be an item":
                                      'derive + 1\n',
}

bad = 0
for name, src in sorted(FIXTURES.items()):
    r = ref_diags(src)
    try:
        p = ply_diags(ply_dump(src))
    except SystemExit as e:
        print("  RUNFAIL %-34s %s" % (name, e)); bad += 1; continue
    if r == p:
        print("  ok      %-34s %d diagnostic(s)" % (name, len(r)))
    else:
        bad += 1
        print("  DIFFER  %-34s" % name)
        print("            rust: %s" % r)
        print("            ply : %s" % p)
print("\n%d of %d agree" % (len(FIXTURES) - bad, len(FIXTURES)))
