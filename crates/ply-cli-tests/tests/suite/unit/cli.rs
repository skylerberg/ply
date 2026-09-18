use clap::{CommandFactory, Parser};
use ply_cli::cli::*;
use ply_cli::style::ColorChoice;
use std::path::PathBuf;

#[test]
fn the_command_tree_is_well_formed() {
    Cli::command().debug_assert();
}

#[test]
fn path_defaults_to_the_current_directory() {
    let cli = Cli::parse_from(["ply", "test"]);
    match cli.command {
        Command::Test(args) => assert_eq!(args.path, PathBuf::from(".")),
        other => panic!("expected `test`, got {other:?}"),
    }
}

#[test]
fn color_is_accepted_after_the_subcommand() {
    let cli = Cli::parse_from(["ply", "test", "--color", "always"]);
    assert_eq!(cli.color, ColorChoice::Always);
}

#[test]
fn zero_jobs_is_rejected_rather_than_silently_meaning_auto() {
    assert!(Cli::try_parse_from(["ply", "test", "--jobs", "0"]).is_err());
    let cli = Cli::parse_from(["ply", "test", "-j", "4"]);
    match cli.command {
        Command::Test(args) => assert_eq!(args.jobs, Some(4)),
        other => panic!("expected `test`, got {other:?}"),
    }
}

#[test]
fn cache_requires_an_action() {
    assert!(Cli::try_parse_from(["ply", "cache"]).is_err());
    assert!(Cli::try_parse_from(["ply", "cache", "stats"]).is_ok());
    assert!(Cli::try_parse_from(["ply", "cache", "clear"]).is_ok());
    assert!(Cli::try_parse_from(["ply", "cache", "compact"]).is_ok());
}

#[test]
fn inspect_needs_something_to_look_up() {
    assert!(Cli::try_parse_from(["ply", "cache", "inspect"]).is_err());
    let cli = Cli::parse_from(["ply", "cache", "inspect", "9f2c", "src"]);
    match cli.command {
        Command::Cache(args) => match args.action {
            CacheAction::Inspect(inspect) => {
                assert_eq!(inspect.query, "9f2c");
                assert_eq!(inspect.path, PathBuf::from("src"));
            }
            other => panic!("expected `inspect`, got {other:?}"),
        },
        other => panic!("expected `cache`, got {other:?}"),
    }
}

#[test]
fn a_tls_credential_parses_into_its_name_and_its_two_files() {
    let cli = Cli::parse_from([
        "ply",
        "run",
        "--host",
        "--tls",
        "api=certs/api.pem,certs/api.key",
        "--tls",
        "admin=certs/admin.pem,certs/admin.key",
    ]);
    let args = match cli.command {
        Command::Run(args) => args,
        other => panic!("expected `run`, got {other:?}"),
    };
    let names: Vec<&str> = args.tls.tls.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, ["api", "admin"]);
    assert_eq!(args.tls.tls[0].certificate, PathBuf::from("certs/api.pem"));
    assert_eq!(args.tls.tls[0].key, PathBuf::from("certs/api.key"));
}

#[test]
fn a_credential_that_is_not_name_cert_key_is_refused_with_the_form() {
    for bad in ["api", "api=only.pem", "=a.pem,b.key", "api=,b.key"] {
        let err = Cli::try_parse_from(["ply", "run", "--host", "--tls", bad])
            .expect_err("`{bad}` must not parse as a credential");
        assert!(
            err.to_string().contains("--tls NAME=CERT.pem,KEY.pem"),
            "`{bad}` was refused without saying what to write: {err}"
        );
    }
}

#[test]
fn tls_without_host_is_refused_rather_than_ignored() {
    assert!(Cli::try_parse_from(["ply", "run", "--tls", "api=a.pem,b.key"]).is_err());
    assert!(Cli::try_parse_from(["ply", "test", "--tls", "api=a.pem,b.key"]).is_err());
    assert!(Cli::try_parse_from(["ply", "hosts", "--tls", "api=a.pem,b.key"]).is_err());
    assert!(Cli::try_parse_from(["ply", "hosts", "--host", "--tls", "api=a.pem,b.key"]).is_ok());
}

#[test]
fn the_bisection_switches_default_to_auto_and_reject_a_fourth_word() {
    let cli = Cli::parse_from(["ply", "test"]);
    match cli.command {
        Command::Test(args) => {
            assert_eq!(args.bisect, When::Auto);
            assert_eq!(args.trace, When::Auto);
            assert_eq!(args.bisect_budget, 64);
        }
        other => panic!("expected `test`, got {other:?}"),
    }
    assert!(Cli::try_parse_from(["ply", "test", "--bisect", "sometimes"]).is_err());
    assert!(Cli::try_parse_from(["ply", "test", "--bisect", "never"]).is_ok());
}
