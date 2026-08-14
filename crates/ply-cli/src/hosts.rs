//! The trusted computing base, as the CLI reads and reports it.
//!
//! Three things live here, and they are one module because they are one claim:
//! *what this binary can do outside the program*. The registry seam, the
//! binding a run gets, and the reporting that keeps every count downstream
//! honest about it.
//!
//! Two rules from ADR 0011 decide every signature below:
//!
//! - **Hermetic is the default and the flag is the only way out.** No
//!   environment variable, no config file: a reviewer reads `--host` in the
//!   command or the run reached nothing.
//! - **A host-backed test is not world-isolated.** A socket cannot be forked, so
//!   `--explain` says `host` and the trivially-parallel count excludes it,
//!   rather than the count quietly over-claiming.

use crate::commands::common::plural;
use ply_core::CheckOutput;
use ply_core::ty::Footprint;
use ply_eval::host::{HostBinding, HostListing, HostRegistry, HostRow, HostRuntime};
use ply_span::Diagnostic;
use serde_json::{Value, json};
use std::rc::Rc;
use std::sync::Arc;

/// The trusted computing base this binary was built with.
///
/// One function and one call site, so the TCB is a list read top to bottom
/// rather than something assembled by link-time magic. `ply_host::Host` is what
/// fills it, and it is the *only* thing that does: no command line, environment
/// variable or file adds a member.
pub fn registry() -> HostRegistry {
    ply_host::registry()
}

/// What a run has bound, and what it *could* have bound.
///
/// The two are separate because `ply hosts` has to tell "hermetic" from "the
/// registry failed to load", and an empty listing cannot.
pub struct Hosts {
    /// The facilities the bound handlers act on, and the source of every
    /// [`HostRuntime`] this run hands to a machine. `None` when nothing real is
    /// behind the binding — a hermetic run, or a test fixture — in which case a
    /// machine gets no runtime and only value-shaped answers are possible.
    ///
    /// One `Host` per run, shared: a registry built over one and a runtime built
    /// over another would mint tokens into a table nothing polls, which is a
    /// hang rather than a failure.
    host: Option<Arc<ply_host::Host>>,
    /// Shared rather than owned because the machine takes it by `Arc`: one
    /// binding serves the whole run, and a run with two would have two answers
    /// to what it can do.
    binding: Arc<HostBinding>,
    /// Every triple the registry resolves against this program, whether or not
    /// it is bound. `--host` decides whether the TCB is *used*, never whether it
    /// exists.
    listing: HostListing,
}

impl Hosts {
    /// The binding a run gets. Resolution — and therefore E0421/E0422/E0423 —
    /// happens only when something is actually being bound: a stale
    /// registration is the host author's bug, and refusing to run a program's
    /// hermetic tests over it would make the hermetic path the fragile one.
    pub fn open(check: &CheckOutput, host: bool) -> Result<Hosts, Vec<Diagnostic>> {
        let facilities = Arc::new(ply_host::Host::new());
        let registry = facilities.registry();
        if !host {
            return Ok(Hosts {
                host: None,
                binding: Arc::new(HostBinding::hermetic_with(registry)),
                listing: HostListing::default(),
            });
        }
        let binding = registry.bind(check)?;
        let listing = binding.listing().clone();
        Ok(Hosts {
            host: Some(facilities),
            binding: Arc::new(binding),
            listing,
        })
    }

    /// [`open`] against an explicit registry, for a test that needs to control
    /// what is registered.
    ///
    /// Nothing behind it can wait: a run bound this way has no [`HostRuntime`],
    /// so a handler that answers `Pending` is a diagnostic rather than a hang.
    ///
    /// [`open`]: Hosts::open
    #[cfg(test)]
    pub fn bind(
        registry: HostRegistry,
        check: &CheckOutput,
        host: bool,
    ) -> Result<Hosts, Vec<Diagnostic>> {
        if !host {
            return Ok(Hosts {
                host: None,
                binding: Arc::new(HostBinding::hermetic_with(registry)),
                listing: HostListing::default(),
            });
        }
        let binding = registry.bind(check)?;
        let listing = binding.listing().clone();
        Ok(Hosts {
            host: None,
            binding: Arc::new(binding),
            listing,
        })
    }

