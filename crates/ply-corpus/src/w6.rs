//! Where a request's time goes now, and what would justify M9.
//!
//! W1 measured one endpoint and concluded that codegen was the *second* lever,
//! because 5.41µs per byte of head was five O(n) folds and not a constant
//! factor. W2 proved it by attacking the algorithm. W3, W4 and W5 then put
//! framing, routing, TLS, a database and a sink on the same path, and nobody
//! has stated what the whole of it costs. W6 states it, and decides M9 from the
//! statement rather than from the assumption that carried M0–M8.
//!
//! Three things live here, and only the first is a measurement:
//!
//! 1. **The ladder** — [`Layer`], [`Point`], [`Ladder`]. W1's method extended to
//!    the full W5 stack: every rung is a **pair of absolutes taken in the same
//!    arena in the same run**, differing in exactly one substitution, so the
//!    layer is their difference and no timer inside the machine has to be
//!    trusted. A rung that cannot be expressed that way is not a rung.
//! 2. **The decision** — [`Criteria`], [`decide`]. The thresholds are pinned
//!    here, in code, with defaults that cannot be supplied from a measurement
//!    file, so the verdict is computed from numbers rather than fitted to them.
//!    ADR 0016 states each threshold and why it is where it is.
//! 3. **The honest account** — [`Report`], [`Report::audit`]. A W6 report that
//!    omits the accumulated table, what a reader gets today, or where this
//!    language is not competitive is not a shorter report; it is a misleading
//!    one, so the omission is an audit finding rather than a blank section.
//!
//! **The residue is printed, and its sign decides how it is charged.** The
//! rungs will not sum to the served total — they are taken in two arenas and a
//! served request pays for things no substitution isolates. [`Ladder`] reports
//! `total − attributed` as [`Ladder::residue_micros`] and never folds it into a
//! neighbouring layer. A **positive** residue is time no substitution
//! separated: it is not credited to the interpreter, which makes the attributed
//! share a lower bound. A **negative** residue is the opposite fact — the
//! layers sum to more than the request they were read against, so the
//! in-process arena over-counts against the served one — and crediting it to
//! nobody would leave the numerator inflated in exactly the direction M9's case
//! rests on. So it is charged back: [`Ladder::conservative_share`] is what
//! [`decide`] reads, and it equals the attributed share whenever the residue is
//! positive.

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

/// A measurement that can be divided by. Zero, negative, infinite and NaN are
/// all measurement failures rather than very fast results, and every ratio here
/// checks before dividing so a broken run reads as broken.
fn usable(micros: f64) -> bool {
    micros.is_finite() && micros > 0.0
}

// --------------------------------------------------------------- the ladder

/// Where a rung is taken. The two are not interchangeable and a ladder that
/// pretended they were would report a process boundary as a layer.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Arena {
    /// In this process, over `SimNet` or a real listener, with no CLI and no
    /// child. What the parse, the route and the machine cost, without a
    /// syscall's variance in the middle of them.
    InProcess,
    /// The real `ply` binary over loopback, driven by client threads. The only
    /// place a flag substitution — `--tls`, `--db`, `--trace` — is available.
    Served,
}

/// One layer of the W5 stack, and the substitution that isolates it.
///
/// The order is the order a request meets them, and it is the order a
/// [`Ladder`] must present them in. Every variant names one thing; a layer that
/// needed two substitutions to isolate would be two layers.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Layer {
    Call,
    Endpoint,
    Framing,
    Routing,
    Machine,
    Socket,
    Tls,
    Database,
    Tracing,
}

impl Layer {
    /// Every layer, in request order. `decide` requires all of them, because a
    /// share taken over a partial stack is a share of the wrong denominator.
    pub const ORDER: [Layer; 9] = [
        Layer::Call,
        Layer::Endpoint,
        Layer::Framing,
        Layer::Routing,
        Layer::Machine,
        Layer::Socket,
        Layer::Tls,
        Layer::Database,
        Layer::Tracing,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Layer::Call => "call",
            Layer::Endpoint => "endpoint",
            Layer::Framing => "framing",
            Layer::Routing => "routing",
            Layer::Machine => "machine",
            Layer::Socket => "socket",
            Layer::Tls => "tls",
            Layer::Database => "database",
            Layer::Tracing => "tracing",
        }
    }

    pub fn rank(self) -> usize {
        Layer::ORDER
            .iter()
            .position(|l| *l == self)
            .expect("every layer is in ORDER")
    }

    /// What this rung's difference is the cost of.
    pub fn isolates(self) -> &'static str {
        match self {
            Layer::Call => "entering the machine at all: one `Machine::call` and its return",
            Layer::Endpoint => "one route's own body, through its derived JSON encoder",
            Layer::Framing => "HTTP/1.1: the request line, the field block, the length, the encode",
            Layer::Routing => "building the route table from its patterns, and matching one path",
            Layer::Machine => "the loop around the pure pieces: recv, perform, handler walk, send",
            Layer::Socket => "the socket, the reactor, the blocking pool and the pending token",
            Layer::Tls => "the TLS record layer in steady state, handshake excluded",
            Layer::Database => "the postgres boundary, the wire, and the server",
            Layer::Tracing => "the sink: encoding a record and writing it",
        }
    }

    /// The one thing that changes between this rung's two measurements. If a
    /// measurement cannot be taken this way it is not this rung.
    pub fn substitution(self) -> &'static str {
        match self {
            Layer::Call => "a function returning a constant, against not calling the machine",
            Layer::Endpoint => "the route's body, against that constant-returning function",
            Layer::Framing => "`parse_head` and `encode` around the same body, against without",
            Layer::Routing => "`table()` and `route_of()` above the framed call, against without",
            Layer::Machine => "the whole `serve_one` over `SimNet`, against calling the pieces",
            Layer::Socket => "the real TCP host under the same loop, against `SimNet`",
            Layer::Tls => "`--tls` on the same route and load, against plaintext",
            Layer::Database => "`run` against `run_memory` — postgres against the twin",
            Layer::Tracing => "`--trace json` to /dev/null, against `--trace off`",
        }
    }

    pub fn arena(self) -> Arena {
        match self {
            Layer::Call
            | Layer::Endpoint
            | Layer::Framing
            | Layer::Routing
            | Layer::Machine
            | Layer::Socket => Arena::InProcess,
            Layer::Tls | Layer::Database | Layer::Tracing => Arena::Served,
        }
    }

    /// Whether a faster execution strategy could reach this layer.
    ///
    /// `Socket`, `Tls`, `Database` and `Tracing` are the host's: a syscall, a
    /// cipher, a postgres server and a JSON writer are not what a codegen
    /// backend compiles. `Tracing` is the **sink** and not the perform — there
    /// is no configuration under which a trace operation is not performed (ADR
    /// 0015 §1.4), so the Ply-side cost of a trace call is inside `Machine`,
    /// where `--trace off` already pays it.
    pub fn is_interpreter(self) -> bool {
        matches!(
            self,
            Layer::Call | Layer::Endpoint | Layer::Framing | Layer::Routing | Layer::Machine
        )
    }
}

/// One rung, as measured: two absolutes taken in one arena in one run.
///
/// Both numbers are required because a layer is a *difference*, and a
/// difference between a number taken today and a number quoted from a milestone
/// ago is a fact about two machines.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Point {
    pub layer: Layer,
    /// The route this rung's pair was taken on.
    ///
    /// A printed column rather than a fact about the harness: two rungs taken
    /// on different routes have a difference that is not one layer, and the one
    /// thing that makes that visible to a reader is seeing the route change.
    /// The ladder is built around one route wherever it can be.
    pub taken_on: String,
    /// The configuration **with** this layer, per request. The best of the
    /// repeats, because the quantity of interest is the cost of the work and
    /// everything a run adds to it is additive.
    pub with_micros: f64,
    /// The same configuration **without** it, same arena, same run. Zero is
    /// legal only for [`Layer::Call`], whose "without" is not calling at all.
    pub without_micros: f64,
    /// The **worst** of the same repeats, when the rung was repeated.
    ///
    /// A best-of number on its own is a point with no width, and a layer is a
    /// *difference* between two of them — so a rung whose layer is 1% of either
    /// side carries both sides' noise and says nothing at the precision it is
    /// printed to. With these, [`Rung::layer_low_micros`] and
    /// [`Rung::layer_high_micros`] bound the layer and [`Ladder::share_low`]
    /// bounds the share M9's case rests on. `None` means the rung was taken
    /// once, which [`Report::audit`] reports rather than assumes away.
    #[serde(default)]
    pub with_worst_micros: Option<f64>,
    #[serde(default)]
    pub without_worst_micros: Option<f64>,
    /// Requests each side of the pair was averaged over.
    pub requests: u32,
}

impl Point {
    /// The layer at its smallest: the fastest `with` against the slowest
    /// `without`. Both bounds exist only when both sides were repeated.
    pub fn low_micros(&self) -> Option<f64> {
        Some(self.with_micros - self.without_worst_micros.unwrap_or(self.without_micros))
            .filter(|_| self.with_worst_micros.is_some() || self.without_worst_micros.is_some())
    }

