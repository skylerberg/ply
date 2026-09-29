//! Diagnostics as length-framed text, as `crates/ply-compiler/ply/diag.ply` writes them.

use crate::{Diagnostic, Edit, Fix, Label, Severity, SourceId, Span, intern_code};

/// The module index of a span outside every module: [`Span::DUMMY`]'s source.
const NO_MODULE: u32 = u32::MAX;

/// Each label's module is read as an index into `sources`; an unknown frame or field, or truncated
/// input, is an error, and any code is taken as written.
pub fn read_diagnostics(dump: &str, sources: &[SourceId]) -> Result<Vec<Diagnostic>, String> {
    let mut frames = Cursor::new(dump.as_bytes(), "frame");
    let mut out = Vec::new();
    while !frames.done() {
        let index = out.len();
        let (words, payload) = frames.unit()?;
        match words[..] {
            ["diag", n] if n == index.to_string() => {}
            ["diag", n] => {
                return Err(format!(
                    "frame {index} is numbered {n}; frames are numbered in report order"
                ));
            }
            [kind, ..] => return Err(format!("frame {index}: unknown frame kind `{kind}`")),
            [] => return Err(format!("frame {index} has no kind")),
        }
        out.push(read_one(payload, sources, index)?);
    }
    Ok(out)
}

fn read_one(payload: &[u8], sources: &[SourceId], index: usize) -> Result<Diagnostic, String> {
    let mut fields = Cursor::new(payload, "field");
    let (mut code, mut severity, mut message) = (None, None, None);
    let mut labels = Vec::new();
    let mut notes = Vec::new();
    let mut fixes: Vec<Fix> = Vec::new();
    while !fields.done() {
        let (words, body) = fields.unit()?;
        let [key] = words[..] else {
            return Err(format!(
                "diagnostic {index}: field header `{}` is not `<key> <length>`",
                words.join(" ")
            ));
        };
        let text = std::str::from_utf8(body)
            .map_err(|e| format!("diagnostic {index}: `{key}` is not UTF-8: {e}"))?;
        match key {
            "code" => once(&mut code, key, text, index)?,
            "severity" => once(&mut severity, key, text, index)?,
            "message" => once(&mut message, key, text, index)?,
            "label" => labels.push(label(text, sources, index)?),
            "note" => notes.push(text.to_string()),
            "fix" => fixes.push(Fix {
                title: text.to_string(),
                edits: Vec::new(),
            }),
            "edit" => match fixes.last_mut() {
                Some(f) => f.edits.push(edit(text, sources, index)?),
                None => return Err(format!("diagnostic {index}: an `edit` before any `fix`")),
            },
            other => return Err(format!("diagnostic {index}: unknown field `{other}`")),
        }
    }
    fn required<'a>(field: Option<&'a str>, key: &str, index: usize) -> Result<&'a str, String> {
        field.ok_or_else(|| format!("diagnostic {index} has no `{key}`"))
    }
    let code = required(code, "code", index)?;
    let severity = match required(severity, "severity", index)? {
        "error" => Severity::Error,
        "warning" => Severity::Warning,
        other => return Err(format!("diagnostic {index}: unknown severity `{other}`")),
    };
    Ok(Diagnostic {
        severity,
        code: intern_code(code),
        message: required(message, "message", index)?.to_string(),
        labels,
        notes,
        fixes,
    })
}

