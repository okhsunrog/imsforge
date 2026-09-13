//! carriers.json — optional. Without it, imsforge patches whatever the inserted SIMs need.

use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::Path;

/// Keys that enable IMS itself. Mirrors what PixelIMS sets, minus
/// carrier_supports_ss_over_ut_bool, which breaks call forwarding when the carrier's XCAP
/// server is unreachable.
pub const VOLTE_CONFIGS: &[(&str, bool)] = &[
    ("carrier_volte_available_bool", true),
    ("carrier_vt_available_bool", true),
    ("editable_enhanced_4g_lte_bool", true),
    ("enhanced_4g_lte_on_by_default_bool", true),
    ("hide_enhanced_4g_lte_bool", false),
    ("carrier_volte_provisioning_required_bool", false),
    ("carrier_ims_gba_required_bool", false),
    ("show_ims_registration_status_bool", true),
    ("vonr_enabled_bool", true),
    ("vonr_setting_visibility_bool", true),
    ("allow_adding_apns_bool", true),
    // Cosmetic: draw "4G" instead of "LTE". Does not touch the radio.
    ("show_4g_for_lte_data_icon_bool", true),
];

/// VoWiFi keys. These only make the "Wi-Fi calling" switch available; whether it is on is the
/// user's call, in Settings, per SIM.
pub const WFC_CONFIGS: &[(&str, bool)] = &[
    ("carrier_wfc_ims_available_bool", true),
    ("carrier_wfc_supports_wifi_only_bool", true),
    ("editable_wfc_mode_bool", true),
    ("editable_wfc_roaming_mode_bool", true),
    ("carrier_default_wfc_ims_roaming_enabled_bool", true),
    ("show_wifi_calling_icon_in_status_bar_bool", true),
    ("carrier_cross_sim_ims_available_bool", true),
];

fn default_true() -> bool {
    true
}

fn default_apn_value() -> String {
    "ims".to_string()
}

/// One carrier. Everything except the name is optional; the defaults are what the common case
/// needs, so an explicit entry is only for overrides.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Carrier {
    pub canonical_name: String,
    #[serde(default)]
    pub ims_apn_name: String,
    #[serde(default = "default_apn_value")]
    pub ims_apn_value: String,
    #[serde(default = "default_true")]
    pub ims_apn: bool,
    #[serde(default)]
    pub bools: BTreeMap<String, bool>,
    #[serde(default)]
    pub int_arrays: BTreeMap<String, Vec<i32>>,
}

impl Carrier {
    pub fn new(canonical_name: String) -> Self {
        Self {
            canonical_name,
            ims_apn_name: String::new(),
            ims_apn_value: default_apn_value(),
            ims_apn: true,
            bools: BTreeMap::new(),
            int_arrays: BTreeMap::new(),
        }
    }

    /// Boolean keys to write, in a stable order: VoLTE block, VoWiFi block, then overrides.
    pub fn configs(&self) -> Vec<(String, bool)> {
        let mut out: Vec<(String, bool)> = VOLTE_CONFIGS
            .iter()
            .chain(WFC_CONFIGS.iter())
            .map(|(k, v)| (k.to_string(), *v))
            .collect();
        for (k, v) in &self.bools {
            match out.iter_mut().find(|(key, _)| key == k) {
                Some(slot) => slot.1 = *v,
                None => out.push((k.clone(), *v)),
            }
        }
        out
    }

    pub fn apn_name(&self) -> String {
        if self.ims_apn_name.is_empty() {
            format!("{} IMS", self.canonical_name)
        } else {
            self.ims_apn_name.clone()
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// Patch carriers of the inserted SIMs automatically. On by default; the whole point is
    /// that a fresh install needs no configuration at all.
    #[serde(default = "default_true")]
    pub auto: bool,
    /// Explicit entries. Patched even when the carrier looks certified — an explicit request
    /// beats the safety check.
    #[serde(default)]
    pub carriers: Vec<Carrier>,
    /// Canonical names never to patch, whatever auto-detection thinks.
    #[serde(default)]
    pub skip: Vec<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            auto: true,
            carriers: Vec::new(),
            skip: Vec::new(),
        }
    }
}

impl Config {
    /// A missing file is not an error: it means "defaults", which is the normal case.
    pub fn load(path: &Path) -> Result<Self, String> {
        match std::fs::read_to_string(path) {
            Ok(text) => {
                serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(format!("{}: {e}", path.display())),
        }
    }
}