    /// The layer at its largest: the slowest `with` against the fastest
    /// `without`.
    pub fn high_micros(&self) -> Option<f64> {
        Some(self.with_worst_micros.unwrap_or(self.with_micros) - self.without_micros)
            .filter(|_| self.with_worst_micros.is_some() || self.without_worst_micros.is_some())
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct Rung {
    pub layer: Layer,
    pub label: &'static str,
    pub isolates: &'static str,
    pub substitution: &'static str,
    pub arena: Arena,
    pub taken_on: String,
    pub with_micros: f64,
    pub without_micros: f64,
    /// `with − without`. Negative is a real outcome and is reported as one: it
    /// means the substitution did not isolate the layer, and [`decide`] refuses
    /// on a ladder carrying a large one rather than reading a share off it.
    pub layer_micros: f64,
    /// The same difference at its smallest and largest over the repeats, when
    /// the rung carries them. A rung whose band spans zero has not resolved its
    /// own sign, and `Report::audit` says so — the alternative is printing two
    /// decimals of a number the measurement did not produce.
    pub layer_low_micros: Option<f64>,
    pub layer_high_micros: Option<f64>,
    pub layer_share: f64,
    pub requests: u32,
}

impl Rung {
    /// Whether the repeats leave this layer's sign undetermined.
    pub fn sign_unresolved(&self) -> bool {
        match (self.layer_low_micros, self.layer_high_micros) {
            (Some(low), Some(high)) => low <= 0.0 && high >= 0.0,
            _ => false,
        }
    }
}

/// What the floor and the total answered, so a multiple between them is
/// readable rather than inferable.
///
/// A `total / floor` whose numerator serves one route over TLS against a
/// database and whose denominator replays another route's response over
/// plaintext is a ratio between two different jobs. The strings are printed
/// beside the multiple and compared by [`Report::audit`].
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Denominators {
    /// What the Rust floor answered, spelled out: the route, the response size
    /// and everything it does *not* have under it.
    pub floor_taken_on: String,
    /// What the measured total served.
    pub total_taken_on: String,
    /// The worst of the total's repeats, when it was repeated.
    pub total_worst_micros: Option<f64>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Ladder {
    /// The same syscalls with no interpreter under them, per request.
    pub floor_micros: f64,
    /// What the served stack actually delivered, end to end, per request. Not a
    /// sum: it is measured, and the rungs are checked against it.
    pub total_micros: f64,
    /// What the rungs account for.
    pub attributed_micros: f64,
    /// `total − attributed`. Everything no substitution separated, printed
    /// rather than folded into a neighbour.
    pub residue_micros: f64,
    pub residue_share: f64,
    pub over_floor: f64,
    pub denominators: Denominators,
    /// The layers a codegen backend could reach, as attributed.
    pub interpreter_micros: f64,
    pub interpreter_share: f64,
    /// The same with a **negative** residue charged back to it.
    ///
    /// A negative residue means the attributed layers sum to more than the
    /// request they are read against, which can only be the in-process arena
    /// over-counting against the served denominator — so the honest numerator
    /// is the smaller one, and it is the one [`decide`] reads. When the residue
    /// is positive this is exactly [`Ladder::interpreter_share`] and the share
    /// is a lower bound, as §1.4 says.
    pub conservative_micros: f64,
    pub conservative_share: f64,
    /// The conservative share at the two ends of the repeats it was read off:
    /// the smallest numerator over the largest denominator, and the reverse.
    /// `None` when no rung carried a second sample.
    pub share_low: Option<f64>,
    pub share_high: Option<f64>,
    /// Whether the interpreter rungs chain — each `without` is the rung below's
    /// `with` — so that their sum is one absolute somebody measured rather than
    /// five differences added up.
    pub telescopes: bool,
    /// The most negative layer as a share of the total, as a positive number.
    /// A ladder above [`Criteria::max_negative_share`] did not separate.
    pub worst_negative_share: f64,
    pub rungs: Vec<Rung>,
}

impl Ladder {
    /// Assemble a ladder, refusing anything a share cannot honestly be read off.
    ///
    /// The refusals are loud on purpose: a duplicated layer double-counts, an
    /// out-of-order one reads as a different stack, and a zero total makes every
    /// share infinite. All three are silent in a table and fatal in a decision.
    pub fn assemble(floor_micros: f64, total_micros: f64, points: &[Point]) -> Result<Ladder> {
        Ladder::assemble_with(floor_micros, total_micros, points, &Denominators::default())
    }

    pub fn assemble_with(
        floor_micros: f64,
        total_micros: f64,
        points: &[Point],
        denominators: &Denominators,
    ) -> Result<Ladder> {
        if points.is_empty() {
            bail!("a ladder with no rungs attributes nothing and decides nothing");
        }
        if !usable(total_micros) {
            bail!("the served total is {total_micros}µs; every share below would be meaningless");
        }
        if !usable(floor_micros) {
            bail!("the floor is {floor_micros}µs; a request cannot be compared against it");
        }

        let mut seen: Vec<Layer> = Vec::new();
        for point in points {
            if seen.contains(&point.layer) {
                bail!(
                    "the `{}` rung appears twice; a repeated layer is counted twice",
                    point.layer.label()
                );
            }
            if let Some(last) = seen.last()
                && last.rank() >= point.layer.rank()
            {
                bail!(
                    "the `{}` rung follows `{}`; a ladder is presented in request order so a \
                     reader can add it up",
                    point.layer.label(),
                    last.label()
                );
            }
            if point.requests == 0 {
                bail!(
                    "the `{}` rung averaged over zero requests",
                    point.layer.label()
                );
            }
            if point.taken_on.trim().is_empty() {
                bail!(
                    "the `{}` rung names no route; two rungs taken on different routes have a \
                     difference that is not one layer, and only the column says so",
                    point.layer.label()
                );
            }
            seen.push(point.layer);
        }

        let mut rungs = Vec::with_capacity(points.len());
        let mut attributed = 0.0;
        let mut interpreter = 0.0;
        let mut worst_negative: f64 = 0.0;
        for point in points {
            let layer_micros = point.with_micros - point.without_micros;
            attributed += layer_micros;
            if point.layer.is_interpreter() {
                interpreter += layer_micros;
            }
            if layer_micros < 0.0 {
                worst_negative = worst_negative.max(-layer_micros / total_micros);
            }
            rungs.push(Rung {
                layer: point.layer,
                label: point.layer.label(),
                isolates: point.layer.isolates(),
                substitution: point.layer.substitution(),
                arena: point.layer.arena(),
                taken_on: point.taken_on.clone(),
                with_micros: point.with_micros,
                without_micros: point.without_micros,
                layer_micros,
                layer_low_micros: point.low_micros(),
                layer_high_micros: point.high_micros(),
                layer_share: layer_micros / total_micros,
                requests: point.requests,
            });
        }

        let residue = total_micros - attributed;
        // Only a negative residue moves the numerator: a positive one is time
        // no substitution separated, and crediting it to the interpreter would
        // be claiming an attribution the ladder did not earn.
        let seam = residue.min(0.0);
        let conservative = interpreter + seam;

        let interpreter_rungs: Vec<&Rung> =
            rungs.iter().filter(|r| r.layer.is_interpreter()).collect();
        let telescopes = interpreter_rungs
            .windows(2)
            .all(|pair| pair[0].with_micros == pair[1].without_micros)
            && interpreter_rungs
                .first()
                .is_some_and(|first| first.without_micros == 0.0);
        // When the rungs chain, the interpreter total is the top rung's own
        // absolute and its band is that rung's. When they do not, the only
        // bound available is every layer at its widest, which compounds — and
        // saying so is the point of the flag.
        let top = points.iter().rfind(|p| p.layer.is_interpreter());
        let (share_low, share_high) = match top.filter(|_| telescopes) {
            Some(top) => match top.with_worst_micros {
                Some(worst) => {
                    let slowest = denominators.total_worst_micros.unwrap_or(total_micros);
                    (
                        Some((top.with_micros + seam) / slowest),
                        Some((worst + seam) / total_micros),
                    )
                }
                None => (None, None),
            },
            None => (None, None),
        };

        Ok(Ladder {
            floor_micros,
            total_micros,
            attributed_micros: attributed,
            residue_micros: residue,
            residue_share: residue / total_micros,
            over_floor: total_micros / floor_micros,
            denominators: denominators.clone(),
            interpreter_micros: interpreter,
            interpreter_share: interpreter / total_micros,
            conservative_micros: conservative,
            conservative_share: conservative / total_micros,
            share_low,
            share_high,
            telescopes,
            worst_negative_share: worst_negative,
            rungs,
        })
    }

    /// Layers the ladder does not carry. A decision over a partial stack is a
    /// share of the wrong denominator, so [`decide`] consults this first.
    pub fn missing(&self) -> Vec<Layer> {
        Layer::ORDER
            .into_iter()
            .filter(|l| !self.rungs.iter().any(|r| r.layer == *l))
            .collect()
    }
}

// -------------------------------------------------------------- the engines

/// The same request under one execution strategy and then another.
///
/// This is the cheapest empirical handle on how much of a request is dispatch
/// rather than native work, and it needs nothing built: both engines exist and
/// `--engine both` already polices their agreement. If swapping one whole
/// interpreter for another whole interpreter barely moves a request, a third
/// one will not either — and if it moves it a lot, the frame representation is
/// a lever that costs a fraction of a codegen backend.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EnginePoint {
    /// `treewalk` or `machine`.
    pub engine: String,
    pub per_request_micros: f64,
    pub requests: u32,
}

/// The ratio between the fastest and slowest engine measured, and which won.
pub fn engine_spread(points: &[EnginePoint]) -> Option<(f64, String, String)> {
    let fastest = points
        .iter()
        .min_by(|a, b| a.per_request_micros.total_cmp(&b.per_request_micros))?;
    let slowest = points
        .iter()
        .max_by(|a, b| a.per_request_micros.total_cmp(&b.per_request_micros))?;
    if !usable(fastest.per_request_micros) {
        return None;
    }
    Some((
        slowest.per_request_micros / fastest.per_request_micros,
        fastest.engine.clone(),
        slowest.engine.clone(),
    ))
}

// ---------------------------------------------------------------- the spike

/// One input the spike and the interpreter both answered.
///
/// Four times rather than two: a speedup read off two best-of numbers is a
/// claim the noise has not been asked about. The conservative ratio compares
/// the interpreter's **best** against the spike's **worst**, so a reported win
/// is one that survived the worst sample of the thing being sold.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SpikeInput {
    pub name: String,
    pub interpreter_best_micros: f64,
    pub interpreter_worst_micros: f64,
    pub spike_best_micros: f64,
    pub spike_worst_micros: f64,
    /// Whether the two produced equal values on this input, by `Value`'s own
    /// ordering. A faster wrong answer is an `E0503`, not a speedup.
    pub agreed: bool,
}

impl SpikeInput {
    /// Interpreter best over spike worst. The number a decision may use.
    pub fn conservative(&self) -> f64 {
        if !usable(self.spike_worst_micros) {
            return 0.0;
        }
        self.interpreter_best_micros / self.spike_worst_micros
    }

    /// Interpreter best over spike best. Reported beside the conservative one
    /// so the width of the claim is visible, and never used to decide.
    pub fn optimistic(&self) -> f64 {
        if !usable(self.spike_best_micros) {
            return 0.0;
        }
        self.interpreter_best_micros / self.spike_best_micros
    }

    /// Whether the two samples separate at all. Overlapping bands mean the
    /// measurement found no difference, whatever the midpoints say.
    pub fn separated(&self) -> bool {
        self.spike_worst_micros < self.interpreter_best_micros
    }
}

/// What the throwaway codegen spike produced.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Spike {
    /// The function compiled, named so the choice is reviewable.
    pub function: String,
    /// Why this one. ADR 0016 §3 fixes the selection rule; this is the rule's
    /// output on this stack.
    pub chosen_because: String,
    /// Nodes in its lowered body, which is the size of what was compiled.
    pub nodes: usize,
    /// What compiling it cost, once. A service compiles at first call, so this
    /// is reported rather than amortized away.
    pub compile_micros: f64,
    pub inputs: Vec<SpikeInput>,
}

