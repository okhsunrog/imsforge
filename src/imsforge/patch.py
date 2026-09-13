"""Patching CarrierSettings protobufs to enable IMS for uncertified carriers.

Pixel phones do not take carrier config from the AOSP `com.android.carrierconfig` app but
from Google's `com.google.android.carrier`, which reads protobuf files out of
/product/etc/CarrierSettings. For carriers Google never certified those files carry APNs
only, with an empty `configs` block — so `carrier_volte_available_bool` stays false and IMS
never comes up. This module fills that block in.
"""

import dataclasses
import pathlib
import tomllib

from . import protos

# Keys that enable IMS itself. This mirrors what PixelIMS sets, minus
# carrier_supports_ss_over_ut_bool, which breaks call forwarding when the carrier's XCAP
# server is not reachable.
VOLTE_CONFIGS: dict[str, bool] = {
    "carrier_volte_available_bool": True,
    "carrier_vt_available_bool": True,
    "editable_enhanced_4g_lte_bool": True,
    "enhanced_4g_lte_on_by_default_bool": True,
    "hide_enhanced_4g_lte_bool": False,
    "carrier_volte_provisioning_required_bool": False,
    "carrier_ims_gba_required_bool": False,
    "show_ims_registration_status_bool": True,
    "vonr_enabled_bool": True,
    "vonr_setting_visibility_bool": True,
    "allow_adding_apns_bool": True,
    # Cosmetic: draw the "4G" icon instead of "LTE". Does not touch the radio.
    # Its sibling show_4g_for_3g_data_icon_bool (draws "4G" on 3G) is deliberately left alone.
    "show_4g_for_lte_data_icon_bool": True,
}

# VoWiFi keys. These only make the "Wi-Fi calling" switch available — the user still turns it
# on themselves. Carriers without a reachable ePDG are the reason `vowifi = false` exists;
# see carriers.toml. The block is written as explicit false in that case rather than omitted,
# otherwise platform defaults apply.
WFC_CONFIGS: dict[str, bool] = {
    "carrier_wfc_ims_available_bool": True,
    "carrier_wfc_supports_wifi_only_bool": True,
    "editable_wfc_mode_bool": True,
    "editable_wfc_roaming_mode_bool": True,
    "carrier_default_wfc_ims_roaming_enabled_bool": True,
    "show_wifi_calling_icon_in_status_bar_bool": True,
    "carrier_cross_sim_ims_available_bool": True,
}


@dataclasses.dataclass(frozen=True)
class Carrier:
    """One carrier to patch, as described in carriers.toml."""

    canonical_name: str
    ims_apn_name: str = ""
    ims_apn_value: str = "ims"
    ims_apn: bool = True
    vowifi: bool = True
    bools: dict[str, bool] = dataclasses.field(default_factory=dict)
    int_arrays: dict[str, list[int]] = dataclasses.field(default_factory=dict)

    @property
    def configs(self) -> dict[str, bool]:
        wfc = WFC_CONFIGS if self.vowifi else dict.fromkeys(WFC_CONFIGS, False)
        return VOLTE_CONFIGS | wfc | self.bools

    @property
    def apn_name(self) -> str:
        return self.ims_apn_name or f"{self.canonical_name} IMS"


def load_carriers(path: pathlib.Path) -> dict[str, Carrier]:
    with path.open("rb") as fh:
        raw = tomllib.load(fh)
    entries = raw.get("carrier", [])
    if not entries:
        raise SystemExit(f"no [[carrier]] entries in {path}")

    carriers = {}
    for entry in entries:
        known = {f.name for f in dataclasses.fields(Carrier)}
        unknown = set(entry) - known
        if unknown:
            raise SystemExit(f"unknown keys for {entry.get('canonical_name', '?')}: {sorted(unknown)}")
        carrier = Carrier(**entry)
        carriers[carrier.canonical_name] = carrier
    return carriers


def _set_bools(settings, carrier: Carrier) -> list[str]:
    existing = {c.key: c for c in settings.configs.config}
    changed = []
    for key, value in carrier.configs.items():
        cfg = existing.get(key)
        if cfg is None:
            cfg = settings.configs.config.add()
            cfg.key = key
        elif cfg.WhichOneof("value") == "bool_value" and cfg.bool_value == value:
            continue
        cfg.bool_value = value
        changed.append(f"{key}={str(value).lower()}")
    return changed


