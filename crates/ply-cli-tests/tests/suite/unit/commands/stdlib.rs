use clap::Parser;
use ply_cli::cli::{Cli, Command};

#[test]
fn the_flags_parse_and_default_to_the_listing() {
    let args = match Cli::parse_from(["ply", "std"]).command {
        Command::Std(args) => args,
        other => panic!("expected `std`, got {other:?}"),
    };
    assert!(!args.json);
    assert!(!args.digest);
    assert_eq!(args.show, None);
}

/// `--digest` is the one-line form a CI check pins.
#[test]
fn digest_and_json_cannot_both_be_asked_for() {
    assert!(Cli::try_parse_from(["ply", "std", "--digest", "--json"]).is_err());
}
