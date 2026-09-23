//! The declared atoms a body never touched.

use ply_ty::DefInfo;
use ply_ty::print::Printer;
use ply_ty::ty::EffectAtom;
use std::collections::BTreeSet;

/// The declared atoms covering nothing this definition's body performed.
pub fn unperformed(def: &DefInfo) -> Vec<String> {
    let undone: BTreeSet<EffectAtom> = def
        .footprint
        .atoms()
        .filter(|a| !def.performed.atoms().any(|p| a.covers(p)))
        .cloned()
        .collect();
    Printer::new().atoms(&undone)
}
