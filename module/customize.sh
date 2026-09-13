#!/system/bin/sh
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
ui_print "- Installed. Reboot to apply."
