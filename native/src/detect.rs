//! Working out which carriers the inserted SIMs belong to.
//!
//! Android resolves a SIM to a CarrierSettings entry through carrier_list.pb: by MCCMNC, and
//! for MVNOs additionally by SPN, IMSI prefix or GID1. We replicate the MCCMNC and SPN halves,
//! which covers the MVNOs that ship their own settings file.

use crate::atomic;
use crate::protos::carrier_list::{CarrierList, carrier_id::Mvno_data};
use protobuf::Message;
use std::fmt::{self, Display};
use std::fs;
use std::io;
use std::path::Path;
use std::process::Command;
use std::thread::sleep;
use std::time::{Duration, Instant};

/// How long to leave between two looks at the SIM properties while waiting for the modem.
const POLL: Duration = Duration::from_secs(2);

/// How many times in a row the properties must come back unchanged before they count as settled.
/// The modem fills them one slot at a time, so a single usable-looking sample can still be a
/// list with the second SIM missing from it.
const STEADY: u32 = 2;

/// One SIM slot, as the modem reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sim {
    pub slot: usize,
    pub mccmnc: String,
    pub spn: String,
}

/// A carrier we believe is in this phone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    /// The name CarrierSettings files it under.
    pub canonical_name: String,
    /// Operator name the SIM reports — empty when the modem was down when we looked. It travels
    /// with the carrier because the IMS APN is labelled after it: CarrierSettings only names the
    /// carriers Google supports, so for the rest a log line would read a bare "25001".
    pub label: String,
}

impl Resolved {
    pub fn new(canonical_name: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            canonical_name: canonical_name.into(),
            label: label.into(),
        }
    }
}

impl Display for Resolved {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.label.is_empty() {
            true => f.write_str(&self.canonical_name),
            false => write!(f, "{} ({})", self.label, self.canonical_name),
        }
    }
}

fn getprop(name: &str) -> String {
    Command::new("getprop")
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
        .filter(|(_, mccmnc)| !mccmnc.is_empty())
        .map(|(slot, mccmnc)| Sim {
            slot,
            mccmnc: mccmnc.to_string(),
            spn: if aligned {
                names[slot].trim().to_string()
            } else {
                String::new()
            },
        })
        .collect()
}

/// Is this sample worth acting on?
///
/// Every slot has to carry both halves. A slot whose name has not arrived yet would have the IMS
/// APN labelled after its MCCMNC for the whole boot, and a list whose two properties disagree in
/// length makes [`pair_sims`] drop the names altogether rather than mis-pair them.
#[cfg(test)]
fn usable(sims: &[Sim]) -> bool {
    !sims.is_empty() && sims.iter().all(|sim| !sim.spn.is_empty())
}

/// The SIMs, once the modem has settled — or whatever is there when `limit` runs out.
///
/// At boot this runs from service.sh, late, while the modem is still coming up: the properties
/// are filled in one slot and one field at a time, so a sample taken too early is empty, missing
/// a SIM, or carrying one carrier's name against another's number.
pub fn settled_sims(limit: Duration) -> Vec<Sim> {
    let deadline = Instant::now() + limit;
    let mut previous: Option<Vec<Sim>> = None;
    let mut steady = 0;

    loop {
        let now = sims();
        steady = if sample_complete(&now) && previous.as_deref() == Some(now.as_slice()) {
            steady + 1
        } else {
            0
        };
        if steady >= STEADY {
            return now;
        }
        if Instant::now() + POLL > deadline {
            eprintln!("the modem never settled; going with what it reports now");
            return now;
        }
        previous = Some(now);
        sleep(POLL);
    }
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

/// Carriers taken from telephony's own carrier config cache.
///
/// At post-fs-data the modem is not up, so the SIM properties are empty and live detection is
/// impossible. Telephony, however, leaves a cache file per SIM from the previous boot, and its
/// `carrier_config_version_string` starts with exactly the canonical name we need — a record the
/// system maintains for us. We read it before the cache is cleared. It carries no operator name.
pub fn from_config_cache(dir: &Path) -> Vec<Resolved> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<Resolved> = Vec::new();
    for entry in entries.flatten() {
        if !is_carrier_config(&entry.file_name().to_string_lossy()) {
            continue;
        }
        let Ok(text) = fs::read_to_string(entry.path()) else {
            continue;
        };
        let Some(canonical) = version_string(&text).and_then(canonical_from_version) else {
            continue;
        };
        if !out.iter().any(|c| c.canonical_name == canonical) {
            out.push(Resolved::new(canonical, ""));
        }
    }
    out
}

