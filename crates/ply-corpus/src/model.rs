//! The corpus as data, plus the reference evaluator.
//!
//! Every generated definition is mirrored here in Rust so a test can assert a
//! real expected value instead of a tautology. That mirror is the load-bearing
//! part of this file: if it disagrees with `ply-eval` by one, the corpus does
//! not pass and the generator refuses to write it.

use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

pub type DefId = usize;
pub type ModuleId = usize;
pub type SpecimenId = usize;

/// `prim::clamp`'s modulus. Every generated body funnels through it, which is
/// what keeps a value in a range where no intermediate can overflow `Int`.
pub const CLAMP: i64 = 100_003;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Eff {
    Db,
    Cache,
    Clock,
    Counter,
}

impl Eff {
    pub fn as_str(self) -> &'static str {
        match self {
            Eff::Db => "db",
            Eff::Cache => "cache",
            Eff::Clock => "clock",
            Eff::Counter => "counter",
        }
    }
}

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct Atom {
    pub effect: Eff,
    /// `None` is the singleton resource of an operation declared without `[r]`.
    pub resource: Option<String>,
    pub write: bool,
}

impl Atom {
    pub fn read(effect: Eff, resource: impl Into<String>) -> Atom {
        Atom {
            effect,
            resource: Some(resource.into()),
            write: false,
        }
    }

    pub fn write(effect: Eff, resource: impl Into<String>) -> Atom {
        Atom {
            effect,
            resource: Some(resource.into()),
            write: true,
        }
    }

    pub fn singleton_read(effect: Eff) -> Atom {
        Atom {
            effect,
            resource: None,
            write: false,
        }
    }

    pub fn render(&self) -> String {
        let mode = if self.write { "write" } else { "read" };
        match &self.resource {
            Some(r) => format!("effects::{}.{mode}[{r}]", self.effect.as_str()),
            None => format!("effects::{}.{mode}", self.effect.as_str()),
        }
    }
}

pub type Footprint = BTreeSet<Atom>;

/// What the generator built a claim to be discharged by.
///
/// The prover is free to do better or worse than this, and a measurement is
/// worth having in both directions: a `proved` where the generator expected
/// sampling is worth a second look, and a sampled result where it expected a
/// decision is reach the prover does not have. It is an expectation, never an
/// assertion — nothing here may be read back as a tier.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Intent {
    /// Settled statically: linear arithmetic, a split over a constructor set, a
    /// ground evaluation or a finite enumeration.
    Decided,
    /// Reaches a recursive definition or a builtin the prover has no rule for,
    /// so the strongest honest answer is a case report.
    Sampled,
    /// The owner performs an effect nothing supplies a handler for, so the
    /// obligation cannot be attempted at all.
    Gap,
}

/// The claim attached to an ordinary generated definition.
///
/// One `requires` and one `ensures`, so that "obligations" and "definitions
/// carrying an obligation" differ by exactly the specimens below — a measurement
/// pricing discharge per obligation should not have to divide first.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Claim {
    pub intent: Intent,
}

/// A shape drives both the emitted source and the reference evaluator, so the
/// two cannot drift apart.
#[derive(Clone, Debug)]
pub struct Def {
    pub id: DefId,
    pub module: ModuleId,
    pub name: String,
    pub arity: usize,
    pub shape: Shape,
    /// Extra calls folded into the definition's tail. This is what pushes the
    /// mean out-degree above what a single shape provides.
    pub extras: Vec<Call>,
    pub footprint: Footprint,
    pub weight: u32,
    pub public: bool,
    /// `Some` at whatever density [`crate::CorpusSpec::spec_fraction`] asks for.
    pub claim: Option<Claim>,
}

#[derive(Clone, Copy, Debug)]
pub struct Call {
    pub target: DefId,
    pub offset: i64,
}

#[derive(Clone, Debug)]
pub enum Shape {
    Arith { a: i64, b: i64 },
    Compose { f: Call, g: Call },
    Guard { m: i64, f: Call, b: i64 },
    Fold { n: i64, k: i64 },
    Record { m: i64, k: i64 },
    Sum { off: i64, f: Call, idle: i64 },
    Chain { inner: Call, outer: DefId, b: i64 },
    ListMap { n: i64, k: i64 },
    Pair { f: Call, a: i64, b: i64 },
    TableCount { table: usize, a: i64, f: Call },
    TableSum { table: usize, a: i64 },
    TableAppend { table: usize, a: i64, b: i64 },
    CachePeek { region: usize, a: i64 },
    CachePoke { region: usize, a: i64 },
    Now { a: i64 },
}

