# Keep phone IDs. Global defaults and historical log messages cannot establish per-SIM state.
/^[[:space:]]*Phone Id[[:space:]]*=/ {
    split($0, a, "="); phone = a[2] + 0; have_phone = 1; next
}
have_phone && /^[[:space:]]*carrier_volte_available_bool = (true|false)[[:space:]]*$/ {
    split($0, a, "="); gsub(/[[:space:]]/, "", a[2]); print "volte", phone, a[2]
}
