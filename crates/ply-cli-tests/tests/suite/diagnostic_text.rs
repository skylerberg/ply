//! One diagnostic, two renderers: `ply_span::render::to_terminal` in Rust and
//! `crates/ply-cli/ply/diagnostic.ply`'s `to_text` in Ply. The bytes have to be the same, so
//! neither shape can drift away from the other.

use ply_cli::commands::common::{build_backend_over, module_texts};
use ply_eval::{BackendKind, BackendSpec, Machine, Value};
use ply_span::{Diagnostic, Edit, SourceId, SourceMap, Span, codes};

const SOURCE: &str = "fn f() -> Int = \"x\"\nfn g() -> Int = 1\n";

/// The same diagnostic as `PROBE`'s, label for label: one placed with a message, one on an empty
/// span with none, one running past the line it opens on, one outside the text and one outside
/// every module.
fn diagnostic(id: SourceId) -> Diagnostic {
    Diagnostic::error(codes::TYPE_MISMATCH, "type mismatch: function body type")
        .primary(Span::new(id, 16, 19), "expected `Int`, found `String`")
        .secondary(Span::new(id, 5, 5), "")
        .secondary(Span::new(id, 16, 25), "and on past the line")
        .secondary(Span::new(id, 900, 901), "past the end")
        .secondary(Span::DUMMY, "nowhere")
        .note("the body must answer the written type")
        .fix(
            "write the type the body answers",
            vec![Edit {
                span: Span::new(id, 11, 14),
                text: "String".to_string(),
            }],
        )
}

/// Ply's side of that diagnostic, rendered through the shipped program's own module and joined
/// the way the renderer's caller writes it: one newline after each line.
const PROBE: &str = r#"
import compiler.resolve (Diag, Label, Fix, Edit)
import diagnostic (Place, to_text)

fn source() -> Bytes = b"fn f() -> Int = \"x\"\nfn g() -> Int = 1\n"

fn places() -> List<Place> = [{ path: "m.ply", text: source() }]

fn diag() -> Diag = {
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

pub fn rendered() -> String =
  fold(to_text(diag(), places()), "", |acc: String, line: String| acc ++ line ++ "\n")
"#;

#[test]
fn the_two_renderers_write_the_same_bytes_for_one_diagnostic() {
    let source = ply_cli::shipped::PROGRAM_SOURCES
        .iter()
        .find(|(name, _)| *name == "diagnostic")
        .map(|(_, text)| *text)
        .expect("the program carries `diagnostic`");

    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("diagnostic.ply"), source).unwrap();
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
    let answered = machine
        .call("probe.rendered", Vec::new(), Span::DUMMY)
        .expect("the Ply renderer answers");
    let Value::Str(ply) = answered else {
        panic!("`probe.rendered` answers a String");
    };

    let mut sources = SourceMap::new();
    let id = sources.add("m.ply", SOURCE);
    let rust = ply_span::render::to_terminal(&diagnostic(id), &sources, false);

    assert_eq!(
        &*ply, rust,
        "the two renderers disagree\n--- Ply ---\n{ply}--- Rust ---\n{rust}"
    );
}
