//! Deciding what to patch.
//!
//! Nothing here reads a carrier out of a phone or writes a file to one: it is handed the stock
//! settings and the carriers that were found, and answers which of them to patch and why.

use crate::at;
use crate::config::{Carrier, Config};
use crate::detect::Resolved;
use crate::patch::{self, Report};
use crate::protos::carrier_settings::MultiCarrierSettings;
use crate::status::{Entry, Outcome};
use serde::{Deserialize, Serialize};
use std::fmt::{self, Display};
use std::fs;
use std::path::Path;

/// Why a carrier is in the patch list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Reason {
    /// Named in carriers.json.
    Configured,
    /// Belongs to a SIM in this phone, and auto-detection is on.
    Detected,
}

impl Reason {
    /// Does a carrier Google already certified stay untouched?
    ///
    /// A certified carrier ships a curated config dozens of keys deep, and writing our blunt set
    /// over it is a real way to break working VoLTE — so detection stops there. Naming a carrier
    /// in carriers.json is a deliberate act and outranks the guard.
    pub fn yields_to_google(self) -> bool {
        matches!(self, Self::Detected)
    }
}

impl Display for Reason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Configured => "configured",
            Self::Detected => "detected",
        })
    }
}

/// Whether we want this carrier at all — before the stock settings are consulted, which the
/// certification guard above still has its say on. `None` means leave it alone.
pub fn decide(name: &str, cfg: &Config) -> Option<Reason> {
    if cfg.skipped(name) {
        return None;
    }
    if cfg.entry(name).is_some() {
        return Some(Reason::Configured);
    }
    cfg.auto.then_some(Reason::Detected)
}

/// The stock CarrierSettings of one run: others.pb, already parsed, and the per-carrier files
/// beside it. A carrier's settings live in one or the other, never both.
pub struct Stock<'a> {
    dir: &'a Path,
    others: &'a MultiCarrierSettings,
}

impl<'a> Stock<'a> {
    pub fn new(dir: &'a Path, others: &'a MultiCarrierSettings) -> Self {
        Self { dir, others }
    }

    /// Does Google already enable VoLTE for this carrier? `None` when the stock data holds no
    /// entry for the name at all.
    pub fn volte_enabled(&self, name: &str) -> Result<Option<bool>, String> {
        crate::config::validate_name(name)?;
        let own = self.dir.join(format!("{name}.pb"));
        if own.exists() {
            let bytes = fs::read(&own).map_err(at(&own))?;
            let settings = patch::parse_single(&bytes).map_err(at(&own))?;
            if settings.canonical_name() != name {
                return Err(format!("{}: carrier name mismatch", own.display()));
            }
            return Ok(Some(patch::volte_enabled(&settings)));
        }
        Ok(self
            .others
            .setting
            .iter()
            .find(|s| s.canonical_name() == name)
            .map(patch::volte_enabled))
    }
}

pub struct Target {
    pub carrier: Carrier,
    pub reason: Reason,
    /// Operator name the SIM reports, for logging. CarrierSettings identifies a carrier by a
    /// canonical name that Google only bothered to make readable for the carriers it supports —
    /// for the rest the entry is called by its MCCMNC, so a log line of bare "25001" tells the
    /// reader nothing.
    pub label: String,
}

impl Display for Target {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = &self.carrier.canonical_name;
        match self.label.is_empty() {
            true => write!(f, "{name} — {}", self.reason),
            false => write!(f, "{} ({name}) — {}", self.label, self.reason),
        }
    }
}

/// Default the IMS APN label to the carrier name the SIM reports.
///
/// Purely cosmetic — the APN works off its value and type, not its label — but without it the
/// entry shows up in Settings as "25001 IMS" next to Google's own "MTS Internet" and "MTS MMS",
/// which reads like a glitch.
pub fn name_apns(targets: &mut [Target], present: &[Resolved]) {
    for target in targets {
        let named = present
            .iter()
            .find(|p| p.canonical_name == target.carrier.canonical_name && !p.label.is_empty());
        if let Some(found) = named {
            target.label.clone_from(&found.label);
        }
        if target.carrier.ims_apn_name.is_empty() && !target.label.is_empty() {
            target.carrier.ims_apn_name = format!("{} IMS", target.label);
        }
    }
}