impl Shape {
    pub fn calls(&self) -> Vec<Call> {
        match self {
            Shape::Compose { f, g } => vec![*f, *g],
            Shape::Guard { f, .. }
            | Shape::Sum { f, .. }
            | Shape::Pair { f, .. }
            | Shape::TableCount { f, .. } => vec![*f],
            Shape::Chain { inner, outer, .. } => vec![
                *inner,
                Call {
                    target: *outer,
                    offset: 0,
                },
            ],
            _ => Vec::new(),
        }
    }

    /// Atoms this shape performs itself, before anything its callees add.
    pub fn own_atoms(&self, tables: &[String], regions: &[String]) -> Footprint {
        let mut out = Footprint::new();
        match self {
            Shape::TableCount { table, .. } | Shape::TableSum { table, .. } => {
                out.insert(Atom::read(Eff::Db, &tables[*table]));
            }
            Shape::TableAppend { table, .. } => {
                out.insert(Atom::read(Eff::Db, &tables[*table]));
                out.insert(Atom::write(Eff::Db, &tables[*table]));
            }
            Shape::CachePeek { region, .. } => {
                out.insert(Atom::read(Eff::Cache, &regions[*region]));
            }
            Shape::CachePoke { region, .. } => {
                out.insert(Atom::read(Eff::Cache, &regions[*region]));
                out.insert(Atom::write(Eff::Cache, &regions[*region]));
            }
            Shape::Now { .. } => {
                out.insert(Atom::singleton_read(Eff::Clock));
            }
            _ => {}
        }
        out
    }

    pub fn is_block(&self) -> bool {
        matches!(self, Shape::TableAppend { .. } | Shape::CachePoke { .. })
    }

    /// A body that emits as exactly one expression on one line. The benchmark's
    /// edit sites need one, because a one-line body can be rewritten textually
    /// without a parser.
    pub fn is_one_liner(&self) -> bool {
        !self.is_block() && !matches!(self, Shape::Sum { .. })
    }
}

/// The per-module `stage` helper: the only definition that returns the module's
/// sum type, and the reason a `match` appears in generated code at all.
#[derive(Clone, Debug)]
pub struct Helper {
    pub name: String,
    pub m: i64,
    pub b: i64,
}

#[derive(Clone, Debug)]
pub struct Module {
    pub id: ModuleId,
    /// Dotted, as the compiler will derive it from the path.
    pub name: String,
    pub path: String,
    pub layer: usize,
    pub imports: Vec<ModuleId>,
    pub defs: Vec<DefId>,
    pub helper: Helper,
    pub status_type: String,
    pub ctor_ready: String,
    pub ctor_idle: String,
    pub needs_effects: bool,
}

impl Module {
    /// The name another module refers to this one by: `ImportDecl::binder` is
    /// the last path segment.
    pub fn binder(&self) -> &str {
        self.name.rsplit('.').next().unwrap_or(&self.name)
    }
}

#[derive(Clone, Debug)]
pub struct Test {
    pub module: ModuleId,
    pub label: String,
    pub nondet: bool,
    pub root: DefId,
    pub calls: Vec<Vec<i64>>,
    pub expected: Vec<i64>,
    pub world: World,
    /// Exactly the atoms the mirror saw performed. A declared atom that never
    /// fires gets no clause, so it survives into the test's own footprint —
    /// which is where a non-trivial conflict graph comes from.
    pub granted: Footprint,
    /// Table index -> length after every call, asserted at the end so a write
    /// that silently does nothing cannot pass.
    pub final_table_len: Vec<(usize, usize)>,
    pub final_region: Vec<(usize, i64)>,
}

/// The state a test's handlers stand in for. Read-only resources never change,
/// so the same value serves as both the handler's literal and the mirror's seed.
#[derive(Clone, Debug, Default)]
pub struct World {
    pub tables: Vec<(usize, Vec<i64>)>,
    pub regions: Vec<(usize, i64)>,
    pub clock: i64,
    /// Every atom performed so far. Under-reporting here becomes an unhandled
    /// effect at runtime, which is why `verify` runs on every generated corpus.
    pub touched: Footprint,
}

impl World {
    pub fn table(&self, index: usize) -> &[i64] {
        self.tables
            .iter()
            .find(|(i, _)| *i == index)
            .map(|(_, v)| v.as_slice())
            .unwrap_or(&[])
    }

    fn read_table(&mut self, label: &str, index: usize) -> &[i64] {
        self.touched.insert(Atom::read(Eff::Db, label));
        self.tables
            .iter()
            .find(|(i, _)| *i == index)
            .map(|(_, v)| v.as_slice())
            .unwrap_or(&[])
    }

