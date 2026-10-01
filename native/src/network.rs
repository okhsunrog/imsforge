//! Live USER network restrictions. CarrierSettings and IMS preferences are untouched.
use crate::atomic;
use clap::{Args as ClapArgs, ValueEnum};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, fs, path::PathBuf, process::Command};

const LTE: u64 = (1 << 12) | (1 << 18);
const NR: u64 = 1 << 19;

#[derive(Clone, Copy, ValueEnum)]
pub enum Action {
    Status,
    Lte,
    LteNr,
    Restore,
}

#[derive(ClapArgs)]
pub struct Args {
    #[arg(value_enum)]
    action: Action,
    #[arg(long)]
    slot: Option<u32>,
    /// Reject an action if the SIM changed since the WebUI was refreshed
    #[arg(long)]
    sub_id: Option<u32>,
    #[arg(long, default_value = "/data/adb/imsforge/network")]
    state_dir: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
struct Sim {
    slot: u32,
    sub_id: u32,
    mcc: String,
    mnc: String,
    mask: u64,
    // Older recovery records predate usage changes; migrate them before changing usage.
    #[serde(default)]
    usage_setting: Option<u8>,
}

fn field<'a>(line: &'a str, key: &str) -> Result<&'a str, String> {
    line.split_whitespace()
        .find_map(|s| s.strip_prefix(key))
        .filter(|s| !s.is_empty())
        .ok_or_else(|| format!("Missing subscription field {key}"))
}

fn inventory(text: &str) -> Result<Vec<Sim>, String> {
    let mut sims = BTreeMap::new();
    // Inactive profiles and logs can retain old slot IDs. Read active subscriptions only.
    let mut active = false;
    for line in text.lines() {
        if line.trim() == "Active subscriptions:" {
            active = true;
            continue;
        }
        if active && line.trim() == "All subscriptions:" {
            break;
        }
        if !active {
            continue;
        }
        let Some((_, line)) = line.split_once("[SubscriptionInfoInternal: ") else {
            continue;
        };
        let slot: i32 = field(line, "simSlotIndex=")?
            .parse()
            .map_err(|_| "Invalid slot")?;
        if slot < 0 {
            continue;
        }
        let sub_id = field(line, "id=")?
            .parse()
            .map_err(|_| "Invalid subscription ID")?;
        let mask = field(line, "allowedNetworkTypesForReasons=")?
            .split(',')
            .find_map(|s| s.strip_prefix("user="))
            .ok_or("USER network mask unavailable")?
            .parse()
            .map_err(|_| "Invalid USER network mask")?;
        let sim = Sim {
            slot: slot as u32,
            sub_id,
            mcc: field(line, "mcc=")?.into(),
            mnc: field(line, "mnc=")?.into(),
            mask,
            usage_setting: Some(match field(line, "usageSetting=")? {
                "DEFAULT" => 0,
                "VOICE_CENTRIC" => 1,
                "DATA_CENTRIC" => 2,
                _ => return Err("Subscription usage setting unavailable".into()),
            }),
        };
        if let Some(previous) = sims.insert(sim.slot, sim.clone())
            && previous != sim
        {
            return Err("Subscription inventory is inconsistent; refresh and retry".into());
        }
    }
    if sims.is_empty() {
        return Err("No active subscription with a readable USER network mask".into());
    }
    Ok(sims.into_values().collect())
}

fn command(program: &str, args: &[&str]) -> Result<String, String> {
    let output = Command::new("timeout")
        .arg("12")
        .arg(program)
        .args(args)
        .output()
        .map_err(|e| e.to_string())?;
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    if !output.status.success()
        || stdout.contains("Permission denied")
        || stdout.starts_with("Error")
    {
        return Err(format!(
            "{program} failed ({}): {stdout} {stderr}",
            output.status
        ));
    }
    Ok(stdout)
}

fn read_sims() -> Result<Vec<Sim>, String> {
    inventory(&command("dumpsys", &["isub"])?)
}

