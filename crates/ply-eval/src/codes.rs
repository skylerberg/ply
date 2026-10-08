//! Stable diagnostic codes.

pub const UNKNOWN_NAME: &str = "E0101";
pub const UNKNOWN_OPERATION: &str = "E0104";
pub const DUPLICATE_DEFINITION: &str = "E0105";
pub const UNKNOWN_MODULE: &str = "E0106";
pub const MODULE_CYCLE: &str = "E0109";
pub const DUPLICATE_IMPORT: &str = "E0110";
pub const INVALID_MODULE_PATH: &str = "E0111";
pub const AMBIGUOUS_ENTRY_POINT: &str = "E0112";
/// A `reuse fn` with an append the cost checker cannot show reuses its list.
pub const REUSE_BROKEN: &str = "E0127";
/// Two packages granting one module prefix, or a package's files claiming another's.
pub const PREFIX_COLLISION: &str = "E0133";
/// A dependency whose path is missing, that holds no `ply.pkg`, or that was never fetched.
pub const DEPENDENCY_UNUSABLE: &str = "E0135";
pub const TYPE_MISMATCH: &str = "E0201";
pub const ARITY_MISMATCH: &str = "E0202";
pub const OCCURS_CHECK: &str = "E0203";
pub const NON_EXHAUSTIVE_MATCH: &str = "E0205";
/// No derivation for the requested deriver; reported at the field that blocks it.
pub const NOT_DERIVABLE: &str = "E0206";
/// `/` applied to `Decimal`.
pub const DECIMAL_DIVISION: &str = "E0209";
pub const EFFECT_NOT_PERMITTED: &str = "E0302";
pub const UNHANDLED_EFFECT: &str = "E0303";
pub const RESOURCE_REQUIRED: &str = "E0304";
/// A `handle` whose body performs an operation, on an atom it handles, that no clause answers.
pub const HANDLER_CLAUSE_MISSING: &str = "E0305";
pub const NONDET_IN_DET_TEST: &str = "E0412";
/// A `Task` in a `simulate` region's result, or a `join` after its region ended.
pub const TASK_ESCAPES_SCOPE: &str = "E0413";
/// A simulated region with nothing enabled and no timer to fire, or out of step budget.
pub const DEADLOCK: &str = "E0414";
/// Replaying a seed did not reproduce the recorded schedule.
pub const SIMULATION_DIVERGENCE: &str = "E0415";
/// A `simulate` region inside a `simulate` region, lexically or through a call.
pub const NESTED_SIMULATION: &str = "E0416";
/// An effectful `requires`/`ensures`/`where` guard, or a law body beyond `{sim.read}`.
pub const EFFECT_IN_SPEC: &str = "E0417";
/// A `forall` binder type with no generator, an effectful function type, or a row variable.
pub const UNQUANTIFIABLE_TYPE: &str = "E0418";
pub const OBLIGATION_REFUTED: &str = "E0419";
/// A host registration names an effect, operation or resource the program does not declare.
pub const HOST_OPERATION_UNKNOWN: &str = "E0421";
/// Two host registrations claim one atom.
pub const HOST_HANDLER_CONFLICT: &str = "E0422";
/// A host handler declared nondeterministic for an effect not declared `nondet`.
pub const HOST_DETERMINISM_MISMATCH: &str = "E0423";
/// An operation reached the host boundary with nothing bound.
pub const HERMETIC_BOUNDARY: &str = "E0424";
/// A host operation performed inside a `simulate` region, or in a test the search re-runs.
pub const HOST_IN_SIMULATION: &str = "E0425";
/// A continuation was resumed a second time across an at-most-once host operation.
pub const HOST_CONTINUATION_RESUMED: &str = "E0426";
/// A host handler answered an atom outside its entry point's declared footprint.
pub const HOST_FOOTPRINT_ESCAPE: &str = "E0427";
/// A handler declared `blocking: true` answered a value inline instead of a pending token.
pub const HOST_BLOCKING_ANSWER: &str = "E0428";
/// A listener, a connection presenting one, or `--mtls` named a credential the binding does not
/// hold.
pub const TLS_CREDENTIAL_UNKNOWN: &str = "E0429";
/// A `--tls` credential that does not load, or whose key does not match its certificate; a
/// certificate to trust that does not load; or `--mtls` with nothing to verify a client against.
pub const TLS_CREDENTIAL_INVALID: &str = "E0430";
/// A `Secret` passed to a host operation whose registration does not accept one.
pub const SECRET_TO_HOST: &str = "E0439";
/// A configuration source, or the `--config-schema` definition, that the run cannot read.
pub const CONFIG_UNAVAILABLE: &str = "E0440";
/// A deployable artifact whose contents do not verify against its own digests.
pub const ARTIFACT_INVALID: &str = "E0443";
/// An artifact built under a different frontend, runtime or body-encoding version.
pub const ARTIFACT_VERSION: &str = "E0444";
/// `trace.exit` naming a span not open on the performing task's stack.
pub const SPAN_UNBALANCED: &str = "E0445";
/// A hold read or let go after its region released what it held.
pub const HOLD_RELEASED: &str = "E0331";
/// A value branded with a region's name would outlive the region.
pub const REGION_ESCAPE: &str = "E0446";
/// A definition the program reaches that the C backend cannot compile.
pub const DEFINITION_REFUSED: &str = "E0448";
/// A cell, task or continuation crossing a runtime boundary, where no type is left to check it.
pub const REGION_ESCAPE_AT_BOUNDARY: &str = "E0449";
pub const BACKEND_UNAVAILABLE: &str = "E0450";
/// An `fs` operation named a resource label the run bound no root to.
pub const FS_ROOT_UNBOUND: &str = "E0451";
/// A path leaving its label's root via `..`, an absolute path, or a symlink.
pub const FS_PATH_ESCAPES_ROOT: &str = "E0452";
/// A read whose answer would be larger than one value holds; `fs.read_at` reads a range.
pub const FS_FILE_TOO_LARGE: &str = "E0453";
/// A `--fs NAME=PATH` root that is missing, not a directory, or unresolvable.
pub const FS_ROOT_INVALID: &str = "E0454";
/// `process.exit` was performed: the machine unwinds and `ply run` exits with the code.
pub const PROCESS_EXIT: &str = "E0455";
/// `process.spawn` or `process.start` named a resource label the run bound no executable to.
pub const PROCESS_EXEC_UNBOUND: &str = "E0456";
/// An `--exec NAME=PATH` that is missing, is not a file, or cannot be executed.
pub const PROCESS_EXEC_INVALID: &str = "E0457";
/// A child wrote more to a stream than the host holds of one, whole or as unread lines.
pub const PROCESS_OUTPUT_TOO_LARGE: &str = "E0458";
/// A `--allow` family the program being run does not declare: nothing would reach it.
pub const CAPABILITY_UNDECLARED: &str = "E0459";
pub const ASSERTION_FAILED: &str = "E0501";
/// A failure the language defines: `panic`, division by zero, overflow, a resource limit.
pub const RUNTIME_ERROR: &str = "E0502";
/// An entry point, test or evaluation spent its step budget without finishing.
pub const STEP_BUDGET: &str = "E0503";
pub const INTERNAL_ERROR: &str = "E0505";
/// Cache codes are warnings: cache trouble is never a fault in the user's program.
pub const CACHE_UNREADABLE: &str = "W0601";
pub const CACHE_CORRUPT: &str = "W0602";
pub const CACHE_VERSION_CHANGED: &str = "W0603";
/// The drain deadline expired with connections still in flight.
pub const DRAIN_INCOMPLETE: &str = "W0608";
/// Spans still open when their task or the entry point ended, reported when the entry point ends.
pub const SPAN_ABANDONED: &str = "W0609";
/// A value was made to reach itself; cycles are not collected, so it leaks.
pub const REFERENCE_CYCLE: &str = "W0610";
/// A definition no `pub` item, `main`, test or law reaches.
pub const UNUSED_DEFINITION: &str = "W0611";
/// A run the harness stopped at its wall clock: a fact about the machine, not about the
/// program, so nothing it did is recorded.
pub const RUN_ABANDONED: &str = "W0612";

/// Whether a failure under `code` is Ply's own rather than the program's.
pub fn is_defect(code: &str) -> bool {
    [
        INTERNAL_ERROR,
        HOST_FOOTPRINT_ESCAPE,
        SECRET_TO_HOST,
        SIMULATION_DIVERGENCE,
    ]
    .contains(&code)
}
