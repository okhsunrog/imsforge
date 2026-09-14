# WebUI 2.3

The main screen separates patch publication from IMS registration. Confirmed patch
publication and current registration are green; intentionally excluded SIMs are
neutral. Connection observations, boot diagnostics and JSON overrides are available
through expandable sections.

The bottom action bar is absent when nothing needs attention. A changed draft
shows Discard / Save changes; a saved configuration awaiting application shows
Restart to apply. Formatting JSON or restoring the saved draft does not leave a
false pending-change state. Disabling a carrier adds an exclusion without deleting
its custom overrides.

IMS diagnostics use current, scoped ImsPhone registration and MmTel voice capability
fields from TelephonyDebugService. The separate last-registration transport is
historical and never interpreted as current LTE/NR. Unknown formats report unavailable
status; P-CSCF and a different SIM's settings cannot establish registration.

## Validation

- Shared checks: 57 Rust unit tests, one Rust CLI integration test, and twenty
  JavaScript/shell tests; formatting, Clippy, ShellCheck and JavaScript syntax checks.
- Real Android probe: both slots parsed independently; MTS registered with voice
  available, the excluded slot not registered. Last MTS registration transport was WLAN.
  The probe ran from a temporary directory without changing radio or carrier settings.
- Chromium with actual device probe data and a simulated root bridge: light/dark
  themes at 320/360/412px, no horizontal overflow, accessible switch focus,
  discarded drafts, saving and pending-restart states. Browser interactions did
  not execute commands on the phone.
- The screenshot in the README renders that device snapshot in the new WebUI.

The ZIP is version 2.3.0, versionCode 9, built with the existing NDK 28.2 environment.
Native patch behavior is unchanged from the boot- and call-tested implementation;
only the package version changes in the native crate. A new installed-manager WebUI
check remains separate from the browser and device-probe checks above.
