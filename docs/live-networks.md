# Live network selection

Per-SIM WebUI actions call `imsforge network lte|lte-nr|restore --slot N --sub-id ID`.
`network status` reports the USER mask, usage priority and recovery availability. It reads only
active subscriptions from `dumpsys isub`; unsupported formats fail without writing
radio preferences. No carrier patch, APN, IMS setting or boot service is changed.

The first change saves the exact USER mask and subscription usage setting under `/data/adb/imsforge/network/ID.json`
with the existing atomic writer and lock. Later selections retain that recovery
point. LTE / LTE+NR select DATA_CENTRIC before changing the mask; restoration returns
the original usage setting as well. A small bundled DEX helper invokes the named
ISub.setUsageSetting framework API as root, avoiding hard-coded Binder transaction
numbers. Actions recheck subscription identity before applying and read back the
mask and usage afterward. A failed action retains recovery. Successful restoration removes
the recovery record. LTE_CA and LTE representations are compared semantically.

## Pixel 8 Pro verification, 2026-10-01

- Android 17, SDK 37, KernelSU Next; MTS subscription 1 in logical slot 1.
- Initial USER mask 64511 on both subscriptions.
- LTE only read back 266240; LTE + NR read back 790528.
- Restore read back the original 64511, retaining all carrier/power restrictions.
- Three packets to 1.1.1.1 bound to MTS's rmnet16 passed in each selected mode and
  after restoration. This verifies mobile IP connectivity independently of Wi-Fi.
- Both SIMs' VoLTE and Wi-Fi calling settings matched the pre-test snapshot;
  subscription 2's USER mask stayed 64511 throughout.
- Installed updated binary, probe, and WebUI without rebooting. Previous files
  saved in `/data/adb/imsforge/backup-network-20261001`.
- Pressed MTS's LTE only button in the actual KsuWebUI app; readback and the UI
  both showed LTE only, restoration became available, and rmnet16 passed 3/3 pings.
- End of the pre-reboot test: LTE only, with its initial 64511 recovery record retained.
- Shared checks passed: 60 Rust unit tests, one Rust integration test, 21
  JavaScript/shell tests, formatting, Clippy, ShellCheck, and JavaScript syntax.
- Incoming-call verification is intentionally deferred to the user. Actual 5G
  connectivity and other phone/Android versions were not tested.

## Normal 2.4.0 update and reboot

- Version bumped to 2.4.0, versionCode 10; checks and arm64 ZIP build passed.
- Installed using `ksud module install`, with the user's physical volume-down
  confirmation. The installer staged the module in `modules_update` successfully.
- Reboot completed; installed metadata and binary both reported 2.4.0. All thirteen
  runtime files matched the ZIP (the installer removes customize.sh by design).
- The current boot report was applied, mounted output matched, and configuration
  was unchanged. The actual KsuWebUI screen showed 2.4.0 and Patch applied.
- Android retained the MTS LTE-only USER mask 266240 and its original recovery
  record. However, MTS did not register on cellular LTE after reboot: CS and PS
  WWAN registrations were NOT_REG_OR_SEARCHING with rejectCause 0, mobile data
  was enabled but no cellular connection was established.
- Restoring the original mask 64511 immediately recovered MTS registration on
  EDGE and mobile IP connectivity. A subsequent LTE-only attempt again produced
  OUT_OF_SERVICE. Restored the original mask again; rmnet16 passed 3/3 pings.
- Final device state: original MTS network selection restored, VoLTE/VoWiFi
  untouched, second SIM untouched. The cause of failed LTE registration remains
  unresolved; the successful pre-reboot tests do not establish a reliable
  data-only setup after reboot.

## 2.4.1 data-priority fix

- Disabling the second SIM and Wi-Fi did not by itself resolve the failure: MTS
  registered on LTE with the original mask but lost service after LTE-only was
  selected. A bound rmnet16 ping then lost 4/4 packets.
- Original subscription usage was DEFAULT, resolved by Android to VOICE_CENTRIC.
  Pixel framework resources support both VOICE_CENTRIC and DATA_CENTRIC.
- Setting MTS to DATA_CENTRIC and selecting LTE-only recovered cellular LTE.
  Multiple bound ping checks passed, and LTE remained available for several minutes.
- The updated CLI saved the original mask 64511 and usage setting DEFAULT (0),
  applied mask 266240 with DATA_CENTRIC (2), restored both 64511/0, and reapplied
  266240/2 successfully. VoLTE and Wi-Fi calling remained off.
- Shared checks passed: 61 Rust unit tests, one integration test, 21 JavaScript/shell
  tests, Java helper compilation, formatting, Clippy and ShellCheck.
- Installed the complete 2.4.1 ZIP through ksud with physical confirmation and
  rebooted. All fourteen runtime files matched the ZIP; installed metadata and
  binary reported 2.4.1. The current boot report was applied and matched mounted output.
- After boot, MTS retained mask 266240 and DATA_CENTRIC (2), registered LTE on
  WWAN, and established mobile data. Bound rmnet16 checks passed 4/4 packets
  twice, including after Wi-Fi was disabled again. The second SIM remained disabled.
- The real KsuWebUI showed 2.4.1, Patch applied, LTE only and IMS not registered.
  Detection reported incomplete while the other SIM was disabled; this did not
  prevent the active MTS subscription or live network controls from being read.
- Final state: MTS LTE only with data priority; original 64511/DEFAULT recovery
  retained. Incoming-call behavior remains untested and will be checked by the user.
