//! A record's fields as `std.trace`'s real handler writes them for its natives: parts, a name and
//! then its field, each field its kind's byte, 0 an integer and 3 a float in eight bytes, the most
//! significant first, 1 a flag's byte, 2 text, 4 a decimal's text, 5 bytes and 6 JSON's text as
//! `std.json` writes it, then what it holds.

use super::sink::Field;
use crate::native::{int_of, parts_of};
use ply_core::Decimal;
use std::str::FromStr;

/// The fields `flat` writes, in the order it writes them; `None` for bytes that write none.
pub fn fields_read(flat: &[u8]) -> Option<Vec<(String, Field)>> {
    let parts = parts_of(flat)?;
    if parts.len() % 2 != 0 {
        return None;
    }
    parts
        .chunks(2)
        .map(|pair| Some((text(pair[0])?.to_string(), field(pair[1])?)))
        .collect()
}

fn field(flat: &[u8]) -> Option<Field> {
    let (kind, rest) = flat.split_first()?;
    Some(match kind {
        0 => Field::Int(int_of(rest)?),
        1 => Field::Bool(match rest {
            [0] => false,
            [1] => true,
            _ => return None,
        }),
        2 => Field::Text(text(rest)?.to_string()),
        3 => Field::Float(f64::from_bits(int_of(rest)? as u64)),
        4 => Field::Decimal(Decimal::from_str(text(rest)?).ok()?),
        5 => Field::Bytes(rest.to_vec()),
        6 => Field::Json(text(rest)?.to_string()),
        _ => return None,
    })
}

fn text(bytes: &[u8]) -> Option<&str> {
    std::str::from_utf8(bytes).ok()
}