    /// Everything the registry resolves to, bound or not: what `ply hosts`
    /// prints and what CI pins a digest of.
    pub fn preview(check: &CheckOutput) -> Result<HostListing, Vec<Diagnostic>> {
        registry().preview(check)
    }

    /// A reactor for one machine, on the thread that will drive it.
    ///
    /// Called per machine rather than shared, because a `Machine` holds it by
    /// `Rc` and never crosses a thread. The facilities behind it are `Arc` and
    /// own the real threads; no Ply value goes near them.
    pub fn runtime(&self) -> Option<Rc<dyn HostRuntime>> {
        self.host.as_ref().map(|host| host.runtime())
    }

    /// The same thing, as something a worker thread can call for itself.
    ///
    /// `Rc<dyn HostRuntime>` cannot cross a thread, so the test runner is handed
    /// a way to make one rather than one that was made — the same shape
    /// `InterpExecutor::with_fixture` uses, and for the same reason.
    pub fn runtime_factory(&self) -> Option<impl Fn() -> Rc<dyn HostRuntime> + Sync + use<>> {
        self.host
            .as_ref()
            .map(Arc::clone)
            .map(|host| move || host.runtime())
    }

    /// What the machine is given. A hermetic one is still a binding: it carries
    /// the registry, which is how `E0424` names the handler that would have
    /// served the operation instead of saying only that nothing did.
    pub fn binding(&self) -> Arc<HostBinding> {
        Arc::clone(&self.binding)
    }

    pub fn listing(&self) -> &HostListing {
        &self.listing
    }

    pub fn is_hermetic(&self) -> bool {
        self.binding.is_hermetic()
    }

    /// `"hermetic"` or `"host"`. The one word a `--json` consumer branches on,
    /// and the same word the human summary prints.
    pub fn label(&self) -> &'static str {
        if self.is_hermetic() {
            "hermetic"
        } else {
            "host"
        }
    }

    /// Whether this footprint can reach a bound host handler.
    ///
    /// A footprint is an upper bound on what is performed, so this
    /// over-approximates in the safe direction: it may name a test that would
    /// not have reached the boundary, and can never miss one that would. False
    /// for every footprint in a hermetic run.
    pub fn reaches(&self, footprint: &Footprint) -> bool {
        self.binding.reaches(footprint)
    }

    /// What a run publishes about its binding. The digest is here rather than
    /// only in `ply hosts` so that a run's artifact says which trusted computing
    /// base produced it.
    pub fn summary_json(&self) -> Value {
        json!({
            "handlers": self.listing.handlers,
            "operations": self.listing.rows.len(),
            "digest": self.listing.digest_short(),
        })
    }
}

/// What the test runner is told it may reach.
///
/// The binding goes in **whether or not `--host` was passed**, because a
/// hermetic binding is not an absent one: it carries the registry, which is what
/// lets a perform that reaches the boundary be `E0424` naming the handler that
/// would have served it, rather than `E0303`, which means inference should have
/// prevented the perform and did not.
pub fn hosting<'a, F>(hosts: &Hosts, runtime: &'a Option<F>) -> ply_test::Hosting<'a>
where
    F: Fn() -> Rc<dyn HostRuntime> + Sync,
{
    let hosting = ply_test::Hosting::hermetic().with_binding(hosts.binding());
    match runtime {
        Some(factory) => hosting.with_runtime(factory),
        None => hosting,
    }
}

/// How the corpus splits once the binding is taken into account.
///
/// `Parallelism` is computed from footprints alone, so under a real binding it
/// counts a host-backed test as trivially parallel. It is not: a socket cannot
/// be forked, world isolation does not apply, and footprint conflict grouping is
/// the only isolation such a test has. Correcting the count here is the
/// difference between an honest `isolated: n of m` and one that silently
/// over-claims — which is the exact failure mode this milestone is built to
/// prevent.
///
/// In a hermetic run `host` is zero and the other two are `Parallelism`'s own
/// numbers, unchanged.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Counts {
    pub total: usize,
    pub isolated: usize,
    pub shared: usize,
    pub host: usize,
}

