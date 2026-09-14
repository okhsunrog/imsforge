//! Writes that either land whole or not at all.
//!
//! Everything here ends up in /data/adb/imsforge, which outlives the boot that wrote it and is
//! read by the next one. A phone switched off by holding the button can interrupt any of these
//! writes, and a half-written file is worse than a missing one: a truncated stock copy is still
//! a parseable protobuf, and a truncated fingerprint still parses as a number.
//!
//! So each write lands in a temporary file beside the target, is flushed, and is then moved into
//! place — a rename within one directory, which the filesystem does whole or not at all.

use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// Unique siblings prevent concurrent writers from truncating each other's temporary file.
pub fn temp_for(path: &Path) -> PathBuf {
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(format!(
        ".tmp.{}.{}.{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    path.with_file_name(name)
}

pub fn sync_parent(path: &Path) -> io::Result<()> {
    File::open(
        path.parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new(".")),
    )?
    .sync_all()
}

/// Held for the operation's lifetime; an interrupted process releases the kernel lock.
pub fn lock(path: &Path) -> io::Result<File> {
    use std::os::fd::AsRawFd;
    let file = File::options()
        .create(true)
        .truncate(false)
        .write(true)
        .open(path)?;
    // SAFETY: flock only uses this live descriptor and scalar flags.
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(file)
}

/// Exchange complete directories without a moment at which the old output is absent.
/// If the filesystem does not support exchange, fail before changing either directory.
pub fn replace_dir(stage: &Path, dest: &Path) -> io::Result<()> {
    use std::os::unix::ffi::OsStrExt;
    File::open(stage)?.sync_all()?;
    if dest.exists() && (!dest.is_dir() || dest.is_symlink()) {
        return Err(io::Error::other("destination must be a real directory"));
    }
    if dest.exists() {
        let a = std::ffi::CString::new(stage.as_os_str().as_bytes())?;
        let b = std::ffi::CString::new(dest.as_os_str().as_bytes())?;
        // SAFETY: both C strings are live and NUL terminated for the duration of the syscall.
        let result = unsafe {
            libc::syscall(
                libc::SYS_renameat2,
                libc::AT_FDCWD,
                a.as_ptr(),
                libc::AT_FDCWD,
                b.as_ptr(),
                libc::RENAME_EXCHANGE,
            )
        };
        if result != 0 {
            return Err(io::Error::last_os_error());
        }
        sync_parent(dest)?;
        // The exchange already committed. Failure to remove the old generation is not a
        // failure of the publication; a subsequent run can clean this directory up.
        if let Err(e) = fs::remove_dir_all(stage) {
            eprintln!("old generation cleanup: {e}");
        }
    } else {
        fs::rename(stage, dest)?;
        sync_parent(dest)?;
    }
    Ok(())
}

/// Replace `path` with `bytes`.
pub fn write(path: &Path, bytes: impl AsRef<[u8]>) -> io::Result<()> {
    let temp = temp_for(path);
    let result = (|| {
        let mut file = File::options().write(true).create_new(true).open(&temp)?;
        file.write_all(bytes.as_ref())?;
        file.sync_all()?;
        fs::rename(&temp, path)?;
        sync_parent(path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

/// Replace `to` with the contents of `from`.
pub fn copy(from: &Path, to: &Path) -> io::Result<()> {
    write(to, fs::read(from)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::tempdir;

    #[test]
    fn a_write_replaces_what_was_there() {
        let dir = tempdir("write");
        let path = dir.join("f");
        write(&path, b"one").unwrap();
        write(&path, b"two").unwrap();

        assert_eq!(fs::read(&path).unwrap(), b"two");
        // Nothing is left lying around for the next run to trip over.
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 1);
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_failed_write_leaves_the_original_alone() {
        let dir = tempdir("fail");
        let path = dir.join("sub/f");
        // The parent does not exist, so the temporary file cannot even be created.
        assert!(write(&path, b"x").is_err());
        assert!(!path.exists());
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn copying_a_file_onto_itself_keeps_it() {
        // fs::copy truncates in this case; reading first and renaming into place does not.
        let dir = tempdir("self");
        let path = dir.join("f");
        fs::write(&path, b"stock bytes").unwrap();

        copy(&path, &path).unwrap();

        assert_eq!(fs::read(&path).unwrap(), b"stock bytes");
        fs::remove_dir_all(&dir).ok();
    }
}
