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

# The modem fills gsm.sim.operator.numeric and .alpha independently and one slot at a time, so
# reading too early yields a half-built list — or one carrier's name against another's MCCMNC.
# Wait until both agree in length and stop changing.
prev=""
stable=0
i=0
while [ "$i" -lt 60 ]; do
    numeric=$(getprop gsm.sim.operator.numeric)
    alpha=$(getprop gsm.sim.operator.alpha)
    slots_n=$(echo "$numeric" | tr ',' '\n' | grep -c .)
    slots_a=$(echo "$alpha" | tr ',' '\n' | grep -c .)

    if [ -n "$numeric" ] && [ "$slots_n" = "$slots_a" ] && [ "$numeric|$alpha" = "$prev" ]; then
        stable=$((stable + 1))
        [ "$stable" -ge 2 ] && break
    else
        stable=0
    fi
    prev="$numeric|$alpha"
    sleep 2
    i=$((i + 1))
done

mkdir -p "$DATADIR"
"$MODDIR/bin/imsforge" detect --save > "$DATADIR/last-detect.json" 2>&1
