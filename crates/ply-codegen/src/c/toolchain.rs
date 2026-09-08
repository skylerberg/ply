//! Which C toolchain this tier compiles with, and the inlining that has to go with it.
//!
//! A warm run of the self-hosted front end reads its unit back out of a cache in milliseconds. An
//! *edit* spends that in one go -- `cc -O2` over the front end's twenty-nine megabytes of C is
//! about thirty-eight seconds.
//!
//! So there are two things to compile for, and one compiler cannot be both:
//!
//! | profile | compiler | inlining | the front end's unit | compile | k1 |
//! | --- | --- | --- | --- | --- | --- |
//! | `development` | `tcc`, else `cc -O0` | depth 0 | 7MB | 0.26s / 1.96s | 6.4ms / 4.7ms |
//! | `release` | `cc -O2` | depth 3 | 29MB | 38.5s | 0.15ms |
//!
//! One code generator, two toolchains, and **`development` is the default** because that is what
//! the numbers say a run is usually for. It costs about forty times on the integer kernel and buys
//! back two orders of magnitude on an edit; the front end's own warm run is *faster* under it
//! (1.38s against 1.51s), because a suite's time is not in the arithmetic those forty times are
//! charged to. A measurement wants the other one and has to say so -- `--profile release`, or
//! `PLY_C_PROFILE=release`, which is what `benches/value-model/run.sh` passes.
//!
//! **The inlining is not a separate knob and this is the trap.** A non-optimising compiler gives
//! every temporary its own stack slot and coalesces nothing across sibling blocks, so the tier's
//! depth-3 bodies compile to frames of up to 128KB against `-O2`'s 8. The front end recurses far
//! enough that this overflows the stack -- not an error, an `abort` with no diagnostic and nothing
//! to attribute it to. `cc -O0`, `cc -O1` and `tcc` all do it at depth 3, and all of them run the
//! whole corpus at depth 0. That is why the profile carries the depth: they are one choice.

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

    /// How hard the inliner is told to work. Read the module's last paragraph before separating
    /// this from the compiler above.
    pub fn inlining(self) -> crate::opt::Inlining {
        match self {
            Profile::Release => crate::opt::Inlining::EMITTED,
            Profile::Development => crate::opt::Inlining {
                depth: 0,
                ..crate::opt::Inlining::EMITTED
            },
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

fn support_flags(cc: &str, resolved: Option<&std::path::Path>) -> Vec<String> {
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The compiler and the inlining are one choice.
    ///
    /// A non-optimising compiler gives every temporary its own stack slot, so this tier's
    /// depth-3 bodies compile to 128KB frames against `-O2`'s 8, and the self-hosted front end
    /// recurses far enough to overflow the stack -- an `abort` with no diagnostic, under `tcc`,
    /// `cc -O0` and `cc -O1` alike. Every one of them runs the whole corpus at depth 0. If this
    /// assertion is in your way, that is what it is in the way of.
    #[test]
    fn the_fast_toolchain_does_not_inline() {
        assert_eq!(Profile::Development.inlining().depth, 0);
        assert_eq!(
            Profile::Release.inlining().depth,
            crate::opt::Inlining::EMITTED.depth
        );
    }

    /// tcc finds `libtcc1.a` relative to `-B`, and a build that was never installed has no default
    /// that finds it. Without the flag the compile *succeeds* and the object will not load, with
    /// an empty reason from `dlerror`.
    #[test]
    fn a_tcc_that_is_not_installed_is_told_where_its_support_library_is() {
        let dir = std::env::temp_dir().join(format!("ply-tcc-probe-{}", std::process::id()));
        let installed = dir.join("bin");
        let lib = dir.join("lib/tcc");
        std::fs::create_dir_all(&installed).expect("a scratch directory");
        std::fs::create_dir_all(&lib).expect("a scratch directory");

        assert!(
            support_flags("cc", Some(&installed.join("cc"))).is_empty(),
            "a compiler that is not tcc was handed a tcc flag"
        );
        assert!(
            support_flags("tcc", Some(&installed.join("tcc"))).is_empty(),
            "a tcc with no support library in reach should be left to its own defaults"
        );

        std::fs::write(lib.join("libtcc1.a"), b"").expect("a scratch file");
        assert_eq!(
            support_flags("tcc", Some(&installed.join("tcc"))),
            vec![format!("-B{}", installed.join("../lib/tcc").display())],
            "an installed layout was not found"
        );

        let beside = dir.join("src");
        std::fs::create_dir_all(&beside).expect("a scratch directory");
        std::fs::write(beside.join("libtcc1.a"), b"").expect("a scratch file");
        assert_eq!(
            support_flags("tcc", Some(&beside.join("tcc"))),
            vec![format!("-B{}", beside.display())],
            "a source build's own directory was not found"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
