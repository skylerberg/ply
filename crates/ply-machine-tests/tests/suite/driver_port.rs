//! The driver's own load of a directory against the compiled front end pulling the shipped modules
//! itself: the same modules placed in the same order, and the same answer or the same refusal.

use crate::fixture::{scratch, write};
use ply_eval::{Diagnostic, SourceId, Span, codes};
use ply_machine::load::{LoadError, Loaded, load};
use std::path::{Path, PathBuf};

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The port's answer over a flat directory, pulling in the shipped modules itself, and the driver's.
fn pulled_and_loaded(dir: &Path) -> (Vec<String>, ply_eval::Analysis, Result<Loaded, LoadError>) {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|x| x == "ply"))
        .collect();
    files.sort();
    let user: Vec<(String, String)> = files
        .iter()
        .map(|p| {
            let name = p.file_stem().unwrap().to_string_lossy().into_owned();
            (name, std::fs::read_to_string(p).unwrap())
        })
        .collect();
    let shipped: Vec<(String, String)> = ply_std::sources()
        .map(|(m, t)| (m.to_string(), t.to_string()))
        .collect();
    let pulled = ply_codegen::c::producer::front_pulling_std(&user, &shipped)
        .unwrap_or_else(|e| panic!("{}: the port does not answer: {e:#}", dir.display()));
    let ids: Vec<SourceId> = (0..user.len() + pulled.modules.len())
        .map(|i| SourceId(i as u32))
        .collect();
    let ours = ply_codegen::c::dump::read(&pulled.dump, &ids)
        .unwrap_or_else(|e| panic!("{}: the port's answer does not read: {e}", dir.display()));

    let theirs = load(dir);
    let sources = match &theirs {
        Ok(loaded) => &loaded.sources,
        Err(err) => &err.sources,
    };
    let placed: Vec<PathBuf> = sources.files().iter().map(|f| f.path.clone()).collect();
    let expected: Vec<PathBuf> = files
        .into_iter()
        .chain(pulled.modules.iter().map(|m| ply_std::pseudo_path(m)))
        .collect();
    assert_eq!(placed, expected, "{}: the modules, in order", dir.display());
    (pulled.modules, ours, theirs)
}

#[test]
fn the_driver_places_the_shipped_modules_the_port_pulls_in_and_answers_as_it_does() {
    let chain = scratch();
    write(
        chain.path(),
        "app.ply",
        "import std.router\nimport util\n\nfn f() -> Int = util::one()\n",
    );
    write(
        chain.path(),
        "util.ply",
        "import std.trace\n\npub fn one() -> Int = 1\n",
    );
    let plain = scratch();
    write(plain.path(), "a.ply", "pub fn a() -> Int = 1\n");
    write(
        plain.path(),
        "b.ply",
        "import a\n\nfn b() -> Int = a::a()\n",
    );

    // A round's imports follow the round before it, so the whole is not in byte order.
    // `std.bytes` is the second round's first: `std.router` and `std.json` both join with it.
    let rounds: &[&str] = &[
        "std.router",
        "std.trace",
        "std.bytes",
        "std.http",
        "std.json",
        "std.net",
    ];
    for (dir, pulls) in [
        (repo().join("examples"), None),
        (chain.path().to_path_buf(), Some(rounds)),
        (plain.path().to_path_buf(), Some(&[][..])),
    ] {
        let (pulled, ours, theirs) = pulled_and_loaded(&dir);
        if let Some(pulls) = pulls {
            assert_eq!(pulled, pulls, "{}", dir.display());
        }
        let loaded = theirs.unwrap_or_else(|e| panic!("{}: {:?}", dir.display(), e.diagnostics));
        assert!(
            ours.hashes == loaded.hashes,
            "{}: the hashes differ",
            dir.display()
        );
        assert!(
            format!("{ours:?}") == format!("{:?}", loaded.front),
            "{}: the answers differ",
            dir.display()
        );
    }
}

fn headlines(ds: &[Diagnostic]) -> Vec<(&'static str, &str, Option<Span>)> {
    ds.iter()
        .map(|d| (d.code, d.message.as_str(), d.primary_span()))
        .collect()
}

#[test]
fn the_driver_refuses_with_the_port_s_diagnostics_alone() {
    let unshipped = scratch();
    write(
        unshipped.path(),
        "app.ply",
        "import std.json\nimport std.nonesuch\nfn f() -> Int = 1\n",
    );
    let unknown = scratch();
    write(
        unknown.path(),
        "app.ply",
        "import std.json\nimport nowhere\nfn f() -> Int = 1\n",
    );
    for dir in [unshipped.path(), unknown.path()] {
        let (_, ours, theirs) = pulled_and_loaded(dir);
        let err = theirs.expect_err("an import nothing answers for is refused");
        assert_eq!(
            ours.diagnostics.first().map(|d| d.code),
            Some(codes::UNKNOWN_MODULE)
        );
        assert_eq!(
            format!("{:?}", ours.diagnostics),
            format!("{:?}", err.diagnostics)
        );
    }

    let broken = scratch();
    write(
        broken.path(),
        "app.ply",
        "import std.json\nfn f() -> Int = )\n",
    );
    let fixtures = ["ambiguous_import", "module_cycle", "duplicate_import"]
        .map(|f| repo().join(format!("tests/fixtures/{f}")));
    for dir in fixtures.iter().map(PathBuf::as_path).chain([broken.path()]) {
        let (_, ours, theirs) = pulled_and_loaded(dir);
        let err = theirs
            .err()
            .unwrap_or_else(|| panic!("{}: the driver accepts it", dir.display()));
        assert_eq!(
            headlines(&ours.diagnostics),
            headlines(&err.diagnostics),
            "{}",
            dir.display()
        );
    }
}
