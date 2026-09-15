use ply_cli::commands::build::*;
use ply_span::Symbol;
use std::path::PathBuf;

#[test]
fn the_default_output_is_named_after_the_entry_points_module() {
    assert_eq!(
        default_output(&Symbol::new("desk.run")),
        PathBuf::from("desk.plyx")
    );
    assert_eq!(
        default_output(&Symbol::new("store.orders.main")),
        PathBuf::from("orders.plyx")
    );
}

#[test]
fn a_long_name_list_is_counted_rather_than_printed_whole() {
    let all: Vec<String> = (0..20).map(|i| format!("m.d{i}")).collect();
    let line = names(&all);
    assert!(line.ends_with("and 12 more"), "{line}");
    assert_eq!(names(&all[..3]), "m.d0, m.d1, m.d2");
}

#[test]
fn sizes_read_as_sizes() {
    assert_eq!(human(512), "512 B");
    assert_eq!(human(2048), "2.0 KB");
    assert_eq!(human(3 * 1024 * 1024), "3.0 MB");
}
