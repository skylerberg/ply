//! What `config.get` and `config.secret` return from: the snapshot the program resolved before
//! anything was bound, which the run's runtime hands `std.config`'s natives.

use std::collections::BTreeMap;

/// One resolved key: its value, and whether the run's schema declared it secret.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Entry {
    pub value: String,
    pub secret: bool,
}

/// The frozen configuration of one run, as the program resolved it before anything was bound.
#[derive(Clone, Default, PartialEq, Eq, Debug)]
pub struct Snapshot {
    values: BTreeMap<String, Entry>,
    /// Whether the run named a `--config-schema`: without one no key is secret.
    has_spec: bool,
}

impl Snapshot {
    /// What a run granted no configuration holds: nothing, whatever the environment has.
    pub fn unopened() -> Snapshot {
        Snapshot::default()
    }

    pub fn new(values: BTreeMap<String, Entry>, has_spec: bool) -> Snapshot {
        Snapshot { values, has_spec }
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        let entry = self.values.get(key)?;
        (!entry.secret).then_some(entry.value.as_str())
    }

    /// The plaintext behind `config.secret`, for the one native that turns it into a `Secret`.
    pub fn plaintext(&self, key: &str) -> Option<&str> {
        let entry = self.values.get(key)?;
        (!self.has_spec || entry.secret).then_some(entry.value.as_str())
    }

    pub fn has_spec(&self) -> bool {
        self.has_spec
    }
}
