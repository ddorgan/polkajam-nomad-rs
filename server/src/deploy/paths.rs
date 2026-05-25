use std::path::{Path, PathBuf};

pub const TARGET_VALIDATORS: u32 = 1023;

#[derive(Debug, Clone)]
pub struct StressPaths {
    pub app_dir: PathBuf,
    pub validators: PathBuf,
    pub builders: PathBuf,
    pub remainder: PathBuf,
}

impl StressPaths {
    pub fn new(app_dir: PathBuf) -> Self {
        Self {
            validators: app_dir.join("stress-net/validators.hcl"),
            builders: app_dir.join("stress-net/builders.hcl"),
            remainder: app_dir.join("stress-net/.validators-remainder.hcl"),
            app_dir,
        }
    }
}

pub fn find_app_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("APP_DIR") {
        return PathBuf::from(dir);
    }
    let cwd = std::env::current_dir().expect("cwd");
    if has_app_markers(&cwd) {
        return cwd;
    }
    if let Some(parent) = cwd.parent() {
        if has_app_markers(parent) {
            return parent.to_path_buf();
        }
    }
    cwd
}

fn has_app_markers(dir: &Path) -> bool {
    dir.join("templates").is_dir() || dir.join("stress-net").is_dir()
}