/// What a run intends to do.
pub struct Plan {
    /// The carriers to patch.
    pub targets: Vec<Target>,
    /// The carriers considered and left alone, for the record the WebUI reads.
    pub left_alone: Vec<Entry>,
}

/// Work out what to patch, given the stock settings and the carriers found in the phone.
pub fn plan(cfg: &Config, stock: &Stock, present: &[Resolved]) -> Result<Plan, String> {
    let label_of = |name: &str| {
        present
            .iter()
            .find(|p| p.canonical_name == name)
            .map_or("", |p| p.label.as_str())
    };

    let mut targets: Vec<Target> = Vec::new();
    let mut left_alone: Vec<Entry> = Vec::new();
    // Configured first, so carriers.json sets the order of the log — and so that an entry for a
    // carrier that is not in the phone is patched all the same.
    for name in cfg
        .carriers
        .iter()
        .map(|c| &c.canonical_name)
        .chain(present.iter().map(|p| &p.canonical_name))
    {
        if targets.iter().any(|t| t.carrier.canonical_name == *name) {
            continue;
        }
        let Some(reason) = decide(name, cfg) else {
            continue;
        };
        // Nothing in the stock data answers to this name — usually a typo in carriers.json.
        // Left unsaid, the run would patch nothing and explain nothing.
        let Some(certified) = stock.volte_enabled(name)? else {
            eprintln!("  {name}: no CarrierSettings entry, skipping");
            left_alone.push(Entry::new(name, label_of(name), Outcome::Missing));
            continue;
        };
        if certified && reason.yields_to_google() {
            println!("  {name}: VoLTE already enabled by Google, leaving alone");
            left_alone.push(Entry::new(name, label_of(name), Outcome::Certified));
            continue;
        }
        targets.push(Target {
            carrier: cfg
                .entry(name)
                .cloned()
                .unwrap_or_else(|| Carrier::new(name)),
            reason,
            label: String::new(),
        });
    }

    name_apns(&mut targets, present);
    Ok(Plan {
        targets,
        left_alone,
    })
}

