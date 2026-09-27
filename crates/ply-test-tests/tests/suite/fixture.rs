use ply_span::{Diagnostic, SourceId};
use ply_ty::{CheckOutput, HashOutput, ModuleName};
use std::collections::HashMap;

#[track_caller]
pub fn port_front(sources: &[(String, String)], ids: &[SourceId]) -> ply_ty::Front {
    ply_codegen::c::producer::checked_front(sources, ids)
        .unwrap_or_else(|e| panic!("the fixture must typecheck: {e:#}"))
}

/// What the port raises over these modules, for a fixture meant to be refused.
#[track_caller]
pub fn port_diagnostics(sources: &[(String, String)], ids: &[SourceId]) -> Vec<Diagnostic> {
    ply_codegen::c::producer::ensure_default();
    ply_codegen::c::producer::front(sources, ids)
        .unwrap_or_else(|e| panic!("the port answers for the fixture: {e:#}"))
        .diagnostics
}

pub struct Compiled {
    /// What the tier is built over and the executor runs.
    pub port: ply_ty::Front,
    pub check: CheckOutput,
    pub hashes: HashOutput,
    /// Keyed by `m.name.to_string()`; the Ply emitter re-parses these.
    pub texts: HashMap<String, String>,
}

impl Compiled {
    /// One module named `m`.
    #[track_caller]
    pub fn new(src: &str) -> Compiled {
        Compiled::modules(&[("m", src)])
    }

    /// The module with no project root, named `""`; the module name reaches every hash.
    #[track_caller]
    pub fn anonymous(src: &str) -> Compiled {
        Compiled::modules(&[("", src)])
    }

    /// Several modules, each one's `SourceId` its position in `sources`.
    #[track_caller]
    pub fn modules(sources: &[(&str, &str)]) -> Compiled {
        let named: Vec<(String, String)> = sources
            .iter()
            .map(|(name, src)| {
                (
                    ModuleName::from_dotted(name).to_string(),
                    (*src).to_string(),
                )
            })
            .collect();
        let ids: Vec<SourceId> = (0..named.len()).map(|i| SourceId(i as u32)).collect();
        let port = port_front(&named, &ids);
        Compiled {
            check: port.check.clone(),
            hashes: port.hashes.clone(),
            port,
            texts: named.into_iter().collect(),
        }
    }

    pub fn footprints(&self) -> Vec<ply_ty::Footprint> {
        self.check
            .tests
            .iter()
            .map(|t| t.footprint.clone())
            .collect()
    }

    /// Leaks the `&'static` unit. A bare machine holds no evaluator, so every run needs this.
    pub fn tier(&self) -> &'static ply_codegen::Unit {
        ply_codegen::Unit::over_front(&self.port, self.texts.clone())
            .expect("this host has a C compiler")
    }
}
