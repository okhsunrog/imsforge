# Parse only scoped, current ImsPhone / ImsPhoneCallTracker fields.
# RegistrationManager states: 0 not registered, 1 registering, 2 registered.
# The registration log supplies a LAST registration transport, never current RAT.
function flush() {
    if (phone == "") return
    if (reg != "") print "ims", phone, reg
    if (voice != "") print "voice", phone, voice
    if (reg == "2" && transport != "") print "last_transport", phone, transport
}
function reset() { phone = ""; reg = ""; voice = ""; transport = ""; section = "" }
BEGIN { reset() }
/^[[:space:]]*GsmCdmaPhone extends:[[:space:]]*$/ { flush(); reset(); next }
/^[[:space:]]*ImsPhone extends:[[:space:]]*$/ { section = "identity"; next }
section == "identity" && /^[[:space:]]*mPhoneId=[0-9]+[[:space:]]*$/ {
    split($0, a, "="); phone = a[2] + 0; next
}
/^[[:space:]]*ImsPhoneCallTracker extends:[[:space:]]*$/ { section = "capabilities"; next }
/^[[:space:]]*ImsPhone:[[:space:]]*$/ { section = "registration"; next }
section == "capabilities" && /^[[:space:]]*mMmTelCapabilities=/ {
    if (match($0, /\[Voice: (true|false)/)) {
        voice = substr($0, RSTART, RLENGTH); sub(/.*: /, "", voice)
    }
}
section == "registration" && /^[[:space:]]*mImsMmTelRegistrationState = [012][[:space:]]*$/ {
    split($0, a, "="); gsub(/[[:space:]]/, "", a[2]); reg = a[2]
}
section == "registration" && /^[[:space:]]*Registration Log:[[:space:]]*$/ { section = "history"; next }
section == "history" && /handleIms(Unregistered|Registering):/ { transport = "" }
section == "history" && /handleImsRegistered: onImsMmTelConnected imsTransportType=/ {
    transport = ""
    if ($0 ~ /imsTransportType=WLAN[[:space:]]*$/) transport = "wifi"
    if ($0 ~ /imsTransportType=WWAN[[:space:]]*$/) transport = "cellular"
}
/^[[:space:]]*[+]+[[:space:]]*$/ { section = "" }
END { flush() }
