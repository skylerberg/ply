use std::path::PathBuf;

pub fn stage_dir(identity: &str) -> PathBuf {
    stage_root().join(identity)
}

pub fn stage_root() -> PathBuf {
    super::cache_dir_for("stage", "PLY_C_STAGE")
}