/// What the run did, carrier by carrier: the patched ones as their own patch reported them,
/// then the ones it considered and left alone.
pub fn record(plan: &Plan, reports: &[Report]) -> Vec<Entry> {
    let mut out: Vec<Entry> = plan
        .targets
        .iter()
        // Every target was checked against the stock data before it got here, so each one has a
        // report; a target without one would mean it was patched into nothing.
        .filter_map(|target| {
            let report = reports
                .iter()
                .find(|r| r.canonical_name == target.carrier.canonical_name)?;
            Some(Entry::patched(report, target.reason, &target.label))
        })
        .collect();
    out.extend(plan.left_alone.iter().cloned());
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::patch::stock_settings;
    use crate::protos::carrier_settings::MultiCarrierSettings;
    use crate::testing::tempdir;
    use std::fs;

    fn cfg(json: &str) -> Config {
        serde_json::from_str(json).unwrap()
    }

    /// others.pb as Google might ship it.
    fn stock(carriers: &[(&str, bool)]) -> MultiCarrierSettings {
        let mut multi = MultiCarrierSettings::new();
        for (name, volte) in carriers {
            multi.setting.push(stock_settings(name, *volte));
        }
        multi
    }

    /// The carriers a phone was found to have.
    fn present(carriers: &[(&str, &str)]) -> Vec<Resolved> {
        carriers
            .iter()
            .map(|(name, label)| Resolved::new(*name, *label))
            .collect()
    }

    #[test]
    fn a_carrier_google_forgot_is_ours_to_patch() {
        let c = cfg("{}");
        assert_eq!(decide("25001", &c), Some(Reason::Detected));
        assert!(Reason::Detected.yields_to_google());
    }

    #[test]
    fn an_explicit_entry_outranks_the_safety_check() {
        // Asking for a carrier by name is a deliberate act; the guard exists to stop detection
        // from touching a curated config, not to overrule the user.
        let c = cfg(r#"{"carriers":[{"canonical_name":"25001"}]}"#);
        assert_eq!(decide("25001", &c), Some(Reason::Configured));
        assert!(!Reason::Configured.yields_to_google());
    }

    #[test]
    fn skip_beats_everything() {
        let c = cfg(r#"{"carriers":[{"canonical_name":"x"}],"skip":["x"]}"#);
        assert_eq!(decide("x", &c), None);
    }

    #[test]
    fn auto_off_leaves_detection_out_of_it() {
        let c = cfg(r#"{"auto":false}"#);
        assert_eq!(decide("25001", &c), None);

        let explicit = cfg(r#"{"auto":false,"carriers":[{"canonical_name":"25001"}]}"#);
        assert_eq!(decide("25001", &explicit), Some(Reason::Configured));
    }

    #[test]
    fn a_detected_carrier_is_named_after_its_sim() {
        let dir = tempdir("plan");
        let others = stock(&[("25001", false), ("25002", true)]);
        let found = present(&[("25001", "МТС"), ("25002", "Beeline")]);

        let plan = plan(&Config::default(), &Stock::new(&dir, &others), &found).unwrap();

        assert_eq!(plan.targets.len(), 1, "the certified carrier is left alone");
        assert_eq!(plan.targets[0].carrier.canonical_name, "25001");
        assert_eq!(plan.targets[0].reason, Reason::Detected);
        assert_eq!(plan.targets[0].carrier.ims_apn_name, "МТС IMS");
        assert_eq!(plan.targets[0].to_string(), "МТС (25001) — detected");
        // ...and the interface is told why it was left alone, rather than guessing from the log.
        assert_eq!(
            plan.left_alone,
            vec![Entry::new("25002", "Beeline", Outcome::Certified)]
        );
    }

    #[test]
    fn an_explicit_entry_is_patched_without_a_sim_for_it() {
        let dir = tempdir("plan");
        let others = stock(&[("25001", true)]);
        let c = cfg(r#"{"auto":false,"carriers":[{"canonical_name":"25001"}]}"#);

        let plan = plan(&c, &Stock::new(&dir, &others), &[]).unwrap();

        assert_eq!(plan.targets.len(), 1);
        assert_eq!(plan.targets[0].reason, Reason::Configured);
        assert_eq!(plan.targets[0].to_string(), "25001 — configured");
    }

    #[test]
    fn a_name_nothing_answers_to_is_dropped() {
        // A typo in carriers.json would otherwise produce a run that patches nothing and says
        // nothing about why.
        let dir = tempdir("plan");
        let others = stock(&[("25001", false)]);
        let c = cfg(r#"{"carriers":[{"canonical_name":"25oo1"}]}"#);

        let plan = plan(&c, &Stock::new(&dir, &others), &[]).unwrap();

        assert!(plan.targets.is_empty());
        assert_eq!(
            plan.left_alone,
            vec![Entry::new("25oo1", "", Outcome::Missing)]
        );
    }

    #[test]
    fn a_carrier_with_its_own_file_is_read_from_it() {
        // A carrier Google gave a file of its own is not in others.pb at all, and the guard has
        // to see the file's contents rather than conclude the carrier does not exist.
        let dir = tempdir("plan");
        let certified = stock_settings("tinkoff_ru", true);
        fs::write(
            dir.join("tinkoff_ru.pb"),
            protobuf::Message::write_to_bytes(&certified).unwrap(),
        )
        .unwrap();
        let found = present(&[("tinkoff_ru", "T-Bank")]);

        let plan = plan(
            &Config::default(),
            &Stock::new(&dir, &MultiCarrierSettings::new()),
            &found,
        )
        .unwrap();

        assert!(plan.targets.is_empty(), "Google already supports this one");
        assert_eq!(
            plan.left_alone,
            vec![Entry::new("tinkoff_ru", "T-Bank", Outcome::Certified)]
        );
    }
}
