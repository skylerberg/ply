//! A test binary's process: the checkout's pack installed, and, when [`TRACES`] names a directory,
//! what the one test it runs reads of the pack and through the host recorded and written there, so
//! a later run can answer the test by its trace when none of what it read has moved.

use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

/// Where a test binary writes the trace of the test it ran.
pub const TRACES: &str = "PLY_TEST_TRACES";

/// The checkout, the test binary as nextest names it, and the record of the process.
struct Traced {
    repo: PathBuf,
    binary: &'static str,
    recorder: Arc<ply_host::observe::Recorder>,
}

static TRACED: OnceLock<Traced> = OnceLock::new();

/// What a test binary's constructor does: the checkout at `repo` installed as its pack, and, when
/// they are asked for, its reads recorded and written as the process exits. `binary` is the test
/// binary as nextest names it, `<package>::<target>`.
pub fn installed(repo: &Path, binary: &'static str) {
    ply_pack::install_checkout(repo);
    if std::env::var_os(TRACES).is_none() {
        return;
    }
    ply_pack::record();
    ply_host::observe::keeps(ply_codegen::c::kept_dirs());
    let traced = Traced {
        repo: repo.canonicalize().unwrap_or_else(|_| repo.to_path_buf()),
        binary,
        recorder: ply_host::observe::begin_process(),
    };
    if TRACED.set(traced).is_ok() {
        ply_host::observe::at_exit(written);
    }
}

extern "C" fn written() {
    let _ = std::panic::catch_unwind(traced);
}

/// The trace of the test this process ran, written under [`TRACES`]. A run of more than one test,
/// or a trace nothing can stand in for, writes none.
fn traced() {
    let (
        Some(dir),
        Some(Traced {
            repo,
            binary,
            recorder,
        }),
    ) = (std::env::var_os(TRACES), TRACED.get())
    else {
        return;
    };
    let Some(test) = one_test() else {
        return;
    };
    let roots = [("repo".to_string(), repo.clone())];
    let world = world(&roots);
    let Some(read) = ply_host::observe::finished(recorder, &world, false) else {
        return;
    };
    let mut text = format!("test\t{binary}\t{test}\n");
    for line in ply_pack::installed().asked_lines() {
        text.push_str(&line);
        text.push('\n');
    }
    text.push_str(&read);
    let id = blake3::hash(format!("{binary} {test}").as_bytes()).to_hex();
    let _ = std::fs::write(PathBuf::from(dir).join(&id[..32]), text);
}

/// The test a libtest binary was asked to run alone: `--exact NAME`, as nextest asks.
fn one_test() -> Option<String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if !args.iter().any(|a| a == "--exact") {
        return None;
    }
    let mut names = args.iter().filter(|a| !a.starts_with('-'));
    match (names.next(), names.next()) {
        (Some(name), None) => Some(name.clone()),
        _ => None,
    }
}

fn world(roots: &[(String, PathBuf)]) -> ply_host::observe::World<'_> {
    ply_host::observe::World {
        roots: Some(roots),
        binding: String::new(),
        binary: ply_host::observe::Binary {
            shipped: &crate::shipped::module_digest,
            program: String::new(),
        },
    }
}

/// A test's trace, as [`traced`] writes it: the test it is of, and whether what it read still
/// answers as it did from the checkout at `repo`, whose pack is installed.
pub struct Answered {
    pub binary: String,
    pub test: String,
    pub stands: bool,
}

/// `text`, a trace [`traced`] wrote, read against the checkout at `repo`. A `ply` the test started
/// reports its own program, which no checkout answers for, so such a test runs again.
pub fn answered(text: &str, repo: &Path) -> Option<Answered> {
    let mut lines = text.lines();
    let header: Vec<&str> = lines.next()?.split('\t').collect();
    let ["test", binary, test] = header.as_slice() else {
        return None;
    };
    let pack = ply_pack::installed();
    let roots = [("repo".to_string(), repo.to_path_buf())];
    let world = world(&roots);
    let mut rest = String::new();
    let mut stands = true;
    for line in lines {
        match pack.stands(line) {
            Some(answers) => stands &= answers,
            None => {
                rest.push_str(line);
                rest.push('\n');
            }
        }
    }
    let probe = ply_eval::host::MachineId::next();
    stands &= ply_host::observe::moved(&rest, &world, probe).is_none();
    Some(Answered {
        binary: binary.to_string(),
        test: test.to_string(),
        stands,
    })
}
