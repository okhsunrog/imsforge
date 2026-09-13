# imsforge

Enable **VoLTE, VoWiFi and VoNR** on Pixel phones for carriers Google never certified — by
patching the CarrierSettings protobufs and shipping them as a KernelSU/Magisk module.

Unlike [PixelIMS](https://github.com/kyujin-cho/pixel-volte-patch), nothing has to be tapped
after a reboot: the config is already correct when telephony starts, with no app and no
Shizuku involved.

*[Русская версия](README.ru.md)*

---

## Why VoLTE is missing in the first place

Pixel phones do not read carrier config from the AOSP `com.android.carrierconfig` app. They
read it from Google's `com.google.android.carrier`, which parses protobuf files in
`/product/etc/CarrierSettings/`. Your carrier is looked up by MCCMNC in `carrier_list.pb`, and
its settings come either from `<canonical_name>.pb` or from the shared `others.pb`.

For a carrier Google never certified those entries contain **APNs only, with an empty `configs`
block**. So `carrier_volte_available_bool` stays `false`, IMS never registers, and calls fall
back to CSFB on 2G/3G — which is a problem as carriers refarm 3G away.

There is a second half to this that flag-flipping tools tend to miss: such carriers usually
have **no APN of type IMS** either. Without one the IMS PDN never comes up, the network hands
out no P-CSCF (the SIP proxy address), and registration cannot even start. imsforge adds the
APN in the same patch.

## Requirements

- A Pixel (or any device using Google's CarrierSettings) with root: KernelSU, KernelSU Next,
  APatch or Magisk.
- **A mount metamodule.** Since KernelSU 3.x the manager itself no longer mounts module files;
  that logic moved into a pluggable backend. Without one installed, a module that ships files
  is silently ignored — it installs, its scripts run, and nothing reaches the filesystem.
  Install one of [NoMount](https://github.com/maxsteeel/nomount) (kernel-level VFS
  redirection, leaves no trace in `/proc/mounts`; needs `CONFIG_NOMOUNT=y` in the kernel),
  [Mountify](https://github.com/backslashxx/mountify) (OverlayFS, works on any kernel) or
  [meta-overlayfs](https://github.com/KernelSU-Modules-Repo/meta-overlayfs).
  `imsforge verify` tells you if this is missing.
- `adb` with root access on the device, and [uv](https://docs.astral.sh/uv/).

## Quick start

```bash
uv run imsforge carriers   # resolve the SIMs in the phone to CarrierSettings names
$EDITOR carriers.toml      # put those names in
uv run imsforge pull       # snapshot the stock CarrierSettings (module disabled!)
uv run imsforge build      # patch the protobufs, build dist/imsforge.zip
uv run imsforge install    # push and install through ksud
# reboot
uv run imsforge verify     # check that it actually took effect
```

`dist/imsforge.zip` is a normal module zip — you can also install it from the KernelSU/Magisk
manager instead of `imsforge install`.

There is also `uv run imsforge apply`, which applies the patched config immediately without a
reboot (temporary bind mount + cache reset + telephony restart; service drops for a few
seconds). Handy while testing, and as a fallback if the mount backend ever breaks.

## Configuring carriers

`carriers.toml` holds the list. The only required field is the carrier's canonical name — the
identifier Google uses inside CarrierSettings:

```toml
[[carrier]]
canonical_name = "25001"      # unnamed carrier: canonical name is just the MCCMNC
ims_apn_name = "MTS IMS"

[carrier.int_arrays]
carrier_nr_availabilities_int_array = [1, 2]   # 1 = NSA, 2 = SA — also enables 5G SA
```

`imsforge carriers` prints the right name for whatever SIMs are in the phone, including MVNOs
matched by SPN/IMSI/GID1, along with a ready-to-paste entry. Optional per-carrier keys
(`vowifi`, `ims_apn`, `ims_apn_value`, `[carrier.bools]`) are documented inline in the file.

The key set written for each carrier mirrors what PixelIMS sets, minus
`carrier_supports_ss_over_ut_bool` — that one breaks call forwarding when the carrier's XCAP
server is unreachable.

## How it works

1. `pull` snapshots `others.pb`, any `<canonical_name>.pb`, and the device fingerprint into
   `stock/`.
2. `build` parses them with the AOSP schemas, fills in the `configs` block, adds an IMS APN,
   bumps the version fields (so the patched config is visible as `carrier_config_version_string`
   in `dumpsys carrier_config`) and packs a module zip.
3. On boot the mount backend lays the patched files over `/product/etc/CarrierSettings/`, and
   the module's `post-fs-data.sh` deletes the carrier config cache. That deletion is not
   optional: telephony invalidates its cache by the *version of the config app's APK*, not by
   the version of the protobuf data, so patched files would otherwise never be read. It runs
   before `system_server` starts, so telephony comes up on the new config directly.

## Verifying and troubleshooting

`imsforge verify` walks the whole chain and prints where it breaks:

| Symptom | Meaning |
|---|---|
| `NO metamodule installed` | No mount backend — the module is a no-op. See Requirements. |
| `others.pb: … (STOCK — not being shadowed)` | The backend did not apply the files. |
| `carrier_volte_available_bool: false` only | Telephony is still on cached config; check that `post-fs-data.sh` ran (`last-boot.log`). |
| No IMS PDN / `P-CSCF: NONE` | The IMS APN did not connect — carrier side, or the APN value is wrong. |

Two failure modes are **not** fixable by any patch, and it is worth telling them apart before
filing a bug:

- **`mVopsSupport = 3`** in `dumpsys telephony.registry` means the network itself is not
  offering voice over LTE to that SIM (`2` means it is). No carrier config can override this.
- **`IWLAN_IKEV2_AUTH_FAILURE`** in the log means the carrier's ePDG answered your VoWiFi
  tunnel and *rejected the authentication* — the subscription is not provisioned for it.

Both mean "ask your carrier", not "the patch failed".

One battery note: if Wi-Fi calling is switched on for a SIM whose carrier has no working ePDG,
Android retries the tunnel roughly every 20 seconds forever. Turn the Settings toggle off for
that SIM, or set `vowifi = false` for it.

## After an OS update

An OTA rewrites `/product`, so the module would start shadowing Google's fresh carrier data
with an old snapshot. Rebuild:

1. disable the module in the manager, reboot;
2. `uv run imsforge pull` — take a fresh stock snapshot;
3. `uv run imsforge build && uv run imsforge install`, enable the module, reboot.

`customize.sh` compares the stock `others.pb` against the snapshot it was built from and warns
when they diverge; `pull` warns if you snapshot while the module is active (which would capture
the patched file as "stock").

## Layout

```
carriers.toml     carriers to patch — the only file most people need to edit
proto/            schemas from AOSP (platform/tools/carrier_settings)
stock/            snapshot pulled off the device + stock.json with its fingerprint
module/           module template (module.prop, post-fs-data.sh, uninstall.sh)
dist/             built module and zip
src/imsforge/
  protos.py       compiles the schemas on the fly via grpcio-tools
  patch.py        protobuf surgery: config keys, IMS APN, int arrays
  build.py        carriers / pull / build / install / apply / verify
```

## Limitations

- The `.pb` files are tied to one Android build, so the module is built per device and
  reinstalled after OTAs. `customize.sh` refuses to install on a different device.
- `vonr_enabled_bool` only does something where the carrier actually runs 5G SA.
- Tested on a Pixel 8 Pro (husky) on Android 17 with KernelSU Next and NoMount. The approach
  is not device-specific, but that is what has been verified.

## Credits

- [PixelIMS](https://github.com/kyujin-cho/pixel-volte-patch) — the app that solves the same
  problem at runtime, and the source of the carrier config key set.
- [carriersettings-extractor](https://github.com/GrapheneOS-Archive/carriersettings-extractor)
  — pointed at the AOSP protobuf schemas.
- [AOSP platform/tools/carrier_settings](https://android.googlesource.com/platform/tools/carrier_settings/)
  — the schemas themselves.
- [NoMount](https://github.com/maxsteeel/nomount),
  [Mountify](https://github.com/backslashxx/mountify),
  [WildKernels](https://github.com/WildKernels/GKI_KernelSU_SUSFS) — the mount backends and the
  kernels that carry them.

## License

MIT — see [LICENSE](LICENSE).