    fn table_mut(&mut self, index: usize) -> Option<&mut Vec<i64>> {
        self.tables
            .iter_mut()
            .find(|(i, _)| *i == index)
            .map(|(_, v)| v)
    }

    pub fn region(&self, index: usize) -> i64 {
        self.regions
            .iter()
            .find(|(i, _)| *i == index)
            .map(|(_, v)| *v)
            .unwrap_or(0)
    }

    fn read_region(&mut self, label: &str, index: usize) -> i64 {
        self.touched.insert(Atom::read(Eff::Cache, label));
        self.region(index)
    }

    fn set_region(&mut self, index: usize, value: i64) {
        if let Some(slot) = self.regions.iter_mut().find(|(i, _)| *i == index) {
            slot.1 = value;
        } else {
            self.regions.push((index, value));
        }
    }
}

/// One task of a concurrent test: a `fn` of its own that bumps one shard
/// `steps.len()` times with a `task.yield()` between each pair.
///
/// A bump is a single operation, so the handler's read-modify-write of the
/// backing cell cannot be split by any schedule and the shard's total is the
/// same under all of them. The *order* the bumps land in is not, which is the
/// point: the outcome is interleaving-invariant so the corpus stays green, and
/// the search still has every order to prune.
#[derive(Clone, Debug)]
pub struct TaskBody {
    pub name: String,
    /// Index into [`Corpus::shards`].
    pub shard: usize,
    pub steps: Vec<i64>,
}

impl TaskBody {
    pub fn contributed(&self) -> i64 {
        self.steps.iter().sum()
    }
}

/// A `simulate` test. Conflict density is expressed as nothing but the mapping
/// of tasks to shards, because that mapping is the whole of what the dependence
/// relation reads.
#[derive(Clone, Debug)]
pub struct ConcurrentTest {
    pub module: ModuleId,
    pub label: String,
    pub tasks: Vec<TaskBody>,
    /// Shard indices used, ascending and deduplicated.
    pub shards: Vec<usize>,
}

impl ConcurrentTest {
    pub fn shard_total(&self, shard: usize) -> i64 {
        self.tasks
            .iter()
            .filter(|t| t.shard == shard)
            .map(TaskBody::contributed)
            .sum()
    }

    pub fn total(&self) -> i64 {
        self.tasks.iter().map(TaskBody::contributed).sum()
    }

    /// Tasks that share their shard with another task, over all tasks. The
    /// measured counterpart of `CorpusSpec::conflict_density`, so a measurement
    /// reports what the corpus has rather than what was asked for.
    pub fn contention(&self) -> f64 {
        if self.tasks.is_empty() {
            return 0.0;
        }
        let shared = self
            .tasks
            .iter()
            .filter(|t| self.tasks.iter().filter(|o| o.shard == t.shard).count() > 1)
            .count();
        shared as f64 / self.tasks.len() as f64
    }
}

/// A definition written for its obligation rather than for its call graph.
///
/// The ordinary generated bodies all funnel through `prim::clamp`, whose `%` is
/// outside the proved fragment on purpose, so every claim about one of them is
/// sampled. A corpus of nothing but sampled obligations measures one column of
/// the tier table and calls it a distribution. These are the other columns.
#[derive(Clone, Debug)]
pub struct Specimen {
    pub id: SpecimenId,
    pub module: ModuleId,
    pub name: String,
    pub kind: SpecimenKind,
}

#[derive(Clone, Copy, Debug)]
pub enum SpecimenKind {
    /// `x * a + b`, with a postcondition that is the same claim rearranged, so
    /// closing it is linear arithmetic rather than syntactic identity.
    Linear { a: i64, b: i64 },
    /// A split over the module's own two-constructor status type. Both arms are
    /// literals, so every branch closes and the split is over the constructor
    /// set rather than the value space.
    Status,
    /// A walk down a list, calling itself. A member of a recursive component is
    /// never unfolded — that is where induction would be needed and there is
    /// none — so a claim about it is sampled however simple it looks.
    Length,
}

impl SpecimenKind {
    pub fn intent(self) -> Intent {
        match self {
            SpecimenKind::Linear { .. } | SpecimenKind::Status => Intent::Decided,
            SpecimenKind::Length => Intent::Sampled,
        }
    }
}

/// A standalone claim. Labelled like a test, so nothing can reference it.
#[derive(Clone, Debug)]
pub struct Law {
    pub module: ModuleId,
    pub label: String,
    pub kind: LawKind,
}

