const FUNCTIONS: [&str; 13] = [
    "acos", "asin", "atan", "atan2", "cos", "exp", "hypot", "log", "log10", "log2", "pow", "sin",
    "tan",
];

fn main() {
    let mut build = cc::Build::new();
    println!("cargo:rerun-if-changed=src/core-math");
    for f in FUNCTIONS {
        build.file(format!("src/core-math/{f}/{f}.c"));
    }
    build
        .flag_if_supported("-ffp-contract=off")
        .flag_if_supported("-fno-fast-math")
        .warnings(false)
        .cargo_warnings(false)
        .compile("core_math");
}
