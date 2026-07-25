use std::io::IsTerminal;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, clap::ValueEnum)]
pub enum ColorChoice {
    #[default]
    Auto,
    Always,
    Never,
}

/// Colour and the ✓/✗ marks are one decision, not two: both are decoration that
/// a pipe should never receive.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Style {
    styled: bool,
}

impl Style {
    pub fn plain() -> Style {
        Style { styled: false }
    }

    pub fn new(styled: bool) -> Style {
        Style { styled }
    }

    pub fn detect(choice: ColorChoice) -> Style {
        let styled = match choice {
            ColorChoice::Always => true,
            ColorChoice::Never => false,
            ColorChoice::Auto => {
                std::io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none()
            }
        };
        Style { styled }
    }

    pub fn is_styled(self) -> bool {
        self.styled
    }

    fn paint(self, code: &str, text: &str) -> String {
        if self.styled {
            format!("\x1b[{code}m{text}\x1b[0m")
        } else {
            text.to_string()
        }
    }

    pub fn bold(self, text: &str) -> String {
        self.paint("1", text)
    }

    pub fn dim(self, text: &str) -> String {
        self.paint("2", text)
    }

    pub fn red(self, text: &str) -> String {
        self.paint("31", text)
    }

    pub fn green(self, text: &str) -> String {
        self.paint("32", text)
    }

    pub fn yellow(self, text: &str) -> String {
        self.paint("33", text)
    }

    /// Terminal diagnostics come back from `ariadne` already coloured, and it
    /// offers no way to ask after the fact. Stripping is the only way to keep
    /// escapes out of a pipe.
    pub fn sanitize(self, text: &str) -> String {
        if self.styled {
            text.to_string()
        } else {
            strip_ansi(text)
        }
    }
}

/// Drops CSI (`ESC [ … final`) and the two-character escapes; anything else
/// after an `ESC` is passed through rather than guessed at.
pub fn strip_ansi(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\x1b' {
            out.push(c);
            continue;
        }
        match chars.peek() {
            Some('[') => {
                chars.next();
                for c in chars.by_ref() {
                    if ('\x40'..='\x7e').contains(&c) {
                        break;
                    }
                }
            }
            Some(_) => {
                chars.next();
            }
            None => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
