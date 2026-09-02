"""Mines `crates/ply-syntax/src/resolve.rs` for the multi-module programs its tests build and
writes them as a program bundle for the resolve differential.

    python3 spikes/ply-parser/mine-programs.py

Every `&[("name", "source"), ...]` array literal in the file is one program, whatever function it
is handed to: the ones under `errors(` are the error paths, which the real `.ply` files in the
tree never exercise. The bundle format: programs separated by a line holding exactly `%%%`,
modules within a program by a line holding exactly `%%`, and each module's first line is its
dotted name.
"""
import pathlib
import re

root = pathlib.Path(__file__).resolve().parents[2]
src = (root / "crates/ply-syntax/src/resolve.rs").read_text()

STRING = re.compile(r'"((?:[^"\\]|\\.)*)"', re.S)


def unescape(s: str) -> str:
    out = []
    i = 0
    while i < len(s):
        c = s[i]
        if c == "\\" and i + 1 < len(s):
            n = s[i + 1]
            if n == "\n":
                i += 2
                while i < len(s) and s[i] in " \t":
                    i += 1
                continue
            out.append({"n": "\n", "t": "\t", '"': '"', "\\": "\\"}.get(n, n))
            i += 2
        else:
            out.append(c)
            i += 1
    return "".join(out)


programs = []
for m in re.finditer(r"&\[", src):
    depth = 0
    i = m.start() + 1
    while i < len(src):
        if src[i] == "[":
            depth += 1
        elif src[i] == "]":
            depth -= 1
            if depth == 0:
                break
        elif src[i] == '"':
            j = STRING.match(src, i)
            i = j.end() - 1 if j else i
        i += 1
    body = src[m.start() + 2 : i]
    strings = [unescape(x.group(1)) for x in STRING.finditer(body)]
    if len(strings) < 2 or len(strings) % 2 != 0:
        continue
    pairs = list(zip(strings[0::2], strings[1::2]))
    if not all(re.fullmatch(r"[A-Za-z_][A-Za-z0-9_.]*", n) for n, _ in pairs):
        continue
    programs.append(pairs)

seen = set()
unique = []
for p in programs:
    key = tuple(p)
    if key in seen:
        continue
    seen.add(key)
    unique.append(p)

out = pathlib.Path(__file__).resolve().parent / "fixtures/reference-programs.corpus"
with out.open("w") as f:
    f.write("Mined from crates/ply-syntax/src/resolve.rs by mine-programs.py. Do not edit.\n")
    for p in unique:
        f.write("\n%%%\n")
        f.write("\n%%\n".join(f"{name}\n{text}" for name, text in p))
    f.write("\n")
print(f"{len(unique)} distinct programs -> {out.relative_to(root)}")
