use ply_host::pool::{FS_FIRST_TOKEN, NET_FIRST_TOKEN};

/// The first facility to claim a token answers it, so overlapping ranges would hang a poll forever.
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
            // Not maximal: `net` starts at 1 (a zeroed `Pending` carries 0), so no gap is 2^62.
            assert!(
                next - first >= 1 << 61,
                "`{whose}` reaches `{other}` after {} operations",
                next - first
            );
        }
    }
}
