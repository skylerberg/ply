use ply_eval::{CheckOutput, DefHash, Diagnostic, HashOutput, ModuleName, Plan, SourceId};
use ply_store::Store;
use ply_test::{Choice, Reason, Selection};
use std::collections::{BTreeMap, HashMap};

/// Files a program's definitions as the CLI does before its tests run: each one's row and scheme,
/// as the compiler prints them, under its hash and name, and the file's fingerprint naming the
/// hashes it has now. `sources[i]` is `(module name, text)` for `SourceId(i)`.
///
/// `Value::Record` holds an `Arc`, and its fields are not `Send`.
#[allow(clippy::arc_with_non_send_sync)]
pub fn file_interfaces(store: &mut Store, file: &std::path::Path, sources: &[(String, String)]) {
    use ply_codegen::c::producer;
    use ply_eval::decode::At;
    use ply_eval::{Fields, Symbol, Value};
    let record = |fields: Vec<(&str, Value)>| {
        Value::Record(std::sync::Arc::new(Fields::from_unsorted(
            fields
                .into_iter()
                .map(|(k, v)| (Symbol::new(k), v))
                .collect(),
        )))
    };
    producer::ensure_default();
    let pulled = producer::front_pulling_std(sources, &[])
        .unwrap_or_else(|e| panic!("the front end answers: {e:#}"));
    let ids: Vec<SourceId> = (0..sources.len() + pulled.modules.len())
        .map(|i| SourceId(i as u32))
        .collect();
    let hashes = ply_codegen::c::dump::read(&pulled.dump, &ids)
        .unwrap_or_else(|e| panic!("{e}"))
        .hashes;
    let printed = |entry: &str, args: Vec<Value>| {
        producer::call(entry, &args).unwrap_or_else(|e| panic!("the compiler prints: {e:#}"))
    };
    let defs = At::new("the front end's answer", &pulled.dump)
        .field("defs")
        .and_then(|d| d.list())
        .unwrap_or_else(|e| panic!("{e}"));
    let mut fingerprint = ply_store::SourceFingerprint::new(ply_store::ContentHash::of(b""));
    for def in defs {
        let read = |field: &str| {
            def.field(field)
                .unwrap_or_else(|e| panic!("{e}"))
                .value()
                .clone()
        };
        let name = Symbol::new(
            def.field("name")
                .and_then(|n| n.utf8())
                .unwrap_or_else(|e| panic!("{e}")),
        );
        let Some(hash) = hashes.defs.get(&name) else {
            continue;
        };
        let atoms =
            |field: &str| printed("tycore.atoms_text", vec![read(field), Value::bytes(",")]);
        let row = record(vec![
            ("name", Value::bytes(name.as_str().as_bytes())),
            ("hash", Value::bytes(hash.0)),
            ("witness", Value::list(Vec::new())),
            ("footprint", atoms("footprint")),
            ("performed", atoms("performed")),
        ]);
        let filed = record(vec![
            ("row", row),
            (
                "scheme",
                printed("tycore.scheme_text", vec![read("scheme")]),
            ),
        ]);
        store.put_def(
            *hash,
            ply_store::Slot {
                name: name.clone(),
                value: ply_eval::codec::encode(&filed).expect("a filed interface is plain data"),
            },
        );
        fingerprint.defs.push(ply_store::DefEntry {
            name: name.clone(),
            hash: *hash,
            span: ply_store::FileSpan { start: 0, end: 0 },
            kind: ply_store::DefKind::Fn,
            members: Vec::new(),
        });
    }
    store.put_source(file, fingerprint);
}

#[track_caller]
pub fn port_front(sources: &[(String, String)], ids: &[SourceId]) -> ply_eval::Front {
    ply_codegen::c::producer::checked_front(sources, ids)
        .unwrap_or_else(|e| panic!("the fixture must typecheck: {e:#}"))
}

/// What the port raises over these modules, for a fixture meant to be refused.
#[track_caller]
pub fn port_diagnostics(sources: &[(String, String)], ids: &[SourceId]) -> Vec<Diagnostic> {
    ply_codegen::c::producer::ensure_default();
    ply_codegen::c::producer::front(sources, ids)
        .unwrap_or_else(|e| panic!("the port answers for the fixture: {e:#}"))
        .diagnostics
}

