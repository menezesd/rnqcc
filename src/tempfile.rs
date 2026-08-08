use std::path::{Path, PathBuf};

#[cfg(any(unix, windows))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FileIdentity {
    volume_or_device: u64,
    file_index_or_inode: u64,
    change_time_seconds: i64,
    change_time_nanos: i64,
}

#[cfg(unix)]
fn file_identity(path: &Path) -> Option<FileIdentity> {
    use std::os::unix::fs::MetadataExt;

    let metadata = std::fs::symlink_metadata(path).ok()?;
    Some(FileIdentity {
        volume_or_device: metadata.dev(),
        file_index_or_inode: metadata.ino(),
        // Device/inode pairs can be reused immediately after unlinking a
        // file. Include ctime so a replacement file is not mistaken for the
        // guarded original on filesystems that reuse the inode.
        change_time_seconds: metadata.ctime(),
        change_time_nanos: metadata.ctime_nsec(),
    })
}

#[cfg(windows)]
fn file_identity(path: &Path) -> Option<FileIdentity> {
    use std::mem::MaybeUninit;
    use std::os::windows::io::AsRawHandle;

    #[repr(C)]
    struct FileTime {
        low_date_time: u32,
        high_date_time: u32,
    }

    #[repr(C)]
    struct ByHandleFileInformation {
        file_attributes: u32,
        creation_time: FileTime,
        last_access_time: FileTime,
        last_write_time: FileTime,
        volume_serial_number: u32,
        file_size_high: u32,
        file_size_low: u32,
        number_of_links: u32,
        file_index_high: u32,
        file_index_low: u32,
    }

    unsafe extern "system" {
        fn GetFileInformationByHandle(
            file: *mut std::ffi::c_void,
            information: *mut ByHandleFileInformation,
        ) -> i32;
    }

    let file = std::fs::File::open(path).ok()?;
    let mut information = MaybeUninit::<ByHandleFileInformation>::uninit();
    // SAFETY: `file` is an open Windows file handle, and `information` points
    // to writable storage with the exact layout required by the API.
    let succeeded =
        unsafe { GetFileInformationByHandle(file.as_raw_handle(), information.as_mut_ptr()) != 0 };
    if !succeeded {
        return None;
    }
    // SAFETY: the API reported success, so it initialized the structure.
    let information = unsafe { information.assume_init() };
    Some(FileIdentity {
        volume_or_device: information.volume_serial_number as u64,
        file_index_or_inode: u64::from(information.file_index_high) << 32
            | u64::from(information.file_index_low),
        change_time_seconds: 0,
        change_time_nanos: 0,
    })
}

/// Removes its path when the guard is dropped.
#[must_use = "a TempFile must be kept alive until the temporary file is no longer needed"]
#[derive(Debug)]
pub struct TempFile {
    path: PathBuf,
    #[cfg(any(unix, windows))]
    identity: Option<FileIdentity>,
}

impl TempFile {
    /// Creates a guard for an existing temporary file path.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        #[cfg(any(unix, windows))]
        let identity = file_identity(&path);
        Self {
            path,
            #[cfg(any(unix, windows))]
            identity,
        }
    }
}

impl Drop for TempFile {
    fn drop(&mut self) {
        #[cfg(any(unix, windows))]
        {
            let Some(identity) = self.identity else {
                return;
            };
            let Some(current_identity) = file_identity(&self.path) else {
                return;
            };
            if identity != current_identity {
                return;
            }
        }
        let _ = std::fs::remove_file(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::TempFile;

    #[test]
    fn removes_original_file_on_drop() {
        let path = std::env::temp_dir().join(format!(
            "rnqcc-tempfile-test-{}-{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("unnamed")
        ));
        let _ = std::fs::remove_file(&path);
        std::fs::write(&path, b"temporary").expect("create temporary file");
        {
            let _guard = TempFile::new(&path);
        }
        assert!(!path.exists());
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn does_not_remove_replacement_file_on_drop() {
        let path = std::env::temp_dir().join(format!(
            "rnqcc-tempfile-replacement-{}-{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("unnamed")
        ));
        let _ = std::fs::remove_file(&path);
        std::fs::write(&path, b"original").expect("create original file");
        let guard = TempFile::new(&path);
        std::fs::remove_file(&path).expect("remove original file");
        std::fs::write(&path, b"replacement").expect("create replacement file");
        drop(guard);
        assert_eq!(
            std::fs::read(&path).expect("read replacement"),
            b"replacement"
        );
        std::fs::remove_file(path).expect("remove replacement file");
    }

    #[cfg(unix)]
    #[test]
    fn does_not_remove_replacement_symlink_to_same_target_on_drop() {
        use std::os::unix::fs::symlink;

        let root = std::env::temp_dir();
        let stem = format!(
            "rnqcc-tempfile-symlink-replacement-{}-{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("unnamed")
        );
        let path = root.join(&stem);
        let target = root.join(format!("{stem}-target"));
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(&target);
        std::fs::write(&path, b"original").expect("create original file");
        std::fs::write(&target, b"target").expect("create symlink target");
        let guard = TempFile::new(&path);
        std::fs::remove_file(&path).expect("remove original file");
        symlink(&target, &path).expect("create replacement symlink");
        drop(guard);
        assert!(std::fs::symlink_metadata(&path)
            .expect("read replacement symlink")
            .file_type()
            .is_symlink());
        assert_eq!(
            std::fs::read(&path).expect("read through replacement symlink"),
            b"target"
        );
        std::fs::remove_file(path).expect("remove replacement symlink");
        std::fs::remove_file(target).expect("remove symlink target");
    }
}
