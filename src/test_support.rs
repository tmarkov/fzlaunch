use std::ffi::OsStr;
use std::fs;
use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub struct TempDir {
    path: PathBuf,
}

impl TempDir {
    pub fn new(name: &str) -> Self {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should be after unix epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!("fzlaunch-{name}-{unique}"));
        fs::create_dir(&path).expect("temp dir should be created");
        Self { path }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl AsRef<OsStr> for TempDir {
    fn as_ref(&self) -> &OsStr {
        self.path.as_os_str()
    }
}

impl AsRef<Path> for TempDir {
    fn as_ref(&self) -> &Path {
        &self.path
    }
}

impl Deref for TempDir {
    type Target = Path;

    fn deref(&self) -> &Self::Target {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

pub fn path_string(dirs: impl IntoIterator<Item = impl AsRef<OsStr>>) -> String {
    std::env::join_paths(dirs)
        .expect("test paths should join")
        .to_str()
        .expect("test path should be utf-8")
        .to_string()
}
