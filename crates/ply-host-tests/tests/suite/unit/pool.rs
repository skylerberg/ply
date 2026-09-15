use ply_host::pool::{FS_FIRST_TOKEN, NET_FIRST_TOKEN};

/// The invariant `NET_FIRST_TOKEN` argues for, asserted rather than
/// described. A composed runtime asks each facility whether it minted a
/// token and the first one to say yes answers it, so two ranges that met
/// would not be a wrong answer — they would be a poll that never resolves.
#[test]
fn no_two_facilities_mint_the_same_token() {
    let ranges = [
        ("net", NET_FIRST_TOKEN),
        ("fs", FS_FIRST_TOKEN),
        ("db", ply_host::db::pool::FIRST_TOKEN),
    ];
    for (i, (whose, first)) in ranges.iter().enumerate() {
        assert!(
            *first > 0,
            "`{whose}` would mint the token a zeroed `Pending` carries"
        );
        for (other, next) in &ranges[i + 1..] {
            assert!(
                first < next,
                "`{whose}` starts at {first} and `{other}` at {next}: the ranges are not ordered"
            );
            // Unreachable rather than maximal: `net` starts at 1 because 0 is the token a
            // zeroed `Pending` carries, so the gap below it is one short of a power of two
            // and no choice of constants makes every gap exactly 2^62.
            assert!(
                next - first >= 1 << 61,
                "`{whose}` reaches `{other}` after {} operations",
                next - first
            );
        }
    }
}
