//! The publication boundary: prepare everything before changing the files a backend mounts.
use crate::{PatchArgs, STATUS, Source, Status, atomic, generate};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;

pub fn run(
    args: &PatchArgs,
    boot: bool,
    prepare: impl FnOnce(&Path) -> Result<(), String>,
) -> Result<(), String> {
    let parent = args
        .out
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    if args.out.file_name().is_none() || args.out.is_symlink() {
        return Err("output must be a real, named directory".into());
    }
    fs::create_dir_all(parent).map_err(crate::at(parent))?;
    let dest = fs::canonicalize(parent)
        .map_err(crate::at(parent))?
        .join(args.out.file_name().unwrap());
    let src = fs::canonicalize(&args.sources.src).map_err(crate::at(&args.sources.src))?;
    if src.starts_with(&dest) || dest.starts_with(&src) {
        return Err("output and source directories must not overlap".into());
    }
    let cache_parent = args
        .sources
        .stock_cache
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    fs::create_dir_all(cache_parent).map_err(crate::at(cache_parent))?;
    let cache = fs::canonicalize(cache_parent)
        .map_err(crate::at(cache_parent))?
        .join(
            args.sources
                .stock_cache
                .file_name()
                .ok_or("invalid cache directory")?,
        );
    if cache.starts_with(&dest) || dest.starts_with(&cache) {
        return Err("output and stock cache directories must not overlap".into());
    }
    let _lock = atomic::lock(&cache.with_extension("lock"))
        .map_err(|e| format!("another patch may be running: {e}"))?;
    let report = args.status.clone().unwrap_or_else(|| {
        if boot {
            STATUS.into()
        } else {
            dest.join(".imsforge.json")
        }
    });
    if boot {
        let mut started = Status::new(Source::Stock, vec![]);
        started.phase = "preparing".into();
        started.write(&report)?;
    }
    let stage = atomic::temp_for(&dest);
    let result: Result<(), String> = (|| {
        fs::create_dir(&stage).map_err(crate::at(&stage))?;
        let mut generated = generate(&PatchArgs {
            out: stage.clone(),
            ..args.clone()
        })?;
        prepare(&stage)?;
        if boot {
            clear_phone_cache(&args.phone_files)?;
        }
        generated.phase = if boot { "applied" } else { "generated" }.into();
        // Default manual reports travel with their files. Boot reports remain outside the
        // mounted tree and are committed only after the directory exchange succeeds.
        if !boot && args.status.is_none() {
            generated.write(&stage.join(".imsforge.json"))?;
        }
        atomic::replace_dir(&stage, &dest).map_err(crate::at(&dest))?;
        if boot || args.status.is_some() {
            generated.write(&report)?;
        }
        Ok(())
    })();
    if let Err(ref error) = result {
        let _ = fs::remove_dir_all(&stage);
        if boot {
            let mut failed = Status::new(Source::Stock, vec![]);
            failed.phase = "failed".into();
            failed.error = Some(error.clone());
            if let Err(e) = failed.write(&report) {
                eprintln!("cannot record failure: {e}");
            }
        }
    }
    result
}

pub fn label(stage: &Path, src: &Path) -> Result<(), String> {
    let files = crate::cache::files(stage).map_err(crate::at(stage))?;
    if files.is_empty() {
        return Ok(());
    }
    let reference = src.join("carrier_list.pb");
    let output = Command::new("ls")
        .arg("-Z")
        .arg(&reference)
        .output()
        .map_err(crate::at(&reference))?;
    let text = String::from_utf8_lossy(&output.stdout);
    let context = text
        .split_whitespace()
        .next()
        .filter(|s| s.starts_with("u:object_r:"))
        .ok_or("cannot read stock SELinux context")?;
    if !output.status.success() {
        return Err("cannot read stock SELinux context".into());
    }
    for name in files.keys() {
        let path = stage.join(name);
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).map_err(crate::at(&path))?;
        let status = Command::new("chcon")
            .arg(context)
            .arg(&path)
            .status()
            .map_err(crate::at(&path))?;
        if !status.success() {
            return Err(format!("{}: chcon failed", path.display()));
        }
        fs::File::open(&path)
            .and_then(|f| f.sync_all())
            .map_err(crate::at(&path))?;
    }
    Ok(())
}

fn clear_phone_cache(dir: &Path) -> Result<(), String> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(format!("{}: {e}", dir.display())),
    };
    for entry in entries {
        let entry = entry.map_err(crate::at(dir))?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with("carrierconfig-com.google.android.carrier-") && name.ends_with(".xml") {
            fs::remove_file(entry.path()).map_err(crate::at(&entry.path()))?;
        }
    }
    Ok(())
}
