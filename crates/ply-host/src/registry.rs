//! The trusted computing base, as one list.
//!
//! This is the file ADR 0008 §2 asks for: the whole set of Rust functions a Ply
//! program's effect operations may resolve to, written by hand, in an order a
//! reviewer reads top to bottom. There is no attribute macro, no link-time
//! registry and no global constructor, because the point of the list is that it
//! is short enough to read and that adding to it is a diff.
//!
//! Everything above the boundary holds *given* these declarations are honest.
//! Nothing here can check that a handler does only what it declared — §7's
//! footprint check catches one answering outside its registration, and nothing
//! catches one that opens a file behind Ply's back. `ply hosts` and review are
//! the whole defence, which is why this file exists as a file.

use crate::db::{self, Postgres};
use crate::{sched, tcp};
use ply_eval::Value;
use ply_eval::host::{HostRegistry, HostRuntime, MachineId, Pending};
use ply_span::{Diagnostic, Span, codes};
use std::rc::Rc;
use std::sync::Arc;

/// Every facility this binary can serve, built once.
///
/// The registry and the runtime come from **one** of these, and that is
/// structural rather than a convention. A registry built over one [`TcpHost`]
/// and a runtime built over another would mint tokens into a table nothing
/// polls: the run would hang rather than fail, which is the worst shape a defect
/// at this boundary can take.
///
/// Constructing one opens nothing. No socket is bound and no thread is started
/// until a handler is actually called, so `ply hosts` — which lists the trusted
/// computing base without binding it — costs an allocation.
pub struct Host {
    net: Arc<tcp::TcpHost>,
    /// The database, when the run named one. `None` is a run with no `--db`,
    /// which registers no `db` operation at all — so a program that performs one
    /// is `E0424` naming the handler that *would* have served it, and a program
    /// that performs none needs no database. That is what makes `E0431` fire
    /// exactly when the driver is in the run and the run named nothing for it to
    /// open.
    db: Option<Arc<Postgres>>,
}

impl Default for Host {
    fn default() -> Host {
        Host::new()
    }
}

impl Host {
    pub fn new() -> Host {
        Host::with_credentials(crate::tls::Credentials::empty())
    }

    /// The same facilities, holding the TLS material this run was configured
    /// with. Loading happens before this — [`Credentials::load`] is what raises
    /// `E0430`, and it does so before anything runs, because a server that
    /// discovers its certificate is unusable on the first handshake has already
    /// told a client it was listening.
    ///
    /// [`Credentials::load`]: crate::tls::Credentials::load
    pub fn with_credentials(credentials: crate::tls::Credentials) -> Host {
        Host {
            net: Arc::new(tcp::TcpHost::with_credentials(credentials)),
            db: None,
        }
    }

    /// The same facilities, plus a database.
    ///
    /// Starting the reactor is what raises `E0431` for a connection string
    /// postgres will not accept, and it happens here — before `bind`, before
    /// anything runs — because a service that discovers its database is
    /// unreachable on the first request has already told a client it was
    /// listening.
    pub fn with_database(
        credentials: crate::tls::Credentials,
        config: db::PoolConfig,
    ) -> Result<Host, Diagnostic> {
        Ok(Host {
            net: Arc::new(tcp::TcpHost::with_credentials(credentials)),
            db: Some(Arc::new(Postgres::start(config)?)),
        })
    }

    /// The database this run was configured with, for the `database` block of
    /// `ply hosts` and for a test that has to assert what a scope left open.
    pub fn database(&self) -> Option<&Arc<Postgres>> {
        self.db.as_ref()
    }

    /// What the run's `--host` summary reports about TLS: how many handshakes
    /// completed, how many were refused, and why.
    pub fn handshakes(&self) -> crate::tls::HandshakeCounts {
        self.net.handshakes()
    }

    /// The credentials this run was configured with, for the `transport` block
    /// of `ply hosts`. By name and fingerprint: nothing here can hand back key
    /// material.
    pub fn credentials(&self) -> &crate::tls::Credentials {
        self.net.credentials()
    }

