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

# The output directory below is removed with rm -rf, as root. Everything that goes into building
# it therefore has to be something this script can vouch for, starting with its own location:
# invoked as "sh post-fs-data.sh", $0 carries no directory at all.
case "$0" in
    */*) MODDIR=${0%/*} ;;
    *) MODDIR=. ;;
esac
case "$MODDIR" in
    /*) ;;
    *) MODDIR=$(cd "$MODDIR" && pwd) || exit 1 ;;
esac
DATADIR=/data/adb/imsforge
mkdir -p "$DATADIR"

# Where the mount backend expects our files, worked out by observation rather than by guessing
# from which root implementation is installed.
#
# The zip ships a marker at system/product/etc/CarrierSettings/.keep, and each manager files it
# where it wants module content: KernelSU relocates system/product to product, while Magisk and
# APatch keep the system/ prefix. Whichever spelling the marker ends up in is the one to write to.
#
# The marker only survives until the first patch replaces that directory, so the answer is
# remembered — as the prefix alone, never as a whole path, so that what comes back out of that
# file can be checked against the two spellings that exist rather than trusted as given.
LAYOUT=""
for prefix in product system/product; do
    if [ -e "$MODDIR/$prefix/etc/CarrierSettings/.keep" ]; then
        LAYOUT=$prefix
        echo "$LAYOUT" > "$DATADIR/layout"
        break
    fi
done

if [ -z "$LAYOUT" ] && [ -f "$DATADIR/layout" ]; then
    LAYOUT=$(cat "$DATADIR/layout")
    # v2.1.x remembered the whole path. Take the prefix back out of it, so an install that
    # predates this is not thrown back on the guess below.
    LAYOUT=${LAYOUT#"$MODDIR/"}
    LAYOUT=${LAYOUT%/etc/CarrierSettings}
fi

case "$LAYOUT" in
    product | system/product) ;;
    # No marker and nothing trustworthy remembered: fall back to what the root implementation
    # implies. This is the last resort for a module whose state was wiped by hand.
    *)
        if [ "$KSU" = "true" ] || [ -d /data/adb/ksu ]; then
            LAYOUT=product
        else
            LAYOUT=system/product
        fi
        ;;
esac
OUT="$MODDIR/$LAYOUT/etc/CarrierSettings"

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
        --stock-cache "$DATADIR/stock"; then
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
