//! Stable diagnostic codes.

pub const UNEXPECTED_TOKEN: &str = "E0001";
pub const UNTERMINATED_STRING: &str = "E0002";
pub const UNKNOWN_NAME: &str = "E0101";
pub const UNKNOWN_TYPE: &str = "E0102";
pub const UNKNOWN_EFFECT: &str = "E0103";
pub const UNKNOWN_OPERATION: &str = "E0104";
pub const DUPLICATE_DEFINITION: &str = "E0105";
pub const UNKNOWN_MODULE: &str = "E0106";
pub const PRIVATE_NAME: &str = "E0107";
pub const AMBIGUOUS_IMPORT: &str = "E0108";
pub const MODULE_CYCLE: &str = "E0109";
pub const DUPLICATE_IMPORT: &str = "E0110";
pub const INVALID_MODULE_PATH: &str = "E0111";
pub const AMBIGUOUS_ENTRY_POINT: &str = "E0112";
/// A row, or an `effect set` body, naming a set the module does not declare.
pub const UNKNOWN_EFFECT_SET: &str = "E0114";
pub const EFFECT_SET_CYCLE: &str = "E0115";
/// The base of a record update `{..b, f: e}` is not a record, or its type is not known there.
pub const RECORD_UPDATE_SHAPE: &str = "E0116";
pub const RECORD_UPDATE_FIELD: &str = "E0117";
/// A `?` whose enclosing function's return type is not readable as `Result` or `Option`.
pub const TRY_SCOPE: &str = "E0118";
/// A `?` whose early exit would change what runs or discard something written.
pub const TRY_POSITION: &str = "E0119";
/// A parameter default on a lambda, an effect operation or a handler clause.
pub const DEFAULT_NOT_ALLOWED: &str = "E0120";
/// A parameter default that is not a pure, closed expression.
pub const DEFAULT_NOT_PURE: &str = "E0121";
/// A default on a `pub fn` that mentions a name the callee's module does not export.
pub const DEFAULT_PRIVATE_NAME: &str = "E0122";
/// A named argument naming no parameter, one named twice, or one filled positionally.
pub const UNKNOWN_ARGUMENT_NAME: &str = "E0123";
/// A positional argument after a named one.
pub const ARGUMENT_ORDER: &str = "E0124";
/// A parameter filled neither positionally nor by name, carrying no default.
pub const MISSING_ARGUMENT: &str = "E0125";
/// A top-level `fn` that leaves a parameter or return type to inference.
pub const MISSING_SIGNATURE: &str = "E0126";
/// A `reuse fn` with an append the cost checker cannot show reuses its list.
pub const REUSE_BROKEN: &str = "E0127";
/// A `ply replace` whose result would not check, or would move a definition it did not name.
pub const REPLACEMENT_REFUSED: &str = "E0128";
/// A `ply.pkg` that is not exactly one `fn package` returning std.pkg's `Manifest`.
pub const MANIFEST_SHAPE: &str = "E0129";
/// A manifest body that runs rather than being a literal, a constructor, a record or a list.
pub const MANIFEST_NOT_LITERAL: &str = "E0130";
/// A manifest literal that does not decode, or whose fields fail validation.
pub const MANIFEST_FIELD: &str = "E0131";
/// An import reaching a package the importing module's manifest does not declare.
pub const DEPENDENCY_NOT_DECLARED: &str = "E0132";
/// Two packages granting one module prefix, or a package's files claiming another's.
pub const PREFIX_COLLISION: &str = "E0133";
/// `ply.pkg` files depending on one another in a cycle.
pub const DEPENDENCY_CYCLE: &str = "E0134";
/// A dependency whose path is missing, that holds no `ply.pkg`, or that is not a path.
pub const DEPENDENCY_UNUSABLE: &str = "E0135";
/// A dependency below the version floor the manifest importing it asks for.
pub const DEPENDENCY_VERSION: &str = "E0136";
/// One package reached at two places, where a closure pins one version.
pub const DEPENDENCY_DIAMOND: &str = "E0137";
/// A dependency whose sources are not what `ply.lock` pinned.
pub const LOCK_MISMATCH: &str = "E0138";
/// A `ply.lock` that does not decode or is from another format.
pub const LOCK_UNREADABLE: &str = "E0139";
/// A git dependency that could not be fetched.
pub const DEPENDENCY_FETCH: &str = "E0140";
pub const TYPE_MISMATCH: &str = "E0201";
pub const ARITY_MISMATCH: &str = "E0202";
pub const OCCURS_CHECK: &str = "E0203";
pub const NOT_A_FUNCTION: &str = "E0204";
pub const NON_EXHAUSTIVE_MATCH: &str = "E0205";
/// No derivation for the requested deriver; reported at the field that blocks it.
pub const NOT_DERIVABLE: &str = "E0206";
pub const UNKNOWN_DERIVER: &str = "E0207";
/// A `derive` in a module other than the one declaring its target type.
pub const ORPHAN_DERIVE: &str = "E0208";
/// `/` applied to `Decimal`.
pub const DECIMAL_DIVISION: &str = "E0209";
/// An operand whose type nothing determines: an arithmetic or ordered-comparison numeric,
/// or the `String`-or-`Bytes` a `++` joins.
pub const NUMERIC_UNDETERMINED: &str = "E0210";
/// An integer literal outside the fixed-width type its context gave it.
pub const LITERAL_OUT_OF_RANGE: &str = "E0211";
pub const UNBOUND_ROW_VAR: &str = "E0301";
pub const EFFECT_NOT_PERMITTED: &str = "E0302";
pub const UNHANDLED_EFFECT: &str = "E0303";
pub const RESOURCE_REQUIRED: &str = "E0304";
/// A `handle` whose body performs an operation, on an atom it handles, that no clause answers.
pub const HANDLER_CLAUSE_MISSING: &str = "E0305";
/// A call leaving a label parameter unfilled by written label or argument row, or passing a
/// label-generic definition as a value.
pub const LABEL_INSTANTIATION: &str = "E0306";
/// Members of one recursive group binding different numbers of label or row parameters.
pub const LABEL_GROUP_BINDERS: &str = "E0307";
/// A call inside a recursive group that would instantiate the group's row, or the callee's own
/// type parameter, at something else: polymorphic recursion.
pub const POLYMORPHIC_RECURSION: &str = "E0308";
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
/// The guard admitted no values, so the obligation says nothing.
pub const VACUOUS_OBLIGATION: &str = "E0420";
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
/// `net.listen_tls` named a credential the binding does not hold.
pub const TLS_CREDENTIAL_UNKNOWN: &str = "E0429";
/// A `--tls` credential that does not load, or whose key does not match its certificate.
pub const TLS_CREDENTIAL_INVALID: &str = "E0430";
/// Postgres is bound but the database is unnamed, unparseable, unsupported or unreachable.
/// The server refused to prepare a statement, or its columns do not fit the row codec.
/// A statement touches a table outside its entry point's declared footprint.
/// A database operation by a task that does not own the open transaction scope.
/// The live schema has a trigger, rule or cascade touching tables no statement names.
/// A `Secret` passed to a host operation whose registration does not accept one.
pub const SECRET_TO_HOST: &str = "E0439";
/// A `--config` file or `--set` that cannot be read or is not `KEY=VALUE`.
pub const CONFIG_UNAVAILABLE: &str = "E0440";
/// A key the run's `--config-schema` marks `required` that no source supplies.
pub const CONFIG_MISSING: &str = "E0441";
pub const CONFIG_INVALID: &str = "E0442";
/// A deployable artifact whose contents do not verify against its own digests.
pub const ARTIFACT_INVALID: &str = "E0443";
/// An artifact built under a different frontend, runtime or body-encoding version.
pub const ARTIFACT_VERSION: &str = "E0444";
/// `trace.exit` naming a span not open on the performing task's stack.
pub const SPAN_UNBALANCED: &str = "E0445";
/// A value branded with a region's name would outlive the region.
pub const REGION_ESCAPE: &str = "E0446";
/// A definition the program reaches that the compiled tier cannot compile.
pub const DEFINITION_REFUSED: &str = "E0448";
/// A region handle crossing a runtime boundary, where no type is left to check.
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
/// An obligation no tier could decide.
pub const OBLIGATION_NOT_DISCHARGED: &str = "W0604";
/// The stdlib shipped with this compiler differs from the one the cache was written under.
pub const STDLIB_CHANGED: &str = "W0605";
/// A host runtime could not hand every resource back when an entry point ended.
/// An explicitly supplied configuration key the run's schema does not declare.
pub const CONFIG_UNDECLARED: &str = "W0607";
/// The drain deadline expired with connections still in flight.
pub const DRAIN_INCOMPLETE: &str = "W0608";
/// A value was made to reach itself; cycles are not collected, so it leaks.
pub const REFERENCE_CYCLE: &str = "W0610";
/// Spans still open when an entry point ended, closed by teardown.
pub const SPAN_ABANDONED: &str = "W0609";
/// A definition no `pub` item, `main`, test or law reaches.
pub const UNUSED_DEFINITION: &str = "W0611";
/// A run the harness stopped at its wall clock: a fact about the machine, not about the
/// program, so nothing it did is recorded.
pub const RUN_ABANDONED: &str = "W0612";
