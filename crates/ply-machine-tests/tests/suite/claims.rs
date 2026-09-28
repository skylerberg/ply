//! The table that says where each type this side marshals is declared, held to the program it is
//! marshalled into. It lives here rather than in `ply-machine`'s own tests because its subject is
//! `crates/ply-cli/ply` -- a project the suite loads and the library does not.

use ply_machine::claims::MARSHALLED;
use std::path::PathBuf;

/// The CLI tree, whose package declares the marshalled types.
fn cli_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../ply-cli/ply")
}

/// Every row has to name a module that declares the type.
///
/// The tag this side builds is `<module>.<constructor>` and the program matches it against the name
/// its own spine gives the constructor, so a type that moved module and left its row behind is a
/// value no arm matches. `replay`'s `the_fixture_declares_the_payload_where_the_machine_names_it`
/// is the same claim about the fixture rather than about the program.
#[test]
fn every_marshalled_type_is_declared_where_this_side_says() {
    let loaded = ply_machine::load::load(&cli_root()).expect("the CLI tree loads");
    for (home, ty) in MARSHALLED {
        assert!(
            loaded
                .check
                .ctors
                .values()
                .any(|c| c.type_name.as_str().rsplit('.').next() == Some(*ty)
                    && c.module.as_str() == *home),
            "no constructor of `{ty}` is declared in `{home}`, so a tag built from it names \
             nothing the program matches"
        );
    }
}
