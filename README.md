# imsforge

Enable **VoLTE, VoWiFi and VoNR** on Pixel phones for carriers Google never certified.

imsforge is a KernelSU/Magisk module that patches the CarrierSettings protobufs **on the device,
at every boot**. No computer, no per-device build, nothing to redo after an OS update — and
nothing to tap after a reboot, unlike runtime tools such as
[PixelIMS](https://github.com/kyujin-cho/pixel-volte-patch).

*[Русская версия](README.ru.md)*

---

## Why VoLTE is missing in the first place

Pixel phones do not read carrier config from the AOSP `com.android.carrierconfig` app. They read
it from Google's `com.google.android.carrier`, which parses protobuf files in
`/product/etc/CarrierSettings/`. Your carrier is looked up by MCCMNC in `carrier_list.pb`, and
its settings come either from `<canonical_name>.pb` or from the shared `others.pb`.

For a carrier Google never certified those entries contain **APNs only, with an empty `configs`
block**. So `carrier_volte_available_bool` stays `false`, IMS never registers, and calls fall
back to CSFB on 2G/3G — a real problem as carriers refarm 3G away.

There is a second half that flag-flipping tools tend to miss: such carriers usually have **no APN
of type IMS** either. Without one the IMS PDN never comes up, the network hands out no P-CSCF
(the SIP proxy address), and registration cannot even start. imsforge adds the APN in the same
patch.

![imsforge WebUI](docs/webui.png)

## Requirements

- A Pixel (or any device using Google's CarrierSettings) with root: KernelSU, KernelSU Next,
  APatch or Magisk.
- **A mount backend.** Magisk has one built in. Since KernelSU 3.x the manager no longer mounts
  module files itself — that moved into a pluggable "metamodule", and without one installed a
  module that ships files is silently ignored: it installs, its scripts run, and nothing reaches
  the filesystem. Install [NoMount](https://github.com/maxsteeel/nomount) (kernel-level VFS
  redirection, leaves no trace in `/proc/mounts`; needs `CONFIG_NOMOUNT=y` in the kernel),
  [Mountify](https://github.com/backslashxx/mountify) (OverlayFS, any kernel) or
  [meta-overlayfs](https://github.com/KernelSU-Modules-Repo/meta-overlayfs). The module's WebUI
  says plainly when this is missing.

## Install

1. Install `imsforge.zip` in your root manager.
2. Reboot.

That is the whole procedure. Carriers of the inserted SIMs are detected and patched
automatically; carriers Google already certified are deliberately left alone, because their
config is curated and overwriting it can break working VoLTE.

## WebUI

Open the module's WebUI from the manager (or from
[KsuWebUI](https://github.com/a13e300/KsuWebUI)) to see, on the phone:

- whether a mount backend is present and whether the patch actually reached `/product`;
- what each SIM resolved to, and whether it was patched, skipped or unknown;
- the carrier config telephony ended up using, IMS PDN state and the P-CSCF address;
- overrides — add, edit or remove them, with a raw JSON editor for the rest.

It also explains the two states no patch can fix, instead of leaving them looking like a bug:

- **`mVopsSupport = 3`** — the network is not offering voice over LTE to that SIM (`2` means it
  is). No carrier config can override this.
- **`IWLAN_IKEV2_AUTH_FAILURE`** — the carrier's ePDG answered your VoWiFi tunnel and *rejected
  the authentication*: the subscription is not provisioned for it.

Both mean "ask your carrier".

## Overrides

Optional, and only for extras: a non-standard IMS APN, additional config keys, or forcing a
carrier that detection skipped. Edit them in the WebUI, or write
`/data/adb/modules/imsforge/carriers.json` yourself — see [carriers.example.json](carriers.example.json).

```json
{
  "carriers": [
    {
      "canonical_name": "25001",
      "int_arrays": { "carrier_nr_availabilities_int_array": [1, 2] }
    }
  ]
}
```

`canonical_name` is the identifier Google uses inside CarrierSettings — for an unnamed carrier
it is just the MCCMNC. The WebUI shows the right one for every inserted SIM, and
`imsforge detect` prints it as JSON. Other keys: `ims_apn_name` (cosmetic: the label shown in
Settings → APNs, which otherwise comes from the name the SIM reports), `ims_apn_value`,
`ims_apn: false`, `bools`, `int_arrays`, plus top-level `auto: false` and `skip: ["name"]`.

The key set written for each carrier mirrors what PixelIMS sets, minus
`carrier_supports_ss_over_ut_bool` — that one breaks call forwarding when the carrier's XCAP
server is unreachable.

## How it works

On every boot, `post-fs-data.sh` runs the patcher **before the mount backend lays module files
over the system**. Both implementations document that ordering — Magisk: *"Scripts run before any
modules are mounted. This allows a module developer to dynamically adjust their modules before it
gets mounted."* So at that moment `/product` still holds Google's originals, and the patch is
derived from whatever this very boot shipped. That is why an OS update can never leave a stale
snapshot behind.

The patcher then:

1. works out which carriers are in the phone. At post-fs-data the modem is not up yet, so the
   SIM properties are empty and live detection is impossible — the boot-time run instead uses the
   list `service.sh` saved once telephony was awake on the previous boot, falling back to the
   canonical names telephony leaves in its own carrier config cache. Live properties, resolved
   through `carrier_list.pb` with MVNOs matched by SPN, are used whenever the tool runs later;
2. skips carriers whose stock entry already has `carrier_volte_available_bool = true`;
3. fills in the `configs` block, adds an IMS APN labelled after the carrier name the SIM
   reports, bumps the version field (so the result is
   visible as `carrier_config_version_string` in `dumpsys carrier_config`);
4. writes into the module's own directory and relabels the files to `system_file`, because files
   created under `/data/adb` inherit a context the carrier config app cannot read;
5. deletes the carrier config cache. This is not optional: telephony invalidates that cache by
   the *version of the config app's APK*, not by the version of the protobuf data, so patched
   files would otherwise never be read.

A copy of the stock inputs is kept in `stock/` next to the module, along with a fingerprint of
what was produced. Run by hand later, `/product` shows imsforge's own output rather than Google's
— reading that would make the "already certified?" check see our own work and skip everything, so
the fingerprint tells the two apart and the cached stock is used instead. A run that changes
nothing is treated as "we are reading ourselves" and never refreshes the cache.

Protobuf surgery uses [rust-protobuf](https://github.com/stepancheg/rust-protobuf) specifically
because it preserves fields that are not in our schema. Google may add fields to CarrierSettings
at any time, and a library that drops unknown ones (prost, quick-protobuf) would silently discard
them for every other carrier in the file.

## Building

Needs the Android NDK, `cargo-ndk` and the `aarch64-linux-android` target:

```bash
rustup target add aarch64-linux-android
cargo install cargo-ndk
export ANDROID_NDK_HOME=~/Android/Sdk/ndk/<version>
./build.sh          # -> dist/imsforge.zip
```

The zip carries no carrier data, so one build works on every device.

## Layout

```
native/           the patcher (Rust)
proto/            CarrierSettings schemas from AOSP
module/           module template: scripts, module.prop, webroot/
build.sh          cross-compiles and packs dist/imsforge.zip
```

## Limitations

- Only useful where Google's CarrierSettings is what supplies carrier config — Pixels and
  devices that ship the same app.
- `vonr_enabled_bool` does something only where the carrier actually runs 5G SA.
- The APN label in Settings can lag. Telephony syncs APN rows by the config version, which
  imsforge derives from Google's, so changing an override that only affects the label leaves the
  old row in place until the stock version itself moves.
- MVNOs are matched by MCCMNC and SPN. Those distinguished only by IMSI prefix or GID1 fall back
  to the generic entry; add an explicit override if that is wrong for you.
- Verified on a Pixel 8 Pro (husky), Android 17, KernelSU Next with NoMount. Magisk is
  expected to work — it documents the same script ordering and mounts module files itself — but
  it has not been tested on a device.

## Credits

- [PixelIMS](https://github.com/kyujin-cho/pixel-volte-patch) — solves the same problem at
  runtime, and is where the carrier config key set comes from.
- [carriersettings-extractor](https://github.com/GrapheneOS-Archive/carriersettings-extractor) —
  pointed at the AOSP protobuf schemas.
- [AOSP platform/tools/carrier_settings](https://android.googlesource.com/platform/tools/carrier_settings/)
  — the schemas themselves.
- [NoMount](https://github.com/maxsteeel/nomount),
  [Mountify](https://github.com/backslashxx/mountify),
  [WildKernels](https://github.com/WildKernels/GKI_KernelSU_SUSFS) — mount backends and the
  kernels that carry them.

## License

MIT — see [LICENSE](LICENSE).
