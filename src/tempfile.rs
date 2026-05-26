use std::path::PathBuf;

#[derive(Debug)]
pub struct TempFile {
    path: PathBuf,
}

impl TempFile {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }
}

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}
