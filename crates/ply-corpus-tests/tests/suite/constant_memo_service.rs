use anyhow::Result;
use ply_corpus::{w3, w6_run};
use std::path::{Path, PathBuf};

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("the workspace root is two directories above this crate")
        .to_path_buf()
}

fn script() -> Vec<(&'static str, Vec<u8>)> {
    vec![
        ("/health", w3::request("GET", "/health", None, false, 0, 0)),
        ("/ready", w3::request("GET", "/ready", None, false, 0, 0)),
        ("/items", w3::request("GET", "/items", None, false, 0, 0)),
        (
            "/items/featured",
            w3::request("GET", "/items/featured", None, false, 0, 0),
        ),
        (
            "/items/bolt",
            w3::request("GET", "/items/bolt", None, false, 0, 0),
        ),
        ("/orders", w3::request("GET", "/orders", None, false, 0, 0)),
        (
            "/orders/1",
            w3::request("GET", "/orders/1", None, false, 0, 0),
        ),
        (
            "/docs/orders/placing",
            w3::request("GET", "/docs/orders/placing", None, false, 0, 0),
        ),
        (
            "/nowhere",
            w3::request("GET", "/nowhere", None, false, 0, 0),
        ),
        (
            "/orders put",
            w3::request("PUT", "/orders", None, false, 0, 0),
        ),
        ("* options", w3::request("OPTIONS", "*", None, false, 0, 0)),
    ]
}

fn variants() -> Result<(w3::Loaded, w3::Loaded)> {
    let service = w3::Service::open(&repo())?;
    let source = service.source(w3::Variant::Sequential)?;
    // The ladder's own rewrite, so this control and the one `w6-ladder` prices against are one program.
    let control = w6_run::without_constants(&source);
    assert_ne!(source, control, "the rewrite found nothing to disable");
    Ok((w3::Loaded::parse(&source)?, w3::Loaded::parse(&control)?))
}

#[test]
fn remembering_a_constant_changes_no_byte_of_any_response() {
    let (memoized, control) = variants().expect("both variants load");
    for (what, request) in script() {
        let a = memoized
            .response_over_sim(&request)
            .unwrap_or_else(|e| panic!("`{what}` raised on the shipped service: {e}"));
        let b = control
            .response_over_sim(&request)
            .unwrap_or_else(|e| panic!("`{what}` raised on the control: {e}"));
        assert_eq!(
            String::from_utf8_lossy(&a),
            String::from_utf8_lossy(&b),
            "`{what}` answered differently once its constants were remembered"
        );
    }
}
