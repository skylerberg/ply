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
