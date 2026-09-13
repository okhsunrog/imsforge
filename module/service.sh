#!/system/bin/sh
# Record which carriers are in the phone, for the next boot.
#
# post-fs-data runs before the modem is up, so the SIM properties are empty there and live
# detection is impossible. This stage runs late, when telephony is awake, and leaves the answer
# where the boot-time run can find it.
MODDIR=${0%/*}
DATADIR=/data/adb/imsforge

until [ "$(getprop sys.boot_completed)" = "1" ]; do
    sleep 2
done

# The modem usually needs a few more seconds after boot_completed to report the SIMs.
i=0
while [ -z "$(getprop gsm.sim.operator.numeric | tr -d ,)" ] && [ "$i" -lt 30 ]; do
    sleep 2
    i=$((i + 1))
done

mkdir -p "$DATADIR"
"$MODDIR/bin/imsforge" detect --save > "$DATADIR/last-detect.json" 2>&1