def _set_int_arrays(settings, carrier: Carrier) -> list[str]:
    existing = {c.key: c for c in settings.configs.config}
    changed = []
    for key, values in carrier.int_arrays.items():
        cfg = existing.get(key)
        if cfg is None:
            cfg = settings.configs.config.add()
            cfg.key = key
        elif cfg.WhichOneof("value") == "int_array" and list(cfg.int_array.item) == values:
            continue
        del cfg.int_array.item[:]
        cfg.int_array.item.extend(values)
        changed.append(f"{key}={values}")
    return changed


def _add_ims_apn(settings, carrier: Carrier) -> str:
    """Add an APN of type IMS unless one is already there.

    Carrier config flags alone are not enough: without an IMS APN the IMS PDN never comes up,
    so the network hands out no P-CSCF (the SIP proxy address) and registration cannot start.
    """
    if not carrier.ims_apn:
        return "disabled in config"
    cs = protos.load()
    if any(cs.ApnItem.IMS in apn.type for apn in settings.apns.apn):
        return "already present, skipped"
    apn = settings.apns.apn.add()
    apn.name = carrier.apn_name
    apn.value = carrier.ims_apn_value
    apn.type.append(cs.ApnItem.IMS)
    apn.protocol = cs.ApnItem.IPV4V6
    apn.roaming_protocol = cs.ApnItem.IPV4V6
    apn.bearer_bitmask = "0"  # any RAT
    apn.user_visible = True  # shows up under Settings -> APNs, handy when debugging
    apn.user_editable = False
    return f"added '{carrier.ims_apn_value}'"


def patch_settings(settings, carrier: Carrier, verbose: bool = True) -> None:
    changed = _set_bools(settings, carrier) + _set_int_arrays(settings, carrier)
    apn = _add_ims_apn(settings, carrier)
    if verbose:
        wfc = "VoWiFi on" if carrier.vowifi else "VoWiFi OFF"
        print(f"  [{carrier.canonical_name}] {wfc}; config keys written: {len(changed)}")
        for item in changed:
            print(f"      {item}")
        print(f"      IMS APN: {apn}")


def patch_tree(
    src: pathlib.Path,
    out: pathlib.Path,
    carriers: dict[str, Carrier],
    verbose: bool = True,
) -> list[pathlib.Path]:
    """Patch others.pb plus any per-carrier <canonical_name>.pb; return the files written.

    Versions are bumped so the patched config is distinguishable from stock in
    `dumpsys carrier_config` (it shows up as carrier_config_version_string).
    """
    cs = protos.load()
    out.mkdir(parents=True, exist_ok=True)
    written: list[pathlib.Path] = []
    patched: set[str] = set()

    multi = cs.MultiCarrierSettings()
    multi.ParseFromString((src / "others.pb").read_bytes())
    hits = [s for s in multi.setting if s.canonical_name in carriers]
    if verbose:
        print(f"others.pb (version {multi.version} -> {multi.version + 1}):")
    for settings in hits:
        patch_settings(settings, carriers[settings.canonical_name], verbose)
        patched.add(settings.canonical_name)
    multi.version += 1
    dst = out / "others.pb"
    dst.write_bytes(multi.SerializeToString())
    written.append(dst)

    for canonical in carriers:
        path = src / f"{canonical}.pb"
        if not path.exists():
            continue
        single = cs.CarrierSettings()
        single.ParseFromString(path.read_bytes())
        if verbose:
            print(f"{path.name} (version {single.version} -> {single.version + 1}):")
        patch_settings(single, carriers[canonical], verbose)
        single.version += 1
        dst = out / path.name
        dst.write_bytes(single.SerializeToString())
        written.append(dst)
        patched.add(canonical)

    missing = sorted(set(carriers) - patched)
    if missing:
        print(f"! not found in the stock snapshot, nothing patched for: {', '.join(missing)}")
        print("  check the canonical name with `imsforge carriers`")

    return written
