//! `stage <dir>`: emits `<dir>/*.ply` into `<dir>/bootstrap`, so `PLY_C_EMITTER=ply:<dir>`
//! runs that compiler. The arming scripts compile every mutant through this.

use std::collections::HashMap;
use std::path::PathBuf;

fn main() {
    let dir = match std::env::args().nth(1) {
        Some(d) => PathBuf::from(d),
        None => {
            eprintln!("usage: stage <dir>   # a directory of the compiler's .ply modules");
            std::process::exit(2);
        }
    };
    ply_codegen::c::producer::ensure_default();

    let mut modules: Vec<(String, String)> = ply_std::sources()
        .map(|(m, t)| (m.to_string(), t.to_string()))
        .collect();
    let mut found = Vec::new();
    for entry in
        std::fs::read_dir(&dir).unwrap_or_else(|e| fail(&format!("{}: {e}", dir.display())))
    {
        let path = entry.expect("a directory entry").path();
        if path.extension().is_some_and(|x| x == "ply") {
            let stem = path
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned();
            let text = std::fs::read_to_string(&path)
                .unwrap_or_else(|e| fail(&format!("{}: {e}", path.display())));
            found.push((stem, text));
        }
    }
    found.sort();
    modules.extend(found);
    let identity = ply_codegen::c::producer::digest_of(&modules);

    let mut sources = ply_span::SourceMap::new();
    let mut inputs = Vec::new();
    let mut texts: HashMap<String, String> = HashMap::new();
    for (module, text) in &modules {
        texts.insert(module.clone(), text.clone());
        let name = ply_syntax::ast::ModuleName::from_dotted(module);
        let id = sources.add(dir.join(format!("{module}.ply")), text.clone());
        let text: &'static str = Box::leak(text.clone().into_boxed_str());
        inputs.push((id, name, text));
    }
    let mut ast = ply_syntax::parse_program(inputs).unwrap_or_else(refused);
    let expanded = ply_derive::expand_program(&mut ast);
    if !expanded.is_empty() {
        refused::<()>(expanded);
    }
    ply_syntax::resolve::resolve(&mut ast).unwrap_or_else(refused);
    // Ids follow `SourceMap` order, which is how the protocol indexes a span's module.
    let ids: Vec<ply_span::SourceId> = (0..modules.len())
        .map(|i| ply_span::SourceId(i as u32))
        .collect();
    let front = Box::leak(Box::new(
        ply_codegen::c::producer::front(&modules, &ids).unwrap_or_else(|e| {
            fail(&format!(
                "the port could not answer for {}: {e:#}",
                dir.display()
            ))
        }),
    ));
    let keys = ply_codegen::emit_keys(front);
    let source: &'static ply_codegen::Source = Box::leak(Box::new(
        ply_codegen::Source::from_front(front, keys).with_texts(texts),
    ));
    let names: Vec<String> = source.functions();
    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
    let produced = ply_codegen::c::produce(source, &refs).unwrap_or_else(|e| {
        fail(&format!(
            "the emitter could not emit {}: {e:#}",
            dir.display()
        ))
    });
    if !produced.refused.is_empty() {
        fail(&format!(
            "the emitter refused {} bodies of {}: {:?}",
            produced.refused.len(),
            dir.display(),
            produced.refused
        ));
    }
    let out = dir.join("bootstrap");
    ply_codegen::c::bundle::write(&out, &produced.text, &identity)
        .unwrap_or_else(|e| fail(&format!("{}: {e:#}", out.display())));
    println!("{} staged as {identity}", out.display());
}

/// The front end refused the copy: the arming scripts count it as an invalid mutant.
fn refused<T>(ds: Vec<ply_span::Diagnostic>) -> T {
    for d in &ds {
        eprintln!("{}", d.message);
    }
    std::process::exit(1)
}

fn fail(msg: &str) -> ! {
    eprintln!("{msg}");
    std::process::exit(1)
}
