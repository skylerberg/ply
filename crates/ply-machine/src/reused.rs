//! The fronts `ply run` keeps: what a load of one closure was handed, filed under the key the
//! closure hashes to once that load held, and read back by a later run whose walk hashes the same,
//! in place of its front end. One file per key under the stage root's [`sweep::RUNS`], written
//! beside itself and renamed into place, so a reader finds a whole entry or none; an entry that
//! does not read is no entry, and the run that finds it so runs the front end and files over it.

use crate::driver::{FrontFile, HandedFront};
use crate::payload::record;
use ply_codegen::c::{bundle, sweep};
use ply_eval::decode::{At, Error};
use ply_eval::{ModuleName, SourceId, Value};
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// What an entry is, as its `format` field says: one of any other shape is no entry.
const FORMAT: &str = "ply run front 1";

/// What an entry says of the `reuse fn` promises the load it was filed from checked.
const HELD: &str = "held";

/// Where the entry for `key` lives. A key is a walk's BLAKE3 in hex; anything else names none.
pub fn path_of(key: &str) -> Option<PathBuf> {
    let hashed = key.len() == 64 && key.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'));
    hashed.then(|| bundle::stage_dir(sweep::RUNS).join(key))
}

/// One file this run's walk read: where, as its reports name it, and its text.
pub struct Walked {
    pub path: String,
    pub text: String,
}

/// The front an earlier run filed under `key`, over this run's own files: the walk's modules, the
/// shelf the filed answer pulled, then the walk's manifests, in the order its ids run. The places
/// are this run's, so a span renders as this run's front end would have rendered it. `None` when
/// no entry reads, and with the entry's path when one does.
pub fn front(
    key: &str,
    modules: Vec<Walked>,
    manifests: Vec<Walked>,
) -> Option<(HandedFront, PathBuf)> {
    let started = Instant::now();
    let path = path_of(key)?;
    let entry = ply_eval::codec::decode(&std::fs::read(&path).ok()?).ok()?;
    let filed = Filed::read(At::new("a front `ply run` filed", &entry)).ok()?;
    let own = modules.len();
    let pulled = filed.files.len().checked_sub(own + manifests.len())?;
    let mut walked = modules.into_iter().chain(manifests);
    let mut files = Vec::with_capacity(filed.files.len());
    for (i, (path, name)) in filed.files.into_iter().enumerate() {
        let (path, text) = if (own..own + pulled).contains(&i) {
            let text = crate::shelf::source(&ModuleName::from_dotted(&name))?;
            (path, text.to_string())
        } else {
            let file = walked.next()?;
            (file.path, file.text)
        };
        files.push(FrontFile { path, name, text });
    }
    let ids: Vec<SourceId> = (0..files.len()).map(|i| SourceId(i as u32)).collect();
    let answer = ply_codegen::c::dump::read(filed.dump, &ids).ok()?;
    let front = HandedFront {
        answer,
        files,
        read: Duration::ZERO,
        front: started.elapsed(),
        write_back: Duration::ZERO,
        cached: false,
    };
    Some((front, path))
}

/// An entry as it reads: every file's place and module, and the front end's answer. Only an entry
/// that says its load's promises held is read at all, since that is the only kind ever filed.
struct Filed<'v> {
    files: Vec<(String, String)>,
    dump: &'v Value,
}

impl<'v> Filed<'v> {
    fn read(entry: At<'v>) -> Result<Filed<'v>, Error> {
        if entry.field("format")?.str()? != FORMAT {
            return Err(entry.error("an entry of another format"));
        }
        if entry.field("promises")?.str()? != HELD {
            return Err(entry.error("an entry whose promises did not hold"));
        }
        Ok(Filed {
            files: entry.field("files")?.items(|file| {
                Ok((
                    file.field("path")?.str()?.to_string(),
                    file.field("name")?.str()?.to_string(),
                ))
            })?,
            dump: entry.field("dump")?.value(),
        })
    }
}

/// Files what a load that held was handed under `key`: every file's place and module, and `dump`,
/// the front end's answer. An entry that cannot be written is left unwritten.
pub fn file(key: &str, files: &[(String, String)], dump: &Value) {
    let Some(path) = path_of(key) else { return };
    let Some(dir) = path.parent() else { return };
    if std::fs::create_dir_all(dir).is_err() {
        return;
    }
    let entry = record(vec![
        ("format", Value::str(FORMAT)),
        (
            "files",
            Value::list(
                files
                    .iter()
                    .map(|(path, name)| {
                        record(vec![("path", Value::str(path)), ("name", Value::str(name))])
                    })
                    .collect(),
            ),
        ),
        ("promises", Value::str(HELD)),
        ("dump", dump.clone()),
    ]);
    let Ok(bytes) = ply_eval::codec::encode(&entry) else {
        return;
    };
    let _ = ply_eval::files::write_atomically(&path, &bytes);
}