#[derive(Clone, Copy, Debug)]
pub enum LawKind {
    /// No binders: a domain of one point, decided by evaluating it.
    Ground { a: i64, b: i64 },
    /// Two `Bool` binders, so the domain is four points and enumerating it is a
    /// decision rather than a sample.
    Finite,
    /// Over a [`SpecimenKind::Length`] definition, so it is sampled.
    Length { specimen: SpecimenId },
}

impl LawKind {
    pub fn intent(self) -> Intent {
        match self {
            LawKind::Ground { .. } | LawKind::Finite => Intent::Decided,
            LawKind::Length { .. } => Intent::Sampled,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Corpus {
    pub modules: Vec<Module>,
    pub defs: Vec<Def>,
    pub tests: Vec<Test>,
    pub concurrent: Vec<ConcurrentTest>,
    pub specimens: Vec<Specimen>,
    pub laws: Vec<Law>,
    pub tables: Vec<String>,
    pub regions: Vec<String>,
    /// `counter` resource labels, one per shard a concurrent test can use.
    pub shards: Vec<String>,
}

pub fn clamp(v: i64) -> i64 {
    v % CLAMP
}

pub fn mix(a: i64, b: i64) -> i64 {
    clamp(a * 31 + b * 17 + 7)
}

pub fn weigh(id: i64, weight: i64) -> i64 {
    clamp(id * 11 + weight)
}

pub fn total(xs: &[i64]) -> i64 {
    xs.iter().fold(0, |acc, v| clamp(acc + v))
}

impl Corpus {
    /// The value `def(args)` evaluates to under `world`, which is mutated by
    /// exactly the writes the definition performs.
    pub fn eval(&self, def: DefId, args: &[i64], world: &mut World) -> i64 {
        let def = &self.defs[def];
        let p0 = args.first().copied().unwrap_or(0);
        let p1 = args.get(1).copied().unwrap_or(0);

        let core = match &def.shape {
            Shape::Arith { a, b } => clamp(p0 * a + b),
            Shape::Compose { f, g } => {
                let left = self.invoke(*f, p0, world);
                let right = self.invoke(*g, p0, world);
                mix(left, right)
            }
            Shape::Guard { m, f, b } => {
                if p0 % m == 0 {
                    self.invoke(*f, p0, world)
                } else {
                    clamp(p0 + b)
                }
            }
            Shape::Fold { n, k } => (0..*n).fold(0, |acc, v| clamp(acc + v * k + p0)),
            Shape::Record { m, k } => weigh(p0 % m, clamp(p0 * k)),
            Shape::Sum { off, f, idle } => {
                let module = &self.modules[def.module];
                let x = p0 + off;
                if x % module.helper.m == 0 {
                    *idle
                } else {
                    self.invoke(*f, clamp(x + module.helper.b), world)
                }
            }
            Shape::Chain { inner, outer, b } => {
                let v = self.invoke(*inner, p0, world);
                self.invoke_with(
                    Call {
                        target: *outer,
                        offset: 0,
                    },
                    v + b,
                    world,
                )
            }
            Shape::ListMap { n, k } => {
                let xs: Vec<i64> = (0..*n).map(|v| clamp(v * k + p0)).collect();
                total(&xs)
            }
            Shape::Pair { f, a, b } => {
                let left = self.invoke(*f, p0, world);
                mix(left, p1 * a + b)
            }
            Shape::TableCount { table, a, f } => {
                let n = world.read_table(&self.tables[*table], *table).len() as i64;
                let rest = self.invoke(*f, p0, world);
                clamp(n * a + rest)
            }
            Shape::TableSum { table, a } => {
                let sum = total(world.read_table(&self.tables[*table], *table));
                clamp(sum + p0 * a)
            }
            Shape::TableAppend { table, a, b } => {
                let label = &self.tables[*table];
                let before = world.read_table(label, *table).len() as i64;
                let pushed = clamp(p0 * a);
                world.touched.insert(Atom::write(Eff::Db, label));
                if let Some(rows) = world.table_mut(*table) {
                    rows.push(pushed);
                }
                clamp(before * b + p0)
            }
            Shape::CachePeek { region, a } => {
                clamp(world.read_region(&self.regions[*region], *region) + p0 * a)
            }
            Shape::CachePoke { region, a } => {
                let label = &self.regions[*region];
                let seen = world.read_region(label, *region);
                world.touched.insert(Atom::write(Eff::Cache, label));
                world.set_region(*region, clamp(seen + p0));
                clamp(seen * a + p0)
            }
            Shape::Now { a } => {
                world.touched.insert(Atom::singleton_read(Eff::Clock));
                clamp(world.clock % a + p0)
            }
        };

        if def.extras.is_empty() {
            return core;
        }
        let mut sum = 0i64;
        for extra in &def.extras {
            sum += self.invoke(*extra, p0, world);
        }
        mix(core, sum)
    }

    fn invoke(&self, call: Call, p0: i64, world: &mut World) -> i64 {
        self.invoke_with(call, p0 + call.offset, world)
    }

    fn invoke_with(&self, call: Call, arg: i64, world: &mut World) -> i64 {
        let args = call_args(self.defs[call.target].arity, arg, call.offset);
        self.eval(call.target, &args, world)
    }

    pub fn module_of(&self, def: DefId) -> &Module {
        &self.modules[self.defs[def].module]
    }

    pub fn effectful_defs(&self) -> usize {
        self.defs.iter().filter(|d| !d.footprint.is_empty()).count()
    }

    pub fn concurrent_in(&self, module: ModuleId) -> impl Iterator<Item = &ConcurrentTest> {
        self.concurrent.iter().filter(move |t| t.module == module)
    }

    pub fn specimens_in(&self, module: ModuleId) -> impl Iterator<Item = &Specimen> {
        self.specimens.iter().filter(move |s| s.module == module)
    }

    pub fn laws_in(&self, module: ModuleId) -> impl Iterator<Item = &Law> {
        self.laws.iter().filter(move |l| l.module == module)
    }

    /// Every obligation the corpus carries, by what the generator built it to be
    /// discharged by. A `requires` is not one: it filters the domain of the
    /// `ensures` beside it rather than making a claim of its own.
    pub fn obligations_by_intent(&self) -> [usize; 3] {
        let mut out = [0usize; 3];
        let mut count = |intent: Intent| match intent {
            Intent::Decided => out[0] += 1,
            Intent::Sampled => out[1] += 1,
            Intent::Gap => out[2] += 1,
        };
        for claim in self.defs.iter().filter_map(|d| d.claim) {
            count(claim.intent);
        }
        for specimen in &self.specimens {
            count(specimen.kind.intent());
        }
        for law in &self.laws {
            count(law.kind.intent());
        }
        out
    }

    pub fn specified_defs(&self) -> usize {
        self.defs.iter().filter(|d| d.claim.is_some()).count()
    }
}

/// A second parameter is synthesized from the call's own offset rather than
/// drawn, so emission and evaluation cannot pick different values for it.
pub fn call_args(arity: usize, first: i64, offset: i64) -> Vec<i64> {
    if arity >= 2 {
        vec![first, second_arg(offset)]
    } else {
        vec![first]
    }
}

pub fn second_arg(offset: i64) -> i64 {
    offset * 7 % 97 + 3
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_atom_renders_as_the_row_syntax_it_will_be_parsed_from() {
        assert_eq!(
            Atom::read(Eff::Db, "users").render(),
            "effects::db.read[users]"
        );
        assert_eq!(
            Atom::write(Eff::Cache, "hot").render(),
            "effects::cache.write[hot]"
        );
        assert_eq!(
            Atom::singleton_read(Eff::Clock).render(),
            "effects::clock.read"
        );
    }

    #[test]
    fn clamp_agrees_with_the_prelude_definition_it_mirrors() {
        assert_eq!(clamp(100_004), 1);
        assert_eq!(clamp(-1), -1);
        assert_eq!(mix(2, 3), 2 * 31 + 3 * 17 + 7);
    }

    #[test]
    fn a_module_binder_is_its_last_dotted_segment() {
        let module = Module {
            id: 0,
            name: "store0.rules_3".into(),
            path: "store0/rules_3.ply".into(),
            layer: 0,
            imports: Vec::new(),
            defs: Vec::new(),
            helper: Helper {
                name: "stage_0".into(),
                m: 3,
                b: 1,
            },
            status_type: "Status0".into(),
            ctor_ready: "Ready0".into(),
            ctor_idle: "Idle0".into(),
            needs_effects: false,
        };
        assert_eq!(module.binder(), "rules_3");
    }

    #[test]
    fn a_written_table_is_the_only_one_a_footprint_reports_writing() {
        let tables = vec!["users".to_string(), "orders".to_string()];
        let regions = vec!["hot".to_string()];
        let append = Shape::TableAppend {
            table: 1,
            a: 2,
            b: 3,
        };
        let atoms = append.own_atoms(&tables, &regions);
        assert_eq!(atoms.len(), 2);
        assert!(atoms.contains(&Atom::write(Eff::Db, "orders")));
        assert!(!atoms.contains(&Atom::write(Eff::Db, "users")));
    }
}
