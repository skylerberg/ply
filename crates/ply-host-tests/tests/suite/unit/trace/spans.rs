use ply_core::host::MachineId;
use ply_core::{Resource, Span, Symbol, TaskId, codes};
use ply_host::trace::Outcome;
use ply_host::trace::spans::*;

fn machine() -> MachineId {
    MachineId::next()
}

fn channel(name: &str) -> Resource {
    Resource::Named(Symbol::new(name))
}

fn name(text: &str) -> std::sync::Arc<str> {
    std::sync::Arc::from(text)
}

fn enter(spans: &mut Spans, owner: Owner, at: &str, what: &str) -> i64 {
    spans.enter(owner, channel(at), name(what), None).id
}

#[test]
fn a_span_opened_inside_another_names_it_as_its_parent() {
    let mut spans = Spans::new();
    let owner = (machine(), None);
    let outer = enter(&mut spans, owner, "http", "request");
    let inner = enter(&mut spans, owner, "http", "query");
    assert_eq!(spans.innermost(owner), (inner, outer));
    assert_eq!(spans.depth(owner), 2);

    let closed = spans
        .exit(owner, inner, &channel("http"), Outcome::Ok)
        .unwrap_or_else(|_| panic!("the inner span is open"));
    assert_eq!(closed.len(), 1);
    assert_eq!(closed[0].open.parent, outer);
    assert_eq!(spans.innermost(owner), (outer, 0));

    let closed = spans
        .exit(owner, outer, &channel("http"), Outcome::Ok)
        .unwrap_or_else(|_| panic!("the outer span is open"));
    assert_eq!(closed.len(), 1);
    assert_eq!(spans.depth(owner), 0);
    assert_eq!(spans.abandoned(), 0);
}

#[test]
fn closing_an_outer_span_also_closes_the_spans_opened_inside_it_and_returns_them_innermost_first() {
    let mut spans = Spans::new();
    let owner = (machine(), None);
    let outer = enter(&mut spans, owner, "orders", "place_order");
    let inner = enter(&mut spans, owner, "orders", "reserve");
    let deeper = enter(&mut spans, owner, "orders", "charge");

    let closed = spans
        .exit(
            owner,
            outer,
            &channel("orders"),
            Outcome::Failed("rolled back".to_string()),
        )
        .unwrap_or_else(|_| panic!("the outer span is open"));
    let ids: Vec<i64> = closed.iter().map(|c| c.open.id).collect();
    assert_eq!(ids, [deeper, inner, outer], "innermost first");
    assert_eq!(closed[0].outcome, Outcome::Abandoned);
    assert_eq!(closed[1].outcome, Outcome::Abandoned);
    assert_eq!(
        closed[2].outcome,
        Outcome::Failed("rolled back".to_string())
    );
    assert_eq!(spans.abandoned(), 2);
    assert_eq!(spans.depth(owner), 0);
}

#[test]
fn two_tasks_of_one_program_run_keep_separate_span_stacks() {
    let mut spans = Spans::new();
    let id = machine();
    let first = (id, Some(TaskId(1)));
    let second = (id, Some(TaskId(2)));

    let a = enter(&mut spans, first, "http", "request");
    let b = enter(&mut spans, second, "http", "request");
    assert_ne!(a, b, "ids are never reused");
    assert_eq!(
        spans.innermost(first),
        (a, 0),
        "task 2 is not task 1's parent"
    );
    assert_eq!(spans.innermost(second), (b, 0));

    let closed = spans
        .exit(second, b, &channel("http"), Outcome::Ok)
        .unwrap_or_else(|_| panic!("task 2's span is open"));
    assert_eq!(closed.len(), 1);
    assert_eq!(spans.depth(first), 1);
    assert_eq!(spans.abandoned(), 0);
}