#[derive(Clone, Debug, Serialize)]
pub struct SpikeVerdict {
    /// The **minimum** conservative ratio over every input. A speedup that
    /// holds on one input and not another is that input's, so the weakest one
    /// is the claim.
    pub speedup: f64,
    /// The best optimistic ratio, for context only.
    pub optimistic: f64,
    pub evidence: bool,
    /// Every rule that failed, named. Empty when the spike is evidence.
    pub failures: Vec<String>,
}

/// Inputs a spike must answer before its ratio means anything.
pub const SPIKE_MIN_INPUTS: usize = 3;

impl Spike {
    /// Whether this is evidence, and what the number is if so.
    pub fn judge(&self) -> SpikeVerdict {
        let mut failures = Vec::new();
        if self.inputs.len() < SPIKE_MIN_INPUTS {
            failures.push(format!(
                "{} input(s); a ratio over fewer than {SPIKE_MIN_INPUTS} is one input's constant",
                self.inputs.len()
            ));
        }
        for input in &self.inputs {
            if !input.agreed {
                failures.push(format!(
                    "the spike and the interpreter disagreed on `{}`; a faster wrong answer is a \
                     divergence, not a speedup",
                    input.name
                ));
            }
            if !input.separated() {
                failures.push(format!(
                    "the samples overlap on `{}`: the spike's worst ({:.3}µs) is not below the \
                     interpreter's best ({:.3}µs)",
                    input.name, input.spike_worst_micros, input.interpreter_best_micros
                ));
            }
        }
        let speedup = self
            .inputs
            .iter()
            .map(|i| i.conservative())
            .fold(f64::INFINITY, f64::min);
        let optimistic = self
            .inputs
            .iter()
            .map(|i| i.optimistic())
            .fold(0.0_f64, f64::max);
        SpikeVerdict {
            speedup: if speedup.is_finite() { speedup } else { 0.0 },
            optimistic,
            evidence: failures.is_empty(),
            failures,
        }
    }
}

// --------------------------------------------------------- the alternatives

/// One entry of ADR 0016 §4's table, **in code**.
///
/// C3 is "*every* alternative in §4 is priced", and a check written against
/// whatever list a measurement file happened to carry is not that check: an
/// empty array satisfies it vacuously, so deleting one field of the file would
/// turn a deferral into an advance. The roster therefore lives here, beside the
/// criteria and out of reach of the run being judged, and [`decide`] reads a
/// file only for what each of these levers *measured*.
pub struct Lever {
    /// The key an [`Alternative`] carries in its `name` to answer for this
    /// lever. Stable, because it is what a measurement file is matched on.
    pub name: &'static str,
    /// What the change is, in the ADR's words.
    pub what: &'static str,
}

/// ADR 0016 §4's seven levers. C3 is decided against this array and nothing
/// else.
pub const LEVERS: [Lever; 7] = [
    Lever {
        name: "more native builtins",
        what: "fold `read_line`, `is_token`, `trim_ows` and `trim_end` into one native head scan; \
               `string_lower`; `add_field`",
    },
    Lever {
        name: "the frame push",
        what: "ADR 0005's four heap allocations per frame push, priced by the engine substitution \
               and by an allocation count",
    },
    Lever {
        name: "Env::lookup",
        what: "a linear walk down an `Rc` chain, so a variable reference costs O(scope depth); \
               priced by a depth sweep and by an indexed alternative",
    },
    Lever {
        name: "boxing on hot paths",
        what: "where a `Value::Int` per element survives; counted per request rather than guessed \
               at",
    },
    Lever {
        name: "caching derived work",
        what: "`table()` rebuilds the route table from its pattern strings on every request, and a \
               derived codec dictionary is a record built per call",
    },
    Lever {
        name: "connection and statement reuse",
        what: "W4's pool and prepared-statement cache: hit rate, and what a miss costs",
    },
    Lever {
        name: "response buffering",
        what: "writes per response, and the copies `bytes_concat` and `bytes_slice` make",
    },
];

/// A lever that is not a codegen backend, and what it measured.
///
/// W2's byte builtins beat W1's predicted codegen win by attacking the
/// algorithm instead of the constant, so the existence of an **unpriced**
/// alternative is itself a reason to keep deferring — which is why `priced` is
/// a field rather than an omission.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Alternative {
    /// The [`Lever`] this answers for. An alternative naming no lever in
    /// [`LEVERS`] prices nothing C3 asks about.
    pub name: String,
    /// What the change is, concretely enough to be built.
    pub what: String,
    pub priced: bool,
    /// End-to-end speedup on the same served workload the ladder's total came
    /// from. `1.0` means "priced, and it bought nothing", which is a result.
    /// Ignored when `priced` is false.
    pub end_to_end: f64,
    /// The two things the ratio is between, and where the numbers came from.
    ///
    /// `priced` is a boolean in a file and `end_to_end` is a number in a file;
    /// either can be written by a hand rather than by a harness. This is what
    /// makes the claim checkable by a reader, and a priced lever without it is
    /// treated as unpriced — the same way a rung with one measurement is not a
    /// rung.
    #[serde(default)]
    pub evidence: String,
    /// What it would cost to keep, in one sentence. This is the column M9 loses
    /// on even when its ratio is comparable.
    pub cost: String,
}

impl Alternative {
    /// Whether this is a measurement C3 may read: priced, with a usable ratio,
    /// and carrying what the ratio is between.
    pub fn is_priced(&self) -> bool {
        self.priced && usable(self.end_to_end) && !self.evidence.trim().is_empty()
    }

    /// The best priced alternative, or none if nothing was priced.
    pub fn best(alternatives: &[Alternative]) -> Option<&Alternative> {
        alternatives
            .iter()
            .filter(|a| a.is_priced())
            .max_by(|a, b| a.end_to_end.total_cmp(&b.end_to_end))
    }

    pub fn unpriced(alternatives: &[Alternative]) -> Vec<&Alternative> {
        alternatives.iter().filter(|a| !a.is_priced()).collect()
    }

    /// What this alternative answers for, if anything.
    pub fn lever(&self) -> Option<&'static Lever> {
        LEVERS.iter().find(|l| l.name == self.name)
    }
}

/// Every §4 lever this file does not price, as the sentence C3 fails on.
///
/// Two ways to fail and they are named apart, because the remedies differ: a
/// lever with no entry at all was never looked at, and a lever with an entry
/// that is not a measurement was looked at and not measured.
pub fn c3_gaps(alternatives: &[Alternative]) -> Vec<String> {
    let mut gaps = Vec::new();
    for lever in &LEVERS {
        match alternatives.iter().find(|a| a.name == lever.name) {
            None => gaps.push(format!(
                "`{}` is in ADR 0016 §4 and this report says nothing about it: {}",
                lever.name, lever.what
            )),
            Some(entry) if !entry.priced => gaps.push(format!(
                "`{}` is unpriced: {}",
                lever.name,
                if entry.what.trim().is_empty() {
                    lever.what
                } else {
                    entry.what.as_str()
                }
            )),
            Some(entry) if entry.evidence.trim().is_empty() => gaps.push(format!(
                "`{}` claims {:.2}x and names nothing the ratio is between; a priced lever with no \
                 evidence is a number in a file",
                lever.name, entry.end_to_end
            )),
            Some(entry) if !usable(entry.end_to_end) => gaps.push(format!(
                "`{}` is priced at {:.2}x, which is not a speedup anything could have measured",
                lever.name, entry.end_to_end
            )),
            Some(_) => {}
        }
    }
    gaps
}

// ------------------------------------------------------------ the criteria

/// The thresholds, pinned before the numbers exist.
///
/// Every one is stated and justified in ADR 0016 §2. There is deliberately no
/// path from a measurement file to these values: [`Report`] carries no
/// criteria, so a run cannot supply the bar it is about to clear.
#[derive(Clone, Copy, Debug, Serialize)]
pub struct Criteria {
    /// Interpreter share at or above which M9's case can be made at all. At
    /// exactly this share an *infinite* codegen speedup is worth 2.0x end to
    /// end, and 2.0x is the least that buys a permanent second execution path.
    pub min_share: f64,
    /// Below this share, defer categorically and say why: the ceiling is
    /// `1/(1−share)` however good the backend is.
    pub defer_share: f64,
    /// Speedup the spike must show on a real request-path function.
    pub min_spike: f64,
    /// Below this, defer: a constant factor this small is inside the range a
    /// cheaper lever has already delivered once.
    pub defer_spike: f64,
    /// Projected end-to-end speedup, by Amdahl over the measured share.
    pub min_projection: f64,
    /// Between [`Criteria::defer_share`] and [`Criteria::min_share`] the share
    /// alone cannot carry M9, so the spike has to be this good instead.
    pub gray_spike: f64,
    /// M9 must beat the best priced alternative by this factor, **on the gains
    /// rather than the ratios**: a 1.5x and a 1.1x are a 50% and a 10%
    /// improvement, and 1.5 against 2×1.1 compares nothing. M9 is a permanent
    /// surface and an alternative is one change, so a near tie goes to the
    /// alternative.
    pub alternative_margin: f64,
    /// A ladder with a negative layer larger than this did not separate, and
    /// nothing is decided from it.
    pub max_negative_share: f64,
}

impl Default for Criteria {
    fn default() -> Criteria {
        Criteria {
            min_share: 0.50,
            defer_share: 0.35,
            min_spike: 3.0,
            defer_spike: 2.0,
            min_projection: 1.5,
            gray_spike: 5.0,
            alternative_margin: 2.0,
            max_negative_share: 0.05,
        }
    }
}

/// The one workload every share in this module is taken on.
///
/// A constant rather than a parameter because it is a property of the
/// **instrument**: [`Ladder::missing`] refuses a ladder without all nine of
/// `Layer::ORDER`'s rungs, so a compute kernel or a lexer over a file cannot be
/// fed to `w6` at all — it answers [`Verdict::Undecided`] for them, correctly.
///
/// It is carried on [`Decision`] and printed beside the verdict because of what
/// ADR 0026 §4.2 withdraws: this ladder's authority over **M9**, on three
/// measured grounds — it refuses every workload but this one, its share moves
/// with the network rather than with Ply, and its reopen sentence names a
/// criterion only a regression can satisfy. The measurement is untouched and
/// stays the best account this project has of where a served request's time
/// goes. What changed is that "35% of a request" may no longer be read as "35%
/// of Ply".
pub const WORKLOAD: &str =
    "the served HTTP workload (examples/desk.ply over a socket, TLS and postgres)";

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Verdict {
    /// Bring a code generator for this workload forward.
    Advance,
    /// The grey band cleared: M9 is justified, and the report says on what
    /// conditions, because the share alone did not carry it.
    Conditional,
    /// Keep deferring, with the number that would reopen it.
    Defer,
    /// The measurement did not produce a decidable answer. Not the same as
    /// `Defer`, and reported as itself: a missing rung and a small share call
    /// for opposite responses.
    Undecided,
}

