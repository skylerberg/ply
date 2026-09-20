use ply_codegen::c::{Library, PRELUDE, helper_addresses, runtime_header, runtime_object, upgrade};

/// Whether `n << 40` fits an immediate. Only an optimiser exploiting the signed shift in
/// `ply_fits_imm` says it does for `n = 2^22`.
fn probe() -> String {
    let mut src = String::from(PRELUDE);
    src.push_str(&runtime_header());
    src.push_str(&runtime_object());
    src.push_str(
        r#"
Word ply_probe(PlyCtx *ctx, const Word *args) {
  (void)ctx;
  int64_t big = (int64_t)((uint64_t)ply_imm_value(args[0]) << 40);
  return ply_imm(ply_fits_imm(big));
}
"#,
    );
    src
}

fn fits(lib: &Library, n: i64) -> i64 {
    let bind = lib.symbol("ply_bind").expect("the unit exports `ply_bind`");
    let bind: unsafe extern "C" fn(*const *mut std::ffi::c_void) =
        unsafe { std::mem::transmute(bind) };
    let addrs = helper_addresses();
    unsafe { bind(addrs.as_ptr()) };
    let probe = lib
        .symbol("ply_probe")
        .expect("the unit exports `ply_probe`");
    let probe: ply_codegen::rt::Entry = unsafe { std::mem::transmute(probe) };
    let args = [ply_codegen::heap::imm(n)];
    ply_codegen::heap::imm_value(unsafe { probe(std::ptr::null_mut(), args.as_ptr()) })
}

fn loads_fast_then_optimised(source: &str) {
    let Some(optimised) = upgrade::object(source) else {
        return;
    };
    let failure =
        || std::fs::read_to_string(optimised.with_extension("failed")).unwrap_or_default();
    let first = match upgrade::load(source, "probe") {
        Ok(lib) => lib,
        Err(e) if e.to_string().contains("could not run") => return,
        Err(e) => panic!("{e:#}"),
    };
    assert_ne!(
        first.path(),
        optimised.as_path(),
        "the first load waited for the optimised compile"
    );
    assert!(
        optimised.with_extension("lock").exists() || optimised.is_file(),
        "the first load neither holds the upgrade's lock nor finds it done: {}",
        failure()
    );
    assert!(
        upgrade::compile(source),
        "the optimised compile failed: {}",
        failure()
    );
    let second = upgrade::load(source, "probe").expect("the optimised object loads");
    assert_eq!(
        second.path(),
        optimised.as_path(),
        "the optimised object is there and was not loaded"
    );
    for (lib, which) in [(&first, "fast"), (&second, "optimised")] {
        assert_eq!(fits(lib, 1), 1, "the {which} object");
        assert_eq!(
            fits(lib, 1 << 22),
            0,
            "the {which} object says 2^62 fits an immediate"
        );
    }
}

#[test]
fn a_load_answers_with_the_fast_object_until_the_optimised_one_lands() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    let _config = super::CONFIG.write().unwrap_or_else(|e| e.into_inner());
    let restore = std::env::var("PLY_C_CACHE").ok();
    unsafe { std::env::set_var("PLY_C_CACHE", dir.path()) };
    loads_fast_then_optimised(&probe());
    unsafe {
        match &restore {
            Some(had) => std::env::set_var("PLY_C_CACHE", had),
            None => std::env::remove_var("PLY_C_CACHE"),
        }
    }
}
