//! Where a request's time goes now, and what would justify M9.

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

/// A measurement that can be divided by.
fn usable(micros: f64) -> bool {
    micros.is_finite() && micros > 0.0
}

/// Where a rung is taken.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Arena {
    /// In this process, over `SimNet` or a real listener, with no CLI and no child.
    InProcess,
    /// The real `ply` binary over loopback, driven by client threads.
    Served,
}

/// One layer of the W5 stack, and the substitution that isolates it.
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
    /// Every layer, in request order.
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

    /// The one thing that changes between this rung's two measurements.
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
    pub fn is_interpreter(self) -> bool {
        matches!(
            self,
            Layer::Call | Layer::Endpoint | Layer::Framing | Layer::Routing | Layer::Machine
        )
    }
}

/// One rung, as measured: two absolutes taken in one arena in one run.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Point {
    pub layer: Layer,
    /// The route this rung's pair was taken on.
    pub taken_on: String,
    /// The configuration **with** this layer, per request.
    pub with_micros: f64,
    /// The same configuration **without** it, same arena, same run.
    pub without_micros: f64,
    /// The **worst** of the same repeats, when the rung was repeated.
    #[serde(default)]
    pub with_worst_micros: Option<f64>,
    #[serde(default)]
    pub without_worst_micros: Option<f64>,
    /// Requests each side of the pair was averaged over.
    pub requests: u32,
}

impl Point {
    /// The layer at its smallest: the fastest `with` against the slowest `without`.
    pub fn low_micros(&self) -> Option<f64> {
        Some(self.with_micros - self.without_worst_micros.unwrap_or(self.without_micros))
            .filter(|_| self.with_worst_micros.is_some() || self.without_worst_micros.is_some())
    }

    /// The layer at its largest: the slowest `with` against the fastest `without`.
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
    /// `with − without`.
    pub layer_micros: f64,
    /// The same difference at its smallest and largest over the repeats, when the rung carries
    /// them.
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

/// What the floor and the total answered, so a multiple between them is readable rather than
/// inferable.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Denominators {
    /// What the Rust floor answered, spelled out: the route, the response size and everything it
    /// does *not* have under it.
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
    /// What the served stack actually delivered, end to end, per request.
    pub total_micros: f64,
    /// What the rungs account for.
    pub attributed_micros: f64,
    /// `total − attributed`.
    pub residue_micros: f64,
    pub residue_share: f64,
    pub over_floor: f64,
    pub denominators: Denominators,
    /// The layers a codegen backend could reach, as attributed.
    pub interpreter_micros: f64,
    pub interpreter_share: f64,
    /// The same with a **negative** residue charged back to it.
    pub conservative_micros: f64,
    pub conservative_share: f64,
    /// The conservative share at the two ends of the repeats it was read off: the smallest
    /// numerator over the largest denominator, and the reverse.
    pub share_low: Option<f64>,
    pub share_high: Option<f64>,
    /// Whether the interpreter rungs chain — each `without` is the rung below's `with` — so that
    /// their sum is one absolute somebody measured rather than five differences added up.
    pub telescopes: bool,
    /// The most negative layer as a share of the total, as a positive number.
    pub worst_negative_share: f64,
    pub rungs: Vec<Rung>,
}

impl Ladder {
    /// Assemble a ladder, refusing anything a share cannot honestly be read off.
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
        // Only a negative residue moves the numerator: a positive one is time no substitution
        // separated, and crediting it to the interpreter would be claiming an attribution the
        // ladder did not earn.
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
        // When the rungs chain, the interpreter total is the top rung's own absolute and its band
        // is that rung's.
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

    /// Layers the ladder does not carry.
    pub fn missing(&self) -> Vec<Layer> {
        Layer::ORDER
            .into_iter()
            .filter(|l| !self.rungs.iter().any(|r| r.layer == *l))
            .collect()
    }
}

/// One input the spike and the interpreter both answered.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SpikeInput {
    pub name: String,
    pub interpreter_best_micros: f64,
    pub interpreter_worst_micros: f64,
    pub spike_best_micros: f64,
    pub spike_worst_micros: f64,
    /// Whether the two produced equal values on this input, by `Value`'s own ordering.
    pub agreed: bool,
}