impl Counts {
    /// `tests` is every test the run reports on, paired with whether the raw
    /// classification called it world-isolated.
    pub fn of<'a>(hosts: &Hosts, tests: impl IntoIterator<Item = (&'a Footprint, bool)>) -> Counts {
        let mut counts = Counts::default();
        for (footprint, world) in tests {
            counts.total += 1;
            if hosts.reaches(footprint) {
                counts.host += 1;
            } else if world {
                counts.isolated += 1;
            } else {
                counts.shared += 1;
            }
        }
        counts
    }
}

// --- `ply hosts` ------------------------------------------------------------

/// The row key and the atom, both, because the operation says *what* was bound
/// and the atom is what scheduling and isolation speak in. Deriving one from the
/// other means reading a mode annotation in another file, which is not work a
/// reviewer should do to answer "what can this program touch".
const HEADERS: [&str; 6] = ["OPERATION", "ATOM", "HANDLER", "DET", "LINEAR", "BLOCKING"];

fn cells(row: &HostRow) -> [String; 6] {
    [
        row.to_string(),
        row.atom.to_string(),
        row.path.to_string(),
        yes_no(row.deterministic),
        row.linearity.as_str().to_string(),
        yes_no(row.blocking),
    ]
}

fn yes_no(flag: bool) -> String {
    if flag { "yes" } else { "no" }.to_string()
}

/// Every line of `ply hosts --host`, without the indent, so the shape is
/// testable without a terminal.
pub fn listing_lines(listing: &HostListing) -> Vec<String> {
    let mut lines = vec![format!(
        "{} {} · {} {} · trusted computing base",
        listing.handlers,
        plural(listing.handlers, "host handler"),
        listing.rows.len(),
        plural(listing.rows.len(), "operation"),
    )];
    lines.push(String::new());

    if listing.rows.is_empty() {
        lines.push(empty_note(listing));
    } else {
        let rows: Vec<[String; 6]> = listing.rows.iter().map(cells).collect();
        // Widths from the content rather than from a guess, so a long Rust path
        // does not push the flag columns out of alignment. Every row is present
        // in every run, so the widths are as deterministic as the rows.
        let mut widths = HEADERS.map(str::len);
        for row in &rows {
            for (width, cell) in widths.iter_mut().zip(row) {
                *width = (*width).max(cell.chars().count());
            }
        }
        let line = |cells: &[String; 6]| {
            let mut out = String::new();
            for (i, (cell, width)) in cells.iter().zip(widths).enumerate() {
                if i + 1 == cells.len() {
                    out.push_str(cell);
                } else {
                    out.push_str(&format!("{cell:<width$}  "));
                }
            }
            out
        };
        lines.push(line(&HEADERS.map(str::to_string)));
        lines.extend(rows.iter().map(line));
    }

    lines.push(String::new());
    lines.push(format!("digest: {}", listing.digest_short()));
    lines
}

/// A bound listing with no rows is three different situations, and a reader who
/// cannot tell them apart will debug the wrong one.
fn empty_note(listing: &HostListing) -> String {
    if listing.handlers == 0 {
        "no host handler is compiled into this binary".to_string()
    } else {
        format!(
            "{} {} registered, and none serves an atom this program performs",
            listing.handlers,
            plural(listing.handlers, "handler")
        )
    }
}

/// What `ply hosts` says without `--host`.
///
/// Hermetic is a statement rather than an empty listing: an empty listing is
/// indistinguishable from a registry that failed to load, and those call for
/// opposite responses.
pub fn hermetic_lines(listing: &HostListing) -> Vec<String> {
    let mut lines = vec![
        "hermetic — no host handler is bound".to_string(),
        String::new(),
    ];
    lines.push(if listing.rows.is_empty() {
        empty_note(listing)
    } else {
        format!(
            "{} {} would bind under `--host`; run `ply hosts --host` to list them",
            listing.rows.len(),
            plural(listing.rows.len(), "operation"),
        )
    });
    lines
}

