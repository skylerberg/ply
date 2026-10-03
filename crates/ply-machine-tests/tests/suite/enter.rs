//! What the launcher enters a built program with: the label is the capability, the code is the
//! program's, and a span its entry left open comes back beside it.

use crate::fixture::{handed, loaded, project, unit_text};
use ply_eval::{Ended, Span, Value, codes};
use ply_host::process::Executables;
use ply_machine::enter::{self, Binds};
use ply_machine::payload::field_of;
use ply_machine::runnable;
use std::path::Path;

/// `source`'s `main`, written as a runnable, read back and entered with `binds`.
fn entered(source: &str, binds: Binds) -> Ended<i32> {
    let dir = project(source);
    let front = handed(dir.path());
    let files: &Value = field_of(&front, "files", Span::DUMMY).expect("a front's files");
    let dump = field_of(&front, "dump", Span::DUMMY).expect("a front's dump");
    let unit = unit_text(&loaded(dir.path()));
    let bytes = runnable::encode("m.main", files, dump, &unit).expect("the runnable encodes");
    let program = runnable::decode(&bytes).expect("the runnable reads back");
    let opened = enter::opened_runnable(program, dir.path()).expect("it opens");
    enter::enter_runnable(opened, Vec::new(), binds)
}

fn binding_cc() -> Binds {
    let mut executables = Executables::new();
    executables
        .bind("cc", Path::new("/bin/sh"), Span::DUMMY)
        .expect("a shell is a program");
    Binds {
        executables,
        ..Binds::default()
    }
}

/// Exits with the code of whatever `cc` names, so nothing but the caller's binding decides what
/// ran.
const SPAWNS: &str = r#"
import std.process (process, exit_code)

fn main() -> Unit / {process.spawn[cc], process.exit[proc]} = {
  let done = process.spawn[cc](["-c", "exit 7"], "", []);
  match exit_code(done) {
    Some(code) -> process.exit[proc](code),
    None -> process.exit[proc](9),
  }
}
"#;

#[test]
fn an_entered_program_cannot_spawn_a_label_nothing_bound() {
    let (entered, _) = entered(SPAWNS, Binds::default()).into_parts();
    let refused = entered.expect_err("`cc` is bound to nothing");
    assert_eq!(refused.code, codes::PROCESS_EXEC_UNBOUND);
}

#[test]
fn an_entered_program_starts_what_its_caller_bound_to_the_label() {
    let (entered, _) = entered(SPAWNS, binding_cc()).into_parts();
    let code = entered.expect("the program runs");
    assert_eq!(code, 7, "the child's own code is what came back");
}

/// Opens a span and answers without closing it.
const LEAVES_A_SPAN: &str = r#"
import std.trace
import std.trace (trace)

fn main() -> Int / {trace.write[orders]} = {
  let order = trace.enter[orders]("order", map_new());
  3
}
"#;

#[test]
fn an_entered_program_hands_back_the_span_it_left_open_beside_its_code() {
    let (code, warnings) = entered(LEAVES_A_SPAN, Binds::default()).into_parts();
    assert_eq!(code.expect("the program runs"), enter::EXIT_OK);
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert_eq!(warnings[0].code, codes::SPAN_ABANDONED, "{warnings:?}");
    assert!(
        warnings[0].message.contains("`order` on `orders`"),
        "{}",
        warnings[0].message
    );
}

/// Exits 7 when its caller bound a program to `cc` and 9 when it did not, and starts nothing.
const ASKS_FOR_CC: &str = r#"
import std.process (process)

fn main() -> Unit / {process.bound[cc], process.exit[proc]} =
  process.exit[proc](if process.bound[cc]() { 7 } else { 9 })
"#;

#[test]
fn an_entered_program_asks_whether_its_caller_bound_the_label_it_would_start() {
    for (binds, code) in [(Binds::default(), 9), (binding_cc(), 7)] {
        let (entered, _) = entered(ASKS_FOR_CC, binds).into_parts();
        assert_eq!(entered.expect("the program runs"), code);
    }
}
