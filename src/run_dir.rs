//! Private per-run scratch directory for files that contain secrets.

use std::fs::{self, DirBuilder};
use std::io;
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const MAX_ATTEMPTS: u32 = 16;

/// A mode-0700 directory removed on drop.
///
/// It is created with plain `mkdir`, which fails if the path already exists
/// (including as a symlink), so another local user cannot pre-create or
/// redirect it, and files written inside are unreadable to them.
pub struct RunDir {
    path: PathBuf,
}

impl RunDir {
    pub fn create() -> io::Result<Self> {
        Self::create_in(&std::env::temp_dir())
    }

    fn create_in(parent: &Path) -> io::Result<Self> {
        let pid = std::process::id();
        for attempt in 0..MAX_ATTEMPTS {
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|elapsed| elapsed.subsec_nanos())
                .unwrap_or_default();
            let path = parent.join(format!("omarchy-hotspot.{pid}.{nanos}.{attempt}"));
            match DirBuilder::new().mode(0o700).create(&path) {
                Ok(()) => return Ok(Self { path }),
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e),
            }
        }
        Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "could not create a unique private directory",
        ))
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for RunDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn is_private_unique_and_removed_on_drop() {
        let parent = std::env::temp_dir();
        let first = RunDir::create_in(&parent).unwrap();
        let second = RunDir::create_in(&parent).unwrap();
        let first_path = first.path().to_path_buf();

        assert_ne!(first.path(), second.path());
        let meta = fs::symlink_metadata(&first_path).unwrap();
        assert!(meta.is_dir());
        assert_eq!(meta.permissions().mode() & 0o777, 0o700);

        fs::write(first_path.join("qr.png"), b"secret").unwrap();
        drop(first);
        assert!(!first_path.exists());
    }
}
