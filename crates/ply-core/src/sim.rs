//! Seeds, the ids of regions, tasks and channels, and what a region reads of a perform it answers.

use std::fmt;

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Default)]
pub struct Seed {
    pub root: u64,
    pub path: Vec<u16>,
}

impl Seed {
    pub fn at(root: u64, path: Vec<u16>) -> Seed {
        Seed { root, path }
    }
}

impl fmt::Display for Seed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.root)?;
        for (i, choice) in self.path.iter().enumerate() {
            f.write_str(if i == 0 { ":" } else { "." })?;
            write!(f, "{choice}")?;
        }
        Ok(())
    }
}

/// A region of one entry, numbered in the order the entry opens them.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct SimId(pub u32);

/// Never reused within a region, and wide enough that a server spawning without pause never wraps.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct TaskId(pub u64);

impl fmt::Display for TaskId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "@{}", self.0)
    }
}

/// A channel of one region, numbered as [`TaskId`] numbers tasks.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct ChanId(pub u64);

impl fmt::Display for ChanId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "#{}", self.0)
    }
}

/// Default scheduling steps per interleaving before the region is [`crate::codes::DEADLOCK`].
pub const DEFAULT_STEPS: u32 = 100_000;

/// The operations of `task`, which a region's scheduler answers.
pub const TASK_OPS: &[&str] = &[
    "spawn", "join", "yield", "cancel", "await", "channel", "send", "recv", "close", "select",
];
