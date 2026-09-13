#!/system/bin/sh
# Clear the cache so telephony goes back to the stock config. Without this it would keep
# running on the patched values until something else invalidates the cache.
rm -f /data/user_de/0/com.android.phone/files/carrierconfig-com.google.android.carrier-*.xml
