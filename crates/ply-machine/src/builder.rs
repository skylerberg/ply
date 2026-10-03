//! What `ply build` reads of the program it builds, as the program in `crates/ply-cli/ply`
//! performs it: the front end that program ran, read back as a load. Which entry is built, what
//! the artifact holds and how it is made, where it lands and what the report says are the
//! program's.

use crate::driver::{
    LoadedAnalysis, load_over_analysis, load_over_analysis_in, loaded_analysis_of,
};
use crate::hosts::LentOp;
use crate::load::{LoadError, Loaded};
use crate::payload::{diags_value, option, places_value, record};
use ply_eval::host::{HostAnswer, HostHandler, HostRequest, HostRuntime, Linearity};
use ply_eval::{Diagnostic, Value as PlyValue};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// The effect `crates/ply-cli/ply/builder.ply` declares.
const EFFECT: &str = "builder";

const HERMETIC: &str = "hermetic_builder";

const OPERATIONS: [(&str, &str); 1] = [("loaded", "ply_machine::builder::loaded")];

const HERMETIC_OPERATIONS: [(&str, &str); 1] =
    [("loaded", "ply_machine::builder::hermetic::loaded")];

/// The ops and the one handler serving them, and the hermetic half's.
pub fn lent() -> Vec<LentOp> {
    let mut ops = lent_by(false);
    ops.extend(lent_by(true));
    ops
}

fn lent_by(hermetic: bool) -> Vec<LentOp> {
    let handler: Arc<dyn HostHandler> = Arc::new(BuilderHandler { hermetic });
    let (effect, operations) = if hermetic {
        (HERMETIC, HERMETIC_OPERATIONS)
    } else {
        (EFFECT, OPERATIONS)
    };
    // A load is a function of the root and front it is handed.
    operations
        .into_iter()
        .map(|(op, path)| {
            let op = if hermetic {
                crate::hosts::hermetic_op(effect, op, Linearity::Repeatable, path)
            } else {
                crate::hosts::privileged_op(effect, op, Linearity::Repeatable, path)
            };
            (op, Arc::clone(&handler))
        })
        .collect()
}

struct BuilderHandler {
    /// Answers from what it is handed alone: it measures no binary.
    hermetic: bool,
}

impl HostHandler for BuilderHandler {
    fn call(&self, _: &dyn HostRuntime, req: &HostRequest<'_>) -> Result<HostAnswer, Diagnostic> {
        let span = req.span;
        let value = match (req.op.op.as_str(), req.args) {
            ("loaded", [path, front]) => {
                let path = PathBuf::from(path.as_str(span, "the program's root")?);
                let front = loaded_analysis_of(front, span)?;
                self.loaded(&self.load(&path, &front))
            }
            (other, _) => return Err(crate::hosts::unserved(EFFECT, other, span)),
        };
        Ok(HostAnswer::Value(value))
    }
}

// --- The program as it loaded -------------------------------------------------

impl BuilderHandler {
    /// A load is of the root and front end it is handed.
    fn load(&self, path: &Path, front: &LoadedAnalysis) -> Result<Loaded, LoadError> {
        if self.hermetic {
            load_over_analysis_in(crate::load::tidy(path), front)
        } else {
            load_over_analysis(path, front)
        }
    }

    fn loaded(&self, program: &Result<Loaded, LoadError>) -> PlyValue {
        let loaded = match program {
            Ok(loaded) => loaded,
            Err(err) => {
                return PlyValue::ctor(
                    "Err",
                    vec![record(vec![
                        ("diags", diags_value(&err.diagnostics)),
                        ("places", places_value(&err.sources)),
                    ])],
                );
            }
        };
        PlyValue::ctor(
            "Ok",
            vec![record(vec![
                ("root", PlyValue::str(loaded.root.display().to_string())),
                ("mains", crate::drive::mains_value(loaded)),
                ("modules", crate::drive::modules_value(loaded)),
                ("places", places_value(&loaded.sources)),
                ("pins", pins_value(&loaded.front.pins)),
                // The running binary's size is no part of what a hermetic build answers.
                (
                    "binary_bytes",
                    option(binary_bytes().filter(|_| !self.hermetic).map(size)),
                ),
                ("version", PlyValue::str(env!("CARGO_PKG_VERSION"))),
            ])],
        )
    }
}

// --- The pins --------------------------------------------------------------------

/// Each dependency as the front end pinned it: its name, its version, and the digest of the
/// modules it contributed. `E0131`'s judgments decide what a package is, so the pin is the front
/// end's answer rather than anything this side derives from a path.
fn pins_value(pins: &[ply_eval::Pinned]) -> PlyValue {
    PlyValue::list(
        pins.iter()
            .map(|pin| {
                record(vec![
                    ("name", PlyValue::str(&pin.name)),
                    ("version", PlyValue::str(&pin.version)),
                    ("digest", PlyValue::str(&pin.digest)),
                ])
            })
            .collect(),
    )
}

// --- Small things -------------------------------------------------------------

/// `None` when the running binary cannot be measured, which never fails a build.
fn binary_bytes() -> Option<u64> {
    std::env::current_exe()
        .and_then(std::fs::metadata)
        .map(|m| m.len())
        .ok()
}

fn size(n: u64) -> PlyValue {
    PlyValue::Int(n as i64)
}