impl Verdict {
    pub fn label(self) -> &'static str {
        match self {
            Verdict::Advance => "advance a code generator for this workload",
            Verdict::Conditional => "advance a code generator for this workload, conditionally",
            Verdict::Defer => "keep deferring a code generator for this workload",
            Verdict::Undecided => "undecided — the measurement did not decide it",
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct Decision {
    pub verdict: Verdict,
    /// What the share was taken on. See [`WORKLOAD`].
    pub workload: &'static str,
    /// [`Ladder::conservative_share`] — the share after a negative residue is
    /// charged back — because that is the one a decision may read.
    pub interpreter_share: f64,
    pub spike_speedup: f64,
    /// Amdahl over the measured share and the spike's ratio.
    pub projected: f64,
    pub best_alternative: Option<String>,
    pub best_alternative_end_to_end: f64,
    /// Why, in the order the rules were applied. Always non-empty.
    pub reasons: Vec<String>,
    /// The number that would change the answer.
    pub reopens_at: String,
    pub criteria: Criteria,
}

/// Amdahl: what a `speedup` on `share` of a request is worth end to end.
pub fn projected(share: f64, speedup: f64) -> f64 {
    if !usable(speedup) {
        return 1.0;
    }
    let share = share.clamp(0.0, 1.0);
    1.0 / ((1.0 - share) + share / speedup)
}

/// The ceiling: what an *infinitely* fast backend on `share` is worth.
pub fn ceiling(share: f64) -> f64 {
    let share = share.clamp(0.0, 1.0);
    if share >= 1.0 {
        return f64::INFINITY;
    }
    1.0 / (1.0 - share)
}

/// Apply the pinned criteria. Nothing here reads a verdict out of a file.
pub fn decide(
    ladder: &Ladder,
    spike: Option<&Spike>,
    alternatives: &[Alternative],
    criteria: &Criteria,
) -> Decision {
    let share = ladder.conservative_share;
    let mut reasons = Vec::new();

    let undecided = |reasons: Vec<String>| Decision {
        verdict: Verdict::Undecided,
        workload: WORKLOAD,
        interpreter_share: share,
        spike_speedup: 0.0,
        projected: 1.0,
        best_alternative: None,
        best_alternative_end_to_end: 0.0,
        reasons,
        reopens_at: "take the missing measurement; nothing is decided until it exists".to_string(),
        criteria: *criteria,
    };

    let missing = ladder.missing();
    if !missing.is_empty() {
        let names: Vec<&str> = missing.iter().map(|l| l.label()).collect();
        reasons.push(format!(
            "the ladder carries no `{}` rung, so its total is not the stack's and no share can be \
             read off it",
            names.join("`, `")
        ));
        return undecided(reasons);
    }
    if ladder.worst_negative_share > criteria.max_negative_share {
        reasons.push(format!(
            "a layer came out {:.0}% negative, above the {:.0}% a ladder may carry: the \
             substitution did not isolate it, so no share here is trustworthy",
            ladder.worst_negative_share * 100.0,
            criteria.max_negative_share * 100.0
        ));
        return undecided(reasons);
    }

    let Some(spike) = spike else {
        reasons.push(
            "no codegen spike was run, so the speedup a backend would deliver is unmeasured and \
             M9 would be chosen on an assumption"
                .to_string(),
        );
        return undecided(reasons);
    };
    let judged = spike.judge();
    if !judged.evidence {
        reasons.push(format!(
            "the spike on `{}` is not evidence: {}",
            spike.function,
            judged.failures.join("; ")
        ));
        return undecided(reasons);
    }

    let k = judged.speedup;
    let e = projected(share, k);
    let best = Alternative::best(alternatives);
    let best_name = best.map(|a| a.name.clone());
    let best_ratio = best.map(|a| a.end_to_end).unwrap_or(0.0);

    reasons.push(format!(
        "the interpreter is {:.0}% of a request ({:.1}µs of {:.1}µs), so the ceiling on any \
         execution-strategy change is {:.2}x",
        share * 100.0,
        ladder.conservative_micros,
        ladder.total_micros,
        ceiling(share)
    ));
    if ladder.residue_micros < 0.0 {
        reasons.push(format!(
            "the residue is {:.1}µs — negative, so the layers sum to more than the request they \
             are read against and the seam is charged to the interpreter rather than to nobody: \
             the attributed share is {:.1}% and the one above is what the decision reads",
            ladder.residue_micros,
            ladder.interpreter_share * 100.0
        ));
    }
    if let (Some(low), Some(high)) = (ladder.share_low, ladder.share_high) {
        reasons.push(format!(
            "over its repeats that share runs {:.1}%–{:.1}%",
            low * 100.0,
            high * 100.0
        ));
    }
    reasons.push(format!(
        "the spike compiled `{}` and held {k:.2}x on its weakest input, which projects {e:.2}x \
         end to end",
        spike.function
    ));

    // The sentence names only the conditions that are **not** met. Naming a met
    // one as what would reopen M9 is how the first version of this document got
    // its reopen sentence wrong (§11): it asked for a share and a spike that had
    // both already cleared.
    let mut wants: Vec<String> = Vec::new();
    if share < criteria.min_share {
        wants.push(format!(
            "the interpreter share reaches {:.0}% (it is {:.0}%, a {:.2}x ceiling)",
            criteria.min_share * 100.0,
            share * 100.0,
            ceiling(share)
        ));
    }
    if k < criteria.min_spike {
        wants.push(format!(
            "the spike reaches {:.1}x (it is {k:.2}x)",
            criteria.min_spike
        ));
    }
    if e < criteria.min_projection {
        wants.push(format!(
            "the projection reaches {:.2}x (it is {e:.2}x)",
            criteria.min_projection
        ));
    }
    let reopens_at = if wants.is_empty() {
        "every criterion this ladder reads is already met".to_string()
    } else {
        format!(
            "a code generator for this workload reopens when {}",
            wants.join(", and ")
        )
    };

    let mut decision = Decision {
        verdict: Verdict::Defer,
        workload: WORKLOAD,
        interpreter_share: share,
        spike_speedup: k,
        projected: e,
        best_alternative: best_name,
        best_alternative_end_to_end: best_ratio,
        reasons,
        reopens_at,
        criteria: *criteria,
    };

    // C3's first clause, against the roster in [`LEVERS`] rather than against
    // whatever list the file carried: a report that mentions no alternative at
    // all has priced none of them, which is the strongest form of this failure
    // and used to be the one that read as success.
    let gaps = c3_gaps(alternatives);
    if !gaps.is_empty() {
        decision.reasons.push(format!(
            "{} of ADR 0016 §4's {} cheaper levers {} not priced, and a cheaper lever that has not \
             been priced is on its own a reason to keep deferring: {}",
            gaps.len(),
            LEVERS.len(),
            if gaps.len() == 1 { "is" } else { "are" },
            gaps.join("; ")
        ));
        // What C3 asks for, added to whatever else is unmet: the levers priced,
        // and the best of them no better than half M9's projected gain. When
        // the projection is at or below 1.00x there is no such number — nothing
        // an alternative could measure would let M9 through — and the sentence
        // says that rather than printing a bar of 1.00x.
        let priced = if e > 1.0 {
            format!(
                "the {} unpriced lever(s) in ADR 0016 §4 are priced and the best of them measures \
                 at or below {:.2}x end to end",
                gaps.len(),
                1.0 + (e - 1.0) / criteria.alternative_margin
            )
        } else {
            format!(
                "the {} unpriced lever(s) in ADR 0016 §4 are priced — though at a {e:.2}x \
                 projection no alternative's ratio would let M9 through",
                gaps.len()
            )
        };
        decision.reopens_at = if decision.reopens_at.starts_with("a code generator") {
            format!("{}, and {priced}", decision.reopens_at)
        } else {
            format!("a code generator for this workload reopens when {priced}")
        };
        return decision;
    }

    // Every criterion left reads the share, so a share whose own repeats fall
    // on both sides of a bar has not answered the criterion — it has answered
    // whichever run was taken. That is not `Defer`: the remedy is more repeats,
    // which is what `Undecided` means here. C3 is checked above it because C3
    // reads no share at all (§2.5).
    if let (Some(low), Some(high)) = (ladder.share_low, ladder.share_high) {
        for (bar, what) in [
            (criteria.min_share, "the share M9 needs"),
            (
                criteria.defer_share,
                "the share below which M9 is refused outright",
            ),
        ] {
            if low < bar && high >= bar {
                decision.verdict = Verdict::Undecided;
                decision.reasons.push(format!(
                    "the share runs {:.1}%–{:.1}% over its own repeats and {what} is {:.0}%: this \
                     ladder answers whichever run was taken, not the criterion",
                    low * 100.0,
                    high * 100.0,
                    bar * 100.0
                ));
                decision.reopens_at = format!(
                    "repeat the ladder until the share's band clears {:.0}%; nothing is decided \
                     while it straddles it",
                    bar * 100.0
                );
                return decision;
            }
        }
    }

    if share < criteria.defer_share {
        decision.reasons.push(format!(
            "{:.0}% is below the {:.0}% floor: even an infinitely fast backend is worth {:.2}x",
            share * 100.0,
            criteria.defer_share * 100.0,
            ceiling(share)
        ));
        return decision;
    }
    if k < criteria.defer_spike {
        decision.reasons.push(format!(
            "{k:.2}x is below the {:.1}x floor a spike must clear on its own function",
            criteria.defer_spike
        ));
        return decision;
    }
    if e < criteria.min_projection {
        decision.reasons.push(format!(
            "{e:.2}x end to end is below the {:.2}x a second execution path has to buy",
            criteria.min_projection
        ));
        return decision;
    }
    if let Some(alternative) = best
        && (e - 1.0) < criteria.alternative_margin * (alternative.end_to_end - 1.0)
    {
        decision.reasons.push(format!(
            "`{}` measured {:.2}x — {:.0}% — for one change; M9 projects {e:.2}x, {:.0}%, and has \
             to beat it {:.1}x over to be worth a permanent surface",
            alternative.name,
            alternative.end_to_end,
            (alternative.end_to_end - 1.0) * 100.0,
            (e - 1.0) * 100.0,
            criteria.alternative_margin
        ));
        return decision;
    }

    if share >= criteria.min_share && k >= criteria.min_spike {
        decision.verdict = Verdict::Advance;
        decision.reasons.push(format!(
            "share ≥ {:.0}%, spike ≥ {:.1}x, projection {e:.2}x, and every alternative priced \
             below it",
            criteria.min_share * 100.0,
            criteria.min_spike
        ));
        decision.reopens_at =
            "decided for this workload; scheduling it is a milestone with an ADR".to_string();
        return decision;
    }
    if k >= criteria.gray_spike {
        decision.verdict = Verdict::Conditional;
        decision.reasons.push(format!(
            "the share ({:.0}%) did not carry it, but the spike did: {k:.2}x is above the {:.1}x \
             the grey band demands, and it still projects {e:.2}x",
            share * 100.0,
            criteria.gray_spike
        ));
        decision.reopens_at =
            "conditional; the scope is the compiled fragment the spike proved, not a whole backend"
                .to_string();
        return decision;
    }

    decision.reasons.push(format!(
        "{:.0}% share with a {k:.2}x spike clears neither the {:.0}% bar nor the grey band's \
         {:.1}x",
        share * 100.0,
        criteria.min_share * 100.0,
        criteria.gray_spike
    ));
    decision
}

// ------------------------------------------------------- the honest account

/// What a reader gets from this language today, on one workload.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Offering {
    pub what: String,
    /// Which stack served it, spelled the way a reader could reproduce.
    pub stack: String,
    pub head_bytes: usize,
    pub concurrency: u32,
    pub per_second: f64,
    pub p50_micros: f64,
    pub p99_micros: f64,
    /// The same workload against the Rust floor on the same machine. `None`
    /// where no floor was taken — printed as such rather than as a multiple the
    /// report does not have.
    pub floor_per_second: Option<f64>,
}

impl Offering {
    pub fn multiple(&self) -> Option<f64> {
        let floor = self.floor_per_second?;
        (self.per_second > 0.0).then(|| floor / self.per_second)
    }
}

/// Somewhere this language is genuinely not competitive, and why.
///
/// A required section. An honest ceiling stated plainly is more useful than a
/// flattering one, and a report with no limits has not looked.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Limit {
    pub what: String,
    pub why: String,
    /// The number that shows it, where one was taken.
    pub evidence: Option<String>,
}