/// `<module> <start> <end>\n<text>`.
fn edit(text: &str, sources: &[SourceId], index: usize) -> Result<Edit, String> {
    let (head, body) = text
        .split_once('\n')
        .ok_or_else(|| format!("diagnostic {index}: an edit with no text line"))?;
    let words: Vec<&str> = head.split(' ').collect();
    let [module, start, end] = words[..] else {
        return Err(format!(
            "diagnostic {index}: edit header `{head}` is not `<module> <start> <end>`"
        ));
    };
    let number = |word: &str| {
        word.parse::<u32>()
            .map_err(|_| format!("diagnostic {index}: edit header `{head}` holds `{word}`"))
    };
    let module = number(module)?;
    let source = if module == NO_MODULE {
        Span::DUMMY.source
    } else {
        sources.get(module as usize).copied().ok_or_else(|| {
            format!(
                "diagnostic {index} edits module {module}, and only {} sources were handed over",
                sources.len()
            )
        })?
    };
    Ok(Edit {
        span: Span::new(source, number(start)?, number(end)?),
        text: body.to_string(),
    })
}

fn once<'a>(
    slot: &mut Option<&'a str>,
    key: &str,
    text: &'a str,
    index: usize,
) -> Result<(), String> {
    if slot.replace(text).is_some() {
        return Err(format!("diagnostic {index} has two `{key}` fields"));
    }
    Ok(())
}

fn label(text: &str, sources: &[SourceId], index: usize) -> Result<Label, String> {
    let (head, message) = text.split_once('\n').ok_or_else(|| {
        format!("diagnostic {index}: a label has no `<module> <start> <end> <0|1>` line")
    })?;
    let number = |word: &str| {
        word.parse::<u32>()
            .map_err(|_| format!("diagnostic {index}: label header `{head}` holds `{word}`"))
    };
    let words: Vec<&str> = head.split(' ').collect();
    let [module, start, end, primary] = words[..] else {
        return Err(format!(
            "diagnostic {index}: label header `{head}` is not `<module> <start> <end> <0|1>`"
        ));
    };
    let module = number(module)?;
    let source = if module == NO_MODULE {
        Span::DUMMY.source
    } else {
        sources.get(module as usize).copied().ok_or_else(|| {
            format!(
                "diagnostic {index} labels module {module}, and only {} sources were handed over",
                sources.len()
            )
        })?
    };
    let primary = match primary {
        "0" => false,
        "1" => true,
        other => {
            return Err(format!(
                "diagnostic {index}: a label's primary flag is `{other}`, not 0 or 1"
            ));
        }
    };
    Ok(Label {
        span: Span::new(source, number(start)?, number(end)?),
        message: message.to_string(),
        primary,
    })
}

/// Reads `<words...> <length>\n<bytes>` units; shared with the other protocols framed this way.
pub struct Cursor<'a> {
    bytes: &'a [u8],
    at: usize,
    what: &'static str,
}

impl<'a> Cursor<'a> {
    /// `what` names a unit in errors: `frame`, `field`.
    pub fn new(bytes: &'a [u8], what: &'static str) -> Self {
        Cursor { bytes, at: 0, what }
    }

    pub fn done(&self) -> bool {
        self.at >= self.bytes.len()
    }

    /// The byte offset the next unit starts at.
    pub fn at(&self) -> usize {
        self.at
    }

    /// The header words (length excluded) and the bytes the length framed.
    pub fn unit(&mut self) -> Result<(Vec<&'a str>, &'a [u8]), String> {
        let rest = &self.bytes[self.at..];
        let Some(nl) = rest.iter().position(|b| *b == b'\n') else {
            return Err(format!(
                "{} header at byte {} never ends",
                self.what, self.at
            ));
        };
        let header = std::str::from_utf8(&rest[..nl])
            .map_err(|e| format!("{} header at byte {} is not UTF-8: {e}", self.what, self.at))?;
        let mut words: Vec<&str> = header.split(' ').collect();
        let length: usize = words
            .pop()
            .and_then(|w| w.parse().ok())
            .ok_or_else(|| format!("{} header `{header}` does not end in a length", self.what))?;
        let body = rest.get(nl + 1..nl + 1 + length).ok_or_else(|| {
            format!(
                "{} `{header}` is truncated: {} of {length} bytes follow it",
                self.what,
                rest.len() - nl - 1
            )
        })?;
        self.at += nl + 1 + length;
        Ok((words, body))
    }
}
