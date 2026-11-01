pub const FAMILIES: [&str; 3] = ["machine", "shipped", "toolchain"];

pub fn names() -> Vec<&'static str> {
    FAMILIES.to_vec()
}
