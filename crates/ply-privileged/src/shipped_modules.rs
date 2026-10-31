const STD_ROOT: &str = "std";
const COMPILER_ROOT: &str = "compiler";

pub fn names() -> Vec<String> {
    let pack = ply_pack::installed();
    let named = |root: &'static str, dir: &'static str, path: &str| {
        let stem = path[dir.len() + 1..].strip_suffix(".ply")?;
        Some(format!("{root}.{}", stem.replace('/', ".")))
    };
    pack.modules_below(ply_pack::STD)
        .filter_map(|path| named(STD_ROOT, ply_pack::STD, path))
        .chain(
            pack.modules_below(ply_pack::COMPILER)
                .filter_map(|path| named(COMPILER_ROOT, ply_pack::COMPILER, path)),
        )
        .collect()
}

pub fn sources() -> Vec<(String, &'static str)> {
    names()
        .into_iter()
        .filter_map(|name| {
            let text = source(&name)?;
            Some((name, text))
        })
        .collect()
}

fn path_of(module: &str) -> Option<String> {
    let (dir, stem) = match module.split_once('.')? {
        (STD_ROOT, stem) => (ply_pack::STD, stem),
        (COMPILER_ROOT, stem) => (ply_pack::COMPILER, stem),
        _ => return None,
    };
    Some(format!("{dir}/{}.ply", stem.replace('.', "/")))
}

pub fn source(module: &str) -> Option<&'static str> {
    let path = path_of(module)?;
    let text = ply_pack::installed().bytes(&path)?;
    Some(
        std::str::from_utf8(text)
            .unwrap_or_else(|e| panic!("`{path}` in the pack is not UTF-8: {e}")),
    )
}

fn data_name(path: &str) -> String {
    format!("{STD_ROOT}{}", &path[ply_pack::STD.len()..])
}

fn data_path_of(name: &str) -> Option<String> {
    let below = name.strip_prefix(STD_ROOT)?.strip_prefix('/')?;
    (!below.ends_with(".ply")).then(|| format!("{}/{below}", ply_pack::STD))
}

pub fn data_names() -> Vec<String> {
    ply_pack::installed()
        .data_below(ply_pack::STD)
        .map(data_name)
        .collect()
}

pub fn data(name: &str) -> Option<&'static [u8]> {
    ply_pack::installed().bytes(&data_path_of(name)?)
}

pub fn digests() -> String {
    let mut lines: Vec<String> = names()
        .into_iter()
        .filter_map(|name| Some(format!("{name}.ply {}\n", digest_of(&name)?)))
        .chain(
            data_names()
                .into_iter()
                .filter_map(|name| Some(format!("{name} {}\n", digest_of(&name)?))),
        )
        .collect();
    lines.sort();
    lines.concat()
}

pub fn digest() -> ply_pack::Digest {
    ply_pack::installed().digest_below(&[ply_pack::STD, ply_pack::COMPILER])
}

pub fn digest_of(name: &str) -> Option<String> {
    let path = data_path_of(name).or_else(|| path_of(name))?;
    let digest = ply_pack::installed().digest_of(&path)?;
    Some(blake3::Hash::from(digest).to_hex().to_string())
}
