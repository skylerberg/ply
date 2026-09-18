use clap::Parser;
use ply_cli::cli::{Cli, Command};
use std::path::PathBuf;

#[test]
fn the_flags_parse_and_default_to_hermetic() {
    let args = match Cli::parse_from(["ply", "hosts"]).command {
        Command::Hosts(args) => args,
        other => panic!("expected `hosts`, got {other:?}"),
    };
    assert!(!args.host, "hermetic is the default");
    assert!(!args.json);
    assert!(!args.digest);
    assert_eq!(args.path, PathBuf::from("."));

    let args = match Cli::parse_from(["ply", "hosts", "src", "--host", "--digest"]).command {
        Command::Hosts(args) => args,
        other => panic!("expected `hosts`, got {other:?}"),
    };
    assert!(args.host);
    assert!(args.digest);
    assert_eq!(args.path, PathBuf::from("src"));
}

/// `--digest` is the one-line form a CI check pins.
#[test]
fn digest_and_json_cannot_both_be_asked_for() {
    assert!(Cli::try_parse_from(["ply", "hosts", "--digest", "--json"]).is_err());
}