/// Where a number came from. Without this a table is a rumour.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Provenance {
    pub machine: String,
    /// `release`, always, and stated so a debug run cannot be mistaken for one.
    pub profile: String,
    pub taken: String,
    pub repeats: usize,
    pub request_head_bytes: usize,
    pub postgres: Option<String>,
    /// What was not measured, and why. An empty vector is itself an audit
    /// finding: every run leaves something out.
    pub not_measured: Vec<String>,
}

/// Everything W6 owes, as one value.
///
/// It carries measurements and nothing else — no verdict and no criteria — so
/// the decision is recomputed by [`decide`] from pinned thresholds every time
/// the report is rendered.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Report {
    pub provenance: Provenance,
    pub floor_micros: f64,
    pub total_micros: f64,
    /// What the floor answered and what the total served, so the multiple
    /// between them is readable rather than assumed to be like for like.
    #[serde(default)]
    pub denominators: Denominators,
    pub points: Vec<Point>,
    #[serde(default)]
    pub engines: Vec<EnginePoint>,
    #[serde(default)]
    pub spike: Option<Spike>,
    #[serde(default)]
    pub alternatives: Vec<Alternative>,
    #[serde(default)]
    pub offerings: Vec<Offering>,
    #[serde(default)]
    pub limits: Vec<Limit>,
}

impl Report {
    pub fn ladder(&self) -> Result<Ladder> {
        Ladder::assemble_with(
            self.floor_micros,
            self.total_micros,
            &self.points,
            &self.denominators,
        )
    }

    pub fn decision(&self, ladder: &Ladder) -> Decision {
        decide(
            ladder,
            self.spike.as_ref(),
            &self.alternatives,
            &Criteria::default(),
        )
    }

    /// What the report owes and does not have.
    ///
    /// W6 closes the web track, so the sections a reader needs are not
    /// optional: a missing one is a finding printed above the tables, not a
    /// shorter document.
    pub fn audit(&self) -> Vec<String> {
        let mut findings = Vec::new();
        match self.ladder() {
            Ok(ladder) => {
                for layer in ladder.missing() {
                    findings.push(format!(
                        "no `{}` rung: {}",
                        layer.label(),
                        layer.substitution()
                    ));
                }
                for window in ladder.rungs.windows(2) {
                    let (below, above) = (&window[0], &window[1]);
                    if below.taken_on != above.taken_on {
                        findings.push(format!(
                            "the `{}` rung is taken on `{}` and `{}` on `{}`, so their difference \
                             is a route change as well as a layer",
                            below.label, below.taken_on, above.label, above.taken_on
                        ));
                    }
                }
                for rung in &ladder.rungs {
                    if rung.layer_micros < 0.0 {
                        findings.push(format!(
                            "the `{}` layer is {:.2}µs — negative, so `{}` did not isolate it",
                            rung.label, rung.layer_micros, rung.substitution
                        ));
                    }
                    if rung.layer_low_micros.is_none() {
                        findings.push(format!(
                            "the `{}` rung was taken once: a layer is a difference between two \
                             numbers, and one sample of each says nothing about how much of the \
                             difference is the layer",
                            rung.label
                        ));
                    } else if rung.sign_unresolved() {
                        findings.push(format!(
                            "the `{}` layer is {:.2}µs but its repeats run {:.2}µs to {:.2}µs, so \
                             the measurement did not resolve its sign — the qualitative reading \
                             may survive and the printed number does not",
                            rung.label,
                            rung.layer_micros,
                            rung.layer_low_micros.unwrap_or_default(),
                            rung.layer_high_micros.unwrap_or_default()
                        ));
                    }
                }
                if !ladder.telescopes {
                    findings.push(
                        "the interpreter rungs do not chain — one rung's `without` is not the rung \
                         below's `with` — so their sum is five differences added up rather than an \
                         absolute anybody measured"
                            .to_string(),
                    );
                }
                if ladder.residue_micros < 0.0 {
                    findings.push(format!(
                        "the residue is {:.2}µs ({:.1}%): the layers sum to more than the request \
                         they are read against, so the in-process arena over-counts against the \
                         served one and the share is not a lower bound. It is charged back — the \
                         decision reads {:.1}% and not {:.1}%",
                        ladder.residue_micros,
                        ladder.residue_share * 100.0,
                        ladder.conservative_share * 100.0,
                        ladder.interpreter_share * 100.0
                    ));
                }
                if ladder.denominators.floor_taken_on.trim().is_empty()
                    || ladder.denominators.total_taken_on.trim().is_empty()
                {
                    findings.push(format!(
                        "the report says a request is {:.1}x its floor and does not say what \
                         either side answered; a multiple whose numerator and denominator do \
                         different work is not a multiple",
                        ladder.over_floor
                    ));
                }
            }
            Err(e) => findings.push(format!("the ladder does not assemble: {e}")),
        }
        if self.engines.len() < 2 {
            findings.push(
                "the engine substitution is missing: one interpreter against another is the \
                 cheapest bound on how much of a request is dispatch, and both engines already \
                 exist"
                    .to_string(),
            );
        }
        if self.spike.is_none() {
            findings.push(
                "no codegen spike: the speedup a backend would deliver is the number M9 turns on"
                    .to_string(),
            );
        }
        findings.extend(c3_gaps(&self.alternatives));
        for alternative in &self.alternatives {
            if alternative.lever().is_none() {
                findings.push(format!(
                    "`{}` answers for no lever in ADR 0016 §4, so nothing C3 asks about is priced \
                     by it",
                    alternative.name
                ));
            }
        }
        if self.offerings.is_empty() {
            findings.push(
                "no offering: a reader cannot tell what this language serves today".to_string(),
            );
        }
        if self.limits.is_empty() {
            findings.push(
                "no limits: a report that names nowhere this language is uncompetitive has not \
                 looked"
                    .to_string(),
            );
        }
        if self.provenance.not_measured.is_empty() {
            findings.push(
                "`not_measured` is empty: every run leaves something out, and the omission is \
                 the reader's to judge"
                    .to_string(),
            );
        }
        findings
    }
}

// ------------------------------------------------------------------ render