#[test]
fn a_span_opened_by_one_task_cannot_be_closed_by_another() {
    let mut spans = Spans::new();
    let id = machine();
    let first = (id, Some(TaskId(1)));
    let second = (id, Some(TaskId(2)));
    let opened = enter(&mut spans, first, "http", "request");

    let Err(reason) = spans.exit(second, opened, &channel("http"), Outcome::Ok) else {
        panic!("task 2 closed task 1's span");
    };
    assert!(matches!(reason, Unbalanced::OtherOwner(owner) if owner == first));

    let diagnostic = err_unbalanced(Span::DUMMY, "`trace.exit`", opened, &reason);
    assert_eq!(diagnostic.code, codes::SPAN_UNBALANCED);
    assert!(
        diagnostic.notes.iter().any(|n| n.contains("task @1")),
        "the rejection must name the task that holds it: {diagnostic:?}"
    );
    assert_eq!(spans.depth(first), 1);
}

#[test]
fn exiting_a_span_returns_a_different_error_for_already_closed_never_opened_and_wrong_channel() {
    let mut spans = Spans::new();
    let owner = (machine(), None);
    let opened = enter(&mut spans, owner, "http", "request");
    spans
        .exit(owner, opened, &channel("http"), Outcome::Ok)
        .unwrap_or_else(|_| panic!("it is open"));

    let Err(closed) = spans.exit(owner, opened, &channel("http"), Outcome::Ok) else {
        panic!("a span closes once");
    };
    assert!(matches!(closed, Unbalanced::AlreadyClosed));

    let Err(never) = spans.exit(owner, 4_100, &channel("http"), Outcome::Ok) else {
        panic!("nothing minted that id");
    };
    assert!(matches!(never, Unbalanced::NeverOpened));

    let again = enter(&mut spans, owner, "http", "request");
    let Err(elsewhere) = spans.exit(owner, again, &channel("orders"), Outcome::Ok) else {
        panic!("the channel disagrees");
    };
    assert!(matches!(elsewhere, Unbalanced::OtherChannel(_)));
    assert_eq!(spans.depth(owner), 1, "a rejection closes nothing");
}

#[test]
fn ending_a_program_closes_its_own_open_spans_and_leaves_other_programs_spans_open() {
    let mut spans = Spans::new();
    let mine = machine();
    let theirs = machine();
    let outer = enter(&mut spans, (mine, None), "http", "request");
    let inner = enter(&mut spans, (mine, Some(TaskId(1))), "db", "query");
    enter(&mut spans, (theirs, None), "http", "request");

    let (closed, abandoned) = spans.end_entry_point(mine);
    let ids: Vec<i64> = closed.iter().map(|c| c.open.id).collect();
    assert_eq!(ids.len(), 2, "both of this machine's owners");
    assert!(ids.contains(&outer) && ids.contains(&inner));
    assert!(closed.iter().all(|c| c.outcome == Outcome::Abandoned));
    assert_eq!(spans.total_open(), 1, "the other machine is untouched");
    assert_eq!(spans.abandoned(), 2);

    let warning = warn_abandoned(&abandoned);
    assert_eq!(warning.code, codes::SPAN_ABANDONED);
    assert!(warning.message.contains('2'), "{}", warning.message);
}

#[test]
fn teardown_reports_the_innermost_span_first() {
    let mut spans = Spans::new();
    let mine = machine();
    let owner = (mine, None);
    enter(&mut spans, owner, "http", "request");
    enter(&mut spans, owner, "http", "query");

    let (closed, abandoned) = spans.end_entry_point(mine);
    assert_eq!(closed[0].open.name.as_ref(), "query");
    assert!(
        warn_abandoned(&abandoned).message.contains("`query`"),
        "the warning names the innermost span"
    );
}

#[test]
fn ending_a_task_closes_only_that_tasks_open_spans_innermost_first() {
    let mut spans = Spans::new();
    let mine = machine();
    let retired = (mine, Some(TaskId(1)));
    let running = (mine, Some(TaskId(2)));
    let outer = enter(&mut spans, retired, "http", "request");
    let inner = enter(&mut spans, retired, "db", "query");
    let other = enter(&mut spans, running, "http", "request");
    enter(&mut spans, (mine, None), "http", "serve");

    let closed = spans.end_task(mine, TaskId(1));
    let ids: Vec<i64> = closed.iter().map(|c| c.open.id).collect();
    assert_eq!(ids, [inner, outer], "innermost first");
    assert!(closed.iter().all(|c| c.outcome == Outcome::Abandoned));
    assert_eq!(spans.abandoned(), 2);
    assert_eq!(spans.depth(retired), 0);
    assert_eq!(
        spans.innermost(running),
        (other, 0),
        "another task's span stays"
    );
    assert_eq!(spans.depth((mine, None)), 1, "the entry point's span stays");
    assert!(
        spans.end_task(mine, TaskId(1)).is_empty(),
        "retiring twice closes nothing"
    );
    assert_eq!(
        spans.retired(mine).map(|noted| noted.count),
        Some(2),
        "noted once, for the entry point's warning"
    );
}

