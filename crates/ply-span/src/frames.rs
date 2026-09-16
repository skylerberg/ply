//! Diagnostics as length-framed text: what the self-hosted front end answers over a program,
//! read back into [`Diagnostic`] values, and the same text written from them, so the two sides
//! of a differential encode one way and cannot drift apart.
//!
//! ```text
//! diag <index> <payload length>\n<payload>
//! ```
//!
//! One frame per diagnostic, in report order. A payload is a run of fields, each
//! `<key> <length>\n<bytes>` with the next field starting right after the bytes:
//!
//! ```text
//! code <n>\nE0201
//! severity <n>\nerror                                   error | warning
//! message <n>\n<message text>
//! label <n>\n<module> <start> <end> <0|1>\n<label text>  one per label, the primary first
//! note <n>\n<note text>                                  one per note, in order
//! ```
//!
//! Every length is in bytes, so no payload byte is a delimiter. A module is its index into the
//! source list both sides were handed, which the caller builds in `SourceMap` order so that the
//! index is the `SourceId`.

use crate::{Diagnostic, Label, Severity, SourceId, Span, codes};

/// The module index of a span outside every module: [`Span::DUMMY`]'s source.
const NO_MODULE: u32 = u32::MAX;

/// The frame text for `diags`, each label's module written as its position in `sources`.
pub fn write_diagnostics(diags: &[Diagnostic], sources: &[SourceId]) -> Result<String, String> {
    let mut out = String::new();
    for (index, d) in diags.iter().enumerate() {
        let mut payload = String::new();
        field(&mut payload, "code", d.code);
        field(&mut payload, "severity", severity_name(d.severity, index)?);
        field(&mut payload, "message", &d.message);
        for l in &d.labels {
            let module = module_index(l.span, sources, index)?;
            field(
                &mut payload,
                "label",
                &format!(
                    "{module} {} {} {}\n{}",
                    l.span.start,
                    l.span.end,
                    u8::from(l.primary),
                    l.message
                ),
            );
        }
        for n in &d.notes {
            field(&mut payload, "note", n);
        }
        out.push_str(&format!("diag {index} {}\n{payload}", payload.len()));
    }
    Ok(out)
}

fn field(out: &mut String, key: &str, text: &str) {
    out.push_str(&format!("{key} {}\n{text}", text.len()));
}

fn severity_name(severity: Severity, index: usize) -> Result<&'static str, String> {
    match severity {
        Severity::Error => Ok("error"),
        Severity::Warning => Ok("warning"),
        Severity::Note => Err(format!(
            "diagnostic {index} is a note, which the frame protocol has no severity for"
        )),
    }
}

fn module_index(span: Span, sources: &[SourceId], index: usize) -> Result<u32, String> {
    if span.is_dummy() {
        return Ok(NO_MODULE);
    }
    sources
        .iter()
        .position(|s| *s == span.source)
        .map(|i| i as u32)
        .ok_or_else(|| {
            format!(
                "diagnostic {index} labels source {}, which is not among the {} sources handed over",
                span.source.0,
                sources.len()
            )
        })
}

/// Every diagnostic in `dump`, each label's module read as an index into `sources`. A frame kind,
/// a field key or a code the protocol does not know is an error naming it, and so is a frame or a
/// field that ends before its length says.
pub fn read_diagnostics(dump: &str, sources: &[SourceId]) -> Result<Vec<Diagnostic>, String> {
    let mut frames = Cursor {
        bytes: dump.as_bytes(),
        at: 0,
        what: "frame",
    };
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
    let mut fields = Cursor {
        bytes: payload,
        at: 0,
        what: "field",
    };
    let (mut code, mut severity, mut message) = (None, None, None);
    let mut labels = Vec::new();
    let mut notes = Vec::new();
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
    let mut d = by_code(code, required(message, "message", index)?)
        .ok_or_else(|| format!("diagnostic {index}: unknown code `{code}`"))?;
    d.severity = severity;
    d.labels = labels;
    d.notes = notes;
    Ok(d)
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

/// Reads `<words...> <length>\n<bytes>` units off a byte string.
struct Cursor<'a> {
    bytes: &'a [u8],
    at: usize,
    what: &'static str,
}

impl<'a> Cursor<'a> {
    fn done(&self) -> bool {
        self.at >= self.bytes.len()
    }

