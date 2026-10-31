use super::descriptor::{Descriptor, FunctionRow};
use super::exports::Exports;
use super::load::Library;
use super::tables::root_id;
use crate::heap::Heap;
use crate::rt::Entry;
use crate::rt::{Ctx, Root, Tables};
use anyhow::{Result, bail};
use ply_core::Symbol;
use std::collections::HashMap;
use std::ffi::c_void;
use std::sync::Arc;
use std::sync::atomic::AtomicI64;

pub struct Native {
    unit: &'static Descriptor,
    entered: HashMap<String, Entry>,
    tables: Arc<Tables>,
    _lib: Library,
}

fn entry_of(row: &FunctionRow) -> Option<Entry> {
    (row.entry()).map(|entry| unsafe { std::mem::transmute::<*mut c_void, Entry>(entry) })
}

impl Native {
    pub fn names(&self) -> Vec<String> {
        (self.unit.taken().iter())
            .map(|row| row.name().to_string())
            .collect()
    }

    pub fn entry(&self, name: &str) -> Option<Entry> {
        self.unit.taking(name).and_then(entry_of)
    }

    pub fn arity(&self, name: &str) -> Option<usize> {
        self.unit.taking(name).map(FunctionRow::arity)
    }

    pub fn entered(&self, name: &str) -> Option<Entry> {
        self.entered.get(name).copied()
    }

    pub fn entry_point_names(&self) -> Vec<String> {
        self.entered.keys().cloned().collect()
    }

    pub fn tables(&self) -> &Arc<Tables> {
        &self.tables
    }

    pub fn context(&self) -> Ctx {
        Ctx::new(self.tables.clone())
    }
}

pub fn load_from_path(path: &std::path::Path) -> Result<Native> {
    finish_unit(Library::open(path)?)
}

pub fn load_linked() -> Result<Native> {
    let Some(lib) = Library::linked() else {
        bail!("this executable was linked with no program");
    };
    finish_unit(lib)
}

fn finish_unit(lib: Library) -> Result<Native> {
    let Some(unit) = lib.descriptor() else {
        bail!(
            "the unit at {} lays out no descriptor for its loader",
            lib.path().display()
        );
    };
    let unit: &'static Descriptor = unsafe { &*std::ptr::from_ref(unit) };
    let exports = Exports::read(unit)?;
    finish(lib, unit, exports)
}

#[allow(clippy::arc_with_non_send_sync)]
fn finish(lib: Library, unit: &'static Descriptor, exports: Exports) -> Result<Native> {
    let helpers = exports.bound()?;
    let Exports {
        helpers: _,
        constants: _,
        modules: _,
        fields,
        entered,
    } = exports;
    let entered = entered
        .into_iter()
        .map(|root| {
            let through = format!("{root}#entered");
            let row = unit.taking(&through);
            match (row.map(FunctionRow::arity), row.and_then(entry_of)) {
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
    let mut roots: Vec<(u64, Symbol)> = (unit.taken().iter())
        .map(|row| (root_id(row.name()), Symbol::new(row.name())))
        .collect();
    roots.sort_by_key(|(id, _)| *id);
    if roots.windows(2).any(|w| w[0].0 == w[1].0) {
        bail!("two roots of this unit share a site id");
    }
    let roots = (roots.into_iter().enumerate())
        .map(|(at, (id, name))| Root {
            id,
            name,
            span: crate::entry::site_span(at),
        })
        .collect();
    let tables = Tables {
        fields,
        memo: functions.iter().map(|_| AtomicI64::new(0)).collect(),
        functions,
        immortals: std::sync::Mutex::new(Heap::persistent()),
        roots,
    };
    Ok(Native {
        _lib: lib,
        unit,
        entered,
        tables: Arc::new(tables),
    })
}

fn bind(unit: &Descriptor, helpers: &[*mut c_void]) {
    let bind: unsafe extern "C" fn(*const *mut c_void) =
        unsafe { std::mem::transmute(unit.bind()) };
    unsafe { bind(helpers.as_ptr()) };
}
