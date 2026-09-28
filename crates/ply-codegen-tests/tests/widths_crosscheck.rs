//! Temporary: the compiler's fixed-width flag against the Rust check it replaces.

const SRC: &str = r#"
fn wide(x: U32) -> U32 = x
fn narrow(x: Int) -> Int = x
fn record_of_width(r: { a: U8, b: Int }) -> Int = r.b
fn nested(xs: List<I64>) -> Int = len(xs)
pub fn generic<a>(x: a) -> a = x
fn wide_in_fn(f: (U16) -> Bool) -> Bool = f(1u16)
"#;

#[test]
fn the_compilers_width_flag_is_the_rust_check() {
    let id = ply_span::SourceId(0);
    let front =
        ply_codegen::c::producer::checked_front(&[("m".to_string(), SRC.to_string())], &[id])
            .expect("checks");
    for root in &front.emitter_roots {
        let want = front
            .check
            .defs
            .get(&root.root)
            .is_some_and(|def| ply_eval::mentions_a_width(&def.scheme.ty));
        assert_eq!(root.width, want, "root `{}`", root.root);
    }
    let flag = |name: &str| {
        front
            .emitter_roots
            .iter()
            .find(|r| r.root.as_str() == name)
            .map(|r| r.width)
    };
    assert_eq!(flag("m.wide"), Some(true));
    assert_eq!(flag("m.record_of_width"), Some(true));
    assert_eq!(flag("m.nested"), Some(true));
    assert_eq!(flag("m.wide_in_fn"), Some(true));
    assert_eq!(flag("m.narrow"), Some(false));
    assert_eq!(flag("m.generic"), Some(false));
}
