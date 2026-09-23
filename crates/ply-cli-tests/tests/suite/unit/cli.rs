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

/// Every long flag `clap` declares anywhere in the command tree, and whether it takes a value.
fn clap_flags() -> Vec<(String, bool)> {
    fn walk(cmd: &clap::Command, out: &mut Vec<(String, bool)>) {
        for arg in cmd.get_arguments() {
            if let Some(long) = arg.get_long() {
                let takes = matches!(
                    arg.get_action(),
                    clap::ArgAction::Set | clap::ArgAction::Append
                );
                out.push((format!("--{long}"), takes));
            }
        }
        for sub in cmd.get_subcommands() {
            walk(sub, out);
        }
    }
    let mut out = Vec::new();
    walk(&Cli::command(), &mut out);
    out.sort();
    out.dedup();
    out
}

/// The string literals of one `fn` in a module of the shipped program, however the formatter
/// laid the list out.
fn ply_list_in(module: &str, name: &str) -> Vec<String> {
    let source = ply_cli::shipped::PROGRAM_SOURCES
        .iter()
        .find(|(m, _)| *m == module)
        .map(|(_, text)| *text)
        .unwrap_or_else(|| panic!("the program carries `{module}`"));
    let at = source
        .find(&format!("fn {name}()"))
        .unwrap_or_else(|| panic!("`{module}.ply` defines `{name}`"));
    let body = &source[at..];
    let close = body.find(']').expect("the list closes");
    body[..close]
        .match_indices('"')
        .step_by(2)
        .map(|(open, _)| {
            let rest = &body[open + 1..];
            rest[..rest.find('"').expect("the literal closes")].to_string()
        })
        .collect()
}

/// The string literals of one `fn` in `args.ply`, however the formatter laid the list out.
fn ply_list(name: &str) -> Vec<String> {
    ply_list_in("args", name)
}

/// `args.ply` must know every flag `clap` declares: one it does not know is read as a path, or
/// refused, and its value is read as a path either way.
#[test]
fn the_ply_parser_knows_every_flag_clap_declares() {
    let mut known: Vec<(String, bool)> = ply_list("machine_valued")
        .into_iter()
        .map(|f| (f, true))
        .chain(ply_list("valued").into_iter().map(|f| (f, true)))
        .chain(ply_list("machine_bool").into_iter().map(|f| (f, false)))
        .collect();
    // The flags the program reads for itself are spelled out in `one`, not in a list.
    for flag in [
        "--all",
        "--check",
        "--costs",
        "--deps",
        "--digest",
        "--explain",
        "--json",
        "--no-cache",
        "--types",
        "--watch",
    ] {
        known.push((flag.to_string(), false));
    }
    known.sort();
    known.dedup();

    let declared = clap_flags();
    let missing: Vec<&String> = declared
        .iter()
        .map(|(name, _)| name)
        .filter(|name| !known.iter().any(|(k, _)| k == *name))
        .collect();
    assert!(
        missing.is_empty(),
        "`clap` declares flags `args.ply` does not know, so a real command line carrying one \
         would be refused or read as a path: {missing:?}"
    );

    let disagreed: Vec<(String, bool, bool)> = declared
        .iter()
        .filter_map(|(name, takes)| {
            known
                .iter()
                .find(|(k, _)| k == name)
                .filter(|(_, mine)| mine != takes)
                .map(|(_, mine)| (name.clone(), *takes, *mine))
        })
        .collect();
    assert!(
        disagreed.is_empty(),
        "`args.ply` disagrees with `clap` about whether these take a value (flag, clap, ply): \
         {disagreed:?}"
    );
}

/// One line per flag `clap` declares, in `surface.ply`'s drift-index shape:
/// `command|long|short|shape`, where the command is dotted for a subcommand (`cache.clear`) and
/// `ply` for the global, and shape is `switch`, `value`, or `choice:` with the values.
fn clap_index() -> Vec<String> {
    fn shape(arg: &clap::Arg) -> String {
        let takes = matches!(
            arg.get_action(),
            clap::ArgAction::Set | clap::ArgAction::Append
        );
        if !takes {
            return "switch".to_string();
        }
        match arg.get_value_parser().possible_values() {
            Some(values) => {
                let names: Vec<String> = values.map(|v| v.get_name().to_string()).collect();
                format!("choice:{}", names.join(","))
            }
            None => "value".to_string(),
        }
    }
    fn walk(cmd: &clap::Command, path: &str, out: &mut Vec<String>) {
        for arg in cmd.get_arguments() {
            let Some(long) = arg.get_long() else { continue };
            // The global is counted at the root, not under every command it propagates to.
            if path != "ply" && arg.is_global_set() {
                continue;
            }
            let short = arg.get_short().map(|c| c.to_string()).unwrap_or_default();
            let mut line = format!("{path}|{long}|{short}|{}", shape(arg));
            if let Some(aliases) = arg.get_aliases() {
                for alias in aliases {
                    line.push_str(&format!("|alias:{alias}"));
                }
            }
            out.push(line);
        }
        for sub in cmd.get_subcommands() {
            let name = if path == "ply" {
                sub.get_name().to_string()
            } else {
                format!("{path}.{}", sub.get_name())
            };
            walk(sub, &name, out);
        }
    }
    let mut out = Vec::new();
    walk(&Cli::command(), "ply", &mut out);
    out.sort();
    out.dedup();
    out
}

/// `surface.ply`'s spec is the surface `clap` declares, flag for flag, until the Rust shell is
/// retired and the spec is the only one. The literals are pinned to the spec by a test in
/// `surface.ply` itself; this pins them to `clap`.
#[test]
fn the_surface_spec_agrees_with_clap() {
    let mut declared = ply_list_in("surface", "drift_index");
    declared.sort();
    declared.dedup();
    let declared = declared;
    let clap = clap_index();
    let only_spec: Vec<&String> = declared.iter().filter(|l| !clap.contains(l)).collect();
    let only_clap: Vec<&String> = clap.iter().filter(|l| !declared.contains(l)).collect();
    assert!(
        only_spec.is_empty() && only_clap.is_empty(),
        "the surface spec and clap disagree\nonly in surface.ply: {only_spec:?}\nonly in clap: {only_clap:?}"
    );
}
