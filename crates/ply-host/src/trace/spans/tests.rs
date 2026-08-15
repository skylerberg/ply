use super::*;
use ply_eval::TaskId;
use ply_span::Symbol;

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

/// The shape every request has: one span inside another, closed innermost first,
/// with the inner one's `parent` naming the outer.
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

/// The rollback case, and the reason `exit` closes more than one span. The
/// clause that answered the rollback discarded the continuation, so the two
/// spans above it never ran their own `exit` and there is nothing else they
/// could be.
#[test]
fn closing_an_outer_span_abandons_every_span_above_it_innermost_first() {
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

/// The defect this whole key exists to prevent. Two tasks of one entry point
/// each open a span; if the table were keyed on the machine alone the second
/// would nest inside the first, and one request's timing would be reported under
/// another request's span.
#[test]
fn two_tasks_of_one_machine_keep_separate_stacks() {
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

    // Interleaved: the second task closes first, and neither closes the other's.
    let closed = spans
        .exit(second, b, &channel("http"), Outcome::Ok)
        .unwrap_or_else(|_| panic!("task 2's span is open"));
    assert_eq!(closed.len(), 1);
    assert_eq!(spans.depth(first), 1);
    assert_eq!(spans.abandoned(), 0);
}

/// A span opened in one task must not close in another, and the refusal names
/// both.
#[test]
fn a_span_opened_by_one_task_cannot_be_closed_by_another() {
    let mut spans = Spans::new();
    let id = machine();
    let first = (id, Some(TaskId(1)));
    let second = (id, Some(TaskId(2)));
    let opened = enter(&mut spans, first, "http", "request");

    let Err(why) = spans.exit(second, opened, &channel("http"), Outcome::Ok) else {
        panic!("task 2 closed task 1's span");
    };
    assert!(matches!(why, Unbalanced::OtherOwner(owner) if owner == first));

    let diagnostic = err_unbalanced(Span::DUMMY, "`trace.exit`", opened, &why);
    assert_eq!(diagnostic.code, codes::SPAN_UNBALANCED);
    assert!(
        diagnostic.notes.iter().any(|n| n.contains("task @1")),
        "the refusal must name the task that holds it: {diagnostic:?}"
    );
    // And the span is still open, because refusing is not closing.
    assert_eq!(spans.depth(first), 1);
}

#[test]
fn the_three_ways_an_exit_names_a_span_that_is_not_open_are_told_apart() {
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

    // A forged `Span` whose id collides with an open one on another channel. A
    // channel is part of a span's identity because it is part of the atom the
    // row carries.
    let again = enter(&mut spans, owner, "http", "request");
    let Err(elsewhere) = spans.exit(owner, again, &channel("orders"), Outcome::Ok) else {
        panic!("the channel disagrees");
    };
    assert!(matches!(elsewhere, Unbalanced::OtherChannel(_)));
    assert_eq!(spans.depth(owner), 1, "a refusal closes nothing");
}

/// The fourth exit, the one no clause can catch: the entry point ended.
#[test]
fn teardown_closes_this_machines_spans_and_leaves_every_other_machines_alone() {
    let mut spans = Spans::new();
    let mine = machine();
    let theirs = machine();
    let outer = enter(&mut spans, (mine, None), "http", "request");
    let inner = enter(&mut spans, (mine, Some(TaskId(1))), "db", "query");
    enter(&mut spans, (theirs, None), "http", "request");

    let closed = spans.end_entry_point(mine);
    let ids: Vec<i64> = closed.iter().map(|c| c.open.id).collect();
    assert_eq!(ids.len(), 2, "both of this machine's owners");
    assert!(ids.contains(&outer) && ids.contains(&inner));
    assert!(closed.iter().all(|c| c.outcome == Outcome::Abandoned));
    assert_eq!(spans.total_open(), 1, "the other machine is untouched");
    assert_eq!(spans.abandoned(), 2);

    let warning = warn_abandoned(&closed);
    assert_eq!(warning.code, codes::SPAN_ABANDONED);
    assert!(warning.message.contains('2'), "{}", warning.message);
}

/// Innermost first inside one task, so the warning names the span the
/// computation was actually inside when it stopped.
#[test]
fn teardown_reports_the_innermost_span_first() {
    let mut spans = Spans::new();
    let mine = machine();
    let owner = (mine, None);
    enter(&mut spans, owner, "http", "request");
    enter(&mut spans, owner, "http", "query");

    let closed = spans.end_entry_point(mine);
    assert_eq!(closed[0].open.name.as_ref(), "query");
    assert!(
        warn_abandoned(&closed).message.contains("`query`"),
        "the warning names the innermost span"
    );
}

/// Ids are the correlation key a log is read by, so a reused one is two
/// requests' records that cannot be told apart.
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
