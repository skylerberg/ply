use super::*;
use ply_span::{SourceId, Span};

fn span(start: u32) -> Span {
    Span {
        source: SourceId(0),
        start,
        end: start + 1,
    }
}

#[test]
fn a_statement_is_scanned_once_and_the_answer_is_reused() {
    let cache = Cache::default();
    let first = cache
        .scan("select sku from items", Span::DUMMY)
        .expect("it scans");
    let second = cache
        .scan("select sku from items", Span::DUMMY)
        .expect("it scans");
    assert_eq!(first, second);
    assert_eq!(cache.len(), 1);
    cache.scan("select sku from orders", Span::DUMMY).ok();
    assert_eq!(cache.len(), 2);
}

/// A refusal is cached too — a statement the driver will not run is refused identically every time
/// — but the *span* is the perform's, not the one that first produced it, or every later refusal
/// would point a reader at the wrong line.
#[test]
fn a_cached_refusal_points_at_the_perform_that_asked_for_it() {
    let cache = Cache::new(8);
    let first = cache
        .scan("drop table items", span(10))
        .expect_err("it is refused");
    let second = cache
        .scan("drop table items", span(99))
        .expect_err("it is refused");
    assert_eq!(first.code, second.code);
    assert_eq!(first.message, second.message);
    assert_eq!(first.labels[0].span, span(10));
    assert_eq!(second.labels[0].span, span(99));
}

/// A program generating statement text is the case where nothing would have hit the cache anyway,
/// so overflow costs a rescan rather than unbounded memory.
#[test]
fn the_cache_is_bounded() {
    let cache = Cache::new(4);
    for i in 0..40 {
        cache
            .scan(&format!("select {i} from items"), Span::DUMMY)
            .expect("it scans");
    }
    assert!(cache.len() <= 4, "{}", cache.len());
    assert!(!cache.is_empty());
}

#[test]
fn the_scan_the_cache_hands_back_is_the_one_the_scanner_computed() {
    let cache = Cache::default();
    let scan = cache
        .scan(
            "select * from orders join items on items.sku = orders.sku",
            Span::DUMMY,
        )
        .expect("it scans");
    assert_eq!(
        scan.tables.all().into_iter().collect::<Vec<_>>(),
        ["items", "orders"]
    );
}
