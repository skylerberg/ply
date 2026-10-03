//! What content addressing publishes: the definition hash and the table a hashed program answers.
//! The hashing itself is the front end's, in `crates/ply-compiler/ply/hash.ply`.

use crate::Symbol;
use indexmap::IndexMap;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Default)]
pub struct DefHash(pub [u8; 32]);

impl DefHash {
    pub fn to_hex(&self) -> String {
        let mut s = String::with_capacity(64);
        for b in self.0 {
            s.push(char::from_digit((b >> 4) as u32, 16).unwrap_or('0'));
            s.push(char::from_digit((b & 0xf) as u32, 16).unwrap_or('0'));
        }
        s
    }

    pub fn short(&self) -> String {
        self.to_hex()[..12].to_string()
    }

    pub fn from_hex(s: &str) -> Option<DefHash> {
        if s.len() != 64 {
            return None;
        }
        let bytes = s.as_bytes();
        let mut out = [0u8; 32];
        for (i, byte) in out.iter_mut().enumerate() {
            let hi = (bytes[2 * i] as char).to_digit(16)?;
            let lo = (bytes[2 * i + 1] as char).to_digit(16)?;
            *byte = ((hi << 4) | lo) as u8;
        }
        Some(DefHash(out))
    }

    pub fn of(bytes: &[u8]) -> DefHash {
        DefHash(*blake3::hash(bytes).as_bytes())
    }
}

impl fmt::Display for DefHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.short())
    }
}

/// Hex, so a hash can be a JSON object key.
impl Serialize for DefHash {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_hex())
    }
}

impl<'de> Deserialize<'de> for DefHash {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        DefHash::from_hex(&s)
            .ok_or_else(|| serde::de::Error::custom(format!("malformed definition hash `{s}`")))
    }
}

/// Every map is keyed by the program-wide name; a test's is `<module>.<label>`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HashOutput {
    pub defs: IndexMap<Symbol, DefHash>,
    /// `type` and `effect` declarations; `defs` holds only `fn`s, which a test can be selected on.
    pub decls: IndexMap<Symbol, DefHash>,
    pub tests: Vec<DefHash>,
    /// Direct references, definition name -> names it mentions.
    pub deps: IndexMap<Symbol, Vec<Symbol>>,
}

impl HashOutput {
    /// Every name `roots` reach through [`HashOutput::deps`], the roots included. A name declared
    /// in two namespaces is one node, as its references were merged.
    pub fn reach<'a>(&self, roots: impl IntoIterator<Item = &'a Symbol>) -> BTreeSet<Symbol> {
        let mut seen: BTreeSet<Symbol> = BTreeSet::new();
        let mut frontier: Vec<&Symbol> = roots.into_iter().collect();
        while let Some(name) = frontier.pop() {
            if seen.insert(name.clone())
                && let Some(deps) = self.deps.get(name)
            {
                frontier.extend(deps.iter().filter(|d| !seen.contains(*d)));
            }
        }
        seen
    }

    /// For each name of `among`, every name of `among` it reaches through [`HashOutput::deps`],
    /// itself included: one pass over the strongly connected components rather than a walk per
    /// name.
    pub fn reaches_among(&self, among: &BTreeSet<Symbol>) -> BTreeMap<Symbol, BTreeSet<Symbol>> {
        let names: Vec<&Symbol> = among.iter().collect();
        let index: HashMap<&Symbol, usize> =
            names.iter().enumerate().map(|(i, n)| (*n, i)).collect();
        let edges: Vec<Vec<usize>> = names
            .iter()
            .map(|name| {
                self.deps
                    .get(*name)
                    .map(|deps| deps.iter().filter_map(|d| index.get(d).copied()).collect())
                    .unwrap_or_default()
            })
            .collect();
        let (component_of, components) = components(&edges);
        let words = names.len().div_ceil(64);
        // Tarjan finishes a component after every component it reaches, so each one's successors
        // are already closed when it is.
        let mut closed: Vec<Vec<u64>> = Vec::with_capacity(components.len());
        for members in &components {
            let mut bits = vec![0u64; words];
            for &m in members {
                bits[m / 64] |= 1 << (m % 64);
                for &to in &edges[m] {
                    let c = component_of[to];
                    if c < closed.len() {
                        for (w, x) in bits.iter_mut().zip(&closed[c]) {
                            *w |= x;
                        }
                    }
                }
            }
            closed.push(bits);
        }
        names
            .iter()
            .enumerate()
            .map(|(i, name)| {
                let bits = &closed[component_of[i]];
                let reached = (0..names.len())
                    .filter(|j| bits[j / 64] & (1 << (j % 64)) != 0)
                    .map(|j| names[j].clone())
                    .collect();
                ((*name).clone(), reached)
            })
            .collect()
    }
}

/// Tarjan's strongly connected components of `edges`, iteratively: each node's component, and the
/// components in the order they finish, a component after every one it reaches.
fn components(edges: &[Vec<usize>]) -> (Vec<usize>, Vec<Vec<usize>>) {
    const UNSEEN: usize = usize::MAX;
    let n = edges.len();
    let mut order = vec![UNSEEN; n];
    let mut low = vec![0; n];
    let mut on_stack = vec![false; n];
    let mut stack: Vec<usize> = Vec::new();
    let mut component_of = vec![UNSEEN; n];
    let mut components: Vec<Vec<usize>> = Vec::new();
    let mut next = 0;
    for start in 0..n {
        if order[start] != UNSEEN {
            continue;
        }
        // (node, how many of its edges are done)
        let mut walk: Vec<(usize, usize)> = vec![(start, 0)];
        order[start] = next;
        low[start] = next;
        next += 1;
        stack.push(start);
        on_stack[start] = true;
        while let Some(&(v, done)) = walk.last() {
            if let Some(&w) = edges[v].get(done) {
                if let Some(top) = walk.last_mut() {
                    top.1 += 1;
                }
                if order[w] == UNSEEN {
                    order[w] = next;
                    low[w] = next;
                    next += 1;
                    stack.push(w);
                    on_stack[w] = true;
                    walk.push((w, 0));
                } else if on_stack[w] {
                    low[v] = low[v].min(order[w]);
                }
                continue;
            }
            walk.pop();
            if let Some(&(parent, _)) = walk.last() {
                low[parent] = low[parent].min(low[v]);
            }
            if low[v] == order[v] {
                let mut members = Vec::new();
                while let Some(w) = stack.pop() {
                    on_stack[w] = false;
                    component_of[w] = components.len();
                    members.push(w);
                    if w == v {
                        break;
                    }
                }
                components.push(members);
            }
        }
    }
    (component_of, components)
}