/// One of telephony's per-SIM cache files? The "nosim" one describes no carrier at all.
fn is_carrier_config(name: &str) -> bool {
    name.starts_with("carrierconfig-") && name.ends_with(".xml") && !name.contains("nosim")
}

/// The value of `<string name="carrier_config_version_string">…</string>`, read out of the cache
/// XML by hand — pulling in an XML parser to reach one element would be a poor trade.
fn version_string(xml: &str) -> Option<&str> {
    let rest = &xml[xml.find("carrier_config_version_string")?..];
    let open = rest.find('>')?;
    let close = rest[open..].find('<')?;
    Some(&rest[open + 1..open + close])
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

/// Carriers saved by service.sh once telephony was up, as "<canonical>\t<operator name>".
pub fn from_saved(path: &Path) -> Vec<Resolved> {
    fs::read_to_string(path)
        .map(|text| text.lines().filter_map(parse_saved_line).collect())
        .unwrap_or_default()
}

/// Remember these for the next boot, when the modem will not be up in time. The mirror of
/// [`from_saved`].
pub fn save(path: &Path, carriers: &[Resolved]) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let text: String = carriers
        .iter()
        .map(|c| format!("{}\t{}\n", c.canonical_name, c.label))
        .collect();
    atomic::write(
        path,
        if text.is_empty() {
            "# empty\n".to_owned()
        } else {
            text
        },
    )
}

fn parse_saved_line(line: &str) -> Option<Resolved> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return None;
    }
    let value = match line.split_once('\t') {
        Some((name, label)) => Resolved::new(name, label.trim()),
        None => Resolved::new(line, ""),
    };
    crate::config::validate_name(&value.canonical_name).ok()?;
    Some(value)
}

/// Which carriers are in this phone, and how we found out.
///
/// Three sources, in order of quality. At post-fs-data the modem is not up yet, so the SIM
/// properties are empty and only the last two work — which is the normal case for the boot-time
/// run, not the exception.
pub fn identify(src: &Path, sims_file: &Path, phone_files: &Path) -> (Vec<Resolved>, &'static str) {
    if let Ok(list) = load_carrier_list(src) {
        let sample = sims();
        let complete = sample_complete(&sample);
        let live: Vec<Resolved> = sample
            .iter()
            .filter_map(|sim| resolve(&list, sim).map(|name| Resolved::new(name, &sim.spn)))
            .collect();
        if complete {
            return (live, "complete live SIM properties");
        }
    }
    if fs::read_to_string(sims_file).is_ok_and(|s| s.trim() == "# empty") {
        return (Vec::new(), "saved empty SIM inventory");
    }
    let saved = from_saved(sims_file);
    if !saved.is_empty() {
        return (saved, "saved by the previous boot");
    }
    (
        from_config_cache(phone_files),
        "telephony's own config cache",
    )
}

pub fn load_carrier_list(dir: &Path) -> Result<CarrierList, String> {
    let path = dir.join("carrier_list.pb");
    let bytes = fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    CarrierList::parse_from_bytes(&bytes).map_err(|e| format!("{}: {e}", path.display()))
}

/// Readiness is per slot; equal samples alone do not prove an absent second modem is ready.
pub fn sample_complete(sims: &[Sim]) -> bool {
    complete_for_states(sims, &getprop("gsm.sim.state"))
}

fn complete_for_states(sims: &[Sim], states: &str) -> bool {
    let states: Vec<_> = states.split(',').collect();
    !states.is_empty()
        && states
            .iter()
            .enumerate()
            .all(|(slot, state)| match state.trim() {
                "ABSENT" => !sims.iter().any(|s| s.slot == slot),
                "LOADED" | "READY" => sims.iter().any(|s| s.slot == slot && !s.spn.is_empty()),
                _ => false,
            })
        && sims.iter().all(|s| s.slot < states.len())
}

#[cfg(test)]
mod readiness_tests {
    use super::*;

    #[test]
    fn partial_and_empty_inventories_have_distinct_readiness() {
        assert!(!complete_for_states(
            &pair_sims("25001,", "MTS,"),
            "LOADED,NOT_READY"
        ));
        assert!(!complete_for_states(
            &pair_sims("25001,", "MTS,"),
            "LOADED,LOADED"
        ));
        assert!(complete_for_states(
            &pair_sims(",25001", ",MTS"),
            "ABSENT,LOADED"
        ));
        assert!(complete_for_states(&[], "ABSENT,ABSENT"));
        assert!(!complete_for_states(&[], "UNKNOWN,UNKNOWN"));
        assert!(!complete_for_states(&[], ""));
    }