fn set_usage(sub_id: u32, usage: u8) -> Result<(), String> {
    let helper = std::env::current_exe()
        .map_err(|e| e.to_string())?
        .with_file_name("usage-setting.dex");
    if !helper.is_file() {
        return Err("Usage-setting helper missing; install the complete module ZIP".into());
    }
    let output = Command::new("timeout")
        .arg("12")
        .arg("app_process")
        .env("CLASSPATH", helper)
        .args([
            "/system/bin",
            "UsageSetting",
            &sub_id.to_string(),
            &usage.to_string(),
        ])
        .output()
        .map_err(|e| e.to_string())?;
    if !output.status.success() {
        return Err(format!(
            "Usage setting failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(())
}
fn same_sim(a: &Sim, b: &Sim) -> bool {
    a.sub_id == b.sub_id && a.mcc == b.mcc && a.mnc == b.mnc
}
fn equivalent(a: u64, b: u64) -> bool {
    // Android may represent LTE_CA as LTE when storing the USER mask.
    fn normalize(mask: u64) -> u64 {
        let mask = if mask & (1 << 18) != 0 {
            mask | (1 << 12)
        } else {
            mask
        };
        mask & !(1 << 18)
    }
    normalize(a) == normalize(b)
}
fn mode(mask: u64) -> &'static str {
    if equivalent(mask, LTE) {
        "lte"
    } else if equivalent(mask, LTE | NR) {
        "lte-nr"
    } else {
        "custom"
    }
}

fn saved(path: &std::path::Path) -> Result<Option<Sim>, String> {
    match fs::read(path) {
        Ok(data) => serde_json::from_slice(&data)
            .map(Some)
            .map_err(|e| format!("Invalid recovery file: {e}")),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.to_string()),
    }
}

pub fn run(args: &Args) -> Result<(), String> {
    fs::create_dir_all(&args.state_dir).map_err(|e| e.to_string())?;
    let _lock = atomic::lock(&args.state_dir.join("lock"))
        .map_err(|e| format!("Network action is busy: {e}"))?;
    let sims = read_sims()?;
    if matches!(args.action, Action::Status) {
        let mut rows = Vec::new();
        for sim in sims {
            let backup = saved(&args.state_dir.join(format!("{}.json", sim.sub_id)))?;
            rows.push(
                serde_json::json!({"slot":sim.slot,"sub_id":sim.sub_id,"mask":sim.mask,
                "mode":mode(sim.mask),"usage_setting":sim.usage_setting,
                "can_restore":backup.as_ref().is_some_and(|b| same_sim(&sim,b))}),
            );
        }
        println!("{}", serde_json::json!({"sims":rows}));
        return Ok(());
    }
    let slot = args.slot.ok_or("--slot is required")?;
    let sub_id = args.sub_id.ok_or("--sub-id is required")?;
    let sim = sims
        .iter()
        .find(|s| s.slot == slot && s.sub_id == sub_id)
        .ok_or("SIM changed or is unavailable; refresh before changing networks")?;
    let path = args.state_dir.join(format!("{sub_id}.json"));
    let mut backup = saved(&path)?;
    if backup.as_ref().is_some_and(|b| !same_sim(sim, b)) {
        return Err("Saved recovery belongs to a different SIM".into());
    }
    if let Some(original) = backup.as_mut()
        && original.usage_setting.is_none()
    {
        original.usage_setting = sim.usage_setting;
        atomic::write(
            &path,
            serde_json::to_vec(original).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
    }
    let usage = if matches!(args.action, Action::Restore) {
        backup
            .as_ref()
            .and_then(|s| s.usage_setting)
            .ok_or("Original usage setting unavailable")?
    } else {
        2
    };
    if usage > 2 {
        return Err("Invalid saved usage setting".into());
    }
    let mask = match args.action {
        Action::Lte => LTE,
        Action::LteNr => LTE | NR,
        Action::Restore => {
            backup
                .as_ref()
                .ok_or("No saved network mode for this SIM")?
                .mask
        }
        Action::Status => unreachable!(),
    };
    if backup.is_none() {
        atomic::write(&path, serde_json::to_vec(sim).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
    }
    // Recheck identity after persisting recovery, immediately before writing to a slot.
    let current = read_sims()?;
    if !current.iter().any(|s| s.slot == slot && same_sim(s, sim)) {
        return Err("SIM changed before applying; refresh and retry".into());
    }
    if sim.usage_setting != Some(usage) {
        set_usage(sub_id, usage)?;
    }
    let current = read_sims()?;
    if !current.iter().any(|s| s.slot == slot && same_sim(s, sim)) {
        return Err("SIM changed while setting data priority; recovery retained".into());
    }
    command(
        "cmd",
        &[
            "phone",
            "set-allowed-network-types-for-users",
            "-s",
            &slot.to_string(),
            &format!("{mask:b}"),
        ],
    )?;
    let current = read_sims()?;
    let actual = current
        .iter()
        .find(|s| s.slot == slot && same_sim(s, sim))
        .ok_or("SIM unavailable after change; recovery retained")?;
    if !equivalent(actual.mask, mask) || actual.usage_setting != Some(usage) {
        return Err(format!(
            "Network mode or usage not confirmed (USER mask {}); recovery retained",
            actual.mask
        ));
    }
    if matches!(args.action, Action::Restore) {
        fs::remove_file(path).map_err(|e| e.to_string())?;
    }
    println!(
        "{}",
        serde_json::json!({"slot":slot,"sub_id":sub_id,"mask":actual.mask,"mode":mode(actual.mask),"usage_setting":actual.usage_setting})
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    const LINE: &str = "Active subscriptions:\n[SubscriptionInfoInternal: id=1 simSlotIndex=1 mcc=250 mnc=01 usageSetting=DEFAULT allowedNetworkTypesForReasons=user=64511,power=654335]";
    #[test]
    fn exact_user_mask_not_carrier_or_history() {
        let text = format!(
            "{LINE}\n{LINE}\nAll subscriptions:\n{}",
            LINE.replace("64511", "4096")
        );
        let sims = inventory(&text).unwrap();
        assert_eq!(sims.len(), 1);
        assert_eq!(sims[0].mask, 64511);
        assert_eq!(sims[0].usage_setting, Some(0));
        assert_eq!(sims[0].slot, 1);
        assert!(inventory(&LINE.replace("user=", "carrier=")).is_err());
    }
    #[test]
    fn recovery_follows_subscription_not_slot() {
        let a = inventory(LINE).unwrap().remove(0);
        let mut b = a.clone();
        b.slot = 0;
        assert!(same_sim(&a, &b));
        b.sub_id = 2;
        assert!(!same_sim(&a, &b));
        assert!(equivalent(LTE, 1 << 12));
        assert!(!equivalent(LTE, LTE | NR));
        assert!(!equivalent(LTE, LTE | 1));
    }
    #[test]
    fn damaged_recovery_is_not_silently_replaced() {
        let dir = crate::testing::tempdir("network-recovery");
        let path = dir.join("1.json");
        assert!(saved(&path).unwrap().is_none());
        fs::write(&path, "{").unwrap();
        assert!(saved(&path).is_err());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn usage_priority_is_saved_exactly_and_legacy_records_remain_readable() {
        for (name, value) in [("DEFAULT", 0), ("VOICE_CENTRIC", 1), ("DATA_CENTRIC", 2)] {
            let sim = inventory(&LINE.replace("DEFAULT", name)).unwrap().remove(0);
            let saved: Sim = serde_json::from_str(&serde_json::to_string(&sim).unwrap()).unwrap();
            assert_eq!(saved.usage_setting, Some(value));
        }
        let mut json = serde_json::to_value(inventory(LINE).unwrap().remove(0)).unwrap();
        json.as_object_mut().unwrap().remove("usage_setting");
        assert_eq!(
            serde_json::from_value::<Sim>(json).unwrap().usage_setting,
            None
        );
        assert!(inventory(&LINE.replace("DEFAULT", "UNKNOWN")).is_err());
    }
}
