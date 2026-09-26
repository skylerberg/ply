//! The generator, pinned: the corpus a seed produces is a function of the seed, and this is
//! the function's current value. When the port changed nothing, these hashes and sites are
//! what the Rust generator produced; when the generator changes on purpose, re-take them.

use ply_corpus::spec::CorpusSpec;

#[test]
fn seed_one_corpus_is_pinned() {
    let spec = CorpusSpec {
        seed: 1,
        modules: 4,
        defs_per_module: 5,
        tests: 8,
        depth: 2,
        tables: 3,
        regions: 2,
        ..CorpusSpec::default()
    };
    let (hash, manifest) = pinned(&spec);
    assert_eq!(
        hash, "ab4868d2f6b44d5bc0c7117f3dd21359034931aca2956dd5cc3c0eb9ed0c94de",
        "the seed-1 corpus changed; if the change is intended, re-take the pin"
    );
    assert_eq!(manifest["hub_edit"]["dependents"].as_i64(), Some(9));
    assert_eq!(manifest["leaf_edit"]["dependents"].as_i64(), Some(1));
    assert_eq!(manifest["rename"]["symbol"].as_str(), Some("render_11"));
}

#[test]
fn seed_seven_corpus_is_pinned() {
    let spec = CorpusSpec {
        seed: 7,
        modules: 6,
        defs_per_module: 8,
        tests: 12,
        depth: 3,
        tables: 4,
        regions: 3,
        hub_modules: 2,
        concurrent_tests: 2,
        spec_fraction: 0.5,
        specimens_per_module: 1,
        ..CorpusSpec::default()
    };
    let (hash, manifest) = pinned(&spec);
    assert_eq!(
        hash, "c2e0cba4f57a689b83ff4d16c158d28bd8ac5e3d4762327852602a84d03c3be9",
        "the seed-7 corpus changed; if the change is intended, re-take the pin"
    );
    assert_eq!(manifest["hub_edit"]["dependents"].as_i64(), Some(8));
    assert_eq!(manifest["leaf_edit"]["dependents"].as_i64(), Some(1));
    assert_eq!(manifest["rename"]["symbol"].as_str(), Some("merge_36"));
}

fn pinned(spec: &CorpusSpec) -> (String, serde_json::Value) {
    let files = crate::support::dump_files(spec);
    let mut h = blake3::Hasher::new();
    for (path, text) in &files {
        h.update(path.as_bytes());
        h.update(&[0]);
        h.update(text.as_bytes());
        h.update(&[0]);
    }
    (
        h.finalize().to_hex().to_string(),
        crate::support::dump_manifest(spec),
    )
}
