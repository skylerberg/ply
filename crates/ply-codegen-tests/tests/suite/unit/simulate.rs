use ply_codegen::c::Native;
use ply_codegen::heap::{Heap, imm};
use ply_eval::{
    Analysis, Determinism, Diagnostic, HostAnswer, HostBinding, HostHandler, HostOp, HostRegistry,
    HostRequest, HostResource, HostRuntime, Linearity, Symbol, Value,
};
use std::sync::Arc;

/// A production region's task handler is listed and never called: the region answers `task`.
struct Scheduled;

impl HostHandler for Scheduled {
    fn call(&self, _: &dyn HostRuntime, req: &HostRequest<'_>) -> Result<HostAnswer, Diagnostic> {
        panic!("`task.{}` reached a handler", req.op.op)
    }
}

/// The front and the unit over `text`, module `m`; `None` on a host with no C compiler.
fn built(text: &str) -> Option<(&'static Analysis, Native)> {
    let (source, native, refused) = crate::fixture::with_refusals(text)?;
    assert!(refused.is_empty(), "{refused:?}");
    Some((source.front, native))
}

/// What `ply run --host` binds `task` to, which is what lets a `task` operation open a region.
pub(super) fn tasks_bound(front: &Analysis) -> HostBinding {
    let mut registry = HostRegistry::new();
    for op in ply_eval::sim::TASK_OPS {
        registry.register(
            HostOp {
                effect: Symbol::new("task"),
                op: Symbol::new(*op),
                resource: HostResource::Any,
                determinism: Determinism::Nondeterministic,
                linearity: Linearity::Repeatable,
                blocking: false,
                secrets: false,
                path: "test::task",
            },
            Arc::new(Scheduled),
        );
    }
    registry.bind(&front.check).expect("the task rows bind")
}

/// Per round, a task nobody joins and one the root joins, each opening a region of its own.
const CHURN: &str = r#"
fn counted(i: Int) -> Int =
  with_cell[work](i) { c -> {
    cell_set(c, cell_get(c) + 1);
    cell_get(c)
  } }

pub fn churn(n: Int) -> Int / {task.write, abort.raise} =
  fold(range(0, n), 0, |sum: Int, i: Int| {
    task.spawn(|| counted(i));
    let t = task.spawn(|| counted(i));
    task.yield();
    sum + task.join(t)
  })
"#;

/// A server spawns per connection for as long as it runs, so its tables must follow the live tasks.
#[test]
fn a_production_region_keeps_its_tables_to_the_tasks_still_live() {
    let Some((front, native)) = built(CHURN) else {
        return;
    };
    let entry = native.entry("m.churn").expect("`churn` compiles");
    let mut ctx = native.context();
    ctx.set_host(Arc::new(tasks_bound(front)), None, None);
    ctx.begin(100_000);
    let rounds = 200;
    let answer = unsafe { entry(&mut ctx, [imm(rounds)].as_ptr()) };
    assert_eq!(
        ctx.failed,
        0,
        "`churn` raised: {:?}",
        ctx.diagnostic.as_ref().map(|d| d.message.clone())
    );

    // The root's body has returned and its region is still open, with what the last round left.
    let region = ctx
        .sims
        .last()
        .filter(|sim| sim.is_production())
        .expect("the first `task` operation opened a production region");
    assert!(
        region.scheduled() <= 3,
        "the scheduler keeps {} tasks after {} spawns",
        region.scheduled(),
        rounds * 2
    );
    assert!(
        region.task_slots() <= 2,
        "the region holds {} task stacks after {} spawns",
        region.task_slots(),
        rounds * 2
    );
    assert!(
        ctx.stack_slots() <= 8,
        "{} frame tables after {} spawns",
        ctx.stack_slots(),
        rounds * 2
    );
    assert!(
        ctx.cell_owners() <= ctx.stack_slots(),
        "the arena keeps {} nestings for {} stacks",
        ctx.cell_owners(),
        ctx.stack_slots()
    );

    let answer = unsafe { ply_codegen::simulate::finish_root(&mut ctx, answer) };
    assert_eq!(ctx.failed, 0, "draining the region raised");
    assert_eq!(
        Heap::to_value(&native.tables().layouts, answer),
        Value::Int(rounds * (rounds + 1) / 2)
    );
    ctx.end();
}

/// A spawn's handle and a `with_cell`'s cell are both bridges: what a server makes per request.
const BRIDGED: &str = r#"
fn counted(i: Int) -> Int =
  with_cell[work](i) { c -> {
    cell_set(c, cell_get(c) + 1);
    cell_get(c)
  } }

pub fn fired(n: Int) -> Int / {task.write} =
  fold(range(0, n), 0, |sum: Int, i: Int| {
    task.spawn(|| counted(i));
    task.yield();
    sum + i
  })

pub fn celled(n: Int) -> Int = fold(range(0, n), 0, |sum: Int, i: Int| sum + counted(i))
"#;

/// Past a handful of bridges, the table is growing with the requests rather than the live values.
const LIVE_BRIDGES: usize = 4;

#[test]
fn a_production_region_that_drops_its_task_handles_gives_their_bridges_back() {
    let Some((front, native)) = built(BRIDGED) else {
        return;
    };
    let entry = native.entry("m.fired").expect("`fired` compiles");
    let mut ctx = native.context();
    ctx.set_host(Arc::new(tasks_bound(front)), None, None);
    ctx.begin(100_000);
    let rounds = 10_000;
    let answer = unsafe { entry(&mut ctx, [imm(rounds)].as_ptr()) };
    assert_eq!(
        ctx.failed,
        0,
        "`fired` raised: {:?}",
        ctx.diagnostic.as_ref().map(|d| d.message.clone())
    );
    assert!(
        ctx.sims.last().is_some_and(|sim| sim.is_production()),
        "the first `task` operation opened no production region"
    );
    assert!(
        ctx.heap.bridges() <= LIVE_BRIDGES,
        "the heap keeps {} bridges after {rounds} dropped handles and cells",
        ctx.heap.bridges()
    );

    let answer = unsafe { ply_codegen::simulate::finish_root(&mut ctx, answer) };
    assert_eq!(ctx.failed, 0, "draining the region raised");
    assert_eq!(
        Heap::to_value(&native.tables().layouts, answer),
        Value::Int(rounds * (rounds - 1) / 2)
    );
    ctx.end();
}

#[test]
fn a_with_cell_opened_in_a_loop_gives_its_cells_bridge_back() {
    let Some((_, native)) = built(BRIDGED) else {
        return;
    };
    let entry = native.entry("m.celled").expect("`celled` compiles");
    let mut ctx = native.context();
    ctx.begin(100_000);
    let rounds = 10_000;
    let answer = unsafe { entry(&mut ctx, [imm(rounds)].as_ptr()) };
    assert_eq!(
        ctx.failed,
        0,
        "`celled` raised: {:?}",
        ctx.diagnostic.as_ref().map(|d| d.message.clone())
    );
    assert!(
        ctx.heap.bridges() <= LIVE_BRIDGES,
        "the heap keeps {} bridges after {rounds} cells",
        ctx.heap.bridges()
    );
    assert_eq!(
        Heap::to_value(&native.tables().layouts, answer),
        Value::Int(rounds * (rounds + 1) / 2)
    );
    ctx.end();
}
