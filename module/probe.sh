#!/system/bin/sh
# Each source has its own outcome. A failed read is never an empty/default configuration.
MODDIR=${0%/*}
DATADIR=/data/adb/imsforge
TMP=$(mktemp -d "${TMPDIR:-/data/local/tmp}/imsforge-probe.XXXXXX") || exit 1
trap 'rm -rf "$TMP"' EXIT
trap 'exit 1' HUP INT TERM

capture() {
    key=$1
    shift
    "$@" > "$TMP/out" 2> "$TMP/err"
    code=$?
    printf '@@%s\n' "$key"
    cat "$TMP/out"
    printf '\n@@%s_ok\n%s\n@@%s_error\n' "$key" "$code" "$key"
    cat "$TMP/err"
    printf '\n'
}

backend() {
    if [ -e /data/adb/metamodule ]; then echo present
    elif [ -d /data/adb/ksu ]; then echo missing
    else echo built-in
    fi
}

radio() {
    dumpsys telephony.registry > "$TMP/radio" || return 1
    awk -f "$MODDIR/probe-radio.awk" "$TMP/radio"
}

carrier() {
    dumpsys carrier_config > "$TMP/carrier" || return 1
    awk -f "$MODDIR/probe-carrier.awk" "$TMP/carrier"
}

capture version cat "$MODDIR/module.prop"
capture config "$MODDIR/bin/imsforge" read-config --config "$DATADIR/carriers.json"
capture detect "$MODDIR/bin/imsforge" detect
capture status "$MODDIR/bin/imsforge" status
capture meta backend
capture log cat "$MODDIR/last-boot.log"
capture radio radio
capture carrier carrier