    /// The trusted computing base of a run served by this `Host`.
    ///
    /// Read it top to bottom. Every line is a member, and every column `ply
    /// hosts` prints — footprint, determinism, linearity, blocking — is decided
    /// by the module the line names.
    pub fn registry(&self) -> HostRegistry {
        let mut registry = HostRegistry::new();

        // `net.*` — six operations over real sockets, one of them terminating
        // TLS through `ply_host::tls`. Nondeterministic, at most once, and
        // blocking wherever the operation waits on a peer.
        tcp::register(&mut registry, Arc::clone(&self.net) as Arc<dyn tcp::Net>);

        // `task.*` — the production scheduler. Repeatable, because spawning or
        // joining a Ply task creates and observes a machine state and changes
        // nothing outside the program.
        for (op, handler) in sched::registrations() {
            registry.register(op, handler);
        }

        // `db.*` — six operations over a real postgres, and only when this run
        // named one. Nondeterministic, at most once, and blocking: every one of
        // them dispatches to the reactor thread and answers `Pending`, so an
        // outstanding query costs a pending token and no blocking-pool thread.
        // `db.rollback` is absent: `std.db`'s `transaction` handles it in Ply
        // and it never reaches the boundary.
        //
        // Registered only when this run named a database, which is the one
        // departure from "the registry is compiled in either way". The reason is
        // `db.begin`, `db.commit` and `db.abort`: they take no table, so they
        // register `HostResource::Only(Singleton)`, and `bind` makes an `Only`
        // registration whose effect nothing declares `E0421` — correctly, since
        // such a registration asserts the program has a resource it does not.
        // Registering them into every program would therefore refuse every
        // program that does not import `std.db`, which is most of them.
        //
        // `NotConfigured` is what serves a run that named a database the
        // reactor could not be started for; the missing-`--db` case is `E0431`
        // at bind, before anything runs.
        if let Some(driver) = &self.db {
            db::register(&mut registry, Arc::clone(driver) as Arc<dyn db::Driver>);
        }

        registry
    }

    /// What a [`ply_eval::host::HostAnswer::Pending`] is polled on.
    ///
    /// A machine holds this by `Rc` because it belongs to the one thread its
    /// values live on, while the facilities behind it are `Arc` and own the real
    /// threads. No Ply value ever crosses that line.
    pub fn runtime(&self) -> Rc<dyn HostRuntime> {
        Rc::new(Facilities {
            net: Arc::clone(&self.net),
            db: self.db.clone(),
        })
    }

    /// The socket table, for a test that needs to know which port it got.
    pub fn net(&self) -> &Arc<tcp::TcpHost> {
        &self.net
    }
}

/// The listing a hermetic run retains.
///
/// `ply test` and `ply run` bind `HostBinding::hermetic_with(registry())`: the
/// registry is compiled in either way, so `E0424` can name the handler that
/// *would* have served an operation, and what `--host` adds is the binding
/// rather than the knowledge.
pub fn registry() -> HostRegistry {
    Host::new().registry()
}

/// The same, plus the `db` operations, served by an implementation that refuses.
///
/// For a *listing* over a program that declares `std.db.db`: `ply hosts` answers
/// "what does this run trust", and the postgres driver belongs in that answer
/// whether or not this invocation named a database. It is separate from
/// [`registry`] rather than folded into it because `db.begin`, `db.commit` and
/// `db.abort` take no table, so they register `HostResource::Only`, and `bind`
/// makes an `Only` registration whose effect nothing declares `E0421` —
/// correctly. A caller therefore has to have looked at the program first.
pub fn registry_with_database() -> HostRegistry {
    let mut registry = registry();
    db::register(&mut registry, Arc::new(db::postgres::NotConfigured));
    registry
}

/// The runtime, routing each token to the facility that minted it.
///
/// One facility today, and the routing is still worth writing: a token nobody
/// owns is answered with a diagnostic rather than polled forever by whichever
/// facility happened to be first. A boundary that hangs tells a reader nothing.
struct Facilities {
    net: Arc<tcp::TcpHost>,
    db: Option<Arc<Postgres>>,
}

impl HostRuntime for Facilities {
    fn poll(&self, pending: &Pending) -> Result<Option<Value>, Diagnostic> {
        if self.net.owns(pending) {
            return self.net.poll(pending);
        }
        if let Some(db) = &self.db
            && db.owns(pending)
        {
            return db.poll(pending);
        }
        Err(err_unowned(pending))
    }

    /// Waits on every facility with work outstanding. Called only with no task
    /// enabled, so a facility with nothing outstanding has nothing to contribute
    /// and parking on it would be the deadlock.
    fn park(&self) -> Result<(), Diagnostic> {
        if self.net.outstanding() > 0 {
            return self.net.park();
        }
        if let Some(db) = &self.db
            && db.reactor().outstanding() > 0
        {
            return db.reactor().park();
        }
        Err(err_nothing_outstanding())
    }