pub fn row_json(row: &HostRow) -> Value {
    json!({
        "effect": row.effect.as_str(),
        "operation": row.op.as_str(),
        // Null for an operation declared without `[r]`: that is one singleton
        // resource, not a resource named "singleton".
        "resource": match &row.resource {
            ply_core::ty::Resource::Named(name) => json!(name.as_str()),
            ply_core::ty::Resource::Singleton => Value::Null,
        },
        "triple": row.to_string(),
        "atom": row.atom.to_string(),
        "handler": row.path,
        "deterministic": row.deterministic,
        "linearity": row.linearity.as_json(),
        "blocking": row.blocking,
        // The other half of the pair E0423 checks. A reviewer who sees only
        // `deterministic` cannot tell a handler that is honestly deterministic
        // from one serving an effect nobody marked `nondet`.
        "declared_nondet": row.declared_nondet,
    })
}

pub fn rows_json(listing: &HostListing) -> Value {
    Value::Array(listing.rows.iter().map(row_json).collect())
}

/// A registry for this crate's tests.
///
/// No handler here answers: what is under test at a CLI seam is what a listing
/// and a binding *say*, never what a socket returns, and a fixture that could
/// answer would let a reporting test pass for the wrong reason.
#[cfg(test)]
pub(crate) mod fixture {
    use super::*;
    use ply_core::ty::Resource;
    use ply_eval::host::{
        Determinism, HostAnswer, HostHandler, HostOp, HostRequest, HostResource, HostRuntime,
        Linearity,
    };
    use ply_span::{Symbol, codes};
    use std::sync::Arc;

    struct Never;

    impl HostHandler for Never {
        fn call(
            &self,
            _: &dyn HostRuntime,
            req: &HostRequest<'_>,
        ) -> Result<HostAnswer, Diagnostic> {
            Err(
                Diagnostic::error(codes::INTERNAL_ERROR, "a reporting test called a handler")
                    .primary(req.span, "here"),
            )
        }
    }

    pub(crate) fn op(
        effect: &str,
        name: &str,
        resource: HostResource,
        linearity: Linearity,
        blocking: bool,
        path: &'static str,
    ) -> HostOp {
        HostOp {
            effect: Symbol::new(effect),
            op: Symbol::new(name),
            resource,
            determinism: Determinism::Nondeterministic,
            linearity,
            blocking,
            path,
        }
    }

    pub(crate) fn named(label: &str) -> HostResource {
        HostResource::Only(Resource::Named(Symbol::new(label)))
    }

    /// The same registration, declared deterministic, which binds against an
    /// effect the program did not mark `nondet`. A nondeterministic one there is
    /// `E0423`, and a fixture that has to declare `nondet` to be bound at all
    /// would test the declaration rather than the reporting.
    pub(crate) fn deterministic(mut op: HostOp) -> HostOp {
        op.determinism = Determinism::Deterministic;
        op
    }

    pub(crate) fn registry(ops: Vec<HostOp>) -> HostRegistry {
        let mut registry = HostRegistry::new();
        for op in ops {
            registry.register(op, Arc::new(Never));
        }
        registry
    }
}

#[cfg(test)]
mod tests {
    use super::fixture::{op, registry};
    use super::*;
    use ply_core::ty::Resource;
    use ply_eval::host::{HostResource, Linearity};
    use ply_span::{SourceId, Symbol};

    const DB: &str = r#"
nondet effect db {
  read  get[r](key: Int) -> Int
  write put[r](key: Int, value: Int) -> Int
}

fn lookup(k: Int) -> Int / {db.read[users]} = db.get[users](k)

fn other(k: Int) -> Int / {db.read[orders]} = db.get[orders](k)

fn store(k: Int) -> Int / {db.write[orders]} = db.put[orders](k, 1)

fn stamp() -> Int / {clock.read} = clock.now()
"#;

    fn check(source: &str) -> CheckOutput {
        let module = ply_syntax::parse(SourceId(0), source).expect("the fixture parses");
        ply_core::check_module(&module).expect("the fixture typechecks")
    }

