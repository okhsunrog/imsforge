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
        .is_some_and(
            |c| matches!(c.value, Some(carrier_config::config::Value::BoolValue(true))),
        )
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
    if apns
        .apn
        .iter()
        .any(|a| a.type_.contains(&EnumOrUnknown::new(apn_item::ApnType::IMS)))
    {
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