    fn block_on(&self, pending: Pending) -> Result<Value, Diagnostic> {
        if self.net.owns(&pending) {
            return self.net.block_on(pending);
        }
        if let Some(db) = &self.db
            && db.owns(&pending)
        {
            return db.block_on(pending);
        }
        Err(err_unowned(&pending))
    }

    /// Rolls back every transaction scope this entry point left open and
    /// releases or discards the connections holding them.
    ///
    /// A connection returned to the pool with a transaction still open makes the
    /// *next* request read uncommitted rows of a request that already failed,
    /// and it is invisible from either request — which is why this runs on the
    /// diagnostic and budget-exhaustion paths and not only on the value path.
    ///
    /// Every worker gets a `Facilities` over one shared `Postgres`, so `machine`
    /// is what keeps this teardown to its own entry point: without it, a test
    /// that never touched the database rolls back the transaction of one running
    /// beside it.
    fn end_entry_point(&self, machine: MachineId) -> Result<(), Diagnostic> {
        let Some(db) = &self.db else {
            return Ok(());
        };
        let report = db.end_entry_point(machine)?;
        match report.describe() {
            None => Ok(()),
            Some(why) => Err(Diagnostic::warning(
                codes::HOST_TEARDOWN,
                format!("the database driver could not hand every connection back: {why}"),
            )
            .note("the entry point's verdict is unchanged: this is the run's own state rather than the program's")
            .note("the pool refills, and a connection it closed rather than returned is one that could not be rolled back")),
        }
    }
}

#[cold]
#[inline(never)]
fn err_unowned(pending: &Pending) -> Diagnostic {
    Diagnostic::error(
        codes::INTERNAL_ERROR,
        format!("no host facility minted the pending token `{pending}`"),
    )
    .primary(Span::DUMMY, "this token belongs to no facility in this run")
    .note("a handler answered `Pending` with a token from a runtime other than the one this run is driving")
    .note("this is Ply's fault: report it with the program that produced it")
}

#[cold]
#[inline(never)]
fn err_nothing_outstanding() -> Diagnostic {
    Diagnostic::error(
        codes::INTERNAL_ERROR,
        "the run asked the host runtime to wait with nothing outstanding to wait for",
    )
    .primary(
        Span::DUMMY,
        "no task is enabled and no host operation is pending",
    )
    .note("waiting here would never return, so it is refused instead")
    .note("this is Ply's fault: report it with the program that produced it")
}

#[cfg(test)]
mod tests {
    use super::*;
    use ply_eval::host::{Determinism, Linearity};

    /// The listing is the artifact this milestone exists to produce, so a change
    /// to it must be a change someone made on purpose. This test is not a
    /// golden-file check — it asserts the properties a reviewer relies on, which
    /// is what a golden file would only imply.
    #[test]
    fn the_trusted_computing_base_declares_everything_it_must() {
        let registry = registry();
        assert!(
            !registry.is_empty(),
            "a registry that loads nothing is indistinguishable from a registry that failed to load"
        );
        for op in registry.ops() {
            assert!(
                op.path.starts_with("ply_host::"),
                "`{op}` is identified as `{}`, which names no Rust path a reviewer can find",
                op.path
            );
            assert_eq!(
                op.determinism,
                Determinism::Nondeterministic,
                "`{op}` claims to be a function of the program state; nothing in W1 is"
            );
        }
    }

    /// `Repeatable` is a claim that replaying the operation changes nothing
    /// outside the program, and it is the one column that silently re-opens
    /// multi-shot resumption over the boundary. Every use of it in the trusted
    /// computing base is enumerated here, so adding one fails this test and has
    /// to be argued for.
    #[test]
    fn every_repeatable_operation_is_one_that_was_argued_for() {
        let repeatable: Vec<String> = registry()
            .ops()
            .filter(|op| op.linearity == Linearity::Repeatable)
            .map(|op| op.to_string())
            .collect();
        assert_eq!(
            repeatable,
            ["task.spawn[..]", "task.join[..]", "task.yield[..]"]
        );
    }

    #[test]
    fn a_registry_and_a_runtime_come_from_one_host() {
        let host = Host::new();
        assert!(!host.registry().is_empty());
        let runtime = host.runtime();
        let stray = Pending {
            token: 0,
            label: "stray",
        };
        assert_eq!(
            runtime
                .poll(&stray)
                .expect_err("a token nothing minted is refused")
                .code,
            codes::INTERNAL_ERROR
        );
    }
}
