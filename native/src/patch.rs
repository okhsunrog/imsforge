//! Protobuf surgery on the CarrierSettings files.

use crate::config::Carrier;
use crate::protos::carrier_settings::{
    ApnItem, CarrierSettings, IntArray, MultiCarrierSettings, apn_item, carrier_config,
};
use carrier_config::config::Value;
use protobuf::{EnumOrUnknown, Message};
use serde::Serialize;
use std::fmt;

pub const VOLTE_KEY: &str = "carrier_volte_available_bool";

/// What became of the carrier's IMS APN.
///
/// An outcome rather than a sentence: whether a run changed anything decides if the stock cache
/// may be refreshed, and the WebUI reads it out of the run's record — neither may hang on how a
/// log line happens to be worded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", content = "value", rename_all = "snake_case")]
pub enum Apn {
    /// Turned off for this carrier in carriers.json.
    Disabled,
    /// The settings already carry an APN of type IMS.
    AlreadyPresent,
    /// We added one, with this value.
    Added(String),
}

impl Apn {
    /// Did this alter the settings?
    pub fn changed(&self) -> bool {
        matches!(self, Self::Added(_))
    }
}

impl fmt::Display for Apn {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Disabled => f.write_str("disabled in config"),
            Self::AlreadyPresent => f.write_str("already present, skipped"),
            Self::Added(value) => write!(f, "added '{value}'"),
        }
    }
}

pub struct Report {
    pub canonical_name: String,
    pub keys_written: usize,
    pub apn: Apn,
}

impl Report {
    /// How much this patch actually altered. Zero across a whole run means the input already
    /// carried our work — that is, we were reading our own output rather than Google's.
    pub fn changes(&self) -> usize {
        self.keys_written + usize::from(self.apn.changed())
    }
}

/// Does Google already enable VoLTE for this carrier?
///
/// This is the guard for auto-detection. A certified carrier ships a curated config with dozens
/// of keys; writing our blunt set over it is a real way to break working VoLTE. An empty or
/// false flag is exactly the "Google forgot about this carrier" case we exist for.
pub fn volte_enabled(settings: &CarrierSettings) -> bool {
    settings
        .configs
        .config
        .iter()
        .find(|c| c.key() == VOLTE_KEY)
        .is_some_and(|c| matches!(c.value, Some(Value::BoolValue(true))))
}

/// Write one config key, unless it already holds exactly this value. Returns whether it wrote —
/// which is what makes a second pass over our own output a no-op.
fn set_config(settings: &mut CarrierSettings, key: &str, value: Value) -> bool {
    let configs = settings.configs.mut_or_insert_default();
    match configs.config.iter_mut().find(|c| c.key() == key) {
        Some(cfg) if cfg.value.as_ref() == Some(&value) => false,
        Some(cfg) => {
            cfg.value = Some(value);
            true
        }
        None => {
            let mut cfg = carrier_config::Config::new();
            cfg.set_key(key.to_string());
            cfg.value = Some(value);
            configs.config.push(cfg);
            true
        }
    }
}

/// The carrier's boolean keys and integer arrays. Returns how many of them actually changed.
fn set_configs(settings: &mut CarrierSettings, carrier: &Carrier) -> usize {
    let mut written = 0;
    for (key, value) in carrier.configs() {
        written += usize::from(set_config(settings, key, Value::BoolValue(value)));
    }
    for (key, values) in &carrier.int_arrays {
        let mut array = IntArray::new();
        array.item.clone_from(values);
        written += usize::from(set_config(settings, key, Value::IntArray(array)));
    }
    written
}

/// Add an APN of type IMS unless one is already there.
///
/// Carrier config flags alone are not enough: without an IMS APN the IMS PDN never comes up,
/// so the network hands out no P-CSCF and registration cannot start.
fn add_ims_apn(settings: &mut CarrierSettings, carrier: &Carrier) -> Apn {
    if !carrier.ims_apn {
        return Apn::Disabled;
    }
    let apns = settings.apns.mut_or_insert_default();
    if apns.apn.iter().any(|a| {
        a.type_
            .contains(&EnumOrUnknown::new(apn_item::ApnType::IMS))
    }) {
        return Apn::AlreadyPresent;
    }

    let mut apn = ApnItem::new();
    apn.set_name(carrier.apn_name());
    apn.set_value(carrier.ims_apn_value.clone());
    apn.type_.push(EnumOrUnknown::new(apn_item::ApnType::IMS));
    apn.set_protocol(apn_item::Protocol::IPV4V6);
    apn.set_roaming_protocol(apn_item::Protocol::IPV4V6);
    apn.set_bearer_bitmask("0".to_string()); // any RAT
    apn.set_user_visible(true); // shows up under Settings -> APNs, handy when debugging
    apn.set_user_editable(false);
    apns.apn.push(apn);

    Apn::Added(carrier.ims_apn_value.clone())
}

fn patch_settings(settings: &mut CarrierSettings, carrier: &Carrier) -> Report {
    Report {
        canonical_name: carrier.canonical_name.clone(),
        keys_written: set_configs(settings, carrier),
        apn: add_ims_apn(settings, carrier),
    }
}

pub fn parse_others(bytes: &[u8]) -> Result<MultiCarrierSettings, protobuf::Error> {
    MultiCarrierSettings::parse_from_bytes(bytes)
}

pub fn parse_single(bytes: &[u8]) -> Result<CarrierSettings, protobuf::Error> {
    CarrierSettings::parse_from_bytes(bytes)
}

