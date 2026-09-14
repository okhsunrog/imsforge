#!/system/bin/sh
# Runs before the mount backend lays module files over the system, which is the whole trick:
# /product still holds Google's stock CarrierSettings at this point, so we derive the patch from
# what this very boot shipped. An OS update can therefore never leave a stale snapshot behind.
#
# Both root implementations document this ordering. Magisk: "Scripts run before any modules are
# mounted. This allows a module developer to dynamically adjust their modules before it gets
# mounted." KernelSU Next runs module post-fs-data.sh, then the metamodule's mount script.
LOG_NAME=last-boot.log
PHONE_FILES=/data/user_de/0/com.android.phone/files

# Where the mount backend expects our files, worked out by observation rather than by guessing
# from which root implementation is installed.
#
# The zip ships a marker at system/product/etc/CarrierSettings/.keep, and each manager files it
# where it wants module content: KernelSU relocates system/product to product, while Magisk and
# APatch keep the system/ prefix. Whichever spelling the marker ends up in is the one to write to.
#
# The marker only survives until the first patch replaces that directory, so the answer is
# remembered. The root-implementation check stays as a last resort for a module whose state was
# wiped by hand.
MODDIR=${0%/*}
DATADIR=/data/adb/imsforge
mkdir -p "$DATADIR"

OUT=""
for d in "$MODDIR/product/etc/CarrierSettings" "$MODDIR/system/product/etc/CarrierSettings"; do
    if [ -e "$d/.keep" ]; then
        OUT="$d"
        echo "$OUT" > "$DATADIR/layout"
        break
    fi
done

if [ -z "$OUT" ] && [ -f "$DATADIR/layout" ]; then
    OUT=$(cat "$DATADIR/layout")
fi

if [ -z "$OUT" ]; then
    if [ "$KSU" = "true" ] || [ -d /data/adb/ksu ]; then
        OUT="$MODDIR/product/etc/CarrierSettings"
    else
        OUT="$MODDIR/system/product/etc/CarrierSettings"
    fi
fi

{
    echo "[$(date)] post-fs-data"
    echo "  output: $OUT"

    # Migrate a config left in the module directory by an older version.
    [ -f "$MODDIR/carriers.json" ] && [ ! -f "$DATADIR/carriers.json" ] &&
        mv "$MODDIR/carriers.json" "$DATADIR/carriers.json" && echo "  migrated carriers.json"

    # Patch into a staging directory and swap it in, rather than writing over what is already
    # there. Nothing else would ever remove a file: turn a carrier off and last boot's patched
    # copy would sit in place and keep being mounted, so the switch would appear to do nothing.
    STAGE="$OUT.new"
    rm -rf "$STAGE"
    mkdir -p "$STAGE"

    if "$MODDIR/bin/imsforge" patch --out "$STAGE" --config "$DATADIR/carriers.json" \
        --cache "$DATADIR/stock"; then
        # Files created at runtime inherit adb_data_file from /data/adb. Mounted over /product
        # with that label, com.google.android.carrier cannot read them — so relabel to match a
        # stock file.
        # ls, because find cannot print an SELinux context and the path is fixed anyway.
        # shellcheck disable=SC2012
        ctx=$(ls -Z /product/etc/CarrierSettings/carrier_list.pb 2>/dev/null | awk '{print $1}')
        if [ -n "$ctx" ]; then
            chcon "$ctx" "$STAGE"/*.pb 2>/dev/null && echo "  relabelled to $ctx"
        fi
        chmod 644 "$STAGE"/*.pb 2>/dev/null

        rm -rf "$OUT"
        if [ -n "$(ls -A "$STAGE" 2>/dev/null)" ]; then
            mv "$STAGE" "$OUT"
        else
            rm -rf "$STAGE"
            echo "  nothing to patch — /product is left as Google shipped it"
        fi
    else
        rm -rf "$STAGE"
        echo "  ! patching failed, keeping the previous output"
    fi

    # Telephony invalidates its carrier config cache by the *version of the config app's APK*,
    # not by the version of the protobuf data, so without this the patched files are never read.
    found=0
    for f in "$PHONE_FILES"/carrierconfig-com.google.android.carrier-*.xml; do
        [ -e "$f" ] || continue
        rm -f "$f" && echo "  cache: removed $(basename "$f")" && found=$((found + 1))
    done
    [ "$found" -eq 0 ] && echo "  cache: already empty"
} > "$MODDIR/$LOG_NAME" 2>&1