impl SpikeInput {
    /// Interpreter best over spike worst.
    pub fn conservative(&self) -> f64 {
        if !usable(self.spike_worst_micros) {
            return 0.0;
        }
        self.interpreter_best_micros / self.spike_worst_micros
    }

    /// Interpreter best over spike best.
    pub fn optimistic(&self) -> f64 {
        if !usable(self.spike_best_micros) {
            return 0.0;
        }
        self.interpreter_best_micros / self.spike_best_micros
    }

    /// Whether the two samples separate at all.
    pub fn separated(&self) -> bool {
        self.spike_worst_micros < self.interpreter_best_micros
    }
}

/// What the throwaway codegen spike produced.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Spike {
    /// The function compiled, named so the choice is reviewable.
    pub function: String,
    /// Why this one.
    pub chosen_because: String,
    /// Nodes in its lowered body, which is the size of what was compiled.
    pub nodes: usize,
    /// What compiling it cost, once.
    pub compile_micros: f64,
    pub inputs: Vec<SpikeInput>,
}

#[derive(Clone, Debug, Serialize)]
pub struct SpikeVerdict {
    /// The **minimum** conservative ratio over every input.
    pub speedup: f64,
    /// The best optimistic ratio, for context only.
    pub optimistic: f64,
    pub evidence: bool,
    /// Every rule that failed, named.
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

/// One entry of the cheaper levers's table, **in code**.
pub struct Lever {
    /// The key an [`Alternative`] carries in its `name` to answer for this lever.
    pub name: &'static str,
    /// What the change is, in one sentence.
    pub what: &'static str,
}

/// The cheaper levers's seven levers.
pub const LEVERS: [Lever; 7] = [
    Lever {
        name: "more native builtins",
        what: "fold `read_line`, `is_token`, `trim_ows` and `trim_end` into one native head scan; \
               `string_lower`; `add_field`",
    },
    Lever {
        name: "the frame push",
        what: "the control-stack design's four heap allocations per frame push, priced by the engine substitution \
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
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Alternative {
    /// The [`Lever`] this answers for.
    pub name: String,
    /// What the change is, concretely enough to be built.
    pub what: String,
    pub priced: bool,
    /// End-to-end speedup on the same served workload the ladder's total came from.
    pub end_to_end: f64,
    /// The two things the ratio is between, and where the numbers came from.
    #[serde(default)]
    pub evidence: String,
    /// What it would cost to keep, in one sentence.
    pub cost: String,
}

impl Alternative {
    /// Whether this is a measurement C3 may read: priced, with a usable ratio, and carrying what
    /// the ratio is between.
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

/// Every cheaper lever this file does not price, as the sentence C3 fails on.
pub fn c3_gaps(alternatives: &[Alternative]) -> Vec<String> {
    let mut gaps = Vec::new();
    for lever in &LEVERS {
        match alternatives.iter().find(|a| a.name == lever.name) {
            None => gaps.push(format!(
                "`{}` is in the cheaper levers and this report says nothing about it: {}",
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

/// The thresholds, pinned before the numbers exist.
#[derive(Clone, Copy, Debug, Serialize)]
pub struct Criteria {
    /// Interpreter share at or above which M9's case can be made at all.
    pub min_share: f64,
    /// Below this share, defer categorically and say why: the ceiling is `1/(1−share)` however good
    /// the backend is.
    pub defer_share: f64,
    /// Speedup the spike must show on a real request-path function.
    pub min_spike: f64,
    /// Below this, defer: a constant factor this small is inside the range a cheaper lever has
    /// already delivered once.
    pub defer_spike: f64,
    /// Projected end-to-end speedup, by Amdahl over the measured share.
    pub min_projection: f64,
    /// Between [`Criteria::defer_share`] and [`Criteria::min_share`] the share alone cannot carry
    /// M9, so the spike has to be this good instead.
    pub gray_spike: f64,
    /// M9 must beat the best priced alternative by this factor, **on the gains rather than the
    /// ratios**: a 1.5x and a 1.1x are a 50% and a 10% improvement, and 1.5 against 2×1.1 compares
    /// nothing.
    pub alternative_margin: f64,
    /// A ladder with a negative layer larger than this did not separate, and nothing is decided
    /// from it.
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
pub const WORKLOAD: &str =
    "the served HTTP workload (examples/desk.ply over a socket, TLS and postgres)";

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Verdict {
    /// Bring a code generator for this workload forward.
    Advance,
    /// The grey band cleared: M9 is justified, and the report says on what conditions, because the
    /// share alone did not carry it.
    Conditional,
    /// Keep deferring, with the number that would reopen it.
    Defer,
    /// The measurement did not produce a decidable answer.
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
    /// What the share was taken on.
    pub workload: &'static str,
    /// [`Ladder::conservative_share`] — the share after a negative residue is charged back —
    /// because that is the one a decision may read.
    pub interpreter_share: f64,
    pub spike_speedup: f64,
    /// Amdahl over the measured share and the spike's ratio.
    pub projected: f64,
    pub best_alternative: Option<String>,
    pub best_alternative_end_to_end: f64,
    /// Why, in the order the rules were applied.
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

/// Apply the pinned criteria.
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

    // The sentence names only the conditions that are **not** met.
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

    // C3's first clause, against the roster in [`LEVERS`] rather than against whatever list the
    // file carried: a report that mentions no alternative at all has priced none of them, which is
    // the strongest form of this failure and used to be the one that read as success.
    let gaps = c3_gaps(alternatives);
    if !gaps.is_empty() {
        decision.reasons.push(format!(
            "{} of the cheaper levers's {} cheaper levers {} not priced, and a cheaper lever that has not \
             been priced is on its own a reason to keep deferring: {}",
            gaps.len(),
            LEVERS.len(),
            if gaps.len() == 1 { "is" } else { "are" },
            gaps.join("; ")
        ));
        // What C3 asks for, added to whatever else is unmet: the levers priced, and the best of
        // them no better than half M9's projected gain.
        let priced = if e > 1.0 {
            format!(
                "the {} unpriced lever(s) in the cheaper levers are priced and the best of them measures \
                 at or below {:.2}x end to end",
                gaps.len(),
                1.0 + (e - 1.0) / criteria.alternative_margin
            )
        } else {
            format!(
                "the {} unpriced lever(s) in the cheaper levers are priced — though at a {e:.2}x \
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

    // Every criterion left reads the share, so a share whose own repeats fall on both sides of a
    // bar has not answered the criterion — it has answered whichever run was taken.
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
            "decided for this workload; scheduling it is a milestone of its own".to_string();
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
    /// The same workload against the Rust floor on the same machine.
    pub floor_per_second: Option<f64>,
}

impl Offering {
    pub fn multiple(&self) -> Option<f64> {
        let floor = self.floor_per_second?;
        (self.per_second > 0.0).then(|| floor / self.per_second)
    }
}

/// Somewhere this language is genuinely not competitive, and why.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Limit {
    pub what: String,
    pub why: String,
    /// The number that shows it, where one was taken.
    pub evidence: Option<String>,
}

/// Where a number came from.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Provenance {
    pub machine: String,
    /// `release`, always, and stated so a debug run cannot be mistaken for one.
    pub profile: String,
    pub taken: String,
    pub repeats: usize,
    pub request_head_bytes: usize,
    pub postgres: Option<String>,
    /// What was not measured, and why.
    pub not_measured: Vec<String>,
}

/// Everything W6 owes, as one value.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Report {
    pub provenance: Provenance,
    pub floor_micros: f64,
    pub total_micros: f64,
    /// What the floor answered and what the total served, so the multiple between them is readable
    /// rather than assumed to be like for like.
    #[serde(default)]
    pub denominators: Denominators,
    pub points: Vec<Point>,
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
                    "`{}` answers for no lever in the cheaper levers, so nothing C3 asks about is priced \
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
