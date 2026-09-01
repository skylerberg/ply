//! What an engine actually did, as against what its type said it might.

use ply_core::ty::{EffectAtom, Footprint};

/// The count is kept because a row is a set: an engine that performed one atom three times and an
/// engine that performed it once agree on the footprint and have not done the same thing.
#[derive(Clone, Debug)]
pub struct Trace {
    footprint: Footprint,
    performs: u64,
}

impl Default for Trace {
    fn default() -> Trace {
        Trace::new()
    }
}

impl Trace {
    pub fn new() -> Trace {
        Trace {
            footprint: Footprint::empty(),
            performs: 0,
        }
    }

    pub fn clear(&mut self) {
        self.footprint.0.clear();
        self.performs = 0;
    }

    pub fn record(&mut self, atom: EffectAtom) {
        self.performs += 1;
        if !self.footprint.0.contains(&atom) {
            self.footprint.0.insert(atom);
        }
    }

    pub fn footprint(&self) -> &Footprint {
        &self.footprint
    }

    pub fn performs(&self) -> u64 {
        self.performs
    }
}
