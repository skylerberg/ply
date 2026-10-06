use ply_eval::host::MachineId;
use ply_eval::{
    Determinism, Diagnostic, EffectAtom, HostAnswer, HostRequest, HostRuntime, Linearity, Mode,
    Resource, Span, Symbol, Value, codes,
};
use ply_host::os::*;

struct Nothing;

impl HostRuntime for Nothing {
    fn watch(&self, _: &ply_eval::Pending) -> Result<(), Diagnostic> {
        Ok(())
    }
    fn resolved(&self) -> Vec<(u64, Result<Value, Diagnostic>)> {
        Vec::new()
    }
    fn park(&self) -> Result<(), Diagnostic> {
        Ok(())
    }
    fn block_on(&self, _: ply_eval::Pending) -> Result<Value, Diagnostic> {
        Ok(Value::Unit)
    }
}

fn call(op: Op, args: &[Value]) -> Result<Value, Diagnostic> {
    let handlers = registrations();
    let (declaration, handler) = handlers
        .iter()
        .find(|(d, _)| d.op.as_str() == op.name())
        .expect("every operation is registered");
    handler
        .call(
            &Nothing,
            &HostRequest {
                atom: EffectAtom::new(Symbol::new(EFFECT), Resource::Singleton, Mode::Read),
                op: declaration,
                args,
                span: Span::DUMMY,
                machine: MachineId(0),
                task: None,
                declared: None,
            },
        )
        .map(|answer| match answer {
            HostAnswer::Value(v) => v,
            HostAnswer::Pending(_) => panic!("a fact about the machine waits on nothing"),
        })
}

fn answer(op: Op) -> Value {
    match call(op, &[]) {
        Ok(v) => v,
        Err(d) => panic!("refused: {} {}", d.code, d.message),
    }
}

fn constructor(value: &Value) -> (String, Vec<Value>) {
    match value {
        Value::Ctor { name, args } => (name.as_str().to_string(), args.to_vec()),
        other => panic!("not a constructor: {other:?}"),
    }
}

fn text(value: &Value) -> String {
    match value {
        Value::Str(s) => s.to_string(),
        other => panic!("not a string: {other:?}"),
    }
}

fn int(value: &Value) -> i64 {
    match value {
        Value::Int(n) => *n,
        other => panic!("not an int: {other:?}"),
    }
}

/// What a command of the system's own writes, without its ending.
fn said(program: &str, args: &[&str]) -> String {
    let done = std::process::Command::new(program)
        .args(args)
        .output()
        .unwrap_or_else(|e| panic!("`{program}` runs: {e}"));
    String::from_utf8_lossy(&done.stdout).trim().to_string()
}

#[test]
fn the_registrations_declare_what_a_reviewer_relies_on() {
    let handlers = registrations();
    assert_eq!(handlers.len(), Op::ALL.len());
    let declaration =
        ply_machine::shipped_modules::source(&ply_eval::ModuleName::from_dotted("std.os"))
            .expect("std.os ships");
    assert!(declaration.contains("pub nondet effect os"));
    for (op, _) in &handlers {
        assert_eq!(op.effect.as_str(), EFFECT);
        assert_eq!(op.determinism, Determinism::Nondeterministic);
        assert_eq!(op.linearity, Linearity::Repeatable, "{op}");
        assert!(!op.blocking, "{op}");
        assert!(!op.secrets, "{op}");
        assert!(op.path.starts_with("ply_host::os::"));
        assert!(
            declaration.contains(&format!("  read {}() ->", op.op)),
            "`{}` is not declared a `read` in std.os",
            op.op
        );
    }
}

#[test]
fn the_family_and_the_architecture_are_the_ones_this_binary_was_built_for() {
    let (family, carried) = constructor(&answer(Op::Family));
    let expected = match std::env::consts::OS {
        "linux" => "std.os.Linux",
        "macos" => "std.os.MacOs",
        "windows" => "std.os.Windows",
        "freebsd" => "std.os.FreeBsd",
        "openbsd" => "std.os.OpenBsd",
        "netbsd" => "std.os.NetBsd",
        _ => "std.os.OtherFamily",
    };
    assert_eq!(family, expected);
    assert_eq!(carried.is_empty(), expected != "std.os.OtherFamily");
    let (arch, _) = constructor(&answer(Op::Arch));
    let expected = match std::env::consts::ARCH {
        "x86_64" => "std.os.X86_64",
        "aarch64" => "std.os.Aarch64",
        "x86" => "std.os.X86",
        "arm" => "std.os.Arm",
        "riscv64" => "std.os.Riscv64",
        _ => "std.os.OtherArch",
    };
    assert_eq!(arch, expected);
    let (endian, _) = constructor(&answer(Op::Endian));
    assert_eq!(
        endian,
        if cfg!(target_endian = "big") {
            "std.os.Big"
        } else {
            "std.os.Little"
        }
    );
    assert_eq!(int(&answer(Op::PointerBits)), i64::from(usize::BITS));
}

#[cfg(unix)]
#[test]
fn the_process_and_the_machine_answer_as_the_systems_own_tools_do() {
    assert_eq!(int(&answer(Op::Pid)), i64::from(std::process::id()));
    assert_eq!(text(&answer(Op::Name)), said("uname", &["-sr"]));
    assert_eq!(text(&answer(Op::Hostname)), said("uname", &["-n"]));
    let answered = answer(Op::User);
    let Value::Record(user) = &answered else {
        panic!("a user is a record");
    };
    let id = int(user.get(&Symbol::new("id")).expect("a user has an id"));
    assert_eq!(id.to_string(), said("id", &["-u"]));
    let (named, name) = constructor(user.get(&Symbol::new("name")).expect("a user has a name"));
    if named == "Some" {
        assert_eq!(text(&name[0]), said("id", &["-un"]));
    }
    let (found, path) = constructor(&answer(Op::Executable));
    assert_eq!(found, "Some");
    assert_eq!(
        text(&path[0]),
        std::env::current_exe()
            .and_then(|path| path.canonicalize())
            .expect("a test binary has a path")
            .to_string_lossy()
    );
}

#[test]
fn the_processors_are_the_ones_the_process_may_use() {
    let cpus = int(&answer(Op::Cpus));
    assert!(cpus >= 1);
    assert_eq!(
        cpus,
        std::thread::available_parallelism().map_or(1, |n| n.get() as i64)
    );
}

// Arity is inference's, so the wrong count means the module was never checked.
#[test]
fn the_wrong_arity_is_a_dispatch_defect() {
    let refused = call(Op::Pid, &[Value::Unit]).expect_err("pid takes nothing");
    assert_eq!(refused.code, codes::INTERNAL_ERROR);
}
