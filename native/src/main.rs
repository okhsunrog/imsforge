//! imsforge — on-device CarrierSettings patcher.
//!
//! Reads the stock protobufs straight off /product, works out which carriers need IMS enabled,
//! patches them and writes the result where the mount backend picks it up. Run from the module's
//! post-fs-data.sh, this re-derives the patch from whatever Google shipped on this boot, so an OS
//! update can never leave a stale snapshot behind.

mod atomic;
mod cache;
mod config;
mod detect;
mod patch;
mod plan;
mod status;
#[cfg(test)]
mod testing;

mod protos {
    include!(concat!(env!("OUT_DIR"), "/protos/mod.rs"));
}

use clap::{Args, Parser, Subcommand};
use config::{Carrier, Config};
use detect::Resolved;
use plan::Stock;
use status::{Source, Status};
use std::fmt::Display;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

// The defaults, in one place: three subcommands name the same carriers.json, and a copy that
// drifts would send one of them to a file nobody else writes.
const STOCK_DIR: &str = "/product/etc/CarrierSettings";
const STOCK_CACHE: &str = "/data/adb/imsforge/stock";
const CONFIG: &str = "/data/adb/imsforge/carriers.json";
const SIMS: &str = "/data/adb/imsforge/sims";
const STATUS: &str = "/data/adb/imsforge/status.json";
const PHONE_FILES: &str = "/data/user_de/0/com.android.phone/files";

#[derive(Parser)]
#[command(name = "imsforge", version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Patch the CarrierSettings protobufs into a directory
    Patch(PatchArgs),
    /// Print, as JSON, what the inserted SIMs resolve to
    Detect(DetectArgs),
    /// Parse a carriers.json and report what it holds, changing nothing
    Check(CheckArgs),
    /// Print, as JSON, what the last patch decided and whose files are at /product now
    Status(StatusArgs),
}

/// Where the CarrierSettings come from: the live directory, and our own copy of what Google
/// shipped there — which is what a run has to fall back on once it has shadowed the original.
#[derive(Args)]
struct Sources {
    /// Stock CarrierSettings directory
    #[arg(long, value_name = "DIR", default_value = STOCK_DIR)]
    src: PathBuf,

    /// Our copy of the stock protobufs, for runs that can no longer see them
    #[arg(long, value_name = "DIR", default_value = STOCK_CACHE)]
    stock_cache: PathBuf,
}

#[derive(Args)]
struct PatchArgs {
    /// Where to write the patched protobufs
    #[arg(long, value_name = "DIR")]
    out: PathBuf,

    #[command(flatten)]
    sources: Sources,

    /// Carrier overrides
    #[arg(long, value_name = "FILE", default_value = CONFIG)]
    config: PathBuf,

    /// Carriers remembered by the previous boot
    #[arg(long, value_name = "FILE", default_value = SIMS)]
    sims: PathBuf,

    /// Where to record what this run decided, for the WebUI to read
    #[arg(long, value_name = "FILE", default_value = STATUS)]
    status: PathBuf,

    /// Telephony's config cache, read to identify carriers when the modem is down
    #[arg(long, value_name = "DIR", default_value = PHONE_FILES)]
    phone_files: PathBuf,
}

#[derive(Args)]
struct DetectArgs {
    /// Remember the carriers for the next boot, when the modem will not be up in time
    #[arg(long)]
    save: bool,

    /// Wait up to this long for the modem to finish reporting the SIMs
    #[arg(long, value_name = "SECONDS", default_value_t = 0)]
    wait: u64,

    /// Stock CarrierSettings directory, read for carrier_list.pb
    #[arg(long, value_name = "DIR", default_value = STOCK_DIR)]
    src: PathBuf,

    /// Carrier overrides, read for the auto flag the WebUI shows
    #[arg(long, value_name = "FILE", default_value = CONFIG)]
    config: PathBuf,

    /// Where --save writes what it found
    #[arg(long, value_name = "FILE", default_value = SIMS)]
    sims: PathBuf,
}

#[derive(Args)]
struct CheckArgs {
    /// The file to parse
    #[arg(long, value_name = "FILE", default_value = CONFIG)]
    config: PathBuf,
}

#[derive(Args)]
struct StatusArgs {
    #[command(flatten)]
    sources: Sources,

    /// The record the last patch left
    #[arg(long, value_name = "FILE", default_value = STATUS)]
    status: PathBuf,
}

/// Attach the path to an error. The module's log is the only thing the user ever sees, and a
/// bare "No such file or directory" there names nothing.
pub fn at<E: Display>(path: &Path) -> impl FnOnce(E) -> String + '_ {
    move |e| format!("{}: {e}", path.display())
}

