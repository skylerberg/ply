//! The fronts `ply run` keeps, as bytes the program files and reads back: what a load of one
//! closure was handed, its front end's answer and its unit's C, encoded and compressed once that
//! load held, and decoded over a later run's walk in place of its front end and its emission. Where
//! an entry lives, and when it is written, are the program's; an entry that does not read is no
//! entry.

use crate::driver::{LoadedAnalysis, LoadedFile};
use crate::payload::record;
use ply_eval::decode::{AnswerValue, Error};
use ply_eval::{ModuleName, SourceId, Value};
use std::io::{Read, Write};
use std::time::{Duration, Instant};

/// What an entry is, as its `format` field says: one of any other shape is no entry.
const FORMAT: &str = "ply run front 4";

/// What an entry says of the `reuse fn` promises the load it was filed from checked.
const HELD: &str = "held";

/// One file this run's walk read: where, as its reports name it, and its text.
pub struct Walked {
    pub path: String,
    pub text: String,
}

/// The front `entry` filed, over this run's own files: the walk's modules, the shipped modules the
/// filed answer pulled, as the load that filed it placed them, then the walk's manifests, in the
/// order its ids run, and the C of the unit it was emitted as. The places are this run's, so a span
/// renders as this run's front end would have rendered it. `None` when the entry does not read or does
/// not fit the walk.
pub fn front(
    entry: &[u8],
    modules: Vec<Walked>,
    manifests: Vec<Walked>,
) -> Option<(LoadedAnalysis, Vec<u8>)> {
    let started = Instant::now();
    let mut encoded = Vec::new();
    flate2::read::GzDecoder::new(entry)
        .read_to_end(&mut encoded)
        .ok()?;
    let entry = ply_eval::codec::decode(&encoded).ok()?;
    let filed = Filed::read(AnswerValue::new("a front `ply run` filed", &entry)).ok()?;
    let own = modules.len();
    let pulled = filed.files.len().checked_sub(own + manifests.len())?;
    let mut walked = modules.into_iter().chain(manifests);
    let mut files = Vec::with_capacity(filed.files.len());
    let ships = crate::shipped_modules::name_set();
    for (i, file) in filed.files.into_iter().enumerate() {
        let (path, text) = if (own..own + pulled).contains(&i) {
            if !ships.contains(ModuleName::from_dotted(&file.name).as_str()) {
                return None;
            }
            (file.path, file.text)
        } else {
            let walked = walked.next()?;
            (walked.path, walked.text)
        };
        files.push(LoadedFile {
            path,
            name: file.name,
            text,
        });
    }
    let ids: Vec<SourceId> = (0..files.len()).map(|i| SourceId(i as u32)).collect();
    let answer = ply_codegen::c::dump::read(filed.dump, &ids).ok()?;
    let front = LoadedAnalysis {
        answer,
        files,
        read: Duration::ZERO,
        front: started.elapsed(),
        write_back: Duration::ZERO,
        cached: false,
    };
    Some((front, filed.unit.to_vec()))
}

/// An entry as it reads: every file's place, module and the text filed for it, the front end's answer
/// and the unit's C. Only an entry that says its load's promises held is read at all, since that is
/// the only kind ever filed.
struct Filed<'v> {
    files: Vec<LoadedFile>,
    dump: &'v Value,
    unit: &'v [u8],
}

impl<'v> Filed<'v> {
    fn read(entry: AnswerValue<'v>) -> Result<Filed<'v>, Error> {
        if entry.field("format")?.str()? != FORMAT {
            return Err(entry.error("an entry of another format"));
        }
        if entry.field("promises")?.str()? != HELD {
            return Err(entry.error("an entry whose promises did not hold"));
        }
        Ok(Filed {
            files: entry.field("files")?.items(|file| {
                Ok(LoadedFile {
                    path: file.field("path")?.str()?.to_string(),
                    name: file.field("name")?.str()?.to_string(),
                    text: file.field("text")?.str()?.to_string(),
                })
            })?,
            dump: entry.field("dump")?.value(),
            unit: entry.field("unit")?.bytes()?,
        })
    }
}

/// The entry for a load that held over the front and unit it was handed: every file's place and
/// module, the text of each shipped module as the load placed it, `dump`, the front end's answer, and
/// the unit's C. A shipped module's placed text is what an importer reads of it, which the emitter
/// parses, so it is filed rather than read off the binary again; any other file is the walk's. `None`
/// when the answer does not encode, or when the entry is more than one read of the file it is filed
/// in would answer.
pub fn entry(files: &[LoadedFile], dump: &Value, unit: &[u8]) -> Option<Vec<u8>> {
    let ships = crate::shipped_modules::name_set();
    let entry = record(vec![
        ("format", Value::str(FORMAT)),
        (
            "files",
            Value::list(
                files
                    .iter()
                    .map(|file| {
                        let shipped = ships.contains(ModuleName::from_dotted(&file.name).as_str());
                        record(vec![
                            ("path", Value::str(&file.path)),
                            ("name", Value::str(&file.name)),
                            (
                                "text",
                                Value::str(if shipped { file.text.as_str() } else { "" }),
                            ),
                        ])
                    })
                    .collect(),
            ),
        ),
        ("promises", Value::str(HELD)),
        ("dump", dump.clone()),
        ("unit", Value::bytes(unit)),
    ]);
    let encoded = ply_eval::codec::encode(&entry).ok()?;
    let mut packed = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    packed.write_all(&encoded).ok()?;
    let packed = packed.finish().ok()?;
    (packed.len() as u64 <= ply_host::fs::MAX_READ_BYTES).then_some(packed)
}
