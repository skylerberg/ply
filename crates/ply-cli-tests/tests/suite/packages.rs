use crate::harness::{json_of, ply};
use serde_json::Value;
use tempfile::TempDir;

fn manifest(name: &str, deps: &str) -> String {
    format!(
        "import std.pkg (Manifest)\nfn package() -> Manifest = {{name: \"{name}\", version: {{major: 0, minor: 0, patch: 1}}, prefix: None, runtime: {{major: 0, minor: 0, patch: 1}}, dependencies: [{deps}], entry: None}}\n"
    )
}

fn dep(name: &str, path: &str) -> String {
    format!(
        "{{name: \"{name}\", prefix: None, min: {{major: 0, minor: 0, patch: 1}}, source: Path(\"{path}\")}}"
    )
}

/// app -> lib -> base, with lib using base and a sibling of its own.
fn graph() -> TempDir {
    let dir = tempfile::tempdir().expect("a temp dir");
    let root = dir.path();
    for pkg in ["app", "lib", "base"] {
        std::fs::create_dir(root.join(pkg)).unwrap();
    }
    std::fs::write(
        root.join("app/ply.pkg"),
        manifest("app", &dep("lib", "../lib")),
    )
    .unwrap();
    std::fs::write(
        root.join("app/main.ply"),
        "import lib.answer\nfn main() -> Int = answer::answer()\ntest \"answers\" { assert_eq(answer::answer(), 42) }\n",
    )
    .unwrap();
    std::fs::write(
        root.join("lib/ply.pkg"),
        manifest("lib", &dep("base", "../base")),
    )
    .unwrap();
    std::fs::write(
        root.join("lib/answer.ply"),
        "import base.deep\nimport extra\npub fn answer() -> Int = deep::deep() + extra::hidden()\n",
    )
    .unwrap();
    std::fs::write(root.join("lib/extra.ply"), "pub fn hidden() -> Int = 35\n").unwrap();
    std::fs::write(root.join("base/ply.pkg"), manifest("base", "")).unwrap();
    std::fs::write(root.join("base/deep.ply"), "pub fn deep() -> Int = 7\n").unwrap();
    dir
}

#[test]
fn a_path_dependency_serves_its_modules_and_runs_them() {
    let dir = graph();
    for (args, want) in [
        (vec!["check", "app"], "checked 4 modules"),
        (vec!["run", "app"], "42"),
        (vec!["test", "app"], "1 passed"),
        (vec!["build", "app", "-o", "app.plyx"], "built main.main"),
    ] {
        let out = ply(dir.path()).args(&args).output().unwrap();
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(
            out.status.success() && text.contains(want),
            "{args:?}: {text}"
        );
    }
}

#[test]
fn a_transitive_dependency_the_importer_does_not_declare_is_refused() {
    let dir = graph();
    std::fs::write(
        dir.path().join("app/main.ply"),
        "import base.deep\nfn main() -> Int = deep::deep()\n",
    )
    .unwrap();
    let out = ply(dir.path()).args(["check", "app"]).output().unwrap();
    assert_eq!(out.status.code(), Some(2));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("E0132"), "{err}");
    assert!(err.contains("`base`"), "{err}");
    assert!(err.contains("`app`"), "{err}");
}

#[test]
fn a_dependency_without_a_manifest_is_unusable_where_it_is_declared() {
    let dir = graph();
    std::fs::remove_file(dir.path().join("lib/ply.pkg")).unwrap();
    let out = ply(dir.path()).args(["run", "app"]).output().unwrap();
    assert_eq!(out.status.code(), Some(2));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("E0135"), "{err}");
    assert!(err.contains("ply.pkg"), "{err}");
}

#[test]
fn a_root_module_may_not_squat_on_a_dependencys_prefix() {
    let dir = graph();
    std::fs::create_dir(dir.path().join("app/lib")).unwrap();
    std::fs::write(
        dir.path().join("app/lib/mine.ply"),
        "fn mine() -> Int = 1\n",
    )
    .unwrap();
    let out = ply(dir.path()).args(["check", "app"]).output().unwrap();
    assert_eq!(out.status.code(), Some(2));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("E0133"), "{err}");
}

