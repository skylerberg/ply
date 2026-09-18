//! Which C toolchain this tier compiles with.
//!
//! A warm run reads its unit back out of a cache. An *edit* recompiles it, and `cc -O2` over a
//! large unit is the slow part of that. So there are two things to compile for, and one compiler
//! cannot be both: `development`, the default, is `tcc` if installed, else `cc -O0`; `release` is
//! `cc -O2`. A measurement wants `release` and has to say so -- `--profile release`, or
//! `PLY_C_PROFILE=release`, which is what `benches/value-model/run.sh` passes.

/// What this run is compiling for.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Profile {
    /// Compile fast for code that runs slowly enough. The default, and what an edit-to-green loop
    /// wants.
    #[default]
    Development,
    /// Compile slowly for code that runs fast. What ships, and what a measurement must ask for.
    Release,
}

impl Profile {
    pub fn name(self) -> &'static str {
        match self {
            Profile::Development => "development",
            Profile::Release => "release",
        }
    }

    /// The profile this process compiles under.
    ///
    /// Three sources, in this order: `PLY_C_PROFILE`, which is how a bench script pins one without
    /// a command line; `select`, which is how the CLI's `--profile` passes one; and the default.
    /// The environment wins because the scripts that set it are the ones that must not be moved by
    /// a change to what the flag defaults to.
    pub fn current() -> Profile {
        match std::env::var("PLY_C_PROFILE").as_deref() {
            Ok("development") => return Profile::Development,
            Ok("release") => return Profile::Release,
            _ => {}
        }
        SELECTED.get().copied().unwrap_or_default()
    }

    /// Parses the spelling `--profile` and `PLY_C_PROFILE` share.
    pub fn parse(name: &str) -> Option<Profile> {
        match name {
            "development" => Some(Profile::Development),
            "release" => Some(Profile::Release),
            _ => None,
        }
    }

    /// The compiler to shell out to. `PLY_CC` still wins over both, because a measurement that
    /// names its compiler should get the one it named.
    pub fn compiler(self) -> String {
        if let Ok(cc) = std::env::var("PLY_CC") {
            return cc;
        }
        match self {
            Profile::Release => "cc".to_string(),
            // tcc when it is here and `cc -O0` when it is not. The fallback is the point: this
            // profile is worth twenty times on an edit with nothing installed that was not already
            // installed, and tcc makes it another seven.
            Profile::Development => match super::load::which("tcc") {
                Some(_) => "tcc".to_string(),
                None => "cc".to_string(),
            },
        }
    }

    /// The optimisation flag, which `PLY_CC_OPT` overrides for the same reason.
    pub fn opt_level(self) -> String {
        if let Ok(o) = std::env::var("PLY_CC_OPT") {
            return o;
        }
        match self {
            Profile::Release => "-O2".to_string(),
            Profile::Development => "-O0".to_string(),
        }
    }
}

/// What the CLI's `--profile` chose, once per process. Set before anything compiles.
static SELECTED: std::sync::OnceLock<Profile> = std::sync::OnceLock::new();

/// Fixes the profile for this process. The first call wins; a second is ignored rather than an
/// error, because a caller that sets it twice with the same value is not doing anything wrong and
/// one that sets it twice with different values has already compiled under the first.
pub fn select(profile: Profile) {
    let _ = SELECTED.set(profile);
}

/// The arguments a particular compiler needs that the others do not.
///
/// tcc is the only one so far. It finds `libtcc1.a` -- which every object it produces needs and
/// which holds nothing else -- relative to `-B`, and a tcc built from source and not installed has
/// no default that finds it. Without the flag the *compile succeeds* and the object will not load,
/// with an empty `dlerror`; that cost an afternoon and a one-line shell wrapper.
pub fn extra_args(cc: &str) -> Vec<String> {
    // Through the symlink, because what sits beside the *binary* is the question and a name on
    // `PATH` is often a link into a source tree that has the library the link's directory has not.
    let path = super::load::which(cc).and_then(|p| std::fs::canonicalize(p).ok());
    support_flags(cc, path.as_deref())
}

pub fn support_flags(cc: &str, resolved: Option<&std::path::Path>) -> Vec<String> {
    if !cc.ends_with("tcc") {
        return Vec::new();
    }
    let Some(dir) = resolved.and_then(|p| p.parent()) else {
        return Vec::new();
    };
    // Beside the binary is where a source build leaves it; `../lib/tcc` is where `make install`
    // puts it. An installed tcc finds its own and needs neither.
    for candidate in [dir.to_path_buf(), dir.join("../lib/tcc")] {
        if candidate.join("libtcc1.a").is_file() {
            return vec![format!("-B{}", candidate.display())];
        }
    }
    Vec::new()
}
