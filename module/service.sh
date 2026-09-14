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

mkdir -p "$DATADIR"
# --wait rather than a polling loop here: the modem fills gsm.sim.operator.numeric and .alpha
# independently and one slot at a time, and what counts as a complete answer — every slot with
# both a number and a name — is the patcher's rule. Stating it a second time in shell, in other
# words, is how the two drift apart.
#
# The streams are kept apart: progress notes and the list of what was saved go to the log, and
# the JSON stays parseable.
"$MODDIR/bin/imsforge" detect --save --wait 120 \
    > "$DATADIR/last-detect.json" 2> "$DATADIR/last-detect.log"
