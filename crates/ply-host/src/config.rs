use std::collections::BTreeMap;

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Entry {
    pub value: String,
    pub secret: bool,
}

#[derive(Clone, Default, PartialEq, Eq, Debug)]
pub struct Snapshot {
    values: BTreeMap<String, Entry>,
    has_spec: bool,
}

impl Snapshot {
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

    pub fn plaintext(&self, key: &str) -> Option<&str> {
        let entry = self.values.get(key)?;
        (!self.has_spec || entry.secret).then_some(entry.value.as_str())
    }

    pub fn has_spec(&self) -> bool {
        self.has_spec
    }
}
