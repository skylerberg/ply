use ply_machine::load::{Loaded, load};

fn fixture(text: &str) -> (tempfile::TempDir, Loaded) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("m.ply"), text).unwrap();
    let loaded = load(dir.path()).unwrap();
    (dir, loaded)
}

#[test]
fn a_reuse_fn_is_refused_only_for_a_copy_its_own_body_causes() {
    // Kept: the append is the last use of a parameter, whatever the caller does with it.
    let (_dir, loaded) = fixture(
        "reuse fn grow(xs: List<Int>, n: Int) -> List<Int> = push(xs, n)\n\
         fn keep(xs: List<Int>) -> Int = len(grow(xs, 1)) + len(xs)\n",
    );
    assert!(loaded.promised);
    assert!(ply_machine::costs::promises(&loaded).is_empty());

    // Broken: the binding is read again after the append, inside the promised body.
    let (_dir, loaded) = fixture(
        "reuse fn grow(xs: List<Int>, n: Int) -> List<Int> = {\n\
         \x20 let ys = push(xs, n);\n\
         \x20 if len(xs) < 0 { xs } else { ys }\n\
         }\n",
    );
    let broken = ply_machine::costs::promises(&loaded);
    assert_eq!(broken.len(), 1, "{broken:#?}");
    assert_eq!(broken[0].code, ply_span::codes::REUSE_BROKEN);
    assert!(broken[0].message.contains("`grow` is a `reuse fn`"));
    assert!(broken[0].notes.iter().any(|n| n.contains("last use")));

    // The same body without the marker is what `--costs` reports, not an error.
    let (_dir, loaded) = fixture(
        "fn grow(xs: List<Int>, n: Int) -> List<Int> = {\n\
         \x20 let ys = push(xs, n);\n\
         \x20 if len(xs) < 0 { xs } else { ys }\n\
         }\n",
    );
    assert!(!loaded.promised);
    assert!(ply_machine::costs::promises(&loaded).is_empty());
}

#[test]
fn the_port_keeps_a_promise_over_a_fresh_list_and_refuses_one_over_a_map_entry() {
    let (_dir, loaded) = fixture(
        "reuse fn fill(n: Int) -> List<Int> = {\n\
         \x20 let xs = range(0, n);\n\
         \x20 push(xs, n)\n\
         }\n",
    );
    assert!(loaded.promised);
    assert!(ply_machine::costs::promises(&loaded).is_empty());

    let (_dir, loaded) = fixture(
        "reuse fn grow(m: Map<Int, List<Int>>, n: Int) -> List<Int> =\n\
         \x20 match map_get(m, 0) { Some(xs) -> push(xs, n), None -> [] }\n",
    );
    let broken = ply_machine::costs::promises(&loaded);
    assert_eq!(broken.len(), 1, "{broken:#?}");
    let d = &broken[0];
    assert_eq!(d.code, ply_span::codes::REUSE_BROKEN);
    assert_eq!(
        d.message,
        "`grow` is a `reuse fn`, and this append copies its list: `map_get` answers a clone the \
         map still holds"
    );
    assert_eq!(d.notes, ["fix: `map_update`"]);
    let text = |span: ply_span::Span| {
        let file = loaded
            .sources
            .get(span.source)
            .expect("a span into the fixture");
        file.text[span.start as usize..span.end as usize].to_string()
    };
    let labels: Vec<(bool, &str, String)> = d
        .labels
        .iter()
        .map(|l| (l.primary, l.message.as_str(), text(l.span)))
        .collect();
    assert_eq!(
        labels,
        [
            (true, "this append", "push(xs, n)".to_string()),
            (false, "the promise", "reuse".to_string()),
        ]
    );
}
