//! Telling Google's CarrierSettings from our own.
//!
//! The patch is derived from the stock files on every boot, which only works while it is still
//! possible to say which files those are: once the mount backend has done its work, /product
//! shows what we wrote. So every run leaves behind a copy of what it read and a fingerprint of
//! what it produced, and later runs use the two to keep from mistaking their own work for
//! Google's.

use crate::atomic;
use serde::Serialize;
use std::collections::BTreeMap;
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
    for generation in generations(cache) {
        if matches!(product_in(src, &generation), Product::Ours)
            && generation.join("others.pb").is_file()
        {
            return generation;
        }
    }
    src.to_path_buf()
}

fn generations(cache: &Path) -> Vec<PathBuf> {
    let mut out = vec![cache.to_path_buf()];
    if let Ok(entries) = fs::read_dir(cache.with_extension("generations")) {
        out.extend(
            entries
                .flatten()
                .filter(|e| {
                    e.file_type().is_ok_and(|t| t.is_dir()) && e.path().join(".complete").is_file()
                })
                .map(|e| e.path()),
        );
    }
    out
}

/// Is the file the system reads right now the one we produced?
#[cfg(test)]
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
    let primary = product_in(src, cache);
    if primary == Product::Ours {
        return primary;
    }
    if generations(cache)
        .iter()
        .skip(1)
        .any(|generation| product_in(src, generation) == Product::Ours)
    {
        return Product::Ours;
    }
    primary
}

fn product_in(src: &Path, cache: &Path) -> Product {
    let Ok(live) = fs::read(src.join("others.pb")) else {
        return Product::Other;
    };
    let generated: Option<Vec<BTreeMap<String, u64>>> = match fs::read(cache.join("outputs.json")) {
        Ok(bytes) => match serde_json::from_slice(&bytes) {
            Ok(value) => Some(value),
            Err(_) => return Product::Other,
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(_) => return Product::Other,
    };
    if generated
        .as_ref()
        .is_some_and(|outputs| outputs.iter().any(|files| matches_files(src, files)))
    {
        return Product::Ours;
    }
    let live = fingerprint(&live);
    // Only pre-format-2 installations used this weaker, single-file fingerprint.
    if generated.is_none() {
        let ours = fs::read_to_string(cache.join("output.fingerprint"))
            .ok()
            .and_then(|t| t.trim().parse::<u64>().ok());
        if ours == Some(live) {
            return Product::Ours;
        }
    }
    match fs::read(cache.join("others.pb")) {
        Ok(stock) if fingerprint(&stock) == live => Product::Stock,
        _ => Product::Other,
    }
}

/// Fingerprints of the protobuf files in a directory.
pub fn files(dir: &Path) -> std::io::Result<BTreeMap<String, u64>> {
    let mut result = BTreeMap::new();
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.ends_with(".pb") && entry.file_type()?.is_file() {
            result.insert(name, fingerprint(&fs::read(entry.path())?));
        }
    }
    Ok(result)
}

pub fn matches_files(dir: &Path, files: &BTreeMap<String, u64>) -> bool {
    !files.is_empty()
        && files.iter().all(|(name, expected)| {
            // Manifests are persistent input too: reject paths before reading them.
            name.strip_suffix(".pb")
                .is_some_and(|n| n == "others" || crate::config::validate_name(n).is_ok())
                && fs::read(dir.join(name)).is_ok_and(|bytes| fingerprint(&bytes) == *expected)
        })
}

/// Publish a full snapshot, including standalone carriers not currently selected. This prevents
/// old standalone files from overriding new entries in others.pb after an OS update.
pub fn refresh(src: &Path, cache: &Path) -> std::io::Result<()> {
    let inputs = files(src)?;
    if inputs == files(cache).unwrap_or_default() && cache.join("others.pb").is_file() {
        return Ok(());
    }
    if let Some(parent) = cache.parent() {
        fs::create_dir_all(parent)?;
    }
    // Keep older inputs while their generated files may still be mounted (or retained after
    // failed preparation). An output must always resolve to its own stock generation.
    if cache.join("others.pb").is_file() {
        let archives = cache.with_extension("generations");
        fs::create_dir_all(&archives)?;
        let saved = atomic::temp_for(&archives.join("stock"));
        fs::create_dir(&saved)?;
        let archived = (|| {
            for entry in fs::read_dir(cache)? {
                let entry = entry?;
                if entry.file_type()?.is_file() {
                    atomic::copy(&entry.path(), &saved.join(entry.file_name()))?;
                }
            }
            atomic::write(&saved.join(".complete"), b"1\n")?;
            fs::File::open(&saved)?.sync_all()?;
            atomic::sync_parent(&saved)
        })();
        if let Err(error) = archived {
            let _ = fs::remove_dir_all(&saved);
            return Err(error);
        }
    }
    let stage = atomic::temp_for(cache);
    fs::create_dir(&stage)?;
    let result = (|| {
        for name in inputs.keys() {
            atomic::copy(&src.join(name), &stage.join(name))?;
        }
        atomic::replace_dir(&stage, cache)
    })();
    if result.is_err() {
        let _ = fs::remove_dir_all(&stage);
    }
    result
}

/// Register each generated result against the current stock generation. A manual generation
/// must not forget the still-mounted previous result.
pub fn remember_output(cache: &Path, output: &BTreeMap<String, u64>) -> std::io::Result<()> {
    if output.is_empty() {
        return Ok(());
    }
    let path = cache.join("outputs.json");
    let mut outputs: Vec<BTreeMap<String, u64>> = match fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes)?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let legacy = fs::read_to_string(cache.join("output.fingerprint"))
                .ok()
                .and_then(|s| s.trim().parse::<u64>().ok());
            legacy
                .into_iter()
                .map(|hash| BTreeMap::from([("others.pb".to_string(), hash)]))
                .collect()
        }
        Err(e) => return Err(e),
    };
    if !outputs.contains(output) {
        outputs.push(output.clone());
    }
    atomic::write(&path, serde_json::to_vec(&outputs)?)
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

        refresh(&dir, &dir).unwrap();

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