    fn full() -> HostRegistry {
        registry(vec![
            op(
                "clock",
                "now",
                HostResource::Only(Resource::Singleton),
                Linearity::Repeatable,
                false,
                "ply_host::clock::now",
            ),
            op(
                "db",
                "get",
                HostResource::Any,
                Linearity::AtMostOnce,
                true,
                "ply_host::postgres::read",
            ),
            op(
                "db",
                "put",
                HostResource::Only(Resource::Named(Symbol::new("orders"))),
                Linearity::AtMostOnce,
                true,
                "ply_host::postgres::write",
            ),
        ])
    }

    fn listing() -> HostListing {
        full().preview(&check(DB)).expect("the fixture binds")
    }

    /// One line per resolved triple, ascending, and an `Any` handler's resources
    /// spelled out rather than hidden behind a `*`.
    #[test]
    fn the_listing_names_every_resource_an_any_handler_got() {
        let listing = listing();
        let triples: Vec<String> = listing.rows.iter().map(|r| r.to_string()).collect();
        assert_eq!(
            triples,
            [
                "clock.now",
                "db.get[orders]",
                "db.get[users]",
                "db.put[orders]"
            ]
        );
        assert_eq!(listing.handlers, 3);
        let text = listing_lines(&listing).join("\n");
        assert!(!text.contains('*'), "a resource was hidden:\n{text}");
    }

    /// The listing whole, rather than a claim per column.
    ///
    /// This is the artifact a reviewer reads and CI diffs, so what is pinned is
    /// the block: a column silently dropped, reordered or renamed changes what
    /// the reader is told, and every one of those passes a per-column check.
    #[test]
    fn the_table_is_exactly_the_shape_the_contract_specifies() {
        let lines = listing_lines(&listing());
        let (rendered, digest) = lines.split_at(lines.len() - 1);
        assert_eq!(
            rendered.join("\n"),
            "\
3 host handlers · 4 operations · trusted computing base

OPERATION       ATOM              HANDLER                    DET  LINEAR        BLOCKING
clock.now       clock.read        ply_host::clock::now       no   repeatable    no
db.get[orders]  db.read[orders]   ply_host::postgres::read   no   at-most-once  yes
db.get[users]   db.read[users]    ply_host::postgres::read   no   at-most-once  yes
db.put[orders]  db.write[orders]  ply_host::postgres::write  no   at-most-once  yes
"
        );
        assert!(digest[0].starts_with("digest: b3:"), "{digest:?}");
    }

    /// The whole ambition of the listing is a one-line diff in a review, which
    /// requires two runs over one program to agree byte for byte.
    #[test]
    fn the_listing_and_its_digest_are_stable_across_runs() {
        let program = check(DB);
        let once = full().preview(&program).unwrap();
        let twice = full().preview(&program).unwrap();
        assert_eq!(listing_lines(&once), listing_lines(&twice));
        assert_eq!(once.digest_short(), twice.digest_short());
        assert_eq!(rows_json(&once), rows_json(&twice));
    }

    /// A handler that quietly became repeatable, or quietly stopped declaring
    /// itself blocking, is exactly the change worth a reviewer's attention.
    #[test]
    fn the_digest_moves_when_a_flag_alone_moves() {
        let program = check(DB);
        let base = full().preview(&program).unwrap().digest_short();

        let clock = |linearity, blocking| {
            registry(vec![op(
                "clock",
                "now",
                HostResource::Only(Resource::Singleton),
                linearity,
                blocking,
                "ply_host::clock::now",
            )])
            .preview(&program)
            .unwrap()
            .digest_short()
        };

        let one = clock(Linearity::Repeatable, false);
        let linear = clock(Linearity::AtMostOnce, false);
        let blocks = clock(Linearity::Repeatable, true);
        assert_ne!(one, base);
        assert_ne!(one, linear, "linearity alone must move the digest");
        assert_ne!(one, blocks, "blocking alone must move the digest");
    }