#[test]
fn a_dependency_cycle_is_refused_naming_the_cycle() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let root = dir.path();
    std::fs::create_dir(root.join("app")).unwrap();
    std::fs::create_dir(root.join("lib")).unwrap();
    std::fs::write(
        root.join("app/ply.pkg"),
        manifest("app", &dep("lib", "../lib")),
    )
    .unwrap();
    std::fs::write(root.join("app/main.ply"), "fn main() -> Int = 1\n").unwrap();
    std::fs::write(
        root.join("lib/ply.pkg"),
        manifest("lib", &dep("app", "../app")),
    )
    .unwrap();
    std::fs::write(root.join("lib/x.ply"), "pub fn x() -> Int = 1\n").unwrap();
    let out = ply(dir.path()).args(["check", "app"]).output().unwrap();
    assert_eq!(out.status.code(), Some(2));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("E0134"), "{err}");
}

#[test]
fn a_git_dependency_that_cannot_be_fetched_says_so() {
    let dir = tempfile::tempdir().expect("a temp dir");
    std::fs::write(
        dir.path().join("ply.pkg"),
        manifest(
            "app",
            "{name: \"glib\", prefix: None, min: {major: 0, minor: 0, patch: 1}, source: Git(\"https://example.invalid/glib\", \"abc123\")}",
        ),
    )
    .unwrap();
    std::fs::write(dir.path().join("main.ply"), "fn main() -> Int = 1\n").unwrap();
    let out = ply(dir.path()).arg("check").output().unwrap();
    assert_eq!(out.status.code(), Some(2));
    let err = String::from_utf8_lossy(&out.stderr);
    // A url that does not resolve is the dependency's trouble, named as such: git could not fetch
    // it, and nothing was read.
    assert!(err.contains("E0140") || err.contains("E0135"), "{err}");
    assert!(err.contains("glib"), "{err}");
}

/// The one source that still has no resolver: a registry arrives with resolution (P15).
#[test]
fn a_registry_dependency_is_told_what_resolves_today() {
    let dir = tempfile::tempdir().expect("a temp dir");
    std::fs::write(
        dir.path().join("ply.pkg"),
        manifest(
            "app",
            "{name: \"glib\", prefix: None, min: {major: 0, minor: 0, patch: 1}, source: Registry}",
        ),
    )
    .unwrap();
    std::fs::write(dir.path().join("main.ply"), "fn main() -> Int = 1\n").unwrap();
    let out = ply(dir.path()).arg("check").output().unwrap();
    assert_eq!(out.status.code(), Some(2));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("E0135"), "{err}");
    assert!(err.contains("path dependencies"), "{err}");
}

#[test]
fn a_dependency_may_not_reach_back_into_the_root_package() {
    let dir = graph();
    // `onlyroot` exists only in the root package. A bare import inside a dependency names
    // that package's own modules and nothing else, so the root's module is invisible to
    // `lib`: the import simply finds no module.
    std::fs::write(
        dir.path().join("app/onlyroot.ply"),
        "fn shared() -> Int = 9\n",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("lib/answer.ply"),
        "import onlyroot\npub fn answer() -> Int = onlyroot::shared()\n",
    )
    .unwrap();
    let out = ply(dir.path()).args(["check", "app"]).output().unwrap();
    assert_eq!(out.status.code(), Some(2));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("E0106"), "{err}");
    assert!(err.contains("no module named `onlyroot`"), "{err}");
}

/// `manifest` with a version of its own and a dependency carrying a floor, for the version
/// judgments. Two manifests asking different floors of one package is legal; two *places* holding
/// one package is not.
fn manifest_at(name: &str, version: (u64, u64, u64), deps: &str) -> String {
    format!(
        "import std.pkg (Manifest)\nfn package() -> Manifest = {{name: \"{name}\", version: {{major: {}, minor: {}, patch: {}}}, prefix: None, runtime: {{major: 0, minor: 0, patch: 1}}, dependencies: [{deps}], entry: None}}\n",
        version.0, version.1, version.2
    )
}

fn dep_at(name: &str, floor: (u64, u64, u64), path: &str) -> String {
    format!(
        "{{name: \"{name}\", prefix: None, min: {{major: {}, minor: {}, patch: {}}}, source: Path(\"{path}\")}}",
        floor.0, floor.1, floor.2
    )
}

