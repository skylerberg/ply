//! The helpers compiled code does arithmetic through.

use crate::bare::{context, helper};
use ply_compiled::rt::Ctx;

type WideOp = unsafe extern "C" fn(*mut Ctx, i64, i64, i64, i64, i64, i64) -> i64;

/// What `rt_wide_op` answers for `code` over `a` and `b`'s bits: whether it holds a value, and the
/// value's bits; `None` where it raised or failed.
fn wide(code: i64, a: u128, b: u128) -> Option<(i64, u128)> {
    let op: WideOp = helper("rt_wide_op");
    let mut ctx = context();
    ctx.begin(1_000);
    let mut out = [0i64; 2];
    let split = |x: u128| (x as u64 as i64, (x >> 64) as u64 as i64);
    let ((a_lo, a_hi), (b_lo, b_hi)) = (split(a), split(b));
    let held = unsafe {
        op(
            &mut ctx,
            code,
            a_lo,
            a_hi,
            b_lo,
            b_hi,
            out.as_mut_ptr() as i64,
        )
    };
    let stopped = ctx.take_failure().is_some();
    ctx.failed = 0;
    ctx.end();
    let bits = (u128::from(out[1] as u64) << 64) | u128::from(out[0] as u64);
    (!stopped).then_some((held, bits))
}

/// The code of operation `k` of the helper's table at a `U128`, or at an `I128`.
fn at(k: i64, signed: bool) -> i64 {
    k * 2 + i64::from(signed)
}

#[test]
fn a_128_bit_widths_arithmetic_is_rusts_at_its_edges() {
    let big = u128::MAX - 1;
    assert_eq!(wide(at(0, false), big, 1), Some((1, u128::MAX)));
    assert_eq!(
        wide(at(0, false), u128::MAX, 1),
        None,
        "a sum past the width"
    );
    assert_eq!(wide(at(1, true), 0, 1), Some((1, (-1i128) as u128)));
    assert_eq!(
        wide(at(2, true), (-3i128) as u128, 7),
        Some((1, (-21i128) as u128))
    );
    assert_eq!(wide(at(3, false), 100, 7), Some((1, 14)));
    assert_eq!(wide(at(4, false), 100, 7), Some((1, 2)));
    assert_eq!(wide(at(3, false), 100, 0), None, "a division by zero");
    assert_eq!(wide(at(5, false), 1, 100), Some((1, 1 << 100)));
    assert_eq!(
        wide(at(6, true), (-256i128) as u128, 4),
        Some((1, (-16i128) as u128))
    );
    assert_eq!(wide(at(7, true), (-1i128) as u128, 120), Some((1, 0xFF)));
    assert_eq!(wide(at(8, true), 5, 0), Some((1, (-5i128) as u128)));
    assert_eq!(wide(at(8, true), i128::MIN as u128, 0), None);
    assert_eq!(wide(at(9, false), u128::MAX, 2), Some((1, 1)));
    assert_eq!(
        wide(at(11, true), i128::MAX as u128, 2),
        Some((1, (-2i128) as u128))
    );
    assert_eq!(wide(at(12, false), u128::MAX, 5), Some((1, u128::MAX)));
    assert_eq!(wide(at(13, false), 3, 5), Some((1, 0)));
    assert_eq!(
        wide(at(14, true), i128::MIN as u128, 2),
        Some((1, i128::MIN as u128))
    );
    assert_eq!(wide(at(15, false), u128::MAX, 1), Some((0, 0)));
    assert_eq!(wide(at(16, false), 5, 3), Some((1, 2)));
    assert_eq!(wide(at(17, true), 1 << 70, 1 << 60), Some((0, 0)));
    assert_eq!(wide(at(18, true), 9, 0), Some((1, (-9i128) as u128)));
    assert_eq!(wide(at(18, false), 9, 0), Some((0, 0)));
    assert_eq!(wide(at(19, false), 1 << 127, 1), Some((1, 1)));
    assert_eq!(wide(at(20, false), 1, 1), Some((1, 1 << 127)));
    assert_eq!(
        wide(at(21, true), (-7i128) as u128, 0),
        Some((1, (-7i64) as u64 as u128))
    );
    assert_eq!(
        wide(at(21, false), 1 << 64, 0),
        None,
        "past what an `Int` holds"
    );
    assert_eq!(wide(at(22, false), 0, 0), None, "a code past the table");
}
