//! imsforge — on-device CarrierSettings patcher.
//!
//! Reads the stock protobufs straight off /product, works out which carriers need IMS enabled,
//! patches them and writes the result where the mount backend picks it up. Run from the module's
//! post-fs-data.sh, this re-derives the patch from whatever Google shipped on this boot, so an OS
//! update can never leave a stale snapshot behind.

mod config;
mod detect;
mod patch;

mod protos {
    include!(concat!(env!("OUT_DIR"), "/protos/mod.rs"));
}

use clap::{Parser, Subcommand};
use config::{Carrier, Config};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

#[derive(Parser)]
#[command(name = "imsforge", version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Patch the CarrierSettings protobufs into a directory
    Patch {
        /// Where to write the patched protobufs
        #[arg(long, value_name = "DIR")]
        out: PathBuf,

        #[command(flatten)]
        paths: Paths,
    },
    /// Print, as JSON, what the inserted SIMs resolve to
    Detect {
        /// Remember the carriers for the next boot, when the modem will not be up in time
        #[arg(long)]
        save: bool,

        #[command(flatten)]
        paths: Paths,
    },
}

#[derive(clap::Args)]
struct Paths {
    /// Stock CarrierSettings directory
    #[arg(
        long,
        value_name = "DIR",
        default_value = "/product/etc/CarrierSettings"
    )]
    src: PathBuf,

    /// Carrier overrides
    #[arg(
        long,
        value_name = "FILE",
        default_value = "/data/adb/imsforge/carriers.json"
    )]
    config: PathBuf,

    /// Copy of the stock protobufs, for runs that can no longer see them
    #[arg(long, value_name = "DIR", default_value = "/data/adb/imsforge/stock")]
    cache: PathBuf,

    /// Carriers remembered for the next boot
    #[arg(long, value_name = "FILE", default_value = "/data/adb/imsforge/sims")]
    sims: PathBuf,

    /// Telephony's config cache, read to identify carriers when the modem is down
    #[arg(
        long,
        value_name = "DIR",
        default_value = "/data/user_de/0/com.android.phone/files"
    )]
    phone_files: PathBuf,
}