#[test]
fn ending_a_task_whose_spans_are_all_closed_removes_its_span_stack_and_reports_no_unclosed_spans() {
    let mut spans = Spans::new();
    let mine = machine();
    let owner = (mine, Some(TaskId(1)));
    let id = enter(&mut spans, owner, "http", "request");
    spans
        .exit(owner, id, &channel("http"), Outcome::Ok)
        .unwrap_or_else(|_| panic!("it is open"));
    assert_eq!(spans.owners(), 1);
    assert!(spans.end_task(mine, TaskId(1)).is_empty());
    assert_eq!(spans.owners(), 0);
    assert_eq!(spans.abandoned(), 0);
    assert!(spans.retired(mine).is_none(), "nothing was left open");
}

#[test]
fn ten_thousand_ended_tasks_that_left_spans_open_record_a_bounded_list_of_span_names_and_a_count() {
    let mut spans = Spans::new();
    let mine = machine();
    for task in 0..10_000 {
        enter(&mut spans, (mine, Some(TaskId(task))), "http", "request");
        assert_eq!(spans.end_task(mine, TaskId(task)).len(), 1);
    }
    let noted = spans.retired(mine).expect("the tasks left spans open");
    assert_eq!(noted.count, 10_000);
    assert_eq!(noted.first.len(), NAMED);
    assert_eq!(spans.owners(), 0, "no retired task keeps a stack");

    let (closed, abandoned) = spans.end_entry_point(mine);
    assert!(closed.is_empty());
    assert_eq!(abandoned.count, 10_000);
    assert_eq!(abandoned.first.len(), NAMED);
    assert!(
        spans.retired(mine).is_none(),
        "the entry point reports them once"
    );
    let message = warn_abandoned(&abandoned).message;
    assert!(
        message.contains("10000 spans were still open when their task or the entry point ended"),
        "{message}"
    );
    assert!(message.contains("9997 more"), "{message}");
}

#[test]
fn ending_a_program_lists_its_own_open_spans_before_those_left_by_its_ended_tasks_and_ignores_other_programs()
 {
    let mut spans = Spans::new();
    let mine = machine();
    let theirs = machine();
    enter(&mut spans, (mine, Some(TaskId(1))), "db", "query");
    spans.end_task(mine, TaskId(1));
    enter(&mut spans, (theirs, Some(TaskId(1))), "http", "elsewhere");
    spans.end_task(theirs, TaskId(1));
    enter(&mut spans, (mine, None), "http", "serve");

    let (_, abandoned) = spans.end_entry_point(mine);
    let named: Vec<&str> = abandoned.first.iter().map(|(n, _)| n.as_ref()).collect();
    assert_eq!(abandoned.count, 2);
    assert_eq!(named, ["serve", "query"]);
    assert_eq!(
        spans.retired(theirs).map(|noted| noted.count),
        Some(1),
        "another machine's retired tasks wait for its own entry point"
    );
}

#[test]
fn ids_ascend_from_one_and_are_never_reused() {
    let mut spans = Spans::new();
    let owner = (machine(), None);
    let mut seen = Vec::new();
    for _ in 0..8 {
        let id = enter(&mut spans, owner, "http", "request");
        seen.push(id);
        spans
            .exit(owner, id, &channel("http"), Outcome::Ok)
            .unwrap_or_else(|_| panic!("it is open"));
    }
    assert_eq!(seen, (1..=8).collect::<Vec<i64>>());
    assert_eq!(spans.opened(), 8);
}
