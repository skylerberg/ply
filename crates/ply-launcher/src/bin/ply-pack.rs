//! `ply-pack BINARY` appends the pack of the checkout around the working directory to BINARY, in
//! place of any it carries, and then what the shipped modules answer: BINARY answers them with
//! `ply std --answers` into the memo store `BINARY-answers` beside it, emptied first, and BINARY is
//! packed once more carrying it. `ply-pack --sources BINARY` appends
//! the checkout's files alone, which a binary of another checkout's sources is made with to test
//! `--check`. `ply-pack --check BINARY` exits 0 when BINARY carries exactly the checkout's files,
//! 1 when it carries others, and 2 when it carries none or the question cannot be answered.
//! `ply-pack --answered DIR` prints `<binary>\t<test>` for each test whose trace in DIR, as a
//! test binary wrote it, still stands against that checkout.

use ply_pack::{Checked, Pack};
use std::path::Path;
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let [flag, dir] = args.as_slice()
        && flag == "--answered"
    {
        return traced(Path::new(dir));
    }
    enum Mode {
        Whole,
        Sources,
        Check,
    }
    let (mode, binary) = match args.as_slice() {
        [binary] if !binary.starts_with('-') => (Mode::Whole, binary),
        [flag, binary] if flag == "--sources" => (Mode::Sources, binary),
        [flag, binary] if flag == "--check" => (Mode::Check, binary),
        _ => {
            eprintln!("usage: ply-pack [--sources | --check] BINARY | ply-pack --answered DIR");
            return ExitCode::from(2);
        }
    };
    let binary = Path::new(binary);
    let checkout = std::env::current_dir()
        .map_err(|e| format!("no working directory: {e}"))
        .and_then(|cwd| ply_pack::checkout_around(&cwd))
        .and_then(|repo| Pack::of_checkout(&repo));
    let pack = match checkout {
        Ok(pack) => pack,
        Err(why) => return refused(&why),
    };
    let packed = match mode {
        Mode::Whole => ply_pack::append(binary, &pack).and_then(|()| answered(binary, pack)),
        Mode::Sources => ply_pack::append(binary, &pack),
        Mode::Check => return checked(binary, &pack),
    };
    match packed {
        Ok(()) => ExitCode::SUCCESS,
        Err(why) => refused(&why),
    }
}

fn checked(binary: &Path, pack: &Pack) -> ExitCode {
    match ply_pack::check(binary, pack) {
        Ok(Checked::Same) => ExitCode::SUCCESS,
        Ok(Checked::Differs(path)) => {
            eprintln!(
                "ply-pack: `{}` carries another pack than this checkout's: `{path}` differs",
                binary.display()
            );
            ExitCode::from(1)
        }
        Ok(Checked::Absent) => refused(&format!("`{}` carries no pack", binary.display())),
        Err(why) => refused(&why),
    }
}

/// `binary`, carrying `pack`, packed again with what it answers of its shipped modules.
fn answered(binary: &Path, pack: Pack) -> Result<(), String> {
    let mut store = binary.as_os_str().to_owned();
    store.push("-answers");
    let store = std::path::PathBuf::from(store);
    if store.exists() {
        std::fs::remove_dir_all(&store)
            .map_err(|e| format!("`{}` could not be emptied: {e}", store.display()))?;
    }
    let status = std::process::Command::new(binary)
        .arg("std")
        .arg("--answers")
        .arg(&store)
        .status()
        .map_err(|e| format!("`{}` could not be run: {e}", binary.display()))?;
    if !status.success() {
        return Err(format!(
            "`{} std --answers` did not answer the shipped modules: {status}",
            binary.display()
        ));
    }
    ply_pack::append(binary, &pack.with_answers(&store)?)
}

fn traced(dir: &Path) -> ExitCode {
    let repo = match std::env::current_dir()
        .map_err(|e| format!("no working directory: {e}"))
        .and_then(|cwd| ply_pack::checkout_around(&cwd))
    {
        Ok(repo) => repo,
        Err(why) => return refused(&why),
    };
    ply_pack::install_checkout(&repo);
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) => return refused(&format!("`{}` could not be listed: {e}", dir.display())),
    };
    for entry in entries.flatten() {
        let Ok(text) = std::fs::read_to_string(entry.path()) else {
            continue;
        };
        if let Some(a) = ply_machine::tested::answered(&text, &repo)
            && a.stands
        {
            println!("{}\t{}", a.binary, a.test);
        }
    }
    ExitCode::SUCCESS
}

fn refused(why: &str) -> ExitCode {
    eprintln!("ply-pack: {why}");
    ExitCode::from(2)
}