#[test]
fn a_dependency_below_the_floor_its_importer_asks_for_is_refused() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let root = dir.path();
    for pkg in ["app", "lib"] {
        std::fs::create_dir(root.join(pkg)).unwrap();
    }
    std::fs::write(
        root.join("app/ply.pkg"),
        manifest_at("app", (0, 1, 0), &dep_at("lib", (0, 2, 0), "../lib")),
    )
    .unwrap();
    std::fs::write(
        root.join("app/main.ply"),
        "import lib.x\nfn main() -> Int = x::x()\n",
    )
    .unwrap();
    std::fs::write(root.join("lib/ply.pkg"), manifest_at("lib", (0, 1, 3), "")).unwrap();
    std::fs::write(root.join("lib/x.ply"), "pub fn x() -> Int = 1\n").unwrap();

    let out = ply(dir.path()).args(["check", "app"]).output().unwrap();
    assert_eq!(out.status.code(), Some(2));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("E0136"), "{err}");
    assert!(err.contains("is version 0.1.3"), "{err}");
    assert!(err.contains("at least 0.2.0"), "{err}");

    // The floor is a floor: the same tree with the dependency at or above it checks.
    std::fs::write(root.join("lib/ply.pkg"), manifest_at("lib", (0, 2, 0), "")).unwrap();
    let out = ply(dir.path()).args(["check", "app"]).output().unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// The ordinary diamond: two packages depend on one package, at two floors of it. One version
/// serves the closure, so this is what a program over a shared dependency looks like.
#[test]
fn two_dependents_of_one_package_resolve_to_its_one_version() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let root = dir.path();
    for pkg in ["app", "left", "right", "shared"] {
        std::fs::create_dir(root.join(pkg)).unwrap();
    }
    std::fs::write(
        root.join("app/ply.pkg"),
        manifest_at(
            "app",
            (0, 1, 0),
            &format!(
                "{}, {}",
                dep_at("left", (0, 1, 0), "../left"),
                dep_at("right", (0, 1, 0), "../right")
            ),
        ),
    )
    .unwrap();
    std::fs::write(
        root.join("app/main.ply"),
        "import left.l\nimport right.r\nfn main() -> Int = l::l() + r::r()\n",
    )
    .unwrap();
    std::fs::write(
        root.join("left/ply.pkg"),
        manifest_at("left", (0, 1, 0), &dep_at("shared", (0, 1, 0), "../shared")),
    )
    .unwrap();
    std::fs::write(
        root.join("left/l.ply"),
        "import shared.s\npub fn l() -> Int = s::s()\n",
    )
    .unwrap();
    std::fs::write(
        root.join("right/ply.pkg"),
        manifest_at(
            "right",
            (0, 1, 0),
            &dep_at("shared", (0, 3, 0), "../shared"),
        ),
    )
    .unwrap();
    std::fs::write(
        root.join("right/r.ply"),
        "import shared.s\npub fn r() -> Int = s::s()\n",
    )
    .unwrap();
    std::fs::write(
        root.join("shared/ply.pkg"),
        manifest_at("shared", (0, 3, 0), ""),
    )
    .unwrap();
    std::fs::write(root.join("shared/s.ply"), "pub fn s() -> Int = 20\n").unwrap();

    let out = ply(dir.path()).args(["run", "app"]).output().unwrap();
    let err = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(0), "{err}");
    assert!(String::from_utf8_lossy(&out.stdout).contains("40"), "{err}");
}

#[test]
fn one_package_of_one_name_at_two_places_has_no_version_to_pick() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let root = dir.path();
    for pkg in ["app", "left", "right", "d1", "d2"] {
        std::fs::create_dir(root.join(pkg)).unwrap();
    }
    std::fs::write(
        root.join("app/ply.pkg"),
        manifest_at(
            "app",
            (0, 1, 0),
            &format!(
                "{}, {}",
                dep_at("left", (0, 1, 0), "../left"),
                dep_at("right", (0, 1, 0), "../right")
            ),
        ),
    )
    .unwrap();
    std::fs::write(root.join("app/main.ply"), "fn main() -> Int = 1\n").unwrap();
    for (side, other) in [("left", "d1"), ("right", "d2")] {
        std::fs::write(
            root.join(side).join("ply.pkg"),
            manifest_at(
                side,
                (0, 1, 0),
                &dep_at("shared", (0, 1, 0), &format!("../{other}")),
            ),
        )
        .unwrap();
        std::fs::write(
            root.join(side).join(format!("{side}.ply")),
            format!("pub fn {side}() -> Int = 1\n"),
        )
        .unwrap();
    }
    for (other, version) in [("d1", (0, 1, 0)), ("d2", (0, 2, 0))] {
        std::fs::write(
            root.join(other).join("ply.pkg"),
            manifest_at("shared", version, ""),
        )
        .unwrap();
        std::fs::write(root.join(other).join("s.ply"), "pub fn s() -> Int = 1\n").unwrap();
    }

    let out = ply(dir.path()).args(["check", "app"]).output().unwrap();
    assert_eq!(out.status.code(), Some(2));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("E0137"), "{err}");
    assert!(err.contains("`shared` is reached at two places"), "{err}");
    assert!(err.contains("`left` wants"), "{err}");
    assert!(err.contains("`right` wants"), "{err}");
}

