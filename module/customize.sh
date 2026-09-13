#!/system/bin/sh
# Read by the module installer, not by this script.
# shellcheck disable=SC2034
SKIPUNZIP=0

ui_print "- imsforge"
ui_print "  patches CarrierSettings on the device at every boot"

if [ ! -f /product/etc/CarrierSettings/others.pb ]; then
    ui_print "! /product/etc/CarrierSettings not found."
    ui_print "! This device does not use Google's CarrierSettings, so there is"
    ui_print "! nothing here for imsforge to patch."
    abort "! Aborted"
fi

if [ "$KSU" = "true" ] && [ ! -e /data/adb/metamodule ]; then
    ui_print "!"
    ui_print "! No mount metamodule installed. Since KernelSU 3.x the manager does"
    ui_print "! not mount module files itself, so this module would do nothing."
    ui_print "! Install NoMount, Mountify or meta-overlayfs, then reboot."
    ui_print "!"
fi

set_perm_recursive "$MODPATH" 0 0 0755 0644
set_perm "$MODPATH/bin/imsforge" 0 0 0755

# Record the carriers now, while the phone is running and the SIMs are visible. The first boot
# after an install runs before the modem is up, and telephony's own config cache — the only other
# thing that remembers carriers across a reboot — may not have an entry for every SIM yet. Without
# this the first boot can silently leave a SIM unpatched.
mkdir -p /data/adb/imsforge
if "$MODPATH/bin/imsforge" detect --save > /dev/null 2>&1; then
    ui_print "- Carriers detected:"
    while IFS="$(printf '\t')" read -r name spn; do
        [ -n "$name" ] && ui_print "    ${spn:-$name} ($name)"
    done < /data/adb/imsforge/sims
else
    ui_print "! Could not read the SIMs now; the first boot may need a second reboot."
fi

ui_print "- Installed. Reboot to apply."
