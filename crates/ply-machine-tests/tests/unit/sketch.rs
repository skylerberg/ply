//! A sketched program loaded as a machine holds one.

use ply_eval::{Analysis, SourceMap};
pub use ply_sketch::{Sketch, atom};

/// The program loaded as a machine holds one: its modules' texts under their sources.
pub fn loaded(sketch: Sketch) -> ply_machine::load::Loaded {
    let (check, texts) = sketch.into_parts();
    let mut sources = SourceMap::new();
    let files = texts
        .iter()
        .map(|(path, text)| {
            sources.add(path, text.as_str());
            ply_machine::load::Found { path: path.into() }
        })
        .collect();
    let front = Analysis {
        check: check.clone(),
        ..Analysis::default()
    };
    ply_machine::load::Loaded {
        root: ".".into(),
        files,
        sources,
        front: std::sync::Arc::new(front),
        check,
        hashes: Default::default(),
        frontend: Default::default(),
    }
}
