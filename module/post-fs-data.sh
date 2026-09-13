#!/system/bin/sh
# Runs before the mount backend lays module files over the system, which is the whole trick:
# /product still holds Google's stock CarrierSettings at this point, so we derive the patch from
# what this very boot shipped. An OS update can therefore never leave a stale snapshot behind.
#
# Both root implementations document this ordering. Magisk: "Scripts run before any modules are
# mounted. This allows a module developer to dynamically adjust their modules before it gets
# mounted." KernelSU Next runs module post-fs-data.sh, then the metamodule's mount script.
MODDIR=${0%/*}
LOG="$MODDIR/last-boot.log"
PHONE_FILES=/data/user_de/0/com.android.phone/files
# Settings and the stock cache live outside the module: updating a module replaces its whole
# directory, which would discard the user's configuration on every upgrade.
DATADIR=/data/adb/imsforge

# Where the mount backend expects our files. KernelSU keeps partitions at the module root
# (NoMount and friends scan $MODDIR/product); Magisk and APatch use the system/ prefix.
if [ "$KSU" = "true" ] || [ -d /data/adb/ksu ]; then
    OUT="$MODDIR/product/etc/CarrierSettings"
else
    OUT="$MODDIR/system/product/etc/CarrierSettings"
fi

{
    echo "[$(date)] post-fs-data"
    echo "  output: $OUT"

    mkdir -p "$DATADIR"
    # Migrate a config left in the module directory by an older version.
    [ -f "$MODDIR/carriers.json" ] && [ ! -f "$DATADIR/carriers.json" ] &&
        mv "$MODDIR/carriers.json" "$DATADIR/carriers.json" && echo "  migrated carriers.json"

    if "$MODDIR/bin/imsforge" patch --out "$OUT" --config "$DATADIR/carriers.json" \
        --cache "$DATADIR/stock"; then
        # Files created at runtime inherit adb_data_file from /data/adb. Mounted over /product
        # with that label, com.google.android.carrier cannot read them — so relabel to match a
        # stock file.
        # ls, because find cannot print an SELinux context and the path is fixed anyway.
        # shellcheck disable=SC2012
        ctx=$(ls -Z /product/etc/CarrierSettings/carrier_list.pb 2>/dev/null | awk '{print $1}')
        if [ -n "$ctx" ]; then
            chcon "$ctx" "$OUT"/*.pb 2>/dev/null && echo "  relabelled to $ctx"
        fi
        chmod 644 "$OUT"/*.pb 2>/dev/null
    else
        echo "  ! patching failed, leaving the system untouched"
    fi

    # Telephony invalidates its carrier config cache by the *version of the config app's APK*,
    # not by the version of the protobuf data, so without this the patched files are never read.
    found=0
    for f in "$PHONE_FILES"/carrierconfig-com.google.android.carrier-*.xml; do
        [ -e "$f" ] || continue
        rm -f "$f" && echo "  cache: removed $(basename "$f")" && found=$((found + 1))
    done
    [ "$found" -eq 0 ] && echo "  cache: already empty"
} > "$LOG" 2>&1
