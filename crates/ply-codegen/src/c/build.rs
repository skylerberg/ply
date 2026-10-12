//! Loading a unit: its C compiled, its object bound to this runtime's helpers, and what it says
//! about itself read back into the tables its code runs against.

use super::Refused;
use super::descriptor::Descriptor;
use super::exports::{Exports, Taken};
use super::load::Library;
use super::tables::{UnitTables, root_id};
use crate::heap::{Heap, Stated, Word, mark_immortal};
use crate::rt::Entry;
use crate::rt::{Ctx, Root, Tables};
use crate::source::Source;
use anyhow::{Result, bail};
use ply_eval::{Span, Symbol};
use std::collections::HashMap;
use std::ffi::c_void;
use std::sync::Arc;
use std::sync::atomic::AtomicI64;

/// A loaded C unit, with the surface `crate::backend::Bodies` asks a compiled unit for.
pub struct Native {
    /// Kept alive because every [`Entry`] below points into its pages.
    _lib: Library,
    entries: Entries,
    constants: HashMap<String, usize>,
    tables: Arc<Tables>,
}

/// The definitions a unit holds, each with the entry the runtime calls it through and how many
/// words it takes.
enum Entries {
    /// As the unit's C lays them out, read where they lie. The library they lie in is the
    /// [`Native`]'s own, which is all that makes the lifetime true.
    Laid(&'static Descriptor),
    /// Each looked up by name, in a unit that lays none out.
    Named(HashMap<String, (Entry, usize)>),
}

impl Entries {
    fn get(&self, name: &str) -> Option<(Entry, usize)> {
        match self {
            Entries::Laid(unit) => {
                let row = unit.taking(name)?;
                Some((entry_at(row.entry()), row.arity()))
            }
            Entries::Named(held) => held.get(name).copied(),
        }
    }

    fn names(&self) -> Box<dyn Iterator<Item = &str> + '_> {
        match self {
            Entries::Laid(unit) => Box::new(unit.taken().iter().map(|row| row.name())),
            Entries::Named(held) => Box::new(held.keys().map(String::as_str)),
        }
    }
}

fn entry_at(address: *mut c_void) -> Entry {
    // SAFETY: a unit defines each entry it names as a function of an entry's type.
    unsafe { std::mem::transmute::<*mut c_void, Entry>(address) }
}

impl Native {
    /// The definitions the unit holds, in name order.
    pub fn names(&self) -> Vec<String> {
        let mut names: Vec<String> = self.entries.names().map(str::to_string).collect();
        names.sort();
        names
    }

    pub fn entry(&self, name: &str) -> Option<Entry> {
        self.entries.get(name).map(|(e, _)| e)
    }