pub fn render(report: &Report) -> String {
    let mut s = String::new();

    let ladder = match report.ladder() {
        Ok(ladder) => ladder,
        Err(e) => {
            s.push_str(&format!("the ladder does not assemble: {e}\n"));
            return s;
        }
    };

    let p = &report.provenance;
    s.push_str(&format!(
        "W6 — {} · {} · {} · best of {} · a {}-byte head{}\n\n",
        p.machine,
        p.profile,
        p.taken,
        p.repeats,
        p.request_head_bytes,
        p.postgres
            .as_deref()
            .map(|v| format!(" · {v}"))
            .unwrap_or_default()
    ));

    s.push_str("the accumulated stack — every rung is one substitution, measured both ways\n");
    s.push_str(&format!(
        "  {:<10} {:>9} {:>9} {:>9} {:>17} {:>7} {:<11} {:<18} {}\n",
        "layer",
        "with µs",
        "without",
        "layer µs",
        "over repeats",
        "share",
        "arena",
        "taken on",
        "what the layer is"
    ));
    for rung in &ladder.rungs {
        s.push_str(&format!(
            "  {:<10} {:>9.2} {:>9.2} {:>9.2} {:>17} {:>6.1}% {:<11} {:<18} {}\n",
            rung.label,
            rung.with_micros,
            rung.without_micros,
            rung.layer_micros,
            match (rung.layer_low_micros, rung.layer_high_micros) {
                (Some(low), Some(high)) => format!("{low:.2}..{high:.2}"),
                _ => "one sample".to_string(),
            },
            rung.layer_share * 100.0,
            match rung.arena {
                Arena::InProcess => "in process",
                Arena::Served => "served",
            },
            rung.taken_on,
            rung.isolates
        ));
    }
    s.push_str(&format!(
        "  {:<10} {:>9} {:>9} {:>9.2} {:>17} {:>6.1}% {:<11} {:<18} everything no substitution separated\n",
        "residue", "", "", ladder.residue_micros, "", ladder.residue_share * 100.0, "", ""
    ));
    s.push_str(&format!(
        "  {:<10} {:>9} {:>9} {:>9.2} {:>17} {:>6.1}% {:<11} {:<18} measured end to end, not a sum\n",
        "TOTAL",
        "",
        "",
        ladder.total_micros,
        ladder
            .denominators
            .total_worst_micros
            .map(|worst| format!("{:.2}..{worst:.2}", ladder.total_micros))
            .unwrap_or_else(|| "one sample".to_string()),
        100.0,
        "",
        ""
    ));
    s.push_str(&format!(
        "\n  a request costs {:.0}x the {:.2}µs floor\n",
        ladder.over_floor, ladder.floor_micros
    ));
    if !ladder.denominators.floor_taken_on.trim().is_empty() {
        s.push_str(&format!(
            "    the floor: {}\n    the total: {}\n",
            ladder.denominators.floor_taken_on, ladder.denominators.total_taken_on
        ));
    }
    if ladder.residue_micros < 0.0 {
        s.push_str(&format!(
            "  {:.0}% of it is interpreter once the {:.1}µs negative residue is charged back to \
             the arena that produced it ({:.0}% as attributed)\n",
            ladder.conservative_share * 100.0,
            ladder.residue_micros,
            ladder.interpreter_share * 100.0
        ));
    } else {
        s.push_str(&format!(
            "  {:.0}% of it is interpreter, a lower bound: the residue is not credited to it\n",
            ladder.interpreter_share * 100.0
        ));
    }
    if let (Some(low), Some(high)) = (ladder.share_low, ladder.share_high) {
        s.push_str(&format!(
            "  over the repeats that share runs {:.1}%–{:.1}%\n",
            low * 100.0,
            high * 100.0
        ));
    }
    s.push('\n');

    if !report.engines.is_empty() {
        s.push_str("one interpreter against another — the cheapest bound on dispatch cost\n");
        s.push_str(&format!(
            "  {:<12} {:>10} {:>10}\n",
            "engine", "µs/req", "reqs"
        ));
        for point in &report.engines {
            s.push_str(&format!(
                "  {:<12} {:>10.2} {:>10}\n",
                point.engine, point.per_request_micros, point.requests
            ));
        }
        if let Some((ratio, fast, slow)) = engine_spread(&report.engines) {
            s.push_str(&format!(
                "  swapping the whole evaluator moves a request {ratio:.2}x ({slow} over {fast})\n"
            ));
        }
        s.push('\n');
    }

    if let Some(spike) = &report.spike {
        let judged = spike.judge();
        s.push_str(&format!(
            "the codegen spike — `{}`, {} nodes, compiled once in {:.1}µs\n",
            spike.function, spike.nodes, spike.compile_micros
        ));
        s.push_str(&format!("  chosen because {}\n", spike.chosen_because));
        s.push_str(&format!(
            "  {:<18} {:>11} {:>11} {:>7} {:>7} {:>8}\n",
            "input", "interp best", "spike worst", "cons.", "optim.", "agreed"
        ));
        for input in &spike.inputs {
            s.push_str(&format!(
                "  {:<18} {:>11.3} {:>11.3} {:>6.2}x {:>6.2}x {:>8}\n",
                input.name,
                input.interpreter_best_micros,
                input.spike_worst_micros,
                input.conservative(),
                input.optimistic(),
                if input.agreed { "yes" } else { "NO" }
            ));
        }
        if judged.evidence {
            s.push_str(&format!(
                "  evidence: {:.2}x on the weakest input\n\n",
                judged.speedup
            ));
        } else {
            s.push_str("  NOT evidence:\n");
            for failure in &judged.failures {
                s.push_str(&format!("    - {failure}\n"));
            }
            s.push('\n');
        }
    }

    if !report.alternatives.is_empty() {
        s.push_str("the cheaper levers, priced alongside\n");
        s.push_str(&format!(
            "  {:<26} {:>9}  {}\n",
            "lever", "end-to-end", "what it is"
        ));
        for alternative in &report.alternatives {
            s.push_str(&format!(
                "  {:<26} {:>9}  {}\n",
                alternative.name,
                if alternative.is_priced() {
                    format!("{:.2}x", alternative.end_to_end)
                } else {
                    "unpriced".to_string()
                },
                alternative.what
            ));
            if alternative.is_priced() {
                s.push_str(&format!(
                    "  {:<26} {:>9}  {}\n",
                    "", "", alternative.evidence
                ));
            }
        }
        s.push('\n');
    }

    if !report.offerings.is_empty() {
        s.push_str("what this language serves today\n");
        s.push_str(&format!(
            "  {:<24} {:<22} {:>5} {:>6} {:>10} {:>9} {:>9} {:>9}\n",
            "workload", "stack", "head", "conns", "req/s", "p50 µs", "p99 µs", "vs floor"
        ));
        for offering in &report.offerings {
            s.push_str(&format!(
                "  {:<24} {:<22} {:>5} {:>6} {:>10.0} {:>9.0} {:>9.0} {:>9}\n",
                offering.what,
                offering.stack,
                offering.head_bytes,
                offering.concurrency,
                offering.per_second,
                offering.p50_micros,
                offering.p99_micros,
                offering
                    .multiple()
                    .map(|m| format!("{m:.0}x"))
                    .unwrap_or_else(|| "—".to_string())
            ));
        }
        s.push('\n');
    }

    if !report.limits.is_empty() {
        s.push_str("where this is genuinely not competitive\n");
        for limit in &report.limits {
            s.push_str(&format!("  - {}: {}\n", limit.what, limit.why));
            if let Some(evidence) = &limit.evidence {
                s.push_str(&format!("      {evidence}\n"));
            }
        }
        s.push('\n');
    }

    if !p.not_measured.is_empty() {
        s.push_str("what W6 did not measure\n");
        for note in &p.not_measured {
            s.push_str(&format!("  - {note}\n"));
        }
        s.push('\n');
    }

    let decision = report.decision(&ladder);
    s.push_str(&format!(
        "verdict: {}\n  workload: {}\n",
        decision.verdict.label(),
        decision.workload
    ));
    for reason in &decision.reasons {
        s.push_str(&format!("  - {reason}\n"));
    }
    s.push_str(&format!("  {}\n", decision.reopens_at));

    let findings = report.audit();
    if !findings.is_empty() {
        s.push_str("\nthis report is incomplete\n");
        for finding in &findings {
            s.push_str(&format!("  - {finding}\n"));
        }
    }

    s
}

/// The rendered report and the decision, for `--json`.
#[derive(Clone, Debug, Serialize)]
pub struct Rendered<'a> {
    pub report: &'a Report,
    pub ladder: Ladder,
    pub spike: Option<SpikeVerdict>,
    pub decision: Decision,
    pub audit: Vec<String>,
}

