# Read only current per-phone fields. History is not registration state.
/^[[:space:]]*Phone Id[[:space:]]*=/ {
    split($0, a, "="); phone = a[2] + 0; have_phone = 1; next
}
have_phone && /^[[:space:]]*mPreciseDataConnectionStates=/ {
    # This is an observed SIP proxy address, not proof of registration or voice capability.
    if ($0 ~ /PcscfAddresses: \[[[:space:]]*\//) print "pcscf", phone, "yes"
    else print "pcscf", phone, "no"
}
have_phone && /^[[:space:]]*mServiceState=/ {
    # NR uses a different enum: its value 3 is not LTE's "unsupported".
    lte = index($0, "LteVopsSupportInfo")
    current = lte ? substr($0, lte) : ""
    if (match(current, /mVopsSupport = [0-9]+/)) {
        value = substr(current, RSTART, RLENGTH)
        sub(/.*= /, "", value)
        print "vops", phone, value
    }
}