    #[test]
    fn hermetic_says_so_and_still_reports_what_would_bind() {
        let lines = hermetic_lines(&listing());
        assert_eq!(lines[0], "hermetic — no host handler is bound");
        assert!(lines[2].contains("4 operations would bind"), "{lines:?}");
        assert!(lines[2].contains("--host"));
    }

    /// An empty listing is indistinguishable from a registry that failed to
    /// load, so neither form is allowed to print one and stop.
    #[test]
    fn an_empty_registry_says_it_is_empty_rather_than_printing_nothing() {
        let empty = HostRegistry::new().preview(&check(DB)).unwrap();
        assert!(hermetic_lines(&empty)[2].contains("no host handler is compiled"));
        assert!(
            listing_lines(&empty)
                .iter()
                .any(|l| l.contains("no host handler is compiled"))
        );

        let idle = registry(vec![op(
            "db",
            "get",
            HostResource::Any,
            Linearity::AtMostOnce,
            true,
            "ply_host::postgres::read",
        )]);
        // A driver linked into a program that declares the effect and never
        // queries is idle, not wrong.
        let quiet =
            check("nondet effect db {\n  read get[r](key: Int) -> Int\n}\nfn f() -> Int = 1\n");
        let idle = idle.preview(&quiet).unwrap();
        assert!(idle.rows.is_empty());
        assert!(
            hermetic_lines(&idle)[2].contains("none serves an atom"),
            "{idle:?}"
        );
    }

    #[test]
    fn the_json_row_carries_the_declaration_side_of_the_determinism_pair() {
        let listing = listing();
        let rows = rows_json(&listing);
        let clock = &rows[0];
        assert_eq!(clock["triple"], "clock.now");
        assert_eq!(clock["atom"], "clock.read");
        assert_eq!(clock["resource"], Value::Null);
        assert_eq!(clock["linearity"], "repeatable");
        assert_eq!(clock["deterministic"], false);
        assert_eq!(clock["declared_nondet"], true);
        assert_eq!(rows[1]["resource"], "orders");
        assert_eq!(rows[1]["handler"], "ply_host::postgres::read");
        assert_eq!(rows[1]["blocking"], true);
    }

    /// The default is the point: nothing binds without the flag, and a hermetic
    /// binding reaches nothing whatever the registry holds.
    #[test]
    fn hermetic_is_the_default_and_reaches_nothing() {
        let program = check(DB);
        let hosts = Hosts::open(&program, false).unwrap();
        assert!(hosts.is_hermetic());
        assert_eq!(hosts.label(), "hermetic");
        assert!(hosts.listing().is_empty());
        for def in program.defs.values() {
            assert!(!hosts.reaches(&def.footprint));
        }
    }

    /// A footprint that meets the binding is host-backed and therefore not
    /// world-isolated, however isolated its atoms would otherwise make it.
    #[test]
    fn a_host_backed_test_leaves_the_trivially_parallel_count() {
        let program = check(DB);
        let hosts = Hosts::bind(full(), &program, true).unwrap();
        assert_eq!(hosts.label(), "host");

        let reads = program
            .defs
            .values()
            .find(|d| d.simple_name.as_str() == "lookup")
            .unwrap();
        let pure = Footprint::empty();
        assert!(hosts.reaches(&reads.footprint));
        assert!(!hosts.reaches(&pure));

        let counts = Counts::of(
            &hosts,
            [(&reads.footprint, true), (&pure, true), (&pure, false)],
        );
        assert_eq!(counts.total, 3);
        assert_eq!(counts.host, 1);
        assert_eq!(counts.isolated, 1);
        assert_eq!(counts.shared, 1);

        // The same corpus under a hermetic binding: the host column is empty and
        // every other number is what it was before W1.
        let hermetic = Hosts::open(&program, false).unwrap();
        let counts = Counts::of(
            &hermetic,
            [(&reads.footprint, true), (&pure, true), (&pure, false)],
        );
        assert_eq!(counts.host, 0);
        assert_eq!(counts.isolated, 2);
        assert_eq!(counts.shared, 1);
    }
}
