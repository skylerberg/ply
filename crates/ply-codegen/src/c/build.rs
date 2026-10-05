//! Loading a unit: its C compiled, its object bound to this runtime's helpers, and what it says
//! about itself read back into the tables its code runs against.

use super::Refused;
use super::exports::{Exports, Taken};
use super::load::{Library, compile_and_load};
use super::tables::{UnitTables, root_id};
use crate::heap::{Heap, Stated, Word, mark_immortal};
use crate::rt::Entry;
use crate::rt::{Ctx, Root, Tables};
use crate::source::Source;
use anyhow::{Result, bail};
use ply_eval::{Span, Symbol};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::AtomicI64;

/// A loaded C unit, with the surface `crate::backend::Bodies` asks a compiled unit for.
pub struct Native {
    /// Kept alive because every [`Entry`] below points into its pages.
    lib: Library,
    entries: HashMap<String, (Entry, usize)>,
    constants: HashMap<String, usize>,
    tables: Arc<Tables>,
}

impl Native {
    /// The definitions the unit holds, in name order.
    pub fn names(&self) -> Vec<String> {
        let mut names: Vec<String> = self.entries.keys().cloned().collect();
        names.sort();
        names
    }

    pub fn entry(&self, name: &str) -> Option<Entry> {
        self.entries.get(name).map(|(e, _)| *e)
    }

    pub fn arity(&self, name: &str) -> Option<usize> {
        self.entries.get(name).map(|(_, a)| *a)
    }

    /// The memo index of a pure nullary root, if `name` is one.
    pub fn constant_index(&self, name: &str) -> Option<usize> {
        self.constants.get(name).copied()
    }

    pub fn tables(&self) -> &Arc<Tables> {
        &self.tables
    }

    pub fn context(&self) -> Ctx {
        Ctx::new(self.tables.clone())
    }

    pub fn source_path(&self) -> &std::path::Path {
        self.lib.path()
    }
}

/// Load a unit produced elsewhere against its own constructor table. `source` places its sites;
/// without one, as the bootstrap loads it, a failure names no place.
pub fn load_unit(
    text: &str,
    source: Option<&Source>,
    stem: &str,
) -> Result<(Native, Vec<Refused>)> {
    finish_unit(compile_and_load(text, stem)?, text, source)
}

/// A unit's object, however it was compiled, finished against what it says about itself and the
/// C it was compiled from.
pub(super) fn finish_unit(
    lib: Library,
    text: &str,
    source: Option<&Source>,
) -> Result<(Native, Vec<Refused>)> {
    let exports = Exports::read(&lib)?;
    let refused = refused_of(&exports);
    let native = finish(lib, exports, text, source)?;
    Ok((native, refused))
}

fn refused_of(exports: &Exports) -> Vec<Refused> {
    exports
        .refusals
        .iter()
        .map(|(function, construct)| Refused {
            function: function.clone(),
            construct: construct.clone(),
        })
        .collect()
}

/// A loaded object plus its `Exports`, made into an enterable `Native`; `source` places its sites.
/// Invariant: every field of `UnitTables` must be recorded in `Exports`, or the ids move.
fn finish(lib: Library, exports: Exports, text: &str, source: Option<&Source>) -> Result<Native> {
    let helpers = exports.bound(text)?;
    let Exports {
        helpers: _,
        ctors,
        taken,
        constants,
        modules: _,
        refusals: _,
        consts,
        fields,
        builtins,
        shapes,
        lambdas,
        buckets,
        instances,
    } = exports;
    bind(&lib, &helpers)?;
    filled(&lib, &buckets)?;
    let Some(unit) =
        UnitTables::from_tables(ctors.clone(), consts, fields, builtins, shapes, lambdas)
    else {
        bail!("a unit's shapes do not intern to the ids its C was emitted against");
    };
    let constants = constants_of(&constants, &taken, &unit)?;
    let mut functions = Vec::with_capacity(unit.lambdas.len());
    for symbol in &unit.lambdas {
        let Some(p) = lib.symbol(symbol) else {
            bail!("the unit the C backend built has no `{symbol}`");
        };
        functions.push(p as usize);
    }
    let mut entries = HashMap::new();
    for t in &taken {
        let Some(p) = lib.symbol(&t.entry) else {
            bail!("the unit the C backend built has no `{}`", t.entry);
        };
        entries.insert(
            t.name.clone(),
            (
                unsafe { std::mem::transmute::<*mut std::ffi::c_void, Entry>(p) },
                t.arity,
            ),
        );
    }
    let mut roots: Vec<Root> = taken
        .iter()
        .map(|t| Root {
            id: root_id(&t.name),
            name: Symbol::new(&t.name),
            span: source
                .and_then(|s| s.span_of(&t.name))
                .unwrap_or(Span::DUMMY),
        })
        .collect();
    roots.sort_by_key(|r| r.id);
    if roots.windows(2).any(|w| w[0].id == w[1].id) {
        bail!("two roots of this unit share a site id");
    }
    let mut tables = tables_of(unit, &ctors);
    let by = |function: &str| match function {
        "-" => Stated::No,
        name => entries
            .get(name)
            .map_or(Stated::Absent, |(entry, _)| Stated::By(*entry as usize)),
    };
    let mut stated = vec![(Stated::No, Stated::No); ctors.len()];
    for (ctor, key, show) in &instances {
        let Some(index) = tables.layouts.ctor_index(ctor) else {
            bail!("the unit states a `key` or a `show` of `{ctor}`, which it does not hold");
        };
        stated[index as usize] = (by(key), by(show));
    }
    tables.layouts.state(stated);
    tables.memo = functions.iter().map(|_| AtomicI64::new(0)).collect();
    tables.memo_costs = functions
        .iter()
        .map(|_| (AtomicI64::new(0), AtomicI64::new(0)))
        .collect();
    tables.functions = functions;
    tables.roots = roots;
    Ok(Native {
        lib,
        entries,
        constants,
        tables: Arc::new(tables),
    })
}

