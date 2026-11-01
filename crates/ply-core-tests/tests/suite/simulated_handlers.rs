use std::path::{Path, PathBuf};

fn sources(dir: &Path, found: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("the crate's own sources are readable") {
        let path = entry.expect("a readable directory entry").path();
        if path.is_dir() {
            sources(&path, found);
        } else if path.extension().is_some_and(|e| e == "rs") {
            found.push(path);
        }
    }
}

const FIXED_GENERATOR: (&str, &str) = ("crypto.rs", "use ring::test::rand::FixedSliceRandom;");

#[test]
fn the_evaluator_reads_no_host_clock_and_no_host_entropy() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("../ply-core/src");
    let mut files = Vec::new();
    sources(&src, &mut files);
    files.sort();
    assert!(files.len() > 5, "found no sources to check under {src:?}");

    for path in files {
        if path
            .file_name()
            .is_some_and(|n| n == "tests.rs" || n == "build.rs")
        {
            continue;
        }
        let whole = std::fs::read_to_string(&path).expect("a readable source");
        let text = whole.split("#[cfg(test)]").next().unwrap_or(&whole);
        let text = if path.file_name().is_some_and(|n| n == FIXED_GENERATOR.0) {
            text.replacen(FIXED_GENERATOR.1, "", 1)
        } else {
            text.to_string()
        };
        for banned in [
            "SystemTime",
            "Instant::now",
            "std::time",
            "rand::",
            "thread_rng",
            "getrandom",
            "RandomState",
            "DefaultHasher",
        ] {
            assert!(
                !text.contains(banned),
                "`{banned}` appears in {}: a simulated run must be a function of \
                 its definitions and its seed, so the evaluator may not reach the \
                 host's clock or entropy. Measure wall clock in the `testing` package, where \
                 no program can observe it.",
                path.display()
            );
        }
    }
}

#[test]
fn the_crate_depends_on_no_random_number_generator_and_no_entropy_source() {
    let manifest = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../ply-core/Cargo.toml"),
    )
    .expect("the crate has a manifest");
    for line in manifest.lines() {
        let Some((key, _)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        assert!(
            !["rand", "fastrand", "nanorand", "getrandom", "oorandom"].contains(&key),
            "`{key}` is a dependency of ply-core; a seeded run may not draw from one"
        );
    }
}