fn cmd_detect(args: &DetectArgs) -> Result<(), String> {
    let cfg = Config::load(&args.config)?;
    let list = detect::load_carrier_list(&args.src)?;
    let sims = match args.wait {
        0 => detect::sims(),
        seconds => detect::settled_sims(Duration::from_secs(seconds)),
    };

    let mut items = Vec::new();
    let mut found: Vec<Resolved> = Vec::new();
    for sim in sims {
        let name = detect::resolve(&list, &sim);
        if let Some(name) = &name {
            found.push(Resolved::new(name.clone(), sim.spn.clone()));
        }
        items.push(serde_json::json!({
            "mccmnc": sim.mccmnc,
            "spn": sim.spn,
            "canonical_name": name,
        }));
    }
    // Serialised properly rather than by hand: an operator name carrying a quote or a backslash
    // would otherwise produce JSON the WebUI cannot parse, and it would show "no SIM detected".
    println!("{}", serde_json::json!({ "auto": cfg.auto, "sims": items }));

    // Persist the mapping for the next boot, when the modem will not be up in time.
    if args.save && !found.is_empty() {
        detect::save(&args.sims, &found).map_err(at(&args.sims))?;
        // On stderr, where it cannot disturb the JSON: the installer shows these lines to the
        // user, and reading them back out of the saved file would be a third place that has to
        // know how that file is written.
        for carrier in &found {
            eprintln!("{carrier}");
        }
    }
    Ok(())
}

/// Parse a configuration and say what it holds.
///
/// The WebUI writes carriers.json from a free-form editor, and `deny_unknown_fields` means one
/// misspelled key stops the next boot from patching anything at all. Far better to find that out
/// here, while the file that works is still in place, than from a log line after a reboot.
fn cmd_check(args: &CheckArgs) -> Result<(), String> {
    let text = fs::read_to_string(&args.config).map_err(at(&args.config))?;
    let cfg = Config::parse(&text).map_err(at(&args.config))?;
    println!(
        "{}",
        serde_json::json!({
            "auto": cfg.auto,
            "carriers": cfg.carriers.len(),
            "skip": cfg.skip.len(),
        })
    );
    Ok(())
}

/// What the last patch decided, plus whose CarrierSettings is at /product now.
///
/// The record is passed through as it was written rather than parsed: a WebUI from another
/// release should see the fields it knows, and this has no reason to understand them.
fn cmd_status(args: &StatusArgs) -> Result<(), String> {
    let run = fs::read_to_string(&args.status)
        .ok()
        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
        .unwrap_or(serde_json::Value::Null);
    println!(
        "{}",
        serde_json::json!({
            "product": cache::product(&args.sources.src, &args.sources.stock_cache),
            "run": run,
        })
    );
    Ok(())
}

fn cmd_patch(args: &PatchArgs) -> Result<(), String> {
    let out = &args.out;
    let cfg = Config::load(&args.config)?;

    let src = cache::effective_src(&args.sources.src, &args.sources.stock_cache);
    let source = if src == args.sources.src {
        Source::Stock
    } else {
        println!("  /product is already shadowed by us, reading the cached stock instead");
        Source::Cache
    };

    // Parsed once and used by both halves: deciding needs it to tell a carrier Google supports
    // from one it forgot, and patching rewrites it.
    let others_path = src.join("others.pb");
    let others_bytes = fs::read(&others_path).map_err(at(&others_path))?;
    let others = patch::parse_others(&others_bytes).map_err(at(&others_path))?;

    // Which carriers are in this phone is a question about the phone, not about the patch, so
    // it is answered here and handed over.
    let (present, from) = detect::identify(&args.sources.src, &args.sims, &args.phone_files);
    if present.is_empty() {
        eprintln!("  no carriers identified — nothing to detect automatically");
    } else {
        println!("  carriers in this phone, from {from}");
    }

    let plan = plan::plan(&cfg, &Stock::new(&src, &others), &present)?;
    if plan.targets.is_empty() {
        println!("nothing to patch");
        // Still recorded: without it the WebUI would go on showing what some earlier boot
        // patched, while this boot left /product as Google shipped it.
        write_status(&args.status, Status::new(source, plan.left_alone));
        return Ok(());
    }
    fs::create_dir_all(out).map_err(at(out))?;
    for target in &plan.targets {
        println!("  {target}");
    }
    let carriers: Vec<Carrier> = plan.targets.iter().map(|t| t.carrier.clone()).collect();

    let mut changed: usize = 0;
    let (patched_others, reports) =
        patch::patch_others(others, &carriers).map_err(at(&others_path))?;
    let mut reported: Vec<patch::Report> = Vec::new();
    let dest = out.join("others.pb");
    fs::write(&dest, &patched_others).map_err(at(&dest))?;
    for report in &reports {
        changed += report.changes();
        println!(
            "others.pb [{}]: {} keys, IMS APN: {}",
            report.canonical_name, report.keys_written, report.apn
        );
    }
    reported.extend(reports);

    for carrier in &carriers {
        let name = format!("{}.pb", carrier.canonical_name);
        let path = src.join(&name);
        if !path.exists() {
            continue;
        }
        let bytes = fs::read(&path).map_err(at(&path))?;
        let parsed = patch::parse_single(&bytes).map_err(at(&path))?;
        let (patched, report) = patch::patch_single(parsed, carrier).map_err(at(&path))?;
        let dest = out.join(&name);
        fs::write(&dest, &patched).map_err(at(&dest))?;
        changed += report.changes();
        println!(
            "{name}: {} keys, IMS APN: {}",
            report.keys_written, report.apn
        );
        reported.push(report);
    }

    write_status(
        &args.status,
        Status::new(source, plan::record(&plan, &reported)),
    );

    // A run that changed nothing means the source already carried our patch — that is, we were
    // reading our own output, not the stock files. Caching that would poison the cache with
    // patched data masquerading as stock, so only refresh when the patch actually did something.
    if src == args.sources.src {
        if changed == 0 {
            println!("  source is already patched, keeping the existing stock cache");
        } else {
            cache::refresh(
                &src,
                &args.sources.stock_cache,
                &plan.candidates,
                &patched_others,
            );
        }
    }
    Ok(())
}

