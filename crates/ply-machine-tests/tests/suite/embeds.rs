//! A program the builder makes embeds as `ply`'s own load does: each path read beside the module
//! that asks, written into it as a literal, and handed to the compiled tier with the answer.

use crate::fixture::{backend, loaded, scratch, write};
use ply_machine::testrun::{self, Executor, Hosting};

#[test]
fn a_program_loaded_from_nothing_runs_what_it_embedded() {
    let dir = scratch();
    write(
        dir.path(),
        "sub/m.ply",
        "fn one() -> Bytes = embed(\"../data.txt\")\n\
         fn all() -> List<{ name: String, bytes: Bytes }> = embed_dir(\"files\")\n\n\
         test \"embedded\" {\n  assert_eq(one(), b\"hi\");\n  \
         assert_eq(map(all(), |f: { name: String, bytes: Bytes }| f.name), \
         [\"a.txt\", \"b.txt\", \"deeper/c.txt\"]);\n  \
         assert_eq(map(all(), |f: { name: String, bytes: Bytes }| f.bytes), [b\"A\", b\"B\", b\"C\"])\n}\n",
    );
    write(dir.path(), "data.txt", "hi");
    write(dir.path(), "sub/files/b.txt", "B");
    write(dir.path(), "sub/files/a.txt", "A");
    write(dir.path(), "sub/files/deeper/c.txt", "C");
    write(dir.path(), "sub/files/.cache/d.txt", "not read");
    write(dir.path(), "sub/files/.hidden", "not read");
    let loaded = loaded(dir.path());
    let executor = Executor {
        front: &loaded.front,
        hosting: Hosting::default(),
        provider: backend(&loaded),
    };
    let ran = testrun::executed(&executor, 0);
    assert!(ran.failure.is_none(), "the test passes: {:?}", ran.failure);
}

#[test]
fn an_embed_nothing_can_be_read_for_refuses_the_load() {
    let dir = scratch();
    write(
        dir.path(),
        "m.ply",
        "fn gone() -> Bytes = embed(\"missing.txt\")\n",
    );
    let refused = crate::fixture::refusal(dir.path());
    assert_eq!(refused.len(), 1, "{refused:?}");
    assert_eq!(refused[0].code, "E0146");
    assert_eq!(refused[0].message, "`missing.txt` could not be embedded");
}
