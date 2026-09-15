use ply_cli::style::*;

#[test]
fn a_plain_style_emits_no_escapes() {
    let s = Style::plain();
    assert_eq!(s.green("ok"), "ok");
    assert_eq!(s.bold("ok"), "ok");
    assert!(!s.is_styled());
}

#[test]
fn a_styled_run_wraps_and_resets() {
    let s = Style::new(true);
    assert_eq!(s.red("no"), "\x1b[31mno\x1b[0m");
}

#[test]
fn strip_ansi_removes_colour_but_keeps_the_text() {
    let painted = "\x1b[31mError\x1b[0m: \x1b[1;38;5;9mboom\x1b[0m\n";
    assert_eq!(strip_ansi(painted), "Error: boom\n");
}

#[test]
fn strip_ansi_leaves_a_lone_escape_at_the_end_alone() {
    assert_eq!(strip_ansi("abc\x1b"), "abc");
    assert_eq!(strip_ansi("a\x1b[mb"), "ab");
}

#[test]
fn strip_ansi_is_a_no_op_on_ordinary_text() {
    let text = "group 0 · 2 tests · {db.read[users]}\n";
    assert_eq!(strip_ansi(text), text);
}

#[test]
fn sanitize_only_strips_when_unstyled() {
    let painted = "\x1b[31mno\x1b[0m";
    assert_eq!(Style::plain().sanitize(painted), "no");
    assert_eq!(Style::new(true).sanitize(painted), painted);
}

#[test]
fn never_beats_a_terminal_and_always_beats_a_pipe() {
    assert!(!Style::detect(ColorChoice::Never).is_styled());
    assert!(Style::detect(ColorChoice::Always).is_styled());
}
