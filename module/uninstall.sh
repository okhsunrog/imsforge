#!/system/bin/sh
# Clear the cache so telephony goes back to the stock config. Without this it would keep
# running on the patched values until something else invalidates the cache.
rm -f /data/user_de/0/com.android.phone/files/carrierconfig-com.google.android.carrier-*.xml

# The stock cache is ours to clean up; the user's configuration is left in place so a
# reinstall picks it up again.
rm -rf /data/adb/imsforge/stock /data/adb/imsforge/stock.generations
