use std::fs::{self, File, OpenOptions, TryLockError};
use std::io;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

/// Keep the returned file open for the entire application lifetime.
pub fn acquire() -> io::Result<Option<File>> {
    let directory = std::env::var_os("XDG_RUNTIME_DIR")
        .filter(|value| !value.is_empty())
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            crate::utils::get_preferences_path()
                .parent()
                .expect("preferences path has a parent")
                .to_path_buf()
        });
    fs::create_dir_all(&directory)?;
    acquire_at(&directory.join("airpods-driver.lock"))
}

fn acquire_at(path: &Path) -> io::Result<Option<File>> {
    // Never remove this file: unlinking it could let another process lock a
    // different inode while the original instance still holds its lock.
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(path)?;
    match file.try_lock() {
        Ok(()) => Ok(Some(file)),
        Err(TryLockError::WouldBlock) => Ok(None),
        Err(TryLockError::Error(error)) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_second_instance_and_releases_lock_on_drop() {
        let path = std::env::temp_dir().join(format!(
            "airpods-driver-single-instance-test-{}.lock",
            std::process::id()
        ));
        let first = acquire_at(&path).unwrap().unwrap();
        assert!(acquire_at(&path).unwrap().is_none());
        drop(first);
        let next = acquire_at(&path).unwrap().unwrap();
        drop(next);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn reports_io_errors() {
        assert!(acquire_at(Path::new("/dev/null/airpods-driver.lock")).is_err());
    }
}
