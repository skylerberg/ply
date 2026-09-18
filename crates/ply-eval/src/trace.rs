//! What an engine actually did, as against what its type said it might.

use ply_ty::{EffectAtom, Footprint};

/// Counts performs too: a footprint is a set, so it cannot tell one perform from three.
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
