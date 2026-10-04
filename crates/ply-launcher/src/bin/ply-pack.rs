//! `ply-pack BINARY` appends the pack of the checkout around the working directory to BINARY, in
//! place of any it carries. `ply-pack --check BINARY` exits 0 when BINARY carries exactly that
//! pack, 1 when it carries another, and 2 when it carries none or the question cannot be answered.

use ply_pack::{Checked, Pack};
use std::path::Path;
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (check, binary) = match args.as_slice() {
        [binary] if !binary.starts_with('-') => (false, binary),
        [flag, binary] if flag == "--check" => (true, binary),
        _ => {
            eprintln!("usage: ply-pack [--check] BINARY");
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

fn refused(why: &str) -> ExitCode {
    eprintln!("ply-pack: {why}");
    ExitCode::from(2)
}
