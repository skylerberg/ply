//! Loading a unit: its C compiled, its object bound to this runtime's helpers, and what it says
//! about itself read back into the tables its code runs against.

use super::Refused;
use super::descriptor::{Descriptor, TakenRow};
use super::exports::Exports;
use super::load::Library;
use super::tables::root_id;
use crate::heap::Heap;
use crate::rt::Entry;
use crate::rt::{Ctx, Root, Tables};
use crate::source::Source;
use anyhow::{Result, bail};
use ply_core::{Span, Symbol};
use std::collections::HashMap;
use std::ffi::c_void;
use std::sync::Arc;
use std::sync::atomic::AtomicI64;

/// A loaded C unit, with the surface `crate::backend::Bodies` asks a compiled unit for.
pub struct Native {
    /// The definitions the unit holds, read where its C lays them out. The library they lie in
    /// is this unit's own, which is all that makes the lifetime true.
    unit: &'static Descriptor,
    constants: HashMap<String, usize>,
    /// Each root the unit is entered at through its `R#entered`, with that definition's entry.
    entered: HashMap<String, Entry>,
    tables: Arc<Tables>,
    /// Kept alive because every [`Entry`], the descriptor and the unit's static data point into its
    /// pages. Last, so it is unmapped after everything here that may hold a word of it.
    _lib: Library,
}

fn entry_of(row: &TakenRow) -> Option<Entry> {
    // SAFETY: a unit defines each entry it names as a function of an entry's type.
    (row.entry()).map(|entry| unsafe { std::mem::transmute::<*mut c_void, Entry>(entry) })
}

impl Native {
    /// The definitions the unit holds, in name order.
    pub fn names(&self) -> Vec<String> {
        (self.unit.taken().iter())
            .map(|row| row.name().to_string())
            .collect()
    }

    pub fn entry(&self, name: &str) -> Option<Entry> {
        self.unit.taking(name).and_then(entry_of)
    }

    pub fn arity(&self, name: &str) -> Option<usize> {
        self.unit.taking(name).map(TakenRow::arity)
    }

    /// The entry of `R#entered` for the root `name`, which takes no words.
    pub fn entered(&self, name: &str) -> Option<Entry> {
        self.entered.get(name).copied()
    }

    /// Each root the unit is entered at through its `R#entered`: a generic one's entry calls a
    /// specialisation, so the root itself may be no body the unit holds.
    pub fn entered_names(&self) -> Vec<String> {
        self.entered.keys().cloned().collect()
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
    let Some(unit) = lib.descriptor() else {
        bail!(
            "the unit at {} lays out no descriptor for its loader",
            lib.path().display()
        );
    };
    // SAFETY: the descriptor lies in `lib`, which the `Native` made here holds beside all that
    // reads it.
    let unit: &'static Descriptor = unsafe { &*std::ptr::from_ref(unit) };
    let exports = Exports::read(unit)?;
    let refused = refused_of(&exports);
    let native = finish(lib, unit, exports, source)?;
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

/// A loaded object, what it lays out and what it says, made into an enterable `Native`; `source`
/// places its sites.
fn finish(
    lib: Library,
    unit: &'static Descriptor,
    exports: Exports,
    source: Option<&Source>,
) -> Result<Native> {
    let helpers = exports.bound()?;
    let Exports {
        helpers: _,
        constants,
        modules: _,
        refusals: _,
        fields,
        builtins,
        entered,
    } = exports;
    let entered = entered
        .into_iter()
        .map(|root| {
            let through = format!("{root}#entered");
            let row = unit.taking(&through);
            match (row.map(TakenRow::arity), row.and_then(entry_of)) {
                (Some(0), Some(entry)) => Ok((root, entry)),
                (Some(arity), _) if arity != 0 => bail!(
                    "`{through}` takes {arity} words, where `{root}` is entered through it with none"
                ),
                _ => {
                    bail!("`{root}` is entered through `{through}`, which the unit does not take")
                }
            }
        })
        .collect::<Result<HashMap<_, _>>>()?;
    bind(unit, &helpers);
    let functions: Vec<usize> = unit.code().iter().map(|p| *p as usize).collect();
    let mut roots: Vec<Root> = (unit.taken().iter())
        .map(|row| Root {
            id: root_id(row.name()),
            name: Symbol::new(row.name()),
            span: (source.and_then(|s| s.span_of(row.name()))).unwrap_or(Span::DUMMY),
        })
        .collect();
    roots.sort_by_key(|r| r.id);
    if roots.windows(2).any(|w| w[0].id == w[1].id) {
        bail!("two roots of this unit share a site id");
    }
    let tables = Tables {
        fields,
        builtins,
        memo: functions.iter().map(|_| AtomicI64::new(0)).collect(),
        memo_costs: functions
            .iter()
            .map(|_| (AtomicI64::new(0), AtomicI64::new(0), AtomicI64::new(0)))
            .collect(),
        functions,
        immortals: std::sync::Mutex::new(Heap::persistent()),
        roots,
    };
    Ok(Native {
        _lib: lib,
        unit,
        constants: constants.into_iter().collect(),
        entered,
        tables: Arc::new(tables),
    })
}

/// `helpers` are the addresses the unit's own table binds, in its order.
fn bind(unit: &Descriptor, helpers: &[*mut c_void]) {
    // SAFETY: a unit lays its binder out as a function of this type.
    let bind: unsafe extern "C" fn(*const *mut c_void) =
        unsafe { std::mem::transmute(unit.bind()) };
    unsafe { bind(helpers.as_ptr()) };
}
