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
    let numerics = getprop("gsm.sim.operator.numeric");
    let names = getprop("gsm.sim.operator.alpha");
    let mut spns = names.split(',');
    numerics
        .split(',')
        .filter(|n| !n.is_empty())
        .map(|mccmnc| Sim {
            mccmnc: mccmnc.to_string(),
            spn: spns.next().unwrap_or("").trim().to_string(),
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

pub fn load_carrier_list(dir: &Path) -> Result<CarrierList, String> {
    let path = dir.join("carrier_list.pb");
    let bytes = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    CarrierList::parse_from_bytes(&bytes).map_err(|e| format!("{}: {e}", path.display()))
}
