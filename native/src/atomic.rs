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

fn temp_for(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".tmp");
    path.with_file_name(name)
}

/// Replace `path` with `bytes`.
pub fn write(path: &Path, bytes: impl AsRef<[u8]>) -> io::Result<()> {
    let temp = temp_for(path);
    let result = (|| {
        let mut file = File::create(&temp)?;
        file.write_all(bytes.as_ref())?;
        file.sync_all()?;
        fs::rename(&temp, path)
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
