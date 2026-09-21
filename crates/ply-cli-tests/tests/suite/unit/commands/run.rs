use ply_cli::commands::common::{backend_spec, build_backend_over, location, module_texts};
use ply_cli::commands::run::*;
use ply_cli::load::{Loaded, load};
use ply_eval::Machine;
use ply_span::{Diagnostic, Span, codes};

fn write(dir: &std::path::Path, rel: &str, text: &str) {
    let path = dir.join(rel);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, text).unwrap();
}

fn loaded(text: &str) -> (tempfile::TempDir, Loaded) {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "m.ply", text);
    let l = load(dir.path()).unwrap();
    (dir, l)
}

/// The default tier attached to a machine over `loaded`'s program: what the commands build inline.
fn attach_tier(machine: &mut Machine<'_>, loaded: &Loaded) -> Result<(), Diagnostic> {
    let Some(spec) = backend_spec(None)? else {
        return Ok(());
    };
    let texts = module_texts(&loaded.check, &loaded.sources);
    let provider = build_backend_over(&spec, &loaded.front, texts)?;
    machine.set_compiled(provider.attach(&spec));
    Ok(())
}

fn eval(l: &Loaded) -> Result<String, Diagnostic> {
    let entry = entry_point(l)?;
    let (name, span) = (entry.name.clone(), entry.span);
    let mut machine = Machine::new(&l.front);
    attach_tier(&mut machine, l)?;
    machine
        .call(name.as_str(), Vec::new(), span)
        .map(|v| v.to_string())
}

#[test]
fn main_is_evaluated_and_its_value_rendered() {
    let (_dir, l) = loaded("fn main() -> Int = 20 + 22\n");
    assert_eq!(eval(&l).unwrap(), "42");
}

#[test]
fn a_missing_main_points_at_the_program_rather_than_nowhere() {
    let (_dir, l) = loaded("fn other() -> Int = 1\n");
    let d = no_main(&l);
    assert_eq!(d.code, codes::UNKNOWN_NAME);
    assert!(!d.primary_span().unwrap().is_dummy());
    assert!(d.notes.iter().any(|n| n.contains("fn main")));
}

#[test]
fn a_missing_main_never_points_at_an_unrelated_definition() {
    let text = "fn other() -> Int = 1\nfn another() -> Int = 2\n";
    let (_dir, l) = loaded(text);
    let d = no_main(&l);

    let span = d.primary_span().expect("one module has one place to point");
    assert_eq!(
        span.start, span.end,
        "the anchor is a position, not an extent"
    );
    assert_eq!(span.start as usize, text.len());

    let items: Vec<Span> = l.check.defs.values().map(|d| d.span).collect();
    assert!(
        items.iter().all(|i| span.start >= i.end),
        "the anchor landed inside `{}`",
        l.sources
            .snippet(*items.iter().find(|i| span.start < i.end).unwrap()),
    );
    assert!(!ply_span::render::to_terminal(&d, &l.sources, false).is_empty());
}

/// Labelling one file would be picking by load order, which `ply run` refuses to do for two `main`s.
#[test]
fn a_missing_main_across_several_modules_labels_no_file_at_all() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "a.ply", "fn f() -> Int = 1\n");
    write(dir.path(), "b.ply", "fn g() -> Int = 2\n");

    let l = load(dir.path()).unwrap();
    let d = no_main(&l);
    assert!(d.labels.is_empty(), "labels: {:?}", d.labels);
    assert!(d.notes.iter().any(|n| n.contains("a, b")), "{:?}", d.notes);
    assert!(
        ply_span::render::to_terminal(&d, &l.sources, false).contains("E0101"),
        "an unlabelled diagnostic still has to render"
    );
}

#[test]
fn a_raising_main_yields_a_diagnostic_not_a_panic() {
    let (_dir, l) = loaded("fn main() -> Unit = panic(\"nope\")\n");
    let err = eval(&l).unwrap_err();
    assert_eq!(err.code, codes::RUNTIME_ERROR);
    assert!(location(&l.sources, err.primary_span().unwrap()).is_some());
}

#[test]
fn the_one_main_in_a_multi_module_program_is_the_entry_point() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "lib.ply", "pub fn answer() -> Int = 42\n");
    write(
        dir.path(),
        "app.ply",
        "import lib\nfn main() -> Int = lib::answer()\n",
    );

    let l = load(dir.path()).unwrap();
    assert_eq!(entry_point(&l).unwrap().name.as_str(), "app.main");
    assert_eq!(eval(&l).unwrap(), "42");
}

#[test]
fn several_mains_are_refused_with_the_candidates_named() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "one.ply", "fn main() -> Int = 1\n");
    write(dir.path(), "two.ply", "fn main() -> Int = 2\n");

    let l = load(dir.path()).unwrap();
    let d = entry_point(&l).unwrap_err();
    assert_eq!(d.code, codes::AMBIGUOUS_ENTRY_POINT);
    assert!(d.message.contains("2 modules"));
    assert_eq!(d.labels.len(), 2);
    assert!(d.labels.iter().all(|l| !l.span.is_dummy()));
    assert!(d.notes.iter().any(|n| n.contains("one.ply")));
    assert!(d.notes.iter().any(|n| n.contains("two.ply")));
}

#[test]
fn naming_the_file_resolves_the_ambiguity() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "one.ply", "fn main() -> Int = 1\n");
    write(dir.path(), "two.ply", "fn main() -> Int = 2\n");

    let l = load(&dir.path().join("two.ply")).unwrap();
    assert_eq!(entry_point(&l).unwrap().name.as_str(), "two.main");
    assert_eq!(eval(&l).unwrap(), "2");
}

#[test]
fn a_missing_main_lists_the_modules_that_were_searched() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "a.ply", "fn f() -> Int = 1\n");
    write(dir.path(), "b.ply", "fn g() -> Int = 2\n");

    let l = load(dir.path()).unwrap();
    let d = entry_point(&l).unwrap_err();
    assert_eq!(d.code, codes::UNKNOWN_NAME);
    assert!(
        d.notes.iter().any(|n| n.contains("a, b")),
        "notes: {:?}",
        d.notes
    );
}