pub fn rendered(report: &Report) -> Result<Rendered<'_>> {
    let ladder = report.ladder()?;
    let decision = report.decision(&ladder);
    Ok(Rendered {
        spike: report.spike.as_ref().map(Spike::judge),
        audit: report.audit(),
        ladder,
        decision,
        report,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn point(layer: Layer, with: f64, without: f64) -> Point {
        Point {
            layer,
            taken_on: "/items".to_string(),
            with_micros: with,
            without_micros: without,
            // A 1% spread on every rung, so a band exists to be reasoned about
            // and no test depends on one having been taken once.
            with_worst_micros: Some(with * 1.01),
            without_worst_micros: Some(without * 1.01),
            requests: 1000,
        }
    }

    /// One rung per layer, summing to 100µs of a 120µs request: 60µs of
    /// interpreter (50%), 40µs of host, 20µs of residue.
    fn full_points() -> Vec<Point> {
        vec![
            point(Layer::Call, 5.0, 0.0),
            point(Layer::Endpoint, 20.0, 5.0),
            point(Layer::Framing, 45.0, 20.0),
            point(Layer::Routing, 55.0, 45.0),
            point(Layer::Machine, 60.0, 55.0),
            point(Layer::Socket, 80.0, 60.0),
            point(Layer::Tls, 90.0, 80.0),
            point(Layer::Database, 95.0, 90.0),
            point(Layer::Tracing, 100.0, 95.0),
        ]
    }

    fn spike(speedup: f64) -> Spike {
        Spike {
            function: "std.http::read_line".to_string(),
            chosen_because: "the stage table's hottest pure function".to_string(),
            nodes: 41,
            compile_micros: 900.0,
            inputs: (0..3)
                .map(|i| SpikeInput {
                    name: format!("head-{i}"),
                    interpreter_best_micros: 10.0,
                    interpreter_worst_micros: 12.0,
                    spike_best_micros: 10.0 / speedup * 0.9,
                    spike_worst_micros: 10.0 / speedup,
                    agreed: true,
                })
                .collect(),
        }
    }

    fn priced(name: &str, ratio: f64) -> Alternative {
        Alternative {
            name: name.to_string(),
            what: "a change".to_string(),
            priced: true,
            end_to_end: ratio,
            evidence: "the served workload with the change and without it".to_string(),
            cost: "one change".to_string(),
        }
    }

    /// Every §4 lever priced, with `best` the ratio of the best of them. C3's
    /// first clause is about the roster rather than about a list, so a test
    /// that wants to reach C1, C2 or C4 has to answer for all seven.
    fn roster(best: f64) -> Vec<Alternative> {
        LEVERS
            .iter()
            .map(|lever| {
                priced(
                    lever.name,
                    if lever.name == "Env::lookup" {
                        best
                    } else {
                        1.0
                    },
                )
            })
            .collect()
    }

    #[test]
    fn every_layer_is_in_the_order_and_carries_its_prose() {
        assert_eq!(Layer::ORDER.len(), 9);
        for (i, layer) in Layer::ORDER.into_iter().enumerate() {
            assert_eq!(layer.rank(), i);
            assert!(!layer.label().is_empty());
            assert!(!layer.isolates().is_empty());
            assert!(!layer.substitution().is_empty());
        }
        let interpreter: Vec<&str> = Layer::ORDER
            .into_iter()
            .filter(|l| l.is_interpreter())
            .map(Layer::label)
            .collect();
        assert_eq!(
            interpreter,
            ["call", "endpoint", "framing", "routing", "machine"]
        );
    }

    /// The residue is the whole point of the table: a ladder that attributed
    /// everything would be hiding what it did not separate.
    #[test]
    fn a_ladder_reports_its_layers_its_residue_and_a_lower_bound_share() {
        let ladder = Ladder::assemble(4.0, 120.0, &full_points()).unwrap();
        assert_eq!(ladder.rungs.len(), 9);
        assert!((ladder.attributed_micros - 100.0).abs() < 1e-9);
        assert!((ladder.residue_micros - 20.0).abs() < 1e-9);
        assert!((ladder.interpreter_micros - 60.0).abs() < 1e-9);
        assert!((ladder.interpreter_share - 0.5).abs() < 1e-9);
        assert!((ladder.over_floor - 30.0).abs() < 1e-9);
        assert!(ladder.missing().is_empty());
    }

    #[test]
    fn a_ladder_refuses_what_a_share_cannot_be_read_off() {
        let mut duplicated = full_points();
        duplicated.push(point(Layer::Tracing, 105.0, 100.0));
        let err = Ladder::assemble(4.0, 120.0, &duplicated)
            .unwrap_err()
            .to_string();
        assert!(err.contains("twice"), "{err}");

        let out_of_order = vec![
            point(Layer::Framing, 45.0, 20.0),
            point(Layer::Endpoint, 20.0, 5.0),
        ];
        let err = Ladder::assemble(4.0, 120.0, &out_of_order)
            .unwrap_err()
            .to_string();
        assert!(err.contains("request order"), "{err}");

        assert!(Ladder::assemble(4.0, 0.0, &full_points()).is_err());
        assert!(Ladder::assemble(0.0, 120.0, &full_points()).is_err());
        assert!(Ladder::assemble(4.0, 120.0, &[]).is_err());

        let zero = vec![Point {
            requests: 0,
            ..point(Layer::Call, 5.0, 0.0)
        }];
        assert!(Ladder::assemble(4.0, 120.0, &zero).is_err());

        let anonymous = vec![Point {
            taken_on: String::new(),
            ..point(Layer::Call, 5.0, 0.0)
        }];
        let err = Ladder::assemble(4.0, 120.0, &anonymous)
            .unwrap_err()
            .to_string();
        assert!(err.contains("names no route"), "{err}");
    }

    /// Two rungs on two routes have a difference that is not one layer. It is
    /// sometimes the only measurement available — a `database` rung needs a
    /// route with a database — so it is disclosed rather than refused.
    #[test]
    fn a_route_change_between_two_rungs_is_an_audit_finding() {
        let mut points = full_points();
        points[1].taken_on = "/health".to_string();
        let report = report(points);
        let findings = report.audit();
        assert!(
            findings
                .iter()
                .any(|f| f.contains("`/health`") && f.contains("route change")),
            "{findings:?}"
        );
    }

    #[test]
    fn a_negative_layer_is_reported_rather_than_clamped() {
        let mut points = full_points();
        points[6] = point(Layer::Tls, 74.0, 80.0);
        let ladder = Ladder::assemble(4.0, 120.0, &points).unwrap();
        assert!(ladder.rungs[6].layer_micros < 0.0);
        assert!((ladder.worst_negative_share - 6.0 / 120.0).abs() < 1e-9);
    }

    /// A layer whose repeats span zero has not measured its own sign, and two
    /// decimals of it are two decimals of the machine it ran on.
    #[test]
    fn a_layer_narrower_than_its_own_repeats_is_named_rather_than_printed() {
        let mut points = full_points();
        points[8] = Point {
            with_micros: 100.0,
            without_micros: 99.0,
            with_worst_micros: Some(112.0),
            without_worst_micros: Some(110.0),
            ..point(Layer::Tracing, 100.0, 99.0)
        };
        let ladder = Ladder::assemble(4.0, 120.0, &points).unwrap();
        let tracing = ladder.rungs.last().unwrap();
        assert!((tracing.layer_micros - 1.0).abs() < 1e-9);
        assert!(tracing.sign_unresolved(), "{tracing:?}");
        let mut report = report(points);
        report.total_micros = 120.0;
        assert!(
            report
                .audit()
                .iter()
                .any(|f| f.contains("did not resolve its sign")),
            "{:?}",
            report.audit()
        );
    }

    /// A negative residue is the layers summing to more than the request they
    /// were read against, which can only be the in-process arena over-counting.
    /// It is charged to the interpreter rather than to nobody, so it can never
    /// flatter the share M9's case rests on.
    #[test]
    fn a_negative_residue_is_charged_to_the_share_the_decision_reads() {
        let mut points = full_points();
        // 140µs of layers against a 120µs request: a −20µs residue.
        points[7] = point(Layer::Database, 125.0, 90.0);
        points[8] = point(Layer::Tracing, 130.0, 125.0);
        let ladder = Ladder::assemble(4.0, 120.0, &points).unwrap();
        assert!(ladder.residue_micros < 0.0);
        assert!((ladder.interpreter_share - 0.5).abs() < 1e-9);
        assert!(
            ladder.conservative_share < ladder.interpreter_share,
            "{:?} against {:?}",
            ladder.conservative_share,
            ladder.interpreter_share
        );
        let decision = decide(
            &ladder,
            Some(&spike(9.0)),
            &roster(1.0),
            &Criteria::default(),
        );
        assert!(
            (decision.interpreter_share - ladder.conservative_share).abs() < 1e-9,
            "the decision read {:.3} and the conservative share is {:.3}",
            decision.interpreter_share,
            ladder.conservative_share
        );

        // And the other direction: a positive residue is credited to nobody, so
        // the share stays exactly what the rungs attributed.
        let positive = Ladder::assemble(4.0, 120.0, &full_points()).unwrap();
        assert!(positive.residue_micros > 0.0);
        assert!((positive.conservative_share - positive.interpreter_share).abs() < 1e-9);
    }

    /// The share is one number read off one run, and M9's whole case is on
    /// which side of 50% it falls. A band that falls on both sides has not
    /// answered that, and answering it anyway is answering with a run.
    #[test]
    fn a_share_whose_repeats_straddle_the_bar_decides_nothing() {
        let mut points = full_points();
        points[4] = Point {
            with_worst_micros: Some(66.0),
            ..point(Layer::Machine, 60.0, 55.0)
        };
        let straddling = Ladder::assemble_with(
            4.0,
            120.0,
            &points,
            &Denominators {
                total_worst_micros: Some(132.0),
                ..Denominators::default()
            },
        )
        .unwrap();
        assert!(straddling.share_low.unwrap() < 0.50);
        assert!(straddling.share_high.unwrap() >= 0.50);
        let decision = decide(
            &straddling,
            Some(&spike(3.0)),
            &roster(1.0),
            &Criteria::default(),
        );
        assert_eq!(
            decision.verdict,
            Verdict::Undecided,
            "{:?}",
            decision.reasons
        );
        assert!(decision.reopens_at.contains("repeat the ladder"));

        // C3 is checked before it, because C3 reads no share: an unpriced lever
        // defers whatever the band does.
        let deferred = decide(&straddling, Some(&spike(3.0)), &[], &Criteria::default());
        assert_eq!(deferred.verdict, Verdict::Defer);
    }

    #[test]
    fn amdahl_is_the_projection_and_the_ceiling_is_its_limit() {
        assert!((projected(0.5, 3.0) - 1.5).abs() < 1e-9);
        assert!((projected(1.0, 4.0) - 4.0).abs() < 1e-9);
        assert!((projected(0.0, 100.0) - 1.0).abs() < 1e-9);
        assert!((projected(0.3, 1.0) - 1.0).abs() < 1e-9);
        assert!((ceiling(0.5) - 2.0).abs() < 1e-9);
        assert!((ceiling(0.35) - 1.5384615).abs() < 1e-6);
    }

    /// A speedup is the weakest input's, and a disagreement or an overlap is
    /// not a slower speedup — it is no measurement at all.
    #[test]
    fn a_spike_is_evidence_only_when_it_agreed_and_separated_on_enough_inputs() {
        let good = spike(4.0);
        let judged = good.judge();
        assert!(judged.evidence, "{:?}", judged.failures);
        assert!((judged.speedup - 4.0).abs() < 1e-9);

        let mut wrong = spike(4.0);
        wrong.inputs[1].agreed = false;
        assert!(!wrong.judge().evidence);
        assert!(wrong.judge().failures[0].contains("disagreed"));

        let mut overlapping = spike(4.0);
        overlapping.inputs[2].spike_worst_micros = 11.0;
        assert!(!overlapping.judge().evidence);
        assert!(overlapping.judge().failures[0].contains("overlap"));

        let mut thin = spike(4.0);
        thin.inputs.truncate(2);
        assert!(!thin.judge().evidence);

        let mut uneven = spike(4.0);
        uneven.inputs[0].spike_worst_micros = 9.0;
        assert!((uneven.judge().speedup - 10.0 / 9.0).abs() < 1e-9);
    }

    #[test]
    fn a_missing_rung_or_a_missing_spike_is_undecided_rather_than_deferred() {
        let partial = Ladder::assemble(4.0, 120.0, &full_points()[..4]).unwrap();
        let decision = decide(&partial, Some(&spike(4.0)), &[], &Criteria::default());
        assert_eq!(decision.verdict, Verdict::Undecided);
        assert!(
            decision.reasons[0].contains("no `machine`, `socket`, `tls`, `database`, `tracing`"),
            "{:?}",
            decision.reasons
        );

        let full = Ladder::assemble(4.0, 120.0, &full_points()).unwrap();
        let decision = decide(&full, None, &[], &Criteria::default());
        assert_eq!(decision.verdict, Verdict::Undecided);
        assert!(decision.reasons[0].contains("no codegen spike"));

        let mut points = full_points();
        points[6] = point(Layer::Tls, 60.0, 80.0);
        let bent = Ladder::assemble(4.0, 120.0, &points).unwrap();
        let decision = decide(&bent, Some(&spike(4.0)), &[], &Criteria::default());
        assert_eq!(decision.verdict, Verdict::Undecided);
        assert!(decision.reasons[0].contains("negative"));
    }

    /// ADR 0026 §4.2, in code: the ladder answers about what it measured.
    ///
    /// Nothing about the thresholds moved and nothing about the arithmetic
    /// moved; a run over the shipped files reads the same share, the same
    /// projection and the same verdict it always did. What may not survive is
    /// the *claim*: a nine-rung HTTP instrument that prints "keep deferring M9"
    /// has answered a question about the language, and `Ladder::missing` refuses
    /// every workload but this one — so it cannot have.
    ///
    /// Seen to fail: restoring `Verdict::label`'s "keep deferring M9" turns this
    /// red on the first assertion, and dropping `Decision::workload` from the
    /// rendered sentence turns it red on the second.
    #[test]
    fn a_verdict_names_the_workload_it_was_taken_on_and_never_names_a_milestone() {
        let full = report(full_points());
        let rendered = rendered(&full).unwrap();
        assert_eq!(rendered.decision.workload, WORKLOAD);
        for verdict in [
            Verdict::Advance,
            Verdict::Conditional,
            Verdict::Defer,
            Verdict::Undecided,
        ] {
            assert!(
                !verdict.label().contains("M9"),
                "`{}` names a milestone; the ladder decides a workload",
                verdict.label()
            );
        }
        assert!(
            !rendered.decision.reopens_at.contains("M9"),
            "the reopen sentence names a milestone: {}",
            rendered.decision.reopens_at
        );

        let text = render(&full);
        assert!(
            text.contains("workload: the served HTTP workload"),
            "the rendered report does not say what its share was taken on:\n{text}"
        );
        assert!(
            !text.contains("M9:"),
            "the rendered report still heads its verdict with a milestone:\n{text}"
        );
    }

    #[test]
    fn an_unpriced_alternative_defers_whatever_the_share_says() {
        let ladder = Ladder::assemble(4.0, 120.0, &full_points()).unwrap();
        let mut alternatives = roster(1.1);
        alternatives[1].priced = false;
        alternatives[1].end_to_end = 0.0;
        let decision = decide(
            &ladder,
            Some(&spike(9.0)),
            &alternatives,
            &Criteria::default(),
        );
        assert_eq!(decision.verdict, Verdict::Defer);
        assert!(
            decision.reasons.iter().any(|r| r.contains("not priced")),
            "{:?}",
            decision.reasons
        );
        // The share and the spike both clear their bars here, so what reopens
        // M9 is the pricing and not either of them: 1 + (1.80 − 1)/2.
        assert!(
            decision.reopens_at.contains("1.40x end to end"),
            "{}",
            decision.reopens_at
        );
    }

    #[test]
    fn an_unpriced_alternative_under_a_small_share_still_names_the_share() {
        let points = vec![
            point(Layer::Call, 1.0, 0.0),
            point(Layer::Endpoint, 4.0, 1.0),
            point(Layer::Framing, 9.0, 4.0),
            point(Layer::Routing, 11.0, 9.0),
            point(Layer::Machine, 12.0, 11.0),
            point(Layer::Socket, 32.0, 12.0),
            point(Layer::Tls, 52.0, 32.0),
            point(Layer::Database, 96.0, 52.0),
            point(Layer::Tracing, 100.0, 96.0),
        ];
        let ladder = Ladder::assemble(4.0, 120.0, &points).unwrap();
        let mut alternatives = roster(1.1);
        alternatives[1].priced = false;
        alternatives[1].end_to_end = 0.0;
        let decision = decide(
            &ladder,
            Some(&spike(9.0)),
            &alternatives,
            &Criteria::default(),
        );
        assert_eq!(decision.verdict, Verdict::Defer);
        assert!(
            decision.reopens_at.contains("reopens"),
            "{}",
            decision.reopens_at
        );
    }

    /// **C3 is checked against ADR 0016 §4, not against the file.** A report
    /// that carries no alternatives at all has priced none of the seven, and
    /// the roster is in code so that deleting the field cannot say otherwise.
    #[test]
    fn a_report_that_prices_no_lever_at_all_defers_and_names_all_seven() {
        let ladder = Ladder::assemble(4.0, 120.0, &full_points()).unwrap();
        let decision = decide(&ladder, Some(&spike(9.0)), &[], &Criteria::default());
        assert_eq!(
            decision.verdict,
            Verdict::Defer,
            "an empty list prices nothing: {:?}",
            decision.reasons
        );
        for lever in &LEVERS {
            assert!(
                decision.reasons.iter().any(|r| r.contains(lever.name)),
                "`{}` is unmentioned in {:?}",
                lever.name,
                decision.reasons
            );
        }
        assert_eq!(c3_gaps(&[]).len(), LEVERS.len());
    }

    /// The same hole through the values rather than through the field: a lever
    /// may be claimed as priced, but a claim with nothing behind it is not a
    /// measurement and does not answer C3.
    #[test]
    fn a_lever_priced_without_evidence_is_not_priced() {
        let ladder = Ladder::assemble(4.0, 120.0, &full_points()).unwrap();
        let mut claimed = roster(1.0);
        for alternative in &mut claimed {
            alternative.evidence.clear();
        }
        let decision = decide(&ladder, Some(&spike(9.0)), &claimed, &Criteria::default());
        assert_eq!(decision.verdict, Verdict::Defer, "{:?}", decision.reasons);
        assert!(
            decision.reasons.iter().any(|r| r.contains("evidence")),
            "{:?}",
            decision.reasons
        );
        assert!(Alternative::best(&claimed).is_none());
    }

    #[test]
    fn a_half_interpreter_request_with_a_three_times_spike_advances() {
        let ladder = Ladder::assemble(4.0, 120.0, &full_points()).unwrap();
        let decision = decide(
            &ladder,
            Some(&spike(3.0)),
            &roster(1.1),
            &Criteria::default(),
        );
        assert_eq!(decision.verdict, Verdict::Advance, "{:?}", decision.reasons);
        assert!((decision.projected - 1.5).abs() < 1e-9);
    }

    #[test]
    fn a_small_share_defers_and_names_the_ceiling() {
        // 12µs of interpreter in a 120µs request: 10%.
        let points = vec![
            point(Layer::Call, 1.0, 0.0),
            point(Layer::Endpoint, 4.0, 1.0),
            point(Layer::Framing, 9.0, 4.0),
            point(Layer::Routing, 11.0, 9.0),
            point(Layer::Machine, 12.0, 11.0),
            point(Layer::Socket, 32.0, 12.0),
            point(Layer::Tls, 52.0, 32.0),
            point(Layer::Database, 96.0, 52.0),
            point(Layer::Tracing, 100.0, 96.0),
        ];
        let ladder = Ladder::assemble(4.0, 120.0, &points).unwrap();
        let decision = decide(
            &ladder,
            Some(&spike(20.0)),
            &roster(1.02),
            &Criteria::default(),
        );
        assert_eq!(decision.verdict, Verdict::Defer);
        assert!(
            decision
                .reasons
                .iter()
                .any(|r| r.contains("infinitely fast")),
            "{:?}",
            decision.reasons
        );
        assert!(decision.reopens_at.contains("reopens"));
    }

    #[test]
    fn a_cheaper_lever_within_half_of_the_projection_defers() {
        let ladder = Ladder::assemble(4.0, 120.0, &full_points()).unwrap();
        let decision = decide(
            &ladder,
            Some(&spike(3.0)),
            &roster(1.4),
            &Criteria::default(),
        );
        assert_eq!(decision.verdict, Verdict::Defer);
        assert!(
            decision
                .reasons
                .iter()
                .any(|r| r.contains("permanent surface")),
            "{:?}",
            decision.reasons
        );
    }

    #[test]
    fn the_grey_band_needs_a_much_better_spike() {
        // 48µs of 120µs is 40%: above the defer floor, below the advance bar.
        let points = vec![
            point(Layer::Call, 4.0, 0.0),
            point(Layer::Endpoint, 16.0, 4.0),
            point(Layer::Framing, 36.0, 16.0),
            point(Layer::Routing, 44.0, 36.0),
            point(Layer::Machine, 48.0, 44.0),
            point(Layer::Socket, 68.0, 48.0),
            point(Layer::Tls, 78.0, 68.0),
            point(Layer::Database, 92.0, 78.0),
            point(Layer::Tracing, 100.0, 92.0),
        ];
        let ladder = Ladder::assemble(4.0, 120.0, &points).unwrap();
        assert!((ladder.interpreter_share - 0.4).abs() < 1e-9);

        let modest = decide(
            &ladder,
            Some(&spike(3.0)),
            &roster(1.05),
            &Criteria::default(),
        );
        assert_eq!(modest.verdict, Verdict::Defer);

        let strong = decide(
            &ladder,
            Some(&spike(8.0)),
            &roster(1.05),
            &Criteria::default(),
        );
        assert_eq!(strong.verdict, Verdict::Conditional, "{:?}", strong.reasons);
    }

    fn report(points: Vec<Point>) -> Report {
        Report {
            provenance: Provenance {
                machine: "an M-series laptop".to_string(),
                profile: "release".to_string(),
                taken: "2026-08-15".to_string(),
                repeats: 3,
                request_head_bytes: 63,
                postgres: Some("postgres 17 on 5433".to_string()),
                not_measured: vec!["cancellation, because W5 has none".to_string()],
            },
            floor_micros: 4.0,
            total_micros: 120.0,
            denominators: Denominators {
                floor_taken_on: "the same bytes over plaintext with no interpreter".to_string(),
                total_taken_on: "/items over postgres over TLS".to_string(),
                total_worst_micros: Some(120.0),
            },
            points,
            engines: vec![
                EnginePoint {
                    engine: "treewalk".to_string(),
                    per_request_micros: 110.0,
                    requests: 1000,
                },
                EnginePoint {
                    engine: "machine".to_string(),
                    per_request_micros: 120.0,
                    requests: 1000,
                },
            ],
            spike: Some(spike(3.0)),
            alternatives: roster(1.1),
            offerings: vec![Offering {
                what: "one route, no db".to_string(),
                stack: "twin, http".to_string(),
                head_bytes: 63,
                concurrency: 8,
                per_second: 9000.0,
                p50_micros: 800.0,
                p99_micros: 2400.0,
                floor_per_second: Some(90000.0),
            }],
            limits: vec![Limit {
                what: "one machine is one core".to_string(),
                why: "a Ply value holds `Rc`, so a task cannot move between OS threads".to_string(),
                evidence: None,
            }],
        }
    }

    /// The audit is what makes the honest account a requirement rather than an
    /// intention: a report missing a section says so above its own tables.
    #[test]
    fn the_audit_names_every_section_the_report_owes() {
        let complete = report(full_points());
        assert!(complete.audit().is_empty(), "{:?}", complete.audit());

        let mut thin = report(full_points());
        thin.spike = None;
        thin.engines.clear();
        thin.offerings.clear();
        thin.limits.clear();
        thin.provenance.not_measured.clear();
        thin.alternatives.retain(|a| a.name != "response buffering");
        let findings = thin.audit();
        for expected in [
            "no codegen spike",
            "engine substitution",
            "no offering",
            "no limits",
            "not_measured",
            "response buffering",
        ] {
            assert!(
                findings.iter().any(|f| f.contains(expected)),
                "`{expected}` missing from {findings:?}"
            );
        }

        let mut partial = report(full_points()[..5].to_vec());
        partial.total_micros = 120.0;
        assert!(
            partial
                .audit()
                .iter()
                .any(|f| f.contains("no `socket` rung"))
        );
    }

    /// A measurement file may not carry the bar it is about to clear, so the
    /// rendered verdict is recomputed from `Criteria::default` every time.
    #[test]
    fn a_report_renders_its_tables_and_recomputes_its_verdict() {
        let out = render(&report(full_points()));
        for expected in [
            "the accumulated stack",
            "residue",
            "one interpreter against another",
            "the codegen spike",
            "the cheaper levers",
            "what this language serves today",
            "where this is genuinely not competitive",
            "what W6 did not measure",
            "verdict: advance a code generator for this workload",
            "workload: the served HTTP workload",
        ] {
            assert!(out.contains(expected), "`{expected}` missing from:\n{out}");
        }
        assert!(!out.contains("this report is incomplete"), "{out}");

        let complete = report(full_points());
        let rendered = rendered(&complete).unwrap();
        assert_eq!(rendered.decision.verdict, Verdict::Advance);
        assert!(rendered.spike.unwrap().evidence);
    }

    /// A report round-trips as JSON: the two measuring agents produce the
    /// halves separately and the decision is taken over the merged file.
    #[test]
    fn a_report_round_trips_through_json_without_carrying_a_verdict() {
        let original = report(full_points());
        let text = serde_json::to_string(&original).unwrap();
        assert!(
            !text.contains("verdict"),
            "a report may not carry a verdict"
        );
        let back: Report = serde_json::from_str(&text).unwrap();
        let ladder = back.ladder().unwrap();
        assert_eq!(
            back.decision(&ladder).verdict,
            original.decision(&original.ladder().unwrap()).verdict
        );
    }
}