pub struct Compiled {
    /// What the tier is built over and the executor runs.
    pub port: ply_eval::Front,
    pub check: CheckOutput,
    pub hashes: HashOutput,
    /// Keyed by `m.name.to_string()`; the Ply emitter re-parses these.
    pub texts: HashMap<String, String>,
}

impl Compiled {
    /// One module named `m`.
    #[track_caller]
    pub fn new(src: &str) -> Compiled {
        Compiled::modules(&[("m", src)])
    }

    /// The module with no project root, named `""`; the module name reaches every hash.
    #[track_caller]
    pub fn anonymous(src: &str) -> Compiled {
        Compiled::modules(&[("", src)])
    }

    /// Several modules, each one's `SourceId` its position in `sources`.
    #[track_caller]
    pub fn modules(sources: &[(&str, &str)]) -> Compiled {
        let named: Vec<(String, String)> = sources
            .iter()
            .map(|(name, src)| {
                (
                    ModuleName::from_dotted(name).to_string(),
                    (*src).to_string(),
                )
            })
            .collect();
        let ids: Vec<SourceId> = (0..named.len()).map(|i| SourceId(i as u32)).collect();
        let port = port_front(&named, &ids);
        Compiled {
            check: port.check.clone(),
            hashes: port.hashes.clone(),
            port,
            texts: named.into_iter().collect(),
        }
    }

    pub fn footprints(&self) -> Vec<ply_eval::Footprint> {
        self.check
            .tests
            .iter()
            .map(|t| t.footprint.clone())
            .collect()
    }

    /// Leaks the `&'static` unit. A bare machine holds no evaluator, so every run needs this.
    pub fn tier(&self) -> &'static ply_codegen::Unit {
        ply_codegen::Unit::over_front(&self.port, self.texts.clone())
            .expect("this host has a C compiler")
    }

    /// Every test, run as one class under the default plan.
    pub fn every(&self) -> Selection {
        every(&self.check, &self.hashes, &Plan::default())
    }
}

/// A stand-in for the key a program files a seeded test's result under. The runtime reads and
/// writes under whatever it is handed, so a runner test needs only a key that moves with the plan;
/// the encoding a program uses is `suite.keys`'s, and its tests pin it.
pub fn plan_key(test: DefHash, plan: &Plan) -> DefHash {
    stand_in(&format!("{test:?} {:?}", plan.clone().normalized()))
}

/// The same for one root of a plan answered root by root.
pub fn root_key(test: DefHash, root: u64) -> DefHash {
    stand_in(&format!("{test:?} root {root}"))
}

fn stand_in(text: &str) -> DefHash {
    DefHash(*blake3::hash(text.as_bytes()).as_bytes())
}

/// A program's choice, stated rather than decided: `runs` execute as one class, each filed under
/// what `filed` names, and every other test is reported cached. Which tests a run owes, how they
/// are coloured and where a pass is filed are `suite`'s decisions, pinned by its own tests; a
/// runner test hands the runtime the answer it needs.
pub fn handed(
    check: &CheckOutput,
    plan: &Plan,
    runs: &[usize],
    narrowed: BTreeMap<usize, Vec<u64>>,
    filed: BTreeMap<usize, Vec<DefHash>>,
) -> Selection {
    let reasons = (0..check.tests.len())
        .map(|i| {
            if runs.contains(&i) {
                Reason::New
            } else {
                Reason::Cached
            }
        })
        .collect();
    let groups = if runs.is_empty() {
        Vec::new()
    } else {
        vec![runs.to_vec()]
    };
    let choice = Choice {
        runs: runs.to_vec(),
        reasons,
        narrowed,
        groups,
        every: Vec::new(),
        filed,
    };
    Selection::chosen(&choice, check, plan)
}

/// `runs` alone, each filed under its test's own hash.
pub fn choose(check: &CheckOutput, hashes: &HashOutput, runs: &[usize], plan: &Plan) -> Selection {
    let filed = runs
        .iter()
        .filter_map(|&i| Some((i, vec![*hashes.tests.get(i)?])))
        .collect();
    handed(check, plan, runs, BTreeMap::new(), filed)
}

/// Every test, as a cold cache has it.
pub fn every(check: &CheckOutput, hashes: &HashOutput, plan: &Plan) -> Selection {
    let all: Vec<usize> = (0..check.tests.len()).collect();
    choose(check, hashes, &all, plan)
}
