//! The real-code row, end to end: the compiler and the CLI, laid out from the embedded sources,
//! front-ended by the product itself, with the digests pinned beside the verdicts.

#[test]
fn the_toolchain_trees_frontend_clean_at_their_digests() {
    let members = ply_corpus::cmd::real_members().expect("the real trees are laid out");
    let [(cname, cdir, cdigest), (lname, ldir, ldigest)] = members;
    let stage = cdir.parent().unwrap().parent().unwrap().to_path_buf();
    let value = ply_corpus::cmd::run_ply_subcommand(
        "real.run",
        vec![
            ply_eval::Value::str(cname),
            ply_eval::Value::str(cdir.to_string_lossy()),
            ply_eval::Value::str(cdigest.clone()),
            ply_eval::Value::str(lname),
            ply_eval::Value::str(ldir.to_string_lossy()),
            ply_eval::Value::str(ldigest.clone()),
            ply_eval::Value::Bool(false),
        ],
        &stage,
        &ply(),
    )
    .expect("the real-code row runs");
    let ply_eval::Value::Ctor { name, args } = &value else {
        panic!("`real.run` answered {value}, not an `Ok` or an `Err`");
    };
    let ply_eval::Value::Str(text) = &args[0] else {
        panic!("`real.run`'s answer is not text: {value}");
    };
    assert_eq!(name.as_str(), "Ok", "the real-code row refused: {text}");
    let report: serde_json::Value =
        serde_json::from_str(text).expect("the real-code report is JSON");
    assert_eq!(report["ok"].as_bool(), Some(true), "{report:#}");
    let members = report["members"].as_array().expect("members is an array");
    assert_eq!(members.len(), 2);
    let compiler = &members[0];
    assert_eq!(compiler["name"].as_str(), Some("compiler"));
    assert_eq!(compiler["digest"].as_str(), Some(cdigest.as_str()));
    assert_eq!(compiler["check"]["ok"].as_bool(), Some(true));
    assert!(
        compiler["definitions"].as_i64().unwrap_or(0) > 1000,
        "the compiler is the real one: {compiler:#}"
    );
    let cli = &members[1];
    assert_eq!(cli["name"].as_str(), Some("cli"));
    assert_eq!(cli["digest"].as_str(), Some(ldigest.as_str()));
    assert_eq!(cli["check"]["ok"].as_bool(), Some(true));
}

fn ply() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/debug/ply")
}
