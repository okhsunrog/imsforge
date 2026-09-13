//! Working out which carriers the inserted SIMs belong to.
//!
//! Android resolves a SIM to a CarrierSettings entry through carrier_list.pb: by MCCMNC, and
//! for MVNOs additionally by SPN, IMSI prefix or GID1. We replicate the MCCMNC and SPN halves,
//! which covers the MVNOs that ship their own settings file.

use crate::protos::carrier_list::{CarrierList, carrier_id::Mvno_data};
use protobuf::Message;
use std::path::Path;

#[derive(Debug)]
pub struct Sim {
    pub mccmnc: String,
    pub spn: String,
}

fn getprop(name: &str) -> String {
    std::process::Command::new("getprop")
        .arg(name)
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .unwrap_or_default()
        .trim()
        .to_string()
}

/// SIMs currently in the phone. Both props are comma separated, one field per slot.
pub fn sims() -> Vec<Sim> {
    let numerics: Vec<&str> = {
        let s = Box::leak(getprop("gsm.sim.operator.numeric").into_boxed_str());
        s.split(',').collect()
    };
    let names: Vec<&str> = {
        let s = Box::leak(getprop("gsm.sim.operator.alpha").into_boxed_str());
        s.split(',').collect()
    };
    // The two properties are filled in independently while the modem comes up, so mid-boot they
    // can disagree in length. Pairing them positionally then attaches one carrier's name to
    // another's MCCMNC, so drop the names entirely rather than report something false.
    let aligned = numerics.len() == names.len();
    numerics
        .iter()
        .enumerate()
        .filter(|(_, n)| !n.is_empty())
        .map(|(i, mccmnc)| Sim {
            mccmnc: mccmnc.to_string(),
            spn: if aligned { names[i].trim().to_string() } else { String::new() },
        })
        .collect()
}

/// Canonical CarrierSettings name for a SIM, preferring an MVNO entry whose SPN matches.
pub fn resolve(list: &CarrierList, sim: &Sim) -> Option<String> {
    let mut generic: Option<String> = None;

    for entry in &list.entry {
        for id in &entry.carrier_id {
            if id.mcc_mnc() != sim.mccmnc {
                continue;
            }
            match &id.mvno_data {
                Some(Mvno_data::Spn(spn)) => {
                    if !sim.spn.is_empty() && spn.eq_ignore_ascii_case(&sim.spn) {
                        return Some(entry.canonical_name().to_string());
                    }
                }
                // IMSI and GID1 need data a shell tool cannot read; those MVNOs fall back to
                // the generic entry, which is the same thing Android does when nothing matches.
                Some(_) => {}
                None => {
                    if generic.is_none() {
                        generic = Some(entry.canonical_name().to_string());
                    }
                }
            }
        }
    }
    generic
}

/// Canonical names taken from telephony's own carrier config cache.
///
/// At post-fs-data the modem is not up, so the SIM properties are empty and live detection is
/// impossible. Telephony, however, leaves a cache file per SIM from the previous boot, and its
/// `carrier_config_version_string` starts with exactly the canonical name we need — a record the
/// system maintains for us. We read it before the cache is cleared.
pub fn from_config_cache(dir: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.starts_with("carrierconfig-") || !name.ends_with(".xml") || name.contains("nosim")
        {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(entry.path()) else {
            continue;
        };
        let Some(i) = text.find("carrier_config_version_string") else {
            continue;
        };
        // <string name="carrier_config_version_string">tinkoff_ru-77000000001.21&#10;…</string>
        let rest = &text[i..];
        let Some(start) = rest.find('>') else { continue };
        let Some(end) = rest[start..].find('<') else { continue };
        let value = &rest[start + 1..start + end];
        // Cut at the first dash followed by a digit: the value is "<canonical>-<version>", the
        // version may carry a trailing date ("...&#10;2025-11-12"), and a canonical name may
        // itself contain dashes — so neither the first nor the last dash is the right one.
        let cut = value
            .char_indices()
            .find(|(i, c)| *c == '-' && value[i + 1..].starts_with(|n: char| n.is_ascii_digit()))
            .map(|(i, _)| i);
        if let Some(cut) = cut {
            let canonical = &value[..cut];
            if !canonical.is_empty() && !out.iter().any(|c| c == canonical) {
                out.push(canonical.to_string());
            }
        }
    }
    out
}

/// Canonical names saved by service.sh once telephony was up.
pub fn from_saved(path: &Path) -> Vec<String> {
    std::fs::read_to_string(path)
        .map(|t| {
            t.lines()
                .map(|l| l.trim().to_string())
                .filter(|l| !l.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

pub fn load_carrier_list(dir: &Path) -> Result<CarrierList, String> {
    let path = dir.join("carrier_list.pb");
    let bytes = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    CarrierList::parse_from_bytes(&bytes).map_err(|e| format!("{}: {e}", path.display()))
}