/// A build records what it resolved, and refuses what the record does not describe.
#[test]
fn a_build_pins_its_dependencies_and_refuses_what_the_lock_pins_differently() {
    let dir = graph();
    let built = |args: &[&str]| ply(dir.path()).args(args).output().unwrap();
    let lock_path = dir.path().join("app/ply.lock");

    let out = built(&["build", "app", "-o", "app.plyx"]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let lock = std::fs::read_to_string(&lock_path).expect("the build pins what it resolved");
    for want in [
        "\"name\":\"lib\"",
        "\"name\":\"base\"",
        "\"version\":\"0.0.1\"",
        "\"digest\":\"b3:",
    ] {
        assert!(lock.contains(want), "`{want}` is not in the lock: {lock}");
    }
    // The closure's packages are pinned, and the project's own sources are not.
    assert!(!lock.contains("\"name\":\"app\""), "{lock}");

    // The same tree builds again without moving the lock: a pin is not rewritten per run.
    let out = built(&["build", "app", "-o", "app.plyx"]);
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(std::fs::read_to_string(&lock_path).unwrap(), lock);

    // A dependency whose bytes moved is not the one the lock pins.
    std::fs::write(
        dir.path().join("lib/answer.ply"),
        "import base.deep\npub fn answer() -> Int = deep::deep() + 1\n",
    )
    .unwrap();
    let out = built(&["build", "app", "-o", "app.plyx"]);
    assert_eq!(out.status.code(), Some(2));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("E0138"), "{err}");
    assert!(err.contains("`lib` is not what `ply.lock` pins"), "{err}");
    assert_eq!(std::fs::read_to_string(&lock_path).unwrap(), lock);

    // Pinning what is on disk now is a deliberate act, and the lock says so afterwards.
    std::fs::remove_file(&lock_path).unwrap();
    let out = built(&["build", "app", "-o", "app.plyx"]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let repinned = std::fs::read_to_string(&lock_path).unwrap();
    assert_ne!(repinned, lock, "the digest did not move with the sources");
    let out = built(&["build", "app", "-o", "app.plyx"]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "the new pin is the one in force"
    );
}

#[test]
fn a_lockfile_nothing_can_read_is_refused_rather_than_ignored() {
    let dir = graph();
    std::fs::write(dir.path().join("app/ply.lock"), "{ not json").unwrap();
    let out = ply(dir.path())
        .args(["build", "app", "-o", "app.plyx"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("E0139"), "{err}");
    assert!(
        !dir.path().join("app.plyx").exists(),
        "an artifact was written"
    );
}

/// `ply resolve` is how a lockfile is written deliberately — after a dependency changed, or in a
/// fresh checkout — and `ply why` says how a package got here.
#[test]
fn resolve_writes_the_lock_and_why_names_the_path_to_a_package() {
    let dir = graph();
    let out = ply(dir.path()).args(["resolve", "app"]).output().unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("resolved 2 packages"), "{stdout}");
    let lock = std::fs::read_to_string(dir.path().join("app/ply.lock")).unwrap();
    assert!(lock.contains("\"name\":\"lib\""), "{lock}");
    assert!(lock.contains("\"name\":\"base\""), "{lock}");

    // Why: the root's own module reaches `lib`, and `lib`'s reaches `base`.
    let out = ply(dir.path())
        .args(["why", "lib", "app"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("app -> lib"), "{stdout}");
    assert!(stdout.contains("0.0.1"), "{stdout}");

    let out = ply(dir.path())
        .args(["why", "base", "app", "--json"])
        .output()
        .unwrap();
    let v: Value = json_of(&out);
    assert_eq!(v["command"], "why");
    assert_eq!(v["package"], "base");
    assert_eq!(v["path"], serde_json::json!(["app", "lib", "base"]));
    assert_eq!(v["root"], false);
    assert!(v["digest"].as_str().unwrap().starts_with("b3:"), "{v}");

    // The root package is what reaches, so its path is itself.
    let out = ply(dir.path())
        .args(["why", "app", "app", "--json"])
        .output()
        .unwrap();
    let v: Value = json_of(&out);
    assert_eq!(v["root"], true);
    assert_eq!(v["path"], serde_json::json!(["app"]));

    // A name nothing declares is a query that found nothing, not a build failure.
    let out = ply(dir.path())
        .args(["why", "nope", "app"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("E0101"), "{err}");
    assert!(err.contains("is not a package in this closure"), "{err}");
}

#[test]
fn a_resolve_re_pins_a_dependency_whose_sources_moved() {
    let dir = graph();
    let out = ply(dir.path()).args(["resolve", "app"]).output().unwrap();
    assert_eq!(out.status.code(), Some(0));
    let before = std::fs::read_to_string(dir.path().join("app/ply.lock")).unwrap();

    std::fs::write(
        dir.path().join("base/deep.ply"),
        "pub fn deep() -> Int = 8\n",
    )
    .unwrap();
    // The build refuses what the lock pins differently, and `resolve` is what accepts it.
    let out = ply(dir.path())
        .args(["build", "app", "-o", "app.plyx"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("E0138"));

    let out = ply(dir.path()).args(["resolve", "app"]).output().unwrap();
    assert_eq!(out.status.code(), Some(0));
    let after = std::fs::read_to_string(dir.path().join("app/ply.lock")).unwrap();
    assert_ne!(after, before, "the pin did not move with the sources");
    let out = ply(dir.path())
        .args(["build", "app", "-o", "app.plyx"])
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// A library is a package, not a program: `ply build` writes the package itself — its sources, its
/// manifest and a compiled unit — under a container of its own.
#[test]
fn a_library_builds_as_a_package_and_never_as_a_program() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let built = |args: &[&str]| ply(dir.path()).args(args).output().unwrap();

    let out = built(&["new", "core", "--lib"]);
    assert_eq!(out.status.code(), Some(0));
    let out = built(&["build", "core", "--json"]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: Value = json_of(&out);
    assert_eq!(v["command"], "build");
    assert_eq!(v["library"], true);
    assert_eq!(v["name"], "core");
    assert_eq!(v["version"], "0.1.0");
    assert_eq!(v["definitions"], 1, "{v}");
    assert!(v["digest"].as_str().unwrap().starts_with("b3:"), "{v}");
    assert_eq!(v["unit"], true);

    let bytes = std::fs::read(dir.path().join("core.plyz")).expect("the library is written");
    assert_eq!(
        &bytes[..8],
        b"PLYLIB01",
        "a library is not a program container"
    );
    assert_eq!(
        String::from_utf8_lossy(&bytes[..8]),
        "PLYLIB01",
        "and its magic is not a program's"
    );

    // A library is never entered: reading it as sources would report a container as text.
    let out = built(&["run", "core.plyz"]);
    assert_eq!(out.status.code(), Some(2));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("is a library, and nothing to run"), "{err}");
    assert!(err.contains("declare it in a `ply.pkg`"), "{err}");

    // A program package still builds a `.plyx`, and the library's own tests still run.
    let out = built(&["new", "app"]);
    assert_eq!(out.status.code(), Some(0));
    let out = built(&["build", "app"]);
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        dir.path().join("main.plyx").exists(),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    let out = built(&["test", "core"]);
    assert_eq!(out.status.code(), Some(0));
}

/// A git repository in a temp directory, with the one commit `HEAD` points at.
fn git_repo(dir: &std::path::Path, source: &str) -> String {
    let run = |args: &[&str]| {
        let out = std::process::Command::new("git")
            .current_dir(dir)
            .args(args)
            .output()
            .unwrap_or_else(|e| panic!("`git {args:?}`: {e}"));
        assert!(
            out.status.success(),
            "`git {args:?}`: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    };
    if !dir.join(".git").exists() {
        run(&["init", "--quiet"]);
    }
    std::fs::write(dir.join("ply.pkg"), manifest("gitlib", "")).unwrap();
    std::fs::write(dir.join("answer.ply"), source).unwrap();
    run(&["add", "-A"]);
    run(&[
        "-c",
        "user.email=t@test",
        "-c",
        "user.name=test",
        "commit",
        "--quiet",
        "-m",
        "a",
    ]);
    let out = std::process::Command::new("git")
        .current_dir(dir)
        .args(["rev-parse", "HEAD"])
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn git_project(dir: &std::path::Path, lib: &std::path::Path, rev: &str) {
    let root = dir.join("app");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(
        root.join("ply.pkg"),
        manifest(
            "app",
            &format!(
                "{{name: \"gitlib\", prefix: None, min: {{major: 0, minor: 0, patch: 1}}, source: Git(\"{}\", \"{rev}\")}}",
                lib.display()
            ),
        ),
    )
    .unwrap();
    std::fs::write(
        root.join("main.ply"),
        "import gitlib.answer\nfn main() -> Int = answer::answer()\ntest \"answers\" { assert_eq(answer::answer(), 7) }\n",
    )
    .unwrap();
}

#[test]
fn a_git_dependency_is_fetched_and_reads_like_any_other_package() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let lib = dir.path().join("lib");
    std::fs::create_dir_all(&lib).unwrap();
    let rev = git_repo(&lib, "pub fn answer() -> Int = 7\n");
    git_project(dir.path(), &lib, &rev);

    // The walk fetches it: `check` loads through the Ply walker, `run` through the machine's.
    for (args, want) in [
        (vec!["check", "app"], "checked 2 modules"),
        (vec!["run", "app"], "7"),
        (vec!["test", "app"], "1 passed"),
    ] {
        let out = ply(dir.path()).args(&args).output().unwrap();
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(out.status.code(), Some(0), "{args:?}: {text}");
        assert!(text.contains(want), "{args:?}: {text}");
    }

    // The tree is fetched under the project's own cache, once, and `resolve` pins what it read.
    assert!(dir.path().join("app/.ply-cache/git").exists());
    let out = ply(dir.path()).args(["resolve", "app"]).output().unwrap();
    assert_eq!(out.status.code(), Some(0));
    let lock = std::fs::read_to_string(dir.path().join("app/ply.lock")).unwrap();
    assert!(lock.contains("\"name\":\"gitlib\""), "{lock}");
    // The cache is reused: a second load does not need a repository to fetch from.
    std::fs::remove_dir_all(&lib).unwrap();
    let out = ply(dir.path()).args(["check", "app"]).output().unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn a_git_dependency_whose_branch_moved_is_not_what_the_lock_pins() {
    let dir = tempfile::tempdir().expect("a temp dir");
    let lib = dir.path().join("lib");
    std::fs::create_dir_all(&lib).unwrap();
    let _rev = git_repo(&lib, "pub fn answer() -> Int = 7\n");
    git_project(dir.path(), &lib, "HEAD");
    std::fs::write(
        dir.path().join("app/main.ply"),
        "import gitlib.answer\nfn main() -> Int = answer::answer()\n",
    )
    .unwrap();

    let out = ply(dir.path()).args(["resolve", "app"]).output().unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let before = std::fs::read_to_string(dir.path().join("app/ply.lock")).unwrap();

    // A branch means what it means today, and the cache holds the tree the resolve fetched. A
    // cleared cache is what asks the branch again — and then the lock is what says the sources
    // moved, before a build writes anything.
    git_repo(&lib, "pub fn answer() -> Int = 8\n");
    std::fs::remove_dir_all(dir.path().join("app/.ply-cache/git")).unwrap();
    let out = ply(dir.path())
        .args(["build", "app", "-o", "app.plyx"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("E0138"), "{err}");
    assert_eq!(
        std::fs::read_to_string(dir.path().join("app/ply.lock")).unwrap(),
        before
    );

    // Accepting the new tip is `ply resolve`, and then the build is green again.
    let out = ply(dir.path()).args(["resolve", "app"]).output().unwrap();
    assert_eq!(out.status.code(), Some(0));
    assert_ne!(
        std::fs::read_to_string(dir.path().join("app/ply.lock")).unwrap(),
        before
    );
    let out = ply(dir.path())
        .args(["build", "app", "-o", "app.plyx"])
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}
