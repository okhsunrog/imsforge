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
    pair_sims(
        &getprop("gsm.sim.operator.numeric"),
        &getprop("gsm.sim.operator.alpha"),
    )
}

/// Pair the two SIM properties into slots.
pub fn pair_sims(numerics_raw: &str, names_raw: &str) -> Vec<Sim> {
    let numerics: Vec<&str> = numerics_raw.split(',').collect();
    let names: Vec<&str> = names_raw.split(',').collect();
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
            spn: if aligned {
                names[i].trim().to_string()
            } else {
                String::new()
            },
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
        let Some(start) = rest.find('>') else {
            continue;
        };
        let Some(end) = rest[start..].find('<') else {
            continue;
        };
        let value = &rest[start + 1..start + end];
        if let Some(canonical) = canonical_from_version(value)
            && !out.iter().any(|c| c == canonical)
        {
            out.push(canonical.to_string());
        }
    }
    out
}

/// Pull the carrier's canonical name out of a carrier_config_version_string.
///
/// The value is "<canonical>-<version>", the version may carry a trailing date
/// ("...&#10;2025-11-12"), and a canonical name may itself contain dashes — so neither the first
/// nor the last dash is the right place to cut. The version always starts with a digit.
pub fn canonical_from_version(value: &str) -> Option<&str> {
    let cut = value
        .char_indices()
        .find(|(i, c)| *c == '-' && value[i + 1..].starts_with(|n: char| n.is_ascii_digit()))
        .map(|(i, _)| i)?;
    let canonical = &value[..cut];
    (!canonical.is_empty()).then_some(canonical)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_stops_before_the_version() {
        // A real value: the version carries a date, whose dash must not be mistaken for the cut.
        assert_eq!(
            canonical_from_version("tinkoff_ru-77000000001.21&#10;2025-11-12"),
            Some("tinkoff_ru")
        );
        // An unnamed carrier's entry is called after its MCCMNC, digits and all.
        assert_eq!(
            canonical_from_version("25001-77000000119.21"),
            Some("25001")
        );
        // A canonical name may contain dashes of its own.
        assert_eq!(
            canonical_from_version("t-mobile_us-123.4"),
            Some("t-mobile_us")
        );
        assert_eq!(canonical_from_version(""), None);
        assert_eq!(canonical_from_version("no-version-here"), None);
    }

    #[test]
    fn sims_pair_by_slot() {
        let sims = pair_sims("25062,25001", "T-Mobile,МТС");
        assert_eq!(sims.len(), 2);
        assert_eq!(sims[0].mccmnc, "25062");
        assert_eq!(sims[0].spn, "T-Mobile");
        assert_eq!(sims[1].spn, "МТС");
    }

    #[test]
    fn misaligned_properties_yield_no_names() {
        // The modem fills the two properties independently, so mid-boot they can disagree in
        // length. Pairing them positionally then attaches one carrier's name to another's
        // MCCMNC — the SIMs are still reported, but without names.
        let sims = pair_sims("25001", "T-Mobile,МТС");
        assert_eq!(sims.len(), 1);
        assert_eq!(sims[0].mccmnc, "25001");
        assert_eq!(sims[0].spn, "");
    }

    #[test]
    fn no_sims_at_all() {
        assert!(pair_sims("", "").is_empty());
    }
}
