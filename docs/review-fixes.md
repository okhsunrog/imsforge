# Validation of the review fixes

The changes following the original 2.2.1 commit address thirteen review findings.
The common local, PR and release gate is `bash scripts/check.sh`.

| Finding | Change | Regression coverage |
| --- | --- | --- |
| Broken JavaScript-embedded awk | Separate shell and awk files, per-source exit codes | Real awk execution; shipped probe with a failing dumpsys |
| Stale boot-success record | Format 2, boot ID, preparing/applied/failed phases; manual reports local to output | Invalid config and failed preparation preserve output and record failure |
| Another SIM's VoLTE masks failure | Per-phone observations; publication is independent of flags | Different SIM flag and failed/stale boot scenarios |
| P-CSCF mistaken for registration/VoLTE | Report the observed address only; registration explicitly unverified | Rendered text assertions; historical events excluded |
| Sparse slots misidentified | Explicit slot in detection JSON | Empty first slot; per-slot UI observations |
| Enable switch does nothing | Explicit selection when auto is off or stock decision requires it | Auto-off and certified/skipped carrier scenarios |
| Minimal JSON crashes rendering | Normalize a temporary draft; validate through Rust | Empty object, malformed shapes, CLI defaults |
| Collapsing editor loses draft | Saving always reads the draft; explicit discard action | Edit/collapse/save, dirty refresh prevention |
| Read errors become defaults | Preserve known data, display source errors, disable saving on failed reads | Bridge and individual-section failures |
| Stale standalone stock files | Complete snapshot exchange; preserve older input generations | OTA relocation and failure retaining old output |
| New cache-derived output is unrecognized | Per-generation registry preserves old and new output manifests | Both results resolve to the correct stock generation |
| Canonical name escapes output | Semantic filename validation, duplicate/conflicting override checks | Traversal rejected by config parser and real CLI save |
| Failed publication removes old output | Checked preparation and atomic directory exchange | Failed preparation/exchange; shell propagates apply failure |

Additional coverage checks generation locks, semantic configuration fingerprints,
confirmed empty versus incomplete SIM inventories, and editable drafts after validation errors.
The radio parser distinguishes LTE VoPS from NR's different enum.

## Device checks, 2026-09-15

On a Pixel 8 Pro running Android 17 with KernelSU Next and ZeroMount:

- The new arm64 binary read the existing config and detected both SIMs with explicit slot IDs.
- Real stock protobufs were copied to an isolated directory under `/data/local/tmp`.
  `apply` targeted only that directory, with separate status, stock cache and phone-cache paths.
- Two successful applications exercised both initial publication and directory exchange on the
  device filesystem. Output files had mode 0644 and `u:object_r:system_file:s0`.
- The generated `others.pb` SHA-256 equalled the already installed working module's file:
  `7e1f94b1e080592d803bf7f22ed81d2f8b7a2ec4ff32f4e2c327aaf1d37ed190`.
- Status correctly reported the current boot, applied phase, unchanged config and matching output.
- A deliberately invalid temporary configuration produced a failed record and retained the
  previous temporary output byte for byte.
- The separate probe ran on Android awk and obtained current per-slot observations with zero
  exit codes for radio, carrier config, detection, configuration and status sources.
- The installed module file and the real configuration retained their pre-test SHA-256 values.

This tests the binary, protobuf output, labelling, filesystem operations and parsers on hardware.
It does **not** establish a new installed-module boot test or end-to-end IMS calling acceptance.
No telephony-cache deletion, module replacement, profile changes, UID exclusions or reboot of
the real installation were needed for these isolated checks. Raw device dumps are not committed.

## Installed boot verification, 2026-09-15

After installing the fixed ZIP through `ksud` and rebooting:

- All twelve installed runtime files matched the local build by SHA-256.
- The boot report used format 2 and the current boot ID, with phase `applied`,
  no error, `matches_run=true`, `product=ours` and `config_changed=false`.
- The installer selected `system/product`; ZeroMount had an active VFS rule for
  the generated `others.pb`. Its system-path SHA-256 matched the module output
  and the previously verified patched hash above, while cached stock differed.
- Root, ordinary shell and a diagnostic process with UID 1001 could read the
  patched system-path bytes. The latter is not an exact replica of the live
  telephony process's SELinux and namespace context.
- The output had mode 0644 and `u:object_r:system_file:s0`. Boot and late-detection
  logs showed successful processing; both SIM slots were saved with `complete=true`.
- All eight sources in the installed probe returned zero exit codes.
- MTS in slot 1 had all nineteen configured boolean overrides and the NR array
  `[1, 2]` in the effective Android CarrierConfig, confirming consumption beyond
  filesystem visibility. The explicit `tinkoff_ru` exclusion in slot 0 remained.
- The current MTS ImsPhone reported MmTel registration state 2, in-service state
  and Voice/Video/SMS capabilities. Its registration log and current IWLAN state
  identify WLAN registration. The excluded slot had registration state 0 and no
  MmTel capabilities, with IMS disabled by its platform configuration.

This establishes successful installed-module boot, effective configuration and
MTS IMS registration over Wi-Fi. No profiles, ZeroMount UID exclusions or radio
settings were changed during the automated checks.

The user subsequently confirmed successful real MTS calls over both VoWiFi and
VoLTE, following the requested Wi-Fi-on and Wi-Fi-off checks. This completes the
manual voice-call acceptance checks for this device and carrier. VoNR remains
untested.

## Compatibility and storage

`patch --out DIR` replaces DIR as a whole; its default report is `DIR/.imsforge.json`.
Only the boot script's `apply` publishes the global boot report. Consumers of the old status
schema must understand format 2. Configuration files remain compatible.

Directory publication requires `renameat2(RENAME_EXCHANGE)` support on the module filesystem.
An unsupported exchange fails before removing the existing directory. Previous stock generations
are retained because an older output may still be mounted or retained after a failed update;
the uninstall script removes both current and archived stock, preserving user configuration.

Known platform limits remain: IMSI/GID-only MVNO identification and APN database refresh on
configuration-version changes need carrier-specific validation. The UI does not present transport
or historical log observations as evidence that these platform behaviors succeeded.
