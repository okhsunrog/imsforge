//! Protobuf surgery on the CarrierSettings files.

use crate::config::Carrier;
use crate::protos::carrier_settings::{
    ApnItem, CarrierSettings, IntArray, MultiCarrierSettings, apn_item, carrier_config,
};
use protobuf::{EnumOrUnknown, Message};

pub const VOLTE_KEY: &str = "carrier_volte_available_bool";

pub struct Report {
    pub canonical_name: String,
    pub keys_written: usize,
    pub apn: String,
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
        .is_some_and(|c| {
            matches!(
                c.value,
                Some(carrier_config::config::Value::BoolValue(true))
            )
        })
}

fn set_bools(settings: &mut CarrierSettings, carrier: &Carrier) -> usize {
    let configs = settings.configs.mut_or_insert_default();
    let mut written = 0;
    for (key, value) in carrier.configs() {
        match configs.config.iter_mut().find(|c| c.key() == key) {
            Some(cfg) => {
                if matches!(cfg.value, Some(carrier_config::config::Value::BoolValue(v)) if v == value)
                {
                    continue;
                }
                cfg.value = Some(carrier_config::config::Value::BoolValue(value));
            }
            None => {
                let mut cfg = carrier_config::Config::new();
                cfg.set_key(key);
                cfg.value = Some(carrier_config::config::Value::BoolValue(value));
                configs.config.push(cfg);
            }
        }
        written += 1;
    }
    written
}

fn set_int_arrays(settings: &mut CarrierSettings, carrier: &Carrier) -> usize {
    let configs = settings.configs.mut_or_insert_default();
    let mut written = 0;
    for (key, values) in &carrier.int_arrays {
        let mut array = IntArray::new();
        array.item = values.clone();

        match configs.config.iter_mut().find(|c| c.key() == *key) {
            Some(cfg) => {
                if matches!(&cfg.value, Some(carrier_config::config::Value::IntArray(a)) if a.item == *values)
                {
                    continue;
                }
                cfg.value = Some(carrier_config::config::Value::IntArray(array));
            }
            None => {
                let mut cfg = carrier_config::Config::new();
                cfg.set_key(key.clone());
                cfg.value = Some(carrier_config::config::Value::IntArray(array));
                configs.config.push(cfg);
            }
        }
        written += 1;
    }
    written
}

/// Add an APN of type IMS unless one is already there.
///
/// Carrier config flags alone are not enough: without an IMS APN the IMS PDN never comes up,
/// so the network hands out no P-CSCF and registration cannot start.
fn add_ims_apn(settings: &mut CarrierSettings, carrier: &Carrier) -> String {
    if !carrier.ims_apn {
        return "disabled in config".to_string();
    }
    let apns = settings.apns.mut_or_insert_default();
    if apns.apn.iter().any(|a| {
        a.type_
            .contains(&EnumOrUnknown::new(apn_item::ApnType::IMS))
    }) {
        return "already present, skipped".to_string();
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

    format!("added '{}'", carrier.ims_apn_value)
}

fn patch_settings(settings: &mut CarrierSettings, carrier: &Carrier) -> Report {
    let keys = set_bools(settings, carrier) + set_int_arrays(settings, carrier);
    let apn = add_ims_apn(settings, carrier);
    Report {
        canonical_name: carrier.canonical_name.clone(),
        keys_written: keys,
        apn,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Carrier;

    fn stock_carrier(name: &str) -> CarrierSettings {
        let mut s = CarrierSettings::new();
        s.set_canonical_name(name.to_string());
        s.set_version(100);
        s
    }

    #[test]
    fn patching_fills_an_empty_entry() {
        let mut settings = stock_carrier("25001");
        let report = patch_settings(&mut settings, &Carrier::new("25001".into()));

        assert!(report.keys_written > 0);
        assert!(report.apn.starts_with("added"));
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
        let carrier = Carrier::new("25001".into());
        let mut settings = stock_carrier("25001");
        patch_settings(&mut settings, &carrier);

        let second = patch_settings(&mut settings, &carrier);
        assert_eq!(second.keys_written, 0);
        assert_eq!(second.apn, "already present, skipped");
    }

    #[test]
    fn a_carrier_google_supports_is_recognised() {
        let mut settings = stock_carrier("some_carrier");
        assert!(!volte_enabled(&settings), "empty config is not certified");

        let configs = settings.configs.mut_or_insert_default();
        let mut cfg = carrier_config::Config::new();
        cfg.set_key(VOLTE_KEY.to_string());
        cfg.value = Some(carrier_config::config::Value::BoolValue(false));
        configs.config.push(cfg);
        assert!(
            !volte_enabled(&settings),
            "an explicit false is not certified"
        );

        settings.configs.mut_or_insert_default().config[0].value =
            Some(carrier_config::config::Value::BoolValue(true));
        assert!(volte_enabled(&settings));
    }

    #[test]
    fn fields_outside_our_schema_survive_a_patch() {
        // The reason this uses rust-protobuf rather than prost: Google may add fields to
        // CarrierSettings at any time, and dropping the ones we do not know would quietly
        // discard data for every other carrier in the file.
        let mut multi = MultiCarrierSettings::new();
        multi.set_version(7);
        multi.setting.push(stock_carrier("25001"));

        let mut bytes = multi.write_to_bytes().unwrap();
        // field 99, varint, value 42 — nothing in our schema claims it
        let unknown = [0x98u8, 0x06, 0x2A];
        bytes.extend_from_slice(&unknown);

        let parsed = parse_others(&bytes).unwrap();
        let (out, reports) = patch_others(parsed, &[Carrier::new("25001".into())]).unwrap();

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
        multi.setting.push(stock_carrier("25001"));

        let (out, _) = patch_others(multi, &[Carrier::new("25001".into())]).unwrap();
        assert_eq!(parse_others(&out).unwrap().version(), 42);
    }
}