/// Each pure nullary root's memo slot: its code-table row, so the seam and `rt_constant` agree.
fn constants_of(
    constants: &[String],
    taken: &[Taken],
    unit: &UnitTables,
) -> Result<HashMap<String, usize>> {
    let entries: HashMap<&str, &str> = taken
        .iter()
        .map(|t| (t.name.as_str(), t.entry.as_str()))
        .collect();
    constants
        .iter()
        .map(|name| {
            let Some(entry) = entries.get(name.as_str()) else {
                bail!("the unit calls `{name}` a constant but does not take it");
            };
            let Some(slot) = unit.lambda(entry) else {
                bail!("the unit's code table has no `{entry}`");
            };
            Ok((name.clone(), slot))
        })
        .collect()
}

/// Each bucket's table, written before any of its C runs: the unit's position of every key the
/// bucket's C names by its own.
fn filled(lib: &Library, buckets: &[(u8, Vec<u32>)]) -> Result<()> {
    for (id, places) in buckets {
        let name = format!("ply_bk_{id:02x}");
        let Some(p) = lib.symbol(&name) else {
            bail!("the unit the C backend built has no `{name}`");
        };
        let table = p as *mut u32;
        for (j, place) in places.iter().enumerate() {
            unsafe { table.add(j).write(*place) };
        }
    }
    Ok(())
}

/// `helpers` are the addresses the unit's own table binds, in its order.
fn bind(lib: &Library, helpers: &[*mut std::ffi::c_void]) -> Result<()> {
    let Some(p) = lib.symbol("ply_bind") else {
        bail!("the unit the C backend built has no `ply_bind`");
    };
    let bind: unsafe extern "C" fn(*const *mut std::ffi::c_void) =
        unsafe { std::mem::transmute(p) };
    unsafe { bind(helpers.as_ptr()) };
    let Some(p) = lib.symbol("ply_bind_singletons") else {
        bail!("the unit the C backend built has no `ply_bind_singletons`");
    };
    let singletons: unsafe extern "C" fn(Word, Word, Word) = unsafe { std::mem::transmute(p) };
    unsafe {
        singletons(
            crate::heap::bool(true),
            crate::heap::bool(false),
            crate::heap::unit(),
        )
    };
    Ok(())
}

fn tables_of(mut unit: UnitTables, ctors: &[(Symbol, usize)]) -> Tables {
    unit.layouts.index_fields(&unit.fields);
    let mut immortals = Heap::persistent();
    let mut const_words = Vec::with_capacity(unit.consts.len());
    for v in &unit.consts {
        const_words.push(immortals.immortal(&unit.layouts, v));
    }
    let mut nullaries = Vec::with_capacity(ctors.len());
    for (index, (_, arity)) in ctors.iter().enumerate() {
        let w = if *arity == 0 {
            let w = immortals.alloc(crate::heap::KIND_CTOR, 0, 0, index as u32) as Word;
            mark_immortal(w);
            w
        } else {
            0
        };
        nullaries.push(w);
    }
    let empty_list = immortals.list_from(&[]);
    mark_immortal(empty_list);
    let empty_map = immortals.map_new();
    mark_immortal(empty_map);
    Tables {
        consts: unit.consts,
        const_words,
        layouts: unit.layouts,
        fields: unit.fields,
        builtins: unit.builtins,
        functions: Vec::new(),
        memo: Default::default(),
        memo_costs: Default::default(),
        immortals: std::sync::Mutex::new(immortals),
        bytes: std::array::from_fn(|_| AtomicI64::new(0)),
        nullaries,
        empty_list,
        empty_map,
        memo_values: Default::default(),
        memo_words: Default::default(),
        calls: Default::default(),
        roots: Vec::new(),
    }
}
