#!/system/bin/sh
# The only job here is to invalidate the carrier config cache.
#
# The .pb files themselves are laid over /product by the mount backend (NoMount, Mountify,
# meta-overlayfs, ...). Since KernelSU 3.x that logic lives in a separate "metamodule", and
# without one a module that ships files is silently ignored — see the README.
#
# Clearing the cache is required: telephony invalidates it by the *version of the config app's
# APK*, not by the version of the protobuf data, so patched settings would never be read
# otherwise. post-fs-data runs before system_server starts, so telephony comes up on the new
# config directly — no races, and no "works after the second reboot".
MODDIR=${0%/*}
LOG="$MODDIR/last-boot.log"
PHONE_FILES=/data/user_de/0/com.android.phone/files

{
    echo "[$(date)] post-fs-data: clearing the carrier config cache"
    found=0
    for f in "$PHONE_FILES"/carrierconfig-com.google.android.carrier-*.xml; do
        [ -e "$f" ] || continue
        rm -f "$f" && echo "  removed $(basename "$f")" && found=$((found + 1))
    done
    [ "$found" -eq 0 ] && echo "  cache was empty, nothing removed"
} > "$LOG" 2>&1
