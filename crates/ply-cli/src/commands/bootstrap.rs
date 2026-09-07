//! The front end, written out as the C that builds it.
//!
//! A self-hosted compiler has to answer one question before it can drop the compiler it was
//! written in: what does a fresh clone read the language *with*? Three answers are available and
//! two are bad. Keeping the Rust front end as the first stage is not dropping it. Committing a
//! built object is a binary per platform, per version, in the history.
//!
//! The third is this. The emitted tier already turns the whole front end into one C file; that
//! file is text, it diffs, it needs nothing but a C compiler, and git already archives every
//! version of it. So the artifact this writes *is* the compiler, in the only form that is neither
//! a binary nor a dependency on what it replaces.
//!
//! Two digests, because they answer different questions. The **source** digest is over every
//! definition's content hash: it names which version of the front end this is, and two trees with
//! the same one hold the same compiler however differently they emit it. The **artifact** digest is
//! over the C: it says whether this file is the one that version produces, which is what `--verify`
//! checks and what makes the archive worth keeping.

use super::common::{diagnostics_json, emit_json, emit_keys, print_diagnostics, report_load_error};
use crate::cli::BootstrapArgs;
use crate::load::load;
use crate::style::Style;
use crate::{EXIT_COMPILE_ERROR, EXIT_OK};
use serde_json::json;

/// What the archive records. Written beside the C and read back by `--verify`.
fn manifest(source: &str, artifact: &str, definitions: usize, refused: usize) -> serde_json::Value {
    let how = ply_codegen::Profile::current().inlining().overridden();
    json!({
        "source": source,
        "artifact": artifact,
        "definitions": definitions,
        "refused": refused,
        "inlining": { "budget": how.budget, "depth": how.depth },
    })
}

pub fn execute(args: &BootstrapArgs, style: Style) -> i32 {
    // An archive is the compiler somebody else will run, so it is emitted at `release` unless the
    // caller says otherwise: `development` is a different artifact, with a different digest, and
    // forty times slower on integer arithmetic.
    if let Err(d) = super::common::select_profile(&args.profile) {
        print_diagnostics(std::slice::from_ref(&d), &ply_span::SourceMap::new(), style);
        return EXIT_COMPILE_ERROR;
    }
    let loaded = match load(&args.path) {
        Ok(loaded) => loaded,
        Err(err) => return report_load_error("bootstrap", &err, args.json, style),
    };
    let hashes = match loaded.hashes() {
        Ok(hashes) => hashes,
        Err(diagnostics) => {
            if args.json {
                emit_json(&json!({
                    "command": "bootstrap",
                    "ok": false,
                    "exit_code": EXIT_COMPILE_ERROR,
                    "diagnostics": diagnostics_json(&diagnostics, &loaded.sources),
                }));
            } else {
                print_diagnostics(&diagnostics, &loaded.sources, style);
            }
            return EXIT_COMPILE_ERROR;
        }
    };
    // The definition hashes, in name order, are the version. Sorted rather than in load order so
    // that moving a definition between files does not rename the compiler.
    let mut pairs: Vec<(String, String)> = hashes
        .defs
        .iter()
        .map(|(name, h)| (name.to_string(), h.to_hex()))
        .collect();
    pairs.sort();
    let mut h = blake3::Hasher::new();
    for (name, hash) in &pairs {
        h.update(name.as_bytes());
        h.update(&[0]);
        h.update(hash.as_bytes());
        h.update(&[0]);
    }
    let source_digest = h.finalize().to_hex().to_string();

    let program = Box::leak(Box::new(loaded.program.clone()));
    let resolved = Box::leak(Box::new(loaded.resolved.clone()));
    let check = Box::leak(Box::new(loaded.check.clone()));
    let keys = emit_keys(program, &hashes);
    let src: &'static ply_codegen::Source = Box::leak(Box::new(ply_codegen::Source::keyed(
        program, resolved, check, keys,
    )));
    let names: Vec<String> = src.functions();
    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
    let (text, refused) = match ply_codegen::c::emit_unit(src, &refs) {
        Ok(pair) => pair,
        Err(e) => {
            eprintln!("the front end could not be emitted: {e:#}");
            return EXIT_COMPILE_ERROR;
        }
    };
    let artifact_digest = blake3::hash(text.as_bytes()).to_hex().to_string();
    let manifest = manifest(&source_digest, &artifact_digest, names.len(), refused.len());

    let dir = args.out.clone();
    let c_path = dir.join("frontend.c");
    let manifest_path = dir.join("manifest.json");

    if args.verify {
        let Ok(recorded) = std::fs::read_to_string(&manifest_path) else {
            eprintln!("no archive at {}", manifest_path.display());
            return EXIT_COMPILE_ERROR;
        };
        let recorded: serde_json::Value = match serde_json::from_str(&recorded) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("{}: {e}", manifest_path.display());
                return EXIT_COMPILE_ERROR;
            }
        };
        if recorded != manifest {
            if args.json {
                emit_json(&json!({
                    "command": "bootstrap",
                    "ok": false,
                    "exit_code": EXIT_COMPILE_ERROR,
                    "recorded": recorded,
                    "emitted": manifest,
                }));
            } else {
                eprintln!(
                    "the archive does not describe this tree:\n  recorded {recorded}\n  emitted  {manifest}"
                );
            }
            return EXIT_COMPILE_ERROR;
        }
        if args.json {
            emit_json(
                &json!({ "command": "bootstrap", "ok": true, "exit_code": EXIT_OK, "verified": manifest }),
            );
        } else {
            println!("the archive describes this tree: {source_digest}");
        }
        return EXIT_OK;
    }

    if let Err(e) = std::fs::create_dir_all(&dir) {
        eprintln!("{}: {e}", dir.display());
        return EXIT_COMPILE_ERROR;
    }
    // The C first, so a manifest is never on disk describing an artifact that is not.
    if let Err(e) = std::fs::write(&c_path, &text) {
        eprintln!("{}: {e}", c_path.display());
        return EXIT_COMPILE_ERROR;
    }
    let rendered = format!(
        "{}\n",
        serde_json::to_string_pretty(&manifest).unwrap_or_default()
    );
    if let Err(e) = std::fs::write(&manifest_path, rendered) {
        eprintln!("{}: {e}", manifest_path.display());
        return EXIT_COMPILE_ERROR;
    }
    if args.json {
        emit_json(&json!({
            "command": "bootstrap",
            "ok": true,
            "exit_code": EXIT_OK,
            "wrote": [c_path.display().to_string(), manifest_path.display().to_string()],
            "manifest": manifest,
        }));
    } else {
        println!(
            "{} definitions, {} refused, {} bytes of C",
            names.len(),
            refused.len(),
            text.len()
        );
        println!("source   {source_digest}");
        println!("artifact {artifact_digest}");
        println!("wrote    {}", c_path.display());
    }
    EXIT_OK
}
