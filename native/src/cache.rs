//! Telling Google's CarrierSettings from our own.
//!
//! The patch is derived from the stock files on every boot, which only works while it is still
//! possible to say which files those are: once the mount backend has done its work, /product
//! shows what we wrote. So every run leaves behind a copy of what it read and a fingerprint of
//! what it produced, and later runs use the two to keep from mistaking their own work for
//! Google's.

use crate::atomic;
use serde::Serialize;
use std::fs;
use std::path::{Path, PathBuf};

/// Cheap content fingerprint (FNV-1a). Only ever compared against one we wrote ourselves, so it
/// needs no cryptographic strength — and no dependency.
pub fn fingerprint(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100_0000_01b3);
    }
    h
}

/// Where to read the stock protobufs from.
///
/// At boot we run before the mount backend, so /product still holds Google's originals. Run by
/// hand later, /product shows our own patched files instead — and reading those would make the
/// "has Google already certified this carrier" check see our own work and skip everything.
///
/// So each successful run records the fingerprint of what it produced. If the live file matches
/// that, we are looking at ourselves and read the cached stock instead.
pub fn effective_src(src: &Path, cache: &Path) -> PathBuf {
    if is_ours(src, cache) && cache.join("others.pb").exists() {
        return cache.to_path_buf();
    }
    src.to_path_buf()
}

/// Is the file the system reads right now the one we produced?
pub fn is_ours(src: &Path, cache: &Path) -> bool {
    matches!(product(src, cache), Product::Ours)
}

/// Whose CarrierSettings is at `src` at this moment.
///
/// The mount backend lays our files over /product only after the patch has run, so nothing at
/// patch time can tell — this is a question only a later look can answer. It is reported as what
/// was seen rather than as a verdict: a root shell does not always get the same view of /product
/// as an app does, so "not ours" is not the same statement as "the patch did not work".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Product {
    /// What we wrote, which is what a working mount backend puts there.
    Ours,
    /// Google's own file, byte for byte the one we cached.
    Stock,
    /// Neither: no cache to compare against yet, or an OS update moved the ground.
    Other,
}

pub fn product(src: &Path, cache: &Path) -> Product {
    let Ok(live) = fs::read(src.join("others.pb")) else {
        return Product::Other;
    };
    let live = fingerprint(&live);
    let ours = fs::read_to_string(cache.join("output.fingerprint"))
        .ok()
        .and_then(|t| t.trim().parse::<u64>().ok());
    if ours == Some(live) {
        return Product::Ours;
    }
    match fs::read(cache.join("others.pb")) {
        Ok(stock) if fingerprint(&stock) == live => Product::Stock,
        _ => Product::Other,
    }
}

/// Keep a copy of the stock inputs plus the fingerprint of our output, so a later manual run has
/// something truthful to read and can tell our work from Google's.
///
/// A cache that only half exists is worse than none: every later run judges carriers by it, and
/// a file missing from it reads as "Google ships nothing for this carrier". So failures are
/// reported rather than swallowed, even though they cannot fail the patch that already happened.
pub fn refresh(src: &Path, cache: &Path, candidates: &[String], output: &[u8]) {
    if let Err(e) = fs::create_dir_all(cache) {
        eprintln!("  cache: {e}");
        return;
    }
    let mut files = vec!["others.pb".to_string()];
    files.extend(candidates.iter().map(|name| format!("{name}.pb")));
    for name in files {
        let from = src.join(&name);
        if !from.exists() {
            continue;
        }
        if let Err(e) = atomic::copy(&from, &cache.join(&name)) {
            eprintln!("  cache: {name}: {e}");
        }
    }
    if let Err(e) = atomic::write(
        &cache.join("output.fingerprint"),
        fingerprint(output).to_string(),
    ) {
        eprintln!("  cache: output.fingerprint: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::tempdir;

    #[test]
    fn refreshing_onto_itself_keeps_the_data() {
        // Pointing a run at its own cache is nonsense, but it must not cost the user their copy
        // of the stock: fs::copy truncates when source and destination are the same file.
        let dir = tempdir("cache");
        let file = dir.join("others.pb");
        fs::write(&file, b"stock bytes").unwrap();

        refresh(&dir, &dir, &[], b"output");

        assert_eq!(fs::read(&file).unwrap(), b"stock bytes");
    }

    #[test]
    fn the_fingerprint_tells_our_output_apart() {
        assert_eq!(fingerprint(b"a"), fingerprint(b"a"));
        assert_ne!(fingerprint(b"a"), fingerprint(b"b"));
    }

    #[test]
    fn the_cache_is_used_when_the_live_file_is_our_own_output() {
        let dir = tempdir("src");
        let cache = tempdir("stock");
        fs::write(dir.join("others.pb"), b"patched").unwrap();
        fs::write(cache.join("others.pb"), b"stock").unwrap();

        // No fingerprint yet: the live file is taken at face value.
        assert_eq!(effective_src(&dir, &cache), dir);

        // Once it matches what we produced, reading it would mean reading ourselves.
        fs::write(
            cache.join("output.fingerprint"),
            fingerprint(b"patched").to_string(),
        )
        .unwrap();
        assert_eq!(effective_src(&dir, &cache), cache);
    }

    #[test]
    fn what_is_at_product_is_reported_as_what_it_is() {
        let dir = tempdir("product");
        let cache = tempdir("product-cache");
        fs::write(dir.join("others.pb"), b"stock").unwrap();

        // Nothing cached yet: there is nothing to compare against.
        assert_eq!(product(&dir, &cache), Product::Other);

        fs::write(cache.join("others.pb"), b"stock").unwrap();
        assert_eq!(product(&dir, &cache), Product::Stock);

        fs::write(dir.join("others.pb"), b"patched").unwrap();
        assert_eq!(product(&dir, &cache), Product::Other, "an OS update, say");

        fs::write(
            cache.join("output.fingerprint"),
            fingerprint(b"patched").to_string(),
        )
        .unwrap();
        assert_eq!(product(&dir, &cache), Product::Ours);
    }
}
