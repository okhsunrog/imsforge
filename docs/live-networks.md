# Live network selection

Per-SIM WebUI actions call `imsforge network lte|lte-nr|restore --slot N --sub-id ID`.
`network status` reports the USER mask and recovery availability. It reads only
active subscriptions from `dumpsys isub`; unsupported formats fail without writing
radio preferences. No carrier patch, APN, IMS setting or boot service is changed.

The first change saves the exact USER mask under `/data/adb/imsforge/network/ID.json`
with the existing atomic writer and lock. Later selections retain that recovery
point. Actions recheck subscription identity before applying and read back the
mask afterward. A failed action retains recovery. Successful restoration removes
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