/// A record we could not write is worth a line in the log and nothing more: the patch itself
/// succeeded, and failing the run over it would keep the phone on the stock config.
fn write_status(path: &Path, status: Status) {
    if let Err(e) = status.write(path) {
        eprintln!("  status: {e}");
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = match &cli.command {
        Command::Patch(args) => cmd_patch(args),
        Command::Detect(args) => cmd_detect(args),
        Command::Check(args) => cmd_check(args),
        Command::Status(args) => cmd_status(args),
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
    use crate::patch::stock_settings;
    use crate::protos::carrier_settings::MultiCarrierSettings;
    use crate::testing::tempdir;

    /// A patch run over a scratch directory, with every path pointing into it.
    fn args_in(dir: &Path) -> PatchArgs {
        PatchArgs {
            out: dir.join("out"),
            sources: Sources {
                src: dir.join("product"),
                stock_cache: dir.join("stock"),
            },
            config: dir.join("carriers.json"),
            sims: dir.join("sims"),
            status: dir.join("status.json"),
            phone_files: dir.join("phone"),
        }
    }

    /// Google's own file, on disk, as a run would find it. Returns the bytes it wrote.
    fn write_stock(dir: &Path, carriers: &[(&str, bool)]) -> Vec<u8> {
        let mut multi = MultiCarrierSettings::new();
        for (name, volte) in carriers {
            multi.setting.push(stock_settings(name, *volte));
        }
        let bytes = protobuf::Message::write_to_bytes(&multi).unwrap();
        fs::create_dir_all(dir).unwrap();
        fs::write(dir.join("others.pb"), &bytes).unwrap();
        bytes
    }

    #[test]
    fn the_cached_stock_survives_a_run_that_reads_our_own_output() {
        let dir = tempdir("run");
        let args = args_in(&dir);
        let stock_bytes = write_stock(&args.sources.src, &[("25001", false)]);
        fs::write(&args.sims, "25001\tМТС\n").unwrap();

        cmd_patch(&args).unwrap();
        let patched = fs::read(args.out.join("others.pb")).unwrap();
        assert_ne!(patched, stock_bytes, "the run must have changed something");
        assert_eq!(
            fs::read(args.sources.stock_cache.join("others.pb")).unwrap(),
            stock_bytes,
            "the stock file is kept for later runs"
        );

        // The mount backend now lays that output over /product, so a run by hand sees our own
        // work where Google's files used to be.
        fs::write(args.sources.src.join("others.pb"), &patched).unwrap();
        let again = PatchArgs {
            out: dir.join("out-again"),
            ..args_in(&dir)
        };
        cmd_patch(&again).unwrap();

        assert_eq!(
            fs::read(again.out.join("others.pb")).unwrap(),
            patched,
            "reading the cached stock must reproduce the same patch, not build on it"
        );
        assert_eq!(
            fs::read(args.sources.stock_cache.join("others.pb")).unwrap(),
            stock_bytes,
            "the cache must still hold Google's file"
        );
    }

    #[test]
    fn a_source_that_already_carries_the_patch_is_never_cached_as_stock() {
        let dir = tempdir("poison");
        let args = args_in(&dir);
        let stock_bytes = write_stock(&args.sources.src, &[("25001", false)]);
        // An explicit entry outranks the certification guard, so this run has a target even
        // when the source already has every key set — which is what makes it reach the guard.
        fs::write(&args.config, r#"{"carriers":[{"canonical_name":"25001"}]}"#).unwrap();

        cmd_patch(&args).unwrap();
        let patched = fs::read(args.out.join("others.pb")).unwrap();

        // A run that cannot recognise its own work: the fingerprint is gone and /product carries
        // the patch already. Copying that into the cache would leave patched data masquerading
        // as Google's, and every later run would judge carriers by it.
        fs::remove_file(args.sources.stock_cache.join("output.fingerprint")).unwrap();
        fs::write(args.sources.src.join("others.pb"), &patched).unwrap();
        cmd_patch(&PatchArgs {
            out: dir.join("out-again"),
            ..args_in(&dir)
        })
        .unwrap();

        assert_eq!(
            fs::read(args.sources.stock_cache.join("others.pb")).unwrap(),
            stock_bytes,
            "the cache must still hold Google's file"
        );
    }
}
