//! The machine-readable record of a patch run.
//!
//! The log beside it is written for people; this is what the WebUI reads. Scraping the log for
//! the same answers makes the wording of a log line an interface — reword "leaving alone" and
//! the interface starts lying about which carriers were patched, with nothing to catch it.

use crate::atomic;
use crate::patch::{Apn, Report};
use crate::plan::Reason;
use serde::Serialize;
use std::fs;
use std::path::Path;

/// Bumped when the shape below changes in a way an older WebUI could misread.
const FORMAT: u32 = 1;

/// Which files the run read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    /// Google's own, straight off /product.
    Stock,
    /// Our cached copy of them, because /product was already shadowed by our output.
    Cache,
}

/// What became of one carrier, and why.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum Outcome {
    /// Patched; `reason` says what put it on the list.
    Patched {
        reason: Reason,
        keys: usize,
        apn: Apn,
    },
    /// Left alone: Google already enables VoLTE here, and its curated config is worth more than
    /// our blunt set of keys.
    Certified,
    /// Left alone: nothing in the stock data answers to this name, so there was nothing to
    /// patch. Usually a typo in carriers.json.
    Missing,
}

/// One carrier in the record.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Entry {
    pub canonical_name: String,
    /// Operator name the SIM reports; left out when the modem was down.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub label: String,
    #[serde(flatten)]
    pub outcome: Outcome,
}

impl Entry {
    pub fn new(
        canonical_name: impl Into<String>,
        label: impl Into<String>,
        outcome: Outcome,
    ) -> Self {
        Self {
            canonical_name: canonical_name.into(),
            label: label.into(),
            outcome,
        }
    }

    /// The record of a carrier we patched, from what the patch itself reported.
    pub fn patched(report: &Report, reason: Reason, label: impl Into<String>) -> Self {
        Self::new(
            report.canonical_name.clone(),
            label,
            Outcome::Patched {
                reason,
                keys: report.keys_written,
                apn: report.apn.clone(),
            },
        )
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Status {
    pub format: u32,
    pub source: Source,
    pub carriers: Vec<Entry>,
}

impl Status {
    pub fn new(source: Source, carriers: Vec<Entry>) -> Self {
        Self {
            format: FORMAT,
            source,
            carriers,
        }
    }

    /// Written beside the configuration rather than into the module directory: a module update
    /// replaces the module's own files wholesale, and /data/adb/imsforge is what survives.
    pub fn write(&self, path: &Path) -> Result<(), String> {
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        let text = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        atomic::write(path, text + "\n").map_err(|e| format!("{}: {e}", path.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn json(status: &Status) -> serde_json::Value {
        serde_json::to_value(status).unwrap()
    }

    #[test]
    fn a_patched_carrier_carries_its_numbers() {
        let report = Report {
            canonical_name: "25001".to_string(),
            keys_written: 19,
            apn: Apn::Added("ims".to_string()),
        };
        let status = Status::new(
            Source::Stock,
            vec![Entry::patched(&report, Reason::Detected, "МТС")],
        );

        assert_eq!(
            json(&status),
            serde_json::json!({
                "format": 1,
                "source": "stock",
                "carriers": [{
                    "canonical_name": "25001",
                    "label": "МТС",
                    "outcome": "patched",
                    "reason": "detected",
                    "keys": 19,
                    "apn": {"state": "added", "value": "ims"},
                }],
            })
        );
    }

    #[test]
    fn a_carrier_left_alone_says_why() {
        let status = Status::new(
            Source::Cache,
            vec![
                Entry::new("25002", "Beeline", Outcome::Certified),
                Entry::new("25oo1", "", Outcome::Missing),
            ],
        );

        assert_eq!(
            json(&status)["carriers"],
            serde_json::json!([
                {"canonical_name": "25002", "label": "Beeline", "outcome": "certified"},
                // No SIM reports a name for a carrier that does not exist, so no label.
                {"canonical_name": "25oo1", "outcome": "missing"},
            ])
        );
    }
}