/// Patch the carriers that live inside others.pb. Returns the new bytes and a report each.
pub fn patch_others(
    mut multi: MultiCarrierSettings,
    carriers: &[Carrier],
) -> Result<(Vec<u8>, Vec<Report>), protobuf::Error> {
    let mut reports = Vec::new();
    for settings in multi.setting.iter_mut() {
        let name = settings.canonical_name().to_string();
        if let Some(carrier) = carriers.iter().find(|c| c.canonical_name == name) {
            reports.push(patch_settings(settings, carrier));
        }
    }
    // Derived from the stock version rather than incremented in place: reading the stock file
    // on every boot then always yields the same number, so the counter never creeps.
    multi.set_version(multi.version() + 1);
    Ok((multi.write_to_bytes()?, reports))
}

/// Patch a standalone `<canonical_name>.pb`.
pub fn patch_single(
    mut settings: CarrierSettings,
    carrier: &Carrier,
) -> Result<(Vec<u8>, Report), protobuf::Error> {
    let report = patch_settings(&mut settings, carrier);
    settings.set_version(settings.version() + 1);
    Ok((settings.write_to_bytes()?, report))
}

/// A carrier entry as Google might ship it; `volte` marks one of the carriers Google certified.
/// Shared with main's tests, which need stock data to make decisions about.
#[cfg(test)]
pub fn stock_settings(name: &str, volte: bool) -> CarrierSettings {
    let mut settings = CarrierSettings::new();
    settings.set_canonical_name(name.to_string());
    settings.set_version(100);
    if volte {
        set_config(&mut settings, VOLTE_KEY, Value::BoolValue(true));
    }
    settings
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Carrier;

    #[test]
    fn patching_fills_an_empty_entry() {
        let mut settings = stock_settings("25001", false);
        let report = patch_settings(&mut settings, &Carrier::new("25001"));

        assert!(report.keys_written > 0);
        assert_eq!(report.apn, Apn::Added("ims".to_string()));
        assert!(volte_enabled(&settings));
        assert!(settings.apns.apn.iter().any(|a| {
            a.type_
                .contains(&EnumOrUnknown::new(apn_item::ApnType::IMS))
        }));
    }

    #[test]
    fn patching_twice_changes_nothing() {
        // The boot-time run must be idempotent: if it ever reads its own output, a second pass
        // has to be a no-op rather than pile changes on top.
        let carrier = Carrier::new("25001");
        let mut settings = stock_settings("25001", false);
        patch_settings(&mut settings, &carrier);

        let second = patch_settings(&mut settings, &carrier);
        assert_eq!(second.keys_written, 0);
        assert_eq!(second.apn, Apn::AlreadyPresent);
        assert_eq!(second.changes(), 0, "an unchanged run must count as such");
    }

    #[test]
    fn an_override_is_written_over_the_stock_value() {
        let mut settings = stock_settings("25001", false);
        let carrier: Carrier = serde_json::from_str(
            r#"{"canonical_name":"25001","bools":{"vonr_enabled_bool":false}}"#,
        )
        .unwrap();
        patch_settings(&mut settings, &carrier);

        let vonr = settings
            .configs
            .config
            .iter()
            .find(|c| c.key() == "vonr_enabled_bool")
            .expect("the key must be written");
        assert_eq!(vonr.value, Some(Value::BoolValue(false)));
    }

    #[test]
    fn an_int_array_is_written_once() {
        let mut settings = stock_settings("25001", false);
        let carrier: Carrier = serde_json::from_str(
            r#"{"canonical_name":"25001","int_arrays":{"some_ints":[1,2,3]}}"#,
        )
        .unwrap();

        patch_settings(&mut settings, &carrier);
        let written = settings
            .configs
            .config
            .iter()
            .find(|c| c.key() == "some_ints")
            .and_then(|c| c.value.clone());
        assert!(
            matches!(written, Some(Value::IntArray(a)) if a.item == vec![1, 2, 3]),
            "the array must be written as an array"
        );

        let second = patch_settings(&mut settings, &carrier);
        assert_eq!(
            second.keys_written, 0,
            "an identical array is not rewritten"
        );
    }

    #[test]
    fn a_carrier_google_supports_is_recognised() {
        let mut settings = stock_settings("some_carrier", false);
        assert!(!volte_enabled(&settings), "empty config is not certified");

        set_config(&mut settings, VOLTE_KEY, Value::BoolValue(false));
        assert!(
            !volte_enabled(&settings),
            "an explicit false is not certified"
        );

        set_config(&mut settings, VOLTE_KEY, Value::BoolValue(true));
        assert!(volte_enabled(&settings));
    }

    #[test]
    fn fields_outside_our_schema_survive_a_patch() {
        // The reason this uses rust-protobuf rather than prost: Google may add fields to
        // CarrierSettings at any time, and dropping the ones we do not know would quietly
        // discard data for every other carrier in the file.
        let mut multi = MultiCarrierSettings::new();
        multi.set_version(7);
        multi.setting.push(stock_settings("25001", false));

        let mut bytes = multi.write_to_bytes().unwrap();
        // field 99, varint, value 42 — nothing in our schema claims it
        let unknown = [0x98u8, 0x06, 0x2A];
        bytes.extend_from_slice(&unknown);

        let parsed = parse_others(&bytes).unwrap();
        let (out, reports) = patch_others(parsed, &[Carrier::new("25001")]).unwrap();

        assert_eq!(reports.len(), 1);
        assert!(
            out.windows(unknown.len()).any(|w| w == unknown),
            "the unknown field was dropped"
        );
    }

    #[test]
    fn version_is_derived_from_the_stock_one() {
        let mut multi = MultiCarrierSettings::new();
        multi.set_version(41);
        multi.setting.push(stock_settings("25001", false));

        let (out, _) = patch_others(multi, &[Carrier::new("25001")]).unwrap();
        assert_eq!(parse_others(&out).unwrap().version(), 42);
    }
}