    #[test]
    fn an_empty_saved_inventory_cannot_resurrect_an_old_carrier_cache() {
        let dir = crate::testing::tempdir("empty-sims");
        let saved = dir.join("sims");
        save(&saved, &[]).unwrap();
        let phone = dir.join("phone");
        fs::create_dir(&phone).unwrap();
        fs::write(
            phone.join("carrierconfig-com.google.android.carrier-test.xml"),
            "<string name=\"carrier_config_version_string\">25001-123.4</string>",
        )
        .unwrap();
        assert!(
            identify(&dir.join("no-live-list"), &saved, &phone)
                .0
                .is_empty()
        );
    }
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
        // A canonical name may contain dashes of its own. This one is a US carrier because a
        // dash inside the name is what the case is about, and the Russian names above have none.
        assert_eq!(
            canonical_from_version("t-mobile_us-123.4"),
            Some("t-mobile_us")
        );
        assert_eq!(canonical_from_version(""), None);
        assert_eq!(canonical_from_version("no-version-here"), None);
    }

    #[test]
    fn the_version_string_is_read_out_of_the_xml() {
        let xml = r#"<?xml version='1.0'?>
<map>
  <string name="carrier_config_version_string">tinkoff_ru-77000000001.21&#10;2025-11-12</string>
  <boolean name="carrier_volte_available_bool" value="true" />
</map>"#;
        assert_eq!(
            version_string(xml),
            Some("tinkoff_ru-77000000001.21&#10;2025-11-12")
        );
        assert_eq!(version_string("<map />"), None);
    }

    #[test]
    fn only_the_per_sim_cache_files_are_read() {
        assert!(is_carrier_config(
            "carrierconfig-com.google.android.carrier-1839.xml"
        ));
        // Written when the slot is empty: it describes no carrier.
        assert!(!is_carrier_config(
            "carrierconfig-com.google.android.carrier-nosim-1839.xml"
        ));
        assert!(!is_carrier_config("carrierconfig-something.txt"));
        assert!(!is_carrier_config("preferred-apn.xml"));
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
    fn saved_lines_carry_the_operator_name() {
        assert_eq!(
            parse_saved_line("25001\tМТС"),
            Some(Resolved::new("25001", "МТС"))
        );
        // Written by an older version, before the name travelled with it.
        assert_eq!(
            parse_saved_line("tinkoff_ru"),
            Some(Resolved::new("tinkoff_ru", ""))
        );
        assert_eq!(parse_saved_line("  "), None);
    }

    #[test]
    fn what_is_saved_is_what_comes_back() {
        let path = std::env::temp_dir().join(format!("imsforge-sims-{}", std::process::id()));
        let carriers = vec![
            Resolved::new("25001", "МТС"),
            Resolved::new("tinkoff_ru", ""),
        ];

        save(&path, &carriers).unwrap();
        assert_eq!(from_saved(&path), carriers);

        fs::remove_file(&path).ok();
    }

    #[test]
    fn a_sample_is_only_acted_on_once_every_slot_is_complete() {
        assert!(usable(&pair_sims("25001", "МТС")));
        assert!(usable(&pair_sims("25062,25001", "T-Mobile,МТС")));
        // Nothing reported yet.
        assert!(!usable(&pair_sims("", "")));
        // The number is there, the name has not arrived: patching now would label the IMS APN
        // "25001 IMS" for the whole boot.
        assert!(!usable(&pair_sims("25001", "")));
        // Both properties are filled in, but they disagree in length, so pair_sims dropped the
        // names rather than pair them wrongly — same outcome, one boot with no labels.
        assert!(!usable(&pair_sims("25062,25001", "T-Mobile")));
    }

    #[test]
    fn a_carrier_reads_as_its_operator_name() {
        assert_eq!(Resolved::new("25001", "МТС").to_string(), "МТС (25001)");
        // Nothing to say it better with: Google names an unsupported carrier after its MCCMNC.
        assert_eq!(Resolved::new("25001", "").to_string(), "25001");
    }

    #[test]
    fn no_sims_at_all() {
        assert!(pair_sims("", "").is_empty());
    }
}
