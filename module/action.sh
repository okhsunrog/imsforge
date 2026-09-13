#!/system/bin/sh
# The "run" button in the root manager: show what imsforge decided, and what telephony ended up
# with. Re-patching is deliberately not done here — the patch is derived at boot from the stock
# files, and by now /product is already shadowed by our own output.
MODDIR=${0%/*}

echo "--- imsforge ---"
echo
echo "SIMs:"
"$MODDIR/bin/imsforge" detect 2>&1
echo
echo "Last boot:"
cat "$MODDIR/last-boot.log" 2>/dev/null || echo "  no log yet"
echo
echo "Carrier config now in use:"
dumpsys carrier_config 2>/dev/null | grep -E "^\s*(carrier_config_version_string|carrier_volte_available_bool|carrier_wfc_ims_available_bool) =" | sed 's/^ */  /' | sort -u
echo
echo "IMS PDN:"
dumpsys telephony.registry 2>/dev/null | grep -oE "PcscfAddresses: \[[^]]*\]" | sort -u | sed 's/^/  /'
echo
echo "Edit $MODDIR/carriers.json to override anything; changes apply on the next reboot."