/// Cheap content fingerprint (FNV-1a). Only ever compared against one we wrote ourselves, so it
/// needs no cryptographic strength — and no dependency.
fn fingerprint(bytes: &[u8]) -> u64 {
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
fn effective_src(src: &Path, cache: &Path) -> PathBuf {
    let Ok(live) = std::fs::read(src.join("others.pb")) else {
        return src.to_path_buf();
    };
    let ours = std::fs::read_to_string(cache.join("output.fingerprint"))
        .ok()
        .and_then(|t| t.trim().parse::<u64>().ok());
    if ours == Some(fingerprint(&live)) && cache.join("others.pb").exists() {
        return cache.to_path_buf();
    }
    src.to_path_buf()
}

/// Keep a copy of the stock inputs plus the fingerprint of our output, so a later manual run has
/// something truthful to read and can tell our work from Google's.
fn refresh_cache(src: &Path, cache: &Path, names: &[String], output: &[u8]) {
    if let Err(e) = std::fs::create_dir_all(cache) {
        eprintln!("  cache: {e}");
        return;
    }
    let mut files = vec!["others.pb".to_string()];
    files.extend(names.iter().map(|n| format!("{n}.pb")));
    for name in files {
        let from = src.join(&name);
        let to = cache.join(&name);
        // Copying a file onto itself truncates it to nothing, so a run pointed at its own cache
        // would destroy the very stock it is meant to preserve.
        if from.exists() && !same_file(&from, &to) {
            let _ = std::fs::copy(&from, &to);
        }
    }
    let _ = std::fs::write(
        cache.join("output.fingerprint"),
        fingerprint(output).to_string(),
    );
}

fn same_file(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

struct Target {
    carrier: Carrier,
    reason: &'static str,
    /// Operator name the SIM reports, for logging. CarrierSettings identifies a carrier by a
    /// canonical name that Google only bothered to make readable for the carriers it supports —
    /// for the rest the entry is called by its MCCMNC, so a log line of bare "25001" tells the
    /// reader nothing.
    label: String,
}

/// Default the IMS APN label to the carrier name the SIM reports.
///
/// Purely cosmetic — the APN works off its value and type, not its label — but without it the
/// entry shows up in Settings as "25001 IMS" next to Google's own "MTS Internet" and "MTS MMS",
/// which reads like a glitch.
fn name_apns(targets: &mut [Target], sims: &[(String, String)]) {
    for t in targets.iter_mut() {
        if let Some((_, spn)) = sims
            .iter()
            .find(|(name, spn)| *name == t.carrier.canonical_name && !spn.is_empty())
        {
            t.label = spn.clone();
        }
        if !t.carrier.ims_apn_name.is_empty() {
            continue;
        }
        if !t.label.is_empty() {
            t.carrier.ims_apn_name = format!("{} IMS", t.label);
        }
    }
}

/// Targets to patch, plus every carrier we considered — the cache needs the stock files of the
/// skipped ones too, or a later run has nothing to judge them by.
fn targets(
    cfg: &Config,
    data_src: &Path,
    paths: &Paths,
) -> Result<(Vec<Target>, Vec<String>), String> {
    // Which carriers are in this phone? Three sources, in order of quality.
    //
    // At post-fs-data the modem is not up yet, so the SIM properties are empty and only the last
    // two work — which is the normal case for the boot-time run, not the exception.
    let mut resolved: Vec<(String, String)> = Vec::new();
    let mut source = "live SIM properties";
    if let Ok(list) = detect::load_carrier_list(&paths.src) {
        for sim in detect::sims() {
            if let Some(name) = detect::resolve(&list, &sim) {
                resolved.push((name, sim.spn));
            }
        }
    }
    if resolved.is_empty() {
        resolved = detect::from_saved(&paths.sims);
        source = "saved by the previous boot";
    }
    if resolved.is_empty() {
        resolved = detect::from_config_cache(&paths.phone_files)
            .into_iter()
            .map(|n| (n, String::new()))
            .collect();
        source = "telephony's own config cache";
    }
    if !resolved.is_empty() {
        println!("  carriers in this phone, from {source}");
    }

    let mut targets: Vec<Target> = cfg
        .carriers
        .iter()
        .filter(|c| !cfg.skip.contains(&c.canonical_name))
        .map(|c| Target {
            carrier: c.clone(),
            reason: "configured",
            label: String::new(),
        })
        .collect();

    let mut candidates: Vec<String> = resolved.iter().map(|(n, _)| n.clone()).collect();
    for name in cfg
        .carriers
        .iter()
        .map(|c| &c.canonical_name)
        .chain(cfg.skip.iter())
    {
        if !candidates.contains(name) {
            candidates.push(name.clone());
        }
    }

    if !cfg.auto || resolved.is_empty() {
        if resolved.is_empty() {
            eprintln!("  no carriers identified — nothing to detect automatically");
        }
        name_apns(&mut targets, &resolved);
        return Ok((targets, candidates));
    }

    let others_bytes =
        std::fs::read(data_src.join("others.pb")).map_err(|e| format!("others.pb: {e}"))?;
    let others = patch::parse_others(&others_bytes).map_err(|e| format!("others.pb: {e}"))?;

    for (name, _) in resolved.clone() {
        if cfg.skip.contains(&name) || targets.iter().any(|t| t.carrier.canonical_name == name) {
            continue;
        }

        // The carrier's settings live either in its own file or inside others.pb.
        let own = data_src.join(format!("{name}.pb"));
        let enabled = if own.exists() {
            let bytes = std::fs::read(&own).map_err(|e| format!("{}: {e}", own.display()))?;
            patch::volte_enabled(&patch::parse_single(&bytes).map_err(|e| e.to_string())?)
        } else {
            match others.setting.iter().find(|s| s.canonical_name() == name) {
                Some(settings) => patch::volte_enabled(settings),
                None => {
                    eprintln!("  {name}: no CarrierSettings entry, skipping");
                    continue;
                }
            }
        };

        if enabled {
            println!("  {name}: VoLTE already enabled by Google, leaving alone");
            continue;
        }
        targets.push(Target {
            carrier: Carrier::new(name),
            reason: "detected",
            label: String::new(),
        });
    }
    name_apns(&mut targets, &resolved);
    Ok((targets, candidates))
}

fn cmd_detect(paths: &Paths, save: bool) -> Result<(), String> {
    let cfg = Config::load(&paths.config)?;
    let list = detect::load_carrier_list(&paths.src)?;
    let mut items = Vec::new();
    let mut lines = Vec::new();
    for sim in detect::sims() {
        let name = detect::resolve(&list, &sim);
        if let Some(n) = &name {
            lines.push(format!("{n}\t{}", sim.spn));
        }
        items.push(format!(
            r#"{{"mccmnc":"{}","spn":"{}","canonical_name":{}}}"#,
            sim.mccmnc,
            sim.spn.replace('"', "'"),
            name.map(|n| format!("\"{n}\"")).unwrap_or("null".into())
        ));
    }
    println!(r#"{{"auto":{},"sims":[{}]}}"#, cfg.auto, items.join(","));

    // Persist the mapping for the next boot, when the modem will not be up in time.
    if save && !lines.is_empty() {
        if let Some(dir) = paths.sims.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        std::fs::write(&paths.sims, lines.join("\n") + "\n")
            .map_err(|e| format!("{}: {e}", paths.sims.display()))?;
    }
    Ok(())
}

fn cmd_patch(out: &Path, paths: &Paths) -> Result<(), String> {
    let cfg = Config::load(&paths.config)?;

    let src = effective_src(&paths.src, &paths.cache);
    if src != paths.src {
        println!("  /product is already shadowed by us, reading the cached stock instead");
    }
    let (targets, candidates) = targets(&cfg, &src, paths)?;
    if targets.is_empty() {
        println!("nothing to patch");
        return Ok(());
    }
    std::fs::create_dir_all(out).map_err(|e| format!("{}: {e}", out.display()))?;

    for t in &targets {
        let who = if t.label.is_empty() {
            t.carrier.canonical_name.clone()
        } else {
            format!("{} ({})", t.label, t.carrier.canonical_name)
        };
        println!("  {who} — {}", t.reason);
    }
    let carriers: Vec<Carrier> = targets.into_iter().map(|t| t.carrier).collect();

    let bytes = std::fs::read(src.join("others.pb")).map_err(|e| format!("others.pb: {e}"))?;
    let parsed = patch::parse_others(&bytes).map_err(|e| format!("others.pb: {e}"))?;
    let (patched_others, reports) =
        patch::patch_others(parsed, &carriers).map_err(|e| format!("others.pb: {e}"))?;
    std::fs::write(out.join("others.pb"), &patched_others)
        .map_err(|e| format!("others.pb: {e}"))?;

    let mut changed: usize = 0;
    for r in &reports {
        changed += r.keys_written + usize::from(r.apn.starts_with("added"));
        println!(
            "others.pb [{}]: {} keys, IMS APN: {}",
            r.canonical_name, r.keys_written, r.apn
        );
    }

    for carrier in &carriers {
        let path = src.join(format!("{}.pb", carrier.canonical_name));
        if !path.exists() {
            continue;
        }
        let bytes = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let parsed = patch::parse_single(&bytes).map_err(|e| format!("{}: {e}", path.display()))?;
        let (patched, report) =
            patch::patch_single(parsed, carrier).map_err(|e| format!("{}: {e}", path.display()))?;
        std::fs::write(out.join(path.file_name().unwrap()), &patched)
            .map_err(|e| format!("{}: {e}", path.display()))?;
        changed += report.keys_written + usize::from(report.apn.starts_with("added"));
        println!(
            "{}.pb: {} keys, IMS APN: {}",
            carrier.canonical_name, report.keys_written, report.apn
        );
    }

    // A run that changed nothing means the source already carried our patch — that is, we were
    // reading our own output, not the stock files. Caching that would poison the cache with
    // patched data masquerading as stock, so only refresh when the patch actually did something.
    if src == paths.src {
        if changed == 0 {
            println!("  source is already patched, keeping the existing stock cache");
        } else {
            refresh_cache(&src, &paths.cache, &candidates, &patched_others);
        }
    }
    Ok(())
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = match &cli.command {
        Command::Patch { out, paths } => cmd_patch(out, paths),
        Command::Detect { save, paths } => cmd_detect(paths, *save),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("imsforge: {e}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refreshing_a_cache_onto_itself_keeps_the_data() {
        // Pointing a run at its own cache is nonsense, but it must not cost the user their copy
        // of the stock: std::fs::copy truncates when source and destination are the same file.
        let dir = std::env::temp_dir().join(format!("imsforge-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("others.pb");
        std::fs::write(&file, b"stock bytes").unwrap();

        refresh_cache(&dir, &dir, &[], b"output");

        assert_eq!(std::fs::read(&file).unwrap(), b"stock bytes");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn the_fingerprint_tells_our_output_apart() {
        assert_eq!(fingerprint(b"a"), fingerprint(b"a"));
        assert_ne!(fingerprint(b"a"), fingerprint(b"b"));
    }
}
