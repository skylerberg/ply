//! The diagnostic codes the runtime raises.

macro_rules! codes {
    ($($(#[$doc:meta])* $name:ident = $number:literal;)*) => {
        $($(#[$doc])* pub const $name: &str = $number;)*

        /// Every code above, in order: the runtime's description lists each, and `ply explain`'s
        /// registry is held to that list.
        pub const ALL: &[&str] = &[$($name),*];
    };
}

codes! {
    UNHANDLED_EFFECT = "E0303";
    /// A `Task` in a simulation's result, or a `join` after its scheduler ended.
    TASK_ESCAPES_SCOPE = "E0413";
    /// A simulation with nothing enabled and no timer to fire, or out of step budget.
    DEADLOCK = "E0414";
    /// Replaying a seed did not reproduce the recorded schedule.
    SIMULATION_DIVERGENCE = "E0415";
    /// A simulation inside a simulation, lexically or through a call.
    NESTED_SIMULATION = "E0416";
    /// An operation reached the host boundary with nothing bound to serve it, or a run that is no
    /// test's read what it cost.
    HOST_UNBOUND = "E0424";
    /// A host operation performed inside a simulation, or in a test the search re-runs.
    HOST_IN_SIMULATION = "E0425";
    /// A listener, a connection presenting one, or `--mtls` named a credential the binding does not
    /// hold.
    TLS_CREDENTIAL_UNKNOWN = "E0429";
    /// A `--tls` credential that does not load, or whose key does not match its certificate; a
    /// certificate to trust that does not load; or `--mtls` with nothing to verify a client against.
    TLS_CREDENTIAL_INVALID = "E0430";
    /// `trace.exit` naming a span not open on the performing task's stack.
    SPAN_UNBALANCED = "E0445";
    /// A scoped value read or let go after its scope released what it held.
    SCOPED_VALUE_RELEASED = "E0331";
    /// A task, channel or continuation marshalled across a runtime boundary, unchecked by any type.
    SCHEDULER_ESCAPE_AT_BOUNDARY = "E0449";
    BACKEND_UNAVAILABLE = "E0450";
    /// An `fs` operation named a resource label the run bound no root to.
    FS_ROOT_UNBOUND = "E0451";
    /// A path leaving its label's root via `..`, an absolute path, or a symlink.
    FS_PATH_ESCAPES_ROOT = "E0452";
    /// A read whose result would be larger than one value holds; `fs.read_at` reads a range.
    FS_FILE_TOO_LARGE = "E0453";
    /// A `--fs NAME=PATH` root that is missing, not a directory, or unresolvable.
    FS_ROOT_INVALID = "E0454";
    /// `process.exit` was performed: the machine unwinds and `ply run` exits with the code.
    PROCESS_EXIT = "E0455";
    /// `process.spawn` or `process.start` named a resource label the run bound no executable to.
    PROCESS_EXEC_UNBOUND = "E0456";
    /// An `--exec NAME=PATH` that is missing, is not a file, or cannot be executed.
    PROCESS_EXEC_INVALID = "E0457";
    /// A child wrote more to a stream than the host holds of one, whole or as unread lines.
    PROCESS_OUTPUT_TOO_LARGE = "E0458";
    /// A `--allow` family the program being run does not declare: nothing would reach it.
    CAPABILITY_UNDECLARED = "E0459";
    ASSERTION_FAILED = "E0501";
    /// A failure the language defines: `panic`, division by zero, overflow, a resource limit.
    RUNTIME_ERROR = "E0502";
    INTERNAL_ERROR = "E0505";
    /// The drain deadline expired with connections still in flight.
    DRAIN_INCOMPLETE = "W0608";
    /// Spans still open when their task or the entry point ended, reported when the entry point ends.
    SPAN_ABANDONED = "W0609";
    /// A run the harness stopped at its wall clock: a fact about the machine, not about the
    /// program, so nothing it did is recorded.
    RUN_TIMED_OUT = "W0612";
}
