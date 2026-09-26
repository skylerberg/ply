//! The corpus's command line, parsed by the corpus package's own `cmd.ply` — the corpus's
//! argument handling is the CLI package's `cli.cmdline` library, not a copy of it. The binary
//! lays its two packages out in a stage directory (the corpus over the CLI, as the manifest's
//! path dependency spells it), builds the front door once per source identity, and asks
//! `cmd.dispatch` for the plan, which the Rust half only ever decodes.

use anyhow::{Context, Result, anyhow, bail};
use ply_span::Span;
use std::path::{Path, PathBuf};

include!(concat!(env!("OUT_DIR"), "/corpus_sources.rs"));

/// What `cmd.dispatch` answered: the plan, or text for the user and the code to exit with.
pub enum Outcome {
    Run(serde_json::Value),
    Help(String),
    Version(String),
    Refused(String),
}

/// Parse `argv` (minus the binary's name) into the plan, through the corpus package's front door.
pub fn dispatch(argv: &[String]) -> Result<Outcome> {
    let stage = stage_dir();
    let artifact_path = stage.join("corpus.plyx");
    let bytes = match std::fs::read(&artifact_path) {
        Ok(bytes) => bytes,
        Err(_) => build(&stage, &artifact_path)?,
    };
    let (artifact, _) = ply_machine::artifact::decode(&bytes, &artifact_path)
        .map_err(|d| anyhow!("the corpus's own front door does not decode: {}", d.message))?;
    let opened = ply_machine::artifact::open(&artifact, &artifact_path).map_err(|diagnostics| {
        anyhow!(
            "the corpus's own front door does not open: {}",
            diagnostics
                .first()
                .map(|d| d.message.clone())
                .unwrap_or_else(|| "nothing said why".to_string())
        )
    })?;
    let name = opened
        .front
        .check
        .defs
        .values()
        .find(|d| d.simple_name.as_str() == "dispatch" && d.module.as_str() == "cmd")
        .map(|d| d.name.to_string())
        .ok_or_else(|| anyhow!("the corpus package declares no `cmd.dispatch`"))?;
    let mut machine = ply_eval::Machine::new(&opened.front);
    if let Some(unit) = artifact.unit.as_ref() {
        let text = ply_codegen::c::bundle::unpack(&unit.text)
            .map_err(|e| anyhow!("the front door's compiled unit would not unpack: {e:#}"))?;
        let unit =
            ply_codegen::Unit::embedded(&opened.front, text).map_err(|e| anyhow!("{e:#}"))?;
        machine.set_compiled(ply_eval::Provider::attach(unit, &crate::tier_spec()));
    }
    let argv_value = ply_eval::Value::list(argv.iter().map(ply_eval::Value::str).collect());
    let value = machine
        .call(&name, vec![argv_value], Span::DUMMY)
        .map_err(|d| anyhow!("`cmd.dispatch` raised [{}]: {}", d.code, d.message))?;
    let ply_eval::Value::Str(plan) = &value else {
        bail!("`cmd.dispatch` answered {value}, which is not the plan's JSON text");
    };
    let plan: serde_json::Value =
        serde_json::from_str(plan).context("the plan `cmd.dispatch` rendered is not JSON")?;
    match plan["outcome"].as_str() {
        Some("run") => Ok(Outcome::Run(plan)),
        Some("help") => Ok(Outcome::Help(text(&plan, "text")?)),
        Some("version") => Ok(Outcome::Version(text(&plan, "text")?)),
        Some("refused") => Ok(Outcome::Refused(text(&plan, "why")?)),
        other => bail!("`cmd.dispatch` answered outcome {other:?}, which is not one it has"),
    }
}

fn text(plan: &serde_json::Value, key: &str) -> Result<String> {
    plan[key]
        .as_str()
        .map(|s| s.to_string())
        .ok_or_else(|| anyhow!("the plan carries no `{key}` text"))
}

/// Build the front door: lay out the two packages in the repository's own shape (so the
/// manifest's `../../ply-cli/ply` answers), load them as the packaged project they are, and
/// land the built artifact where the next run reads it.
fn build(stage: &Path, artifact_path: &Path) -> Result<Vec<u8>> {
    let corpus = lay_out(stage)?;
    let loaded = ply_machine::load::load(&corpus).map_err(|err| {
        anyhow!(
            "the corpus's own command line does not check: {}",
            err.diagnostics
                .first()
                .map(|d| d.message.clone())
                .unwrap_or_else(|| "nothing said why".to_string())
        )
    })?;
    let entry = loaded
        .sole_entry_point()
        .map_err(|d| anyhow!("the corpus's command line has no one entry: {}", d.message))?;
    let built = ply_machine::artifact::build(&loaded, entry, &[]).map_err(|diagnostics| {
        anyhow!(
            "the corpus's command line would not build: {}",
            diagnostics
                .first()
                .map(|d| d.message.clone())
                .unwrap_or_else(|| "nothing said why".to_string())
        )
    })?;
    let bytes: Vec<u8> = built
        .artifact
        .encode()
        .map_err(|d| anyhow!("the built front door would not encode: {}", d.message))?;
    let tmp = stage.join(format!("corpus.{}.plyx.tmp", std::process::id()));
    std::fs::write(&tmp, &bytes)?;
    std::fs::rename(&tmp, artifact_path)?;
    Ok(bytes)
}

/// The corpus package, then the CLI package, as `crates/ply-corpus/ply` and
/// `crates/ply-cli/ply` under the stage.
fn lay_out(stage: &Path) -> Result<PathBuf> {
    let corpus = stage.join("crates/ply-corpus/ply");
    let cli = stage.join("crates/ply-cli/ply");
    let marker = stage.join("LAID.out");
    if marker.exists() {
        return Ok(corpus);
    }
    std::fs::create_dir_all(&corpus)?;
    std::fs::create_dir_all(&cli)?;
    write_all(&corpus, CORPUS_SOURCES, CORPUS_MANIFEST)?;
    write_all(
        &cli,
        ply_launcher::shipped::PROGRAM_SOURCES,
        ply_launcher::shipped::PROGRAM_MANIFEST,
    )?;
    std::fs::write(&marker, b"")?;
    Ok(corpus)
}

fn write_all(dir: &Path, modules: &[(&str, &str)], manifest: &str) -> Result<()> {
    for (name, text) in modules {
        std::fs::write(dir.join(format!("{name}.ply")), text)?;
    }
    std::fs::write(dir.join("ply.pkg"), manifest)?;
    Ok(())
}

/// The stage is keyed by everything the front door is a function of: both packages' sources and
/// manifests, and the toolchain stamp covering the shelf, the emitter and the store versions.
fn stage_dir() -> PathBuf {
    let mut inputs: Vec<(String, String)> = CORPUS_SOURCES
        .iter()
        .map(|(n, t)| (n.to_string(), t.to_string()))
        .collect();
    inputs.push(("corpus.pkg".to_string(), CORPUS_MANIFEST.to_string()));
    for (name, text) in ply_launcher::shipped::PROGRAM_SOURCES {
        inputs.push((format!("cli.{name}"), text.to_string()));
    }
    inputs.push((
        "cli.pkg".to_string(),
        ply_launcher::shipped::PROGRAM_MANIFEST.to_string(),
    ));
    ply_codegen::c::bundle::stage_dir(&format!(
        "corpus-{}",
        ply_machine::artifact::toolchain_stamp(&ply_codegen::c::producer::digest_of(&inputs))
    ))
}
