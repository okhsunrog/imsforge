//! imsforge — on-device CarrierSettings patcher.
//!
//! Reads the stock protobufs straight off /product, works out which carriers need IMS enabled,
//! patches them and writes the result where the mount backend picks it up. Run from the
//! module's post-fs-data.sh, this re-derives the patch from whatever Google shipped on this
//! boot, so an OS update can never leave a stale snapshot behind.

mod config;
mod detect;
mod patch;

mod protos {
    include!(concat!(env!("OUT_DIR"), "/protos/mod.rs"));
}

use config::{Carrier, Config};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

const DEFAULT_SRC: &str = "/product/etc/CarrierSettings";

enum Cmd {
    Patch,
    Detect,
}

struct Args {
    cmd: Cmd,
    src: PathBuf,
    out: Option<PathBuf>,
    config: PathBuf,
    cache: Option<PathBuf>,
}

fn usage() -> ! {
    eprintln!(
        "usage: imsforge patch --out <dir> [--src <dir>] [--config <carriers.json>]\n\
                imsforge detect [--src <dir>]\n\
         \n\
         patch    patch the CarrierSettings protobufs into <dir>\n\
         detect   print, as JSON, what the inserted SIMs resolve to and whether they need us\n\
         \n\
         --src     stock CarrierSettings directory (default: {DEFAULT_SRC})\n\
         --config  carrier overrides (default: <module dir>/carriers.json)"
    );
    std::process::exit(2)
}

fn parse_args() -> Args {
    let mut it = std::env::args().skip(1);
    let cmd = match it.next().as_deref() {
        Some("patch") => Cmd::Patch,
        Some("detect") => Cmd::Detect,
        _ => usage(),
    };
    let mut src = PathBuf::from(DEFAULT_SRC);
    let mut out = None;
    let mut cache = None;
    // Config and cache live outside the module directory: a module update replaces that
    // directory wholesale, which would throw away the user's settings every time.
    let mut config = PathBuf::from("/data/adb/imsforge/carriers.json");

    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--src" => src = PathBuf::from(it.next().unwrap_or_else(|| usage())),
            "--out" => out = Some(PathBuf::from(it.next().unwrap_or_else(|| usage()))),
            "--config" => config = PathBuf::from(it.next().unwrap_or_else(|| usage())),
            "--cache" => cache = Some(PathBuf::from(it.next().unwrap_or_else(|| usage()))),
            _ => usage(),
        }
    }
    Args {
        cmd,
        src,
        out,
        config,
        cache,
    }
}

/// Cheap content fingerprint (FNV-1a). Only ever compared against one we wrote ourselves, so
/// it needs no cryptographic strength — and no dependency.
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
        if from.exists() {
            let _ = std::fs::copy(&from, cache.join(&name));
        }
    }
    let _ = std::fs::write(cache.join("output.fingerprint"), fingerprint(output).to_string());
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
        if let Some((_, spn)) = sims.iter().find(|(name, spn)| {
            *name == t.carrier.canonical_name && !spn.is_empty()
        }) {
            t.carrier.ims_apn_name = format!("{spn} IMS");
        }
    }
}

/// Decide what to patch: explicit entries always, auto-detected ones only where Google has not
/// already enabled VoLTE.
///
/// `data_src` is where the carrier settings come from (possibly our cached stock), while
/// `list_src` is always the live /product: carrier_list.pb is never something we patch, so it
/// is never shadowed and the cache has no reason to hold a copy.
fn targets(cfg: &Config, data_src: &Path, list_src: &Path) -> Result<Vec<Target>, String> {
    // canonical name -> SPN, for both auto-detection and APN labelling
    let mut resolved: Vec<(String, String)> = Vec::new();
    if let Ok(list) = detect::load_carrier_list(list_src) {
        for sim in detect::sims() {
            if let Some(name) = detect::resolve(&list, &sim) {
                resolved.push((name, sim.spn));
            }
        }
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

    if !cfg.auto {
        name_apns(&mut targets, &resolved);
        return Ok(targets);
    }

    // Auto-detection is a convenience: if the carrier list is unreadable we say so and still
    // honour whatever the config asked for, rather than failing the whole run.
    if resolved.is_empty() {
        eprintln!("  auto-detection found nothing (no SIM, or carrier_list.pb unreadable)");
        name_apns(&mut targets, &resolved);
        return Ok(targets);
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
    Ok(targets)
}

fn cmd_detect(args: &Args) -> Result<(), String> {
    let cfg = Config::load(&args.config)?;
    let list = detect::load_carrier_list(&args.src)?;
    let mut items = Vec::new();
    for sim in detect::sims() {
        let name = detect::resolve(&list, &sim);
        items.push(format!(
            r#"{{"mccmnc":"{}","spn":"{}","canonical_name":{}}}"#,
            sim.mccmnc,
            sim.spn.replace('"', "'"),
            name.map(|n| format!("\"{n}\"")).unwrap_or("null".into())
        ));
    }
    println!(r#"{{"auto":{},"sims":[{}]}}"#, cfg.auto, items.join(","));
    Ok(())
}

fn cmd_patch(args: &Args) -> Result<(), String> {
    let out = args.out.as_ref().ok_or("patch needs --out")?;
    let cfg = Config::load(&args.config)?;

    let cache = args
        .cache
        .clone()
        .unwrap_or_else(|| PathBuf::from("/data/adb/imsforge/stock"));
    let src = effective_src(&args.src, &cache);
    if src != args.src {
        println!("  /product is already shadowed by us, reading the cached stock instead");
    }
    let targets = targets(&cfg, &src, &args.src)?;
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

    let others_path = src.join("others.pb");
    let bytes = std::fs::read(&others_path).map_err(|e| format!("others.pb: {e}"))?;
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
    if src == args.src {
        if changed == 0 {
            println!("  source is already patched, keeping the existing stock cache");
        } else {
            let names: Vec<String> = carriers.iter().map(|c| c.canonical_name.clone()).collect();
            refresh_cache(&src, &cache, &names, &patched_others);
        }
    }
    Ok(())
}

fn main() -> ExitCode {
    let args = parse_args();
    let result = match args.cmd {
        Cmd::Patch => cmd_patch(&args),
        Cmd::Detect => cmd_detect(&args),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("imsforge: {e}");
            ExitCode::FAILURE
        }
    }
}