    pub fn arity(&self, name: &str) -> Option<usize> {
        self.entries.get(name).map(|(_, a)| a)
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
}

/// The unit compiled into the library at `path`, finished against its own constructor table.
/// `source` places its sites; without one, a failure names no place.
pub fn load_opened(
    path: &std::path::Path,
    source: Option<&Source>,
) -> Result<(Native, Vec<Refused>)> {
    finish_unit(Library::open(path)?, source)
}

/// The program this executable was linked with, finished as a loaded unit is.
pub fn load_linked(source: Option<&Source>) -> Result<(Native, Vec<Refused>)> {
    let Some(lib) = Library::linked() else {
        bail!("this executable was linked with no program");
    };
    finish_unit(lib, source)
}

/// A unit's object, however it was compiled or linked, finished against what it says about itself.
fn finish_unit(lib: Library, source: Option<&Source>) -> Result<(Native, Vec<Refused>)> {
    let exports = Exports::read(&lib)?;
    let refused = refused_of(&exports);
    let native = finish(lib, exports, source)?;
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
fn finish(lib: Library, exports: Exports, source: Option<&Source>) -> Result<Native> {
    let helpers = exports.bound()?;
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
        numerics,
        layout,
    } = exports;
    // SAFETY: the descriptor lies in `lib`, which the `Native` made here holds beside all that
    // reads it.
    let laid: Option<&'static Descriptor> = lib
        .descriptor()
        .map(|unit| unsafe { &*std::ptr::from_ref(unit) });
    bind(&lib, laid, &helpers)?;
    filled(&lib, &buckets)?;
    let Some(unit) =
        UnitTables::from_tables(ctors, consts, fields, builtins, shapes, lambdas, layout)
    else {
        bail!("a unit's shapes do not intern to the ids its C was emitted against");
    };
    let constants = constants_of(constants, &taken, &unit)?;
    let (functions, entries) = match laid {
        Some(laid) => (
            laid.code().iter().map(|p| *p as usize).collect(),
            Entries::Laid(laid),
        ),
        None => named(&lib, &unit, &taken)?,
    };
    let mut roots: Vec<Root> = entries
        .names()
        .map(|name| Root {
            id: root_id(name),
            name: Symbol::new(name),
            span: source.and_then(|s| s.span_of(name)).unwrap_or(Span::DUMMY),
        })
        .collect();
    roots.sort_by_key(|r| r.id);
    if roots.windows(2).any(|w| w[0].id == w[1].id) {
        bail!("two roots of this unit share a site id");
    }
    let mut tables = tables_of(unit);
    let taking = |function: &str, takes: usize| match (function, entries.get(function)) {
        ("-", _) => Ok(Stated::No),
        (_, None) => Ok(Stated::Absent),
        (_, Some((entry, words))) if words == takes => Ok(Stated::By(entry as usize)),
        (name, Some((_, words))) => bail!(
            "`{name}` is stated of a type and takes {words} words, where it is called with {takes}"
        ),
    };
    let by = |function: &str| taking(function, 1);
    let mut stated = vec![(Stated::No, Stated::No); tables.layouts.ctors.len()];
    for (ctor, key, show) in &instances {
        let Some(index) = tables.layouts.ctor_index(ctor) else {
            bail!("the unit states a `key` or a `show` of `{ctor}`, which it does not hold");
        };
        stated[index as usize] = (by(key)?, by(show)?);
    }
    tables.layouts.state(stated);
    let mut stated = vec![[Stated::No; 5]; tables.layouts.ctors.len()];
    for (ctor, functions) in &numerics {
        let Some(index) = tables.layouts.ctor_index(ctor) else {
            bail!("the unit states a `numeric` of `{ctor}`, which it does not hold");
        };
        for (role, function) in functions.iter().enumerate() {
            let takes = ply_eval::instances::NUMERIC_WORDS[role];
            stated[index as usize][role] = taking(function, takes)?;
        }
    }
    tables.layouts.state_numeric(stated);
    tables.memo = functions.iter().map(|_| AtomicI64::new(0)).collect();
    tables.memo_costs = functions
        .iter()
        .map(|_| (AtomicI64::new(0), AtomicI64::new(0), AtomicI64::new(0)))
        .collect();
    tables.functions = functions;
    tables.roots = roots;
    Ok(Native {
        _lib: lib,
        entries,
        constants,
        tables: Arc::new(tables),
    })
}

/// Each pure nullary root's memo slot: its code-table row, so the seam and `rt_constant` agree. A
/// unit that names its code table's rows says no slot, and each is found by its entry's name.
fn constants_of(
    constants: Vec<(String, Option<usize>)>,
    taken: &[Taken],
    unit: &UnitTables,
) -> Result<HashMap<String, usize>> {
    let entries: HashMap<&str, &str> = taken
        .iter()
        .map(|t| (t.name.as_str(), t.entry.as_str()))
        .collect();
    constants
        .into_iter()
        .map(|(name, slot)| {
            if let Some(slot) = slot {
                return Ok((name, slot));
            }
            let Some(entry) = entries.get(name.as_str()) else {
                bail!("the unit calls `{name}` a constant but does not take it");
            };
            let Some(slot) = unit.lambda(entry) else {
                bail!("the unit's code table has no `{entry}`");
            };
            Ok((name, slot))
        })
        .collect()
}

/// The code table and the entries of a unit that names them, each looked up in `lib`.
fn named(lib: &Library, unit: &UnitTables, taken: &[Taken]) -> Result<(Vec<usize>, Entries)> {
    let mut functions = Vec::with_capacity(unit.lambdas.len());
    for symbol in &unit.lambdas {
        let Some(p) = lib.symbol(symbol) else {
            bail!("the unit the C backend built has no `{symbol}`");
        };
        functions.push(p as usize);
    }
    let mut entries = HashMap::new();
    for t in taken {
        let Some(p) = lib.symbol(&t.entry) else {
            bail!("the unit the C backend built has no `{}`", t.entry);
        };
        entries.insert(t.name.clone(), (entry_at(p), t.arity));
    }
    Ok((functions, Entries::Named(entries)))
}

/// Each bucket's table, written before any of its C runs, in a unit whose C leaves them to the
/// loader: the unit's position of every key the bucket's C names by its own.
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

/// `helpers` are the addresses the unit's own table binds, in its order. The unit's two binders
/// are the ones `laid` holds, or else looked up by name.
fn bind(lib: &Library, laid: Option<&Descriptor>, helpers: &[*mut c_void]) -> Result<()> {
    let binder = |held: Option<*mut c_void>, name: &str| match held.or_else(|| lib.symbol(name)) {
        Some(p) if !p.is_null() => Ok(p),
        _ => bail!("the unit the C backend built has no `{name}`"),
    };
    let p = binder(laid.map(Descriptor::bind), "ply_bind")?;
    let bind: unsafe extern "C" fn(*const *mut c_void) = unsafe { std::mem::transmute(p) };
    unsafe { bind(helpers.as_ptr()) };
    let p = binder(laid.map(Descriptor::bind_singletons), "ply_bind_singletons")?;
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

fn tables_of(mut unit: UnitTables) -> Tables {
    unit.layouts.index_fields(&unit.fields);
    let mut immortals = Heap::persistent();
    let mut const_words = Vec::with_capacity(unit.consts.len());
    for v in &unit.consts {
        const_words.push(immortals.immortal(&unit.layouts, v));
    }
    let mut nullaries = Vec::with_capacity(unit.layouts.ctors.len());
    for (index, (_, arity)) in unit.layouts.ctors.iter().enumerate() {
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
