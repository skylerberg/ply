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
    units: Vec<&'static Descriptor>,
    entered: HashMap<String, Entry>,
    tables: Arc<Tables>,
    _libraries: Vec<Library>,
}

fn entry_of(row: &FunctionRow) -> Option<Entry> {
    (row.entry()).map(|entry| unsafe { std::mem::transmute::<*mut c_void, Entry>(entry) })
}

impl Native {
    fn taking(&self, name: &str) -> Option<&FunctionRow> {
        self.units.iter().find_map(|unit| unit.taking(name))
    }

    pub fn names(&self) -> Vec<String> {
        (self.units.iter())
            .flat_map(|unit| unit.taken().iter())
            .map(|row| row.name().to_string())
            .collect()
    }

    pub fn entry(&self, name: &str) -> Option<Entry> {
        self.taking(name).and_then(entry_of)
    }

    pub fn arity(&self, name: &str) -> Option<usize> {
        self.taking(name).map(FunctionRow::arity)
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
    if path.extension().is_some_and(|e| e == SET_EXTENSION) {
        return load_set(path);
    }
    finish_units(vec![Library::open(path)?])
}

pub fn load_linked() -> Result<Native> {
    let Some(lib) = Library::linked() else {
        bail!("this executable was linked with no program");
    };
    finish_units(vec![lib])
}

const SET_EXTENSION: &str = "plyset";

fn load_set(path: &std::path::Path) -> Result<Native> {
    let listed = std::fs::read_to_string(path)?;
    let libraries = (listed.lines())
        .map(|line| Library::open(std::path::Path::new(line)))
        .collect::<Result<Vec<_>>>()?;
    finish_units(libraries)
}

fn descriptor_of(lib: &Library) -> Result<&'static Descriptor> {
    let Some(unit) = lib.descriptor() else {
        bail!(
            "the unit at {} lays out no descriptor for its loader",
            lib.path().display()
        );
    };
    Ok(unsafe { &*std::ptr::from_ref(unit) })
}

#[derive(Default)]
struct Numbers {
    fields: Vec<Symbol>,
    field_at: HashMap<String, usize>,
    tag_at: HashMap<String, usize>,
    function_at: HashMap<String, usize>,
    functions: Vec<usize>,
}

impl Numbers {
    fn field(&mut self, name: &str) -> usize {
        if let Some(at) = self.field_at.get(name) {
            return *at;
        }
        self.fields.push(Symbol::new(name));
        self.field_at
            .insert(name.to_string(), self.fields.len() - 1);
        self.fields.len() - 1
    }

    fn tag(&mut self, name: &str) -> usize {
        let next = self.tag_at.len();
        *self.tag_at.entry(name.to_string()).or_insert(next)
    }

    fn key(&mut self, key: &str) -> Option<usize> {
        let (kind, name) = key.split_at(1);
        match kind {
            "T" => Some(self.tag(name)),
            "f" => Some(self.field(name)),
            _ => self.function_at.get(name).copied(),
        }
    }
}

fn numbered(units: &[&'static Descriptor], exports: &[Exports]) -> Numbers {
    let mut numbers = Numbers::default();
    for (unit, said) in units.iter().zip(exports) {
        match unit.links() {
            Some(links) => {
                for (name, code) in links.code_names().zip(unit.code()) {
                    let at = numbers.functions.len();
                    numbers.function_at.insert(name.to_string(), at);
                    numbers.functions.push(*code as usize);
                }
            }
            None => {
                numbers.fields.extend(said.fields.iter().cloned());
                numbers
                    .functions
                    .extend(unit.code().iter().map(|p| *p as usize));
            }
        }
    }
    numbers
}

fn linked_across(units: &[&'static Descriptor], numbers: &mut Numbers) {
    let exported: HashMap<&str, *mut c_void> = (units.iter())
        .filter_map(|unit| unit.links())
        .flat_map(|links| links.exported())
        .map(|row| (row.name(), row.cell()))
        .collect();
    for links in units.iter().filter_map(|unit| unit.links()) {
        for row in links.keys() {
            if let Some(number) = numbers.key(row.name()) {
                unsafe { row.cell().cast::<u32>().write(number as u32) };
            }
        }
        for row in links.field_words() {
            let number = numbers.field(row.name());
            unsafe { row.cell().cast::<i64>().write(number as i64) };
        }
        for row in links.slots() {
            if let Some(code) = exported.get(row.name()) {
                unsafe { row.cell().cast::<*mut c_void>().write(*code) };
            }
        }
    }
}

fn entered_of(unit: &Descriptor, roots: Vec<String>) -> Result<Vec<(String, Entry)>> {
    roots
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
        .collect()
}

#[allow(clippy::arc_with_non_send_sync)]
fn finish_units(libraries: Vec<Library>) -> Result<Native> {
    let units = (libraries.iter())
        .map(descriptor_of)
        .collect::<Result<Vec<_>>>()?;
    let exports = (units.iter())
        .map(|unit| Exports::read(unit))
        .collect::<Result<Vec<_>>>()?;
    let bound = (exports.iter())
        .map(Exports::bound)
        .collect::<Result<Vec<_>, _>>()?;
    let mut numbers = numbered(&units, &exports);
    linked_across(&units, &mut numbers);
    let mut entered = HashMap::new();
    for (unit, said) in units.iter().zip(&exports) {
        entered.extend(entered_of(unit, said.entered.clone())?);
    }
    let Numbers {
        fields, functions, ..
    } = numbers;
    for (unit, helpers) in units.iter().zip(bound) {
        bind(unit, helpers, &functions);
    }
    let mut roots: Vec<(u64, Symbol)> = (units.iter())
        .flat_map(|unit| unit.taken().iter())
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
        _libraries: libraries,
        units,
        entered,
        tables: Arc::new(tables),
    })
}

fn bind(unit: &Descriptor, mut helpers: Vec<*mut c_void>, functions: &[usize]) {
    helpers.push(functions.as_ptr().cast_mut().cast());
    let bind: unsafe extern "C" fn(*const *mut c_void) =
        unsafe { std::mem::transmute(unit.bind()) };
    unsafe { bind(helpers.as_ptr()) };
}
