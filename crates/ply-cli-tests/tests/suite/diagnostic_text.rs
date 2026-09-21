//! One diagnostic, two renderers: `ply_span::render::to_terminal` in Rust and
//! `crates/ply-cli/ply/diagnostic.ply`'s `to_text` in Ply. The bytes have to be the same, painted
//! or plain, so neither shape can drift away from the other.

use ply_cli::commands::common::{build_backend_over, module_texts};
use ply_eval::{BackendKind, BackendSpec, Machine, Value};
use ply_span::{Diagnostic, Edit, SourceId, SourceMap, Span, codes};

/// The modules the probe needs beside itself.
const CARRIED: [&str; 2] = ["diagnostic", "style"];

const ASCII: &str = "fn f() -> Int = \"x\"\nfn g() -> Int = 1\n";

/// `é` is two bytes, so a span can land inside it.
const WIDE: &str = "fn é() -> Int = 1\n";

/// The same two diagnostics as `PROBE`'s, label for label: one placed with a message, one on an
/// empty span with none, one running past the line it opens on, one outside the text, one outside
/// every module, one over a character and one cutting a character in half.
fn diagnostics(m: SourceId, u: SourceId) -> Vec<Diagnostic> {
    vec![
        Diagnostic::error(codes::TYPE_MISMATCH, "type mismatch: function body type")
            .primary(Span::new(m, 16, 19), "expected `Int`, found `String`")
            .secondary(Span::new(m, 5, 5), "")
            .secondary(Span::new(m, 16, 25), "and on past the line")
            .secondary(Span::new(m, 900, 901), "past the end")
            .secondary(Span::DUMMY, "nowhere")
            .note("the body must answer the written type")
            .fix(
                "write the type the body answers",
                vec![Edit {
                    span: Span::new(m, 11, 14),
                    text: "String".to_string(),
                }],
            ),
        Diagnostic::warning(codes::UNUSED_DEFINITION, "the name is never used")
            .primary(Span::new(u, 3, 5), "declared here")
            .secondary(Span::new(u, 4, 6), "half a character")
            .note("a leading `_` in its name keeps it quiet"),
    ]
}

/// Ply's side of those two, rendered through the shipped program's own module and joined the way
/// the renderer's caller writes them: one newline after each line.
const PROBE: &str = r#"
import compiler.resolve (Diag, Label, Fix, Edit)
import diagnostic (Place, all_to_text)
import style (Style, plain)

fn ascii() -> Bytes = b"fn f() -> Int = \"x\"\nfn g() -> Int = 1\n"

fn wide() -> Bytes = b"fn \xc3\xa9() -> Int = 1\n"

fn places() -> List<Place> =
  [{ path: "m.ply", text: ascii() }, { path: "u.ply", text: wide() }]

fn mismatch() -> Diag = {
  let labels: List<Label> =
    [{ module: 0, start: 16, end: 19, primary: true, text: b"expected `Int`, found `String`" },
     { module: 0, start: 5, end: 5, primary: false, text: b"" },
     { module: 0, start: 16, end: 25, primary: false, text: b"and on past the line" },
     { module: 0, start: 900, end: 901, primary: false, text: b"past the end" },
     { module: 4294967295, start: 0, end: 0, primary: false, text: b"nowhere" }];
  let edits: List<Edit> = [{ module: 0, start: 11, end: 14, text: b"String" }];
  let fixes: List<Fix> = [{ title: b"write the type the body answers", edits: edits }];
  { code: b"E0201", notes: 1, labels: labels, text: b"",
    message: b"type mismatch: function body type",
    notes_text: [b"the body must answer the written type"], severity: b"error", fixes: fixes }
}

fn unused() -> Diag = {
  let labels: List<Label> =
    [{ module: 1, start: 3, end: 5, primary: true, text: b"declared here" },
     { module: 1, start: 4, end: 6, primary: false, text: b"half a character" }];
  { code: b"W0611", notes: 1, labels: labels, text: b"", message: b"the name is never used",
    notes_text: [b"a leading `_` in its name keeps it quiet"], severity: b"warning", fixes: [] }
}

fn rendered(s: Style) -> String =
  fold(all_to_text(s, [mismatch(), unused()], places()), "",
       |acc: String, line: String| acc ++ line ++ "\n")

pub fn unstyled_text() -> String = rendered(plain())

pub fn styled_text() -> String = rendered({ styled: true })
"#;

/// What the shipped module wrote, as one string.
fn through_ply(machine: &mut Machine<'_>, entry: &str) -> String {
    let answered = machine
        .call(entry, Vec::new(), Span::DUMMY)
        .unwrap_or_else(|d| panic!("`{entry}`: {} [{}]", d.message, d.code));
    let Value::Str(text) = &answered else {
        panic!("`{entry}` answers a String");
    };
    text.to_string()
}

#[test]
fn the_two_renderers_write_the_same_bytes_for_one_diagnostic() {
    let dir = tempfile::tempdir().unwrap();
    for module in CARRIED {
        let text = ply_cli::shipped::PROGRAM_SOURCES
            .iter()
            .find(|(name, _)| *name == module)
            .map(|(_, text)| *text)
            .unwrap_or_else(|| panic!("the program carries `{module}`"));
        std::fs::write(dir.path().join(format!("{module}.ply")), text).unwrap();
    }
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

    let mut sources = SourceMap::new();
    let m = sources.add("m.ply", ASCII);
    let u = sources.add("u.ply", WIDE);
    let diagnostics = diagnostics(m, u);

    for (entry, styled) in [("probe.unstyled_text", false), ("probe.styled_text", true)] {
        let ply = through_ply(&mut machine, entry);
        let rust = ply_span::render::all_to_terminal(&diagnostics, &sources, styled);
        assert_eq!(
            ply, rust,
            "`{entry}` and the Rust renderer disagree\n--- Ply ---\n{ply}--- Rust ---\n{rust}"
        );
    }
}