    /// The next unit's header words, without its length, and the bytes the length framed.
    fn unit(&mut self) -> Result<(Vec<&'a str>, &'a [u8]), String> {
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

/// The codes the self-hosted front end raises, each built through its own constructor so that a
/// code the Rust front end stops raising stays constructed here. A code outside this set is not one
/// the port answers, and the reader refuses it rather than guessing a constant for it.
fn by_code(code: &str, message: &str) -> Option<Diagnostic> {
    Some(match code {
        "E0001" => Diagnostic::error(codes::UNEXPECTED_TOKEN, message),
        "E0002" => Diagnostic::error(codes::UNTERMINATED_STRING, message),
        "E0101" => Diagnostic::error(codes::UNKNOWN_NAME, message),
        "E0102" => Diagnostic::error(codes::UNKNOWN_TYPE, message),
        "E0103" => Diagnostic::error(codes::UNKNOWN_EFFECT, message),
        "E0104" => Diagnostic::error(codes::UNKNOWN_OPERATION, message),
        "E0105" => Diagnostic::error(codes::DUPLICATE_DEFINITION, message),
        "E0106" => Diagnostic::error(codes::UNKNOWN_MODULE, message),
        "E0107" => Diagnostic::error(codes::PRIVATE_NAME, message),
        "E0108" => Diagnostic::error(codes::AMBIGUOUS_IMPORT, message),
        "E0109" => Diagnostic::error(codes::MODULE_CYCLE, message),
        "E0110" => Diagnostic::error(codes::DUPLICATE_IMPORT, message),
        "E0114" => Diagnostic::error(codes::UNKNOWN_EFFECT_SET, message),
        "E0115" => Diagnostic::error(codes::EFFECT_SET_CYCLE, message),
        "E0116" => Diagnostic::error(codes::RECORD_UPDATE_SHAPE, message),
        "E0117" => Diagnostic::error(codes::RECORD_UPDATE_FIELD, message),
        "E0118" => Diagnostic::error(codes::TRY_SCOPE, message),
        "E0119" => Diagnostic::error(codes::TRY_POSITION, message),
        "E0120" => Diagnostic::error(codes::DEFAULT_NOT_ALLOWED, message),
        "E0121" => Diagnostic::error(codes::DEFAULT_NOT_PURE, message),
        "E0122" => Diagnostic::error(codes::DEFAULT_PRIVATE_NAME, message),
        "E0123" => Diagnostic::error(codes::UNKNOWN_ARGUMENT_NAME, message),
        "E0124" => Diagnostic::error(codes::ARGUMENT_ORDER, message),
        "E0125" => Diagnostic::error(codes::MISSING_ARGUMENT, message),
        "E0126" => Diagnostic::error(codes::MISSING_SIGNATURE, message),
        "E0201" => Diagnostic::error(codes::TYPE_MISMATCH, message),
        "E0202" => Diagnostic::error(codes::ARITY_MISMATCH, message),
        "E0203" => Diagnostic::error(codes::OCCURS_CHECK, message),
        "E0204" => Diagnostic::error(codes::NOT_A_FUNCTION, message),
        "E0205" => Diagnostic::error(codes::NON_EXHAUSTIVE_MATCH, message),
        "E0206" => Diagnostic::error(codes::NOT_DERIVABLE, message),
        "E0207" => Diagnostic::error(codes::UNKNOWN_DERIVER, message),
        "E0208" => Diagnostic::error(codes::ORPHAN_DERIVE, message),
        "E0209" => Diagnostic::error(codes::DECIMAL_DIVISION, message),
        "E0210" => Diagnostic::error(codes::NUMERIC_UNDETERMINED, message),
        "E0211" => Diagnostic::error(codes::LITERAL_OUT_OF_RANGE, message),
        "E0301" => Diagnostic::error(codes::UNBOUND_ROW_VAR, message),
        "E0302" => Diagnostic::error(codes::EFFECT_NOT_PERMITTED, message),
        "E0304" => Diagnostic::error(codes::RESOURCE_REQUIRED, message),
        "E0412" => Diagnostic::error(codes::NONDET_IN_DET_TEST, message),
        "E0413" => Diagnostic::error(codes::TASK_ESCAPES_SCOPE, message),
        "E0416" => Diagnostic::error(codes::NESTED_SIMULATION, message),
        "E0417" => Diagnostic::error(codes::EFFECT_IN_SPEC, message),
        "E0418" => Diagnostic::error(codes::UNQUANTIFIABLE_TYPE, message),
        "E0446" => Diagnostic::error(codes::REGION_ESCAPE, message),
        "E0447" => Diagnostic::error(codes::REGION_ALREADY_OPEN, message),
        "E0505" => Diagnostic::error(codes::INTERNAL_ERROR, message),
        _ => return None,
    })
}
