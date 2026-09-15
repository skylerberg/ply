use clap::Parser;
use ply_cli::cli::{Cli, Command};
use ply_cli::commands::stdlib::*;

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

/// `--digest` is the one-line form a CI check pins, so it may not also carry a table for a
/// machine.
#[test]
fn digest_and_json_cannot_both_be_asked_for() {
    assert!(Cli::try_parse_from(["ply", "std", "--digest", "--json"]).is_err());
}

#[test]
fn the_listing_names_every_shipped_module_and_ends_with_the_digest() {
    let rows = rows().expect("every shipped module parses");
    assert_eq!(rows.len(), ply_std::MODULES.len());
    let text = lines(&rows).join("\n");
    for (name, _) in ply_std::sources() {
        assert!(text.contains(name), "`{name}` is missing from:\n{text}");
    }
    assert!(
        lines(&rows).last().unwrap().starts_with("digest: b3:"),
        "{text}"
    );
    assert!(rows.iter().all(|r| r.definitions > 0), "a module is empty");
}

/// Two runs of one binary have to agree byte for byte, or pinning the digest in CI pins
/// nothing.
#[test]
fn the_listing_is_stable_across_runs() {
    let once = lines(&rows().unwrap());
    let twice = lines(&rows().unwrap());
    assert_eq!(once, twice);
    assert_eq!(ply_std::digest_short(), ply_std::digest_short());
}
