//! `ply-pack BINARY` appends the pack of the checkout around the working directory to BINARY, in
//! place of any it carries. `ply-pack --check BINARY` exits 0 when BINARY carries exactly that
//! pack, 1 when it carries another, and 2 when it carries none or the question cannot be answered.
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
        return answered(Path::new(dir));
    }
    let (check, binary) = match args.as_slice() {
        [binary] if !binary.starts_with('-') => (false, binary),
        [flag, binary] if flag == "--check" => (true, binary),
        _ => {
            eprintln!("usage: ply-pack [--check] BINARY | ply-pack --answered DIR");
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
    if !check {
        return match ply_pack::append(binary, &pack) {
            Ok(()) => ExitCode::SUCCESS,
            Err(why) => refused(&why),
        };
    }
    match ply_pack::check(binary, &pack) {
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

fn answered(dir: &Path) -> ExitCode {
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
