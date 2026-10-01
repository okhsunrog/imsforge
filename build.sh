#!/usr/bin/env bash
# Builds the module zip. The result is device-independent: it carries no carrier data, only the
# patcher, which derives everything on the phone at boot.
set -euo pipefail

cd "$(dirname "$0")"
ABI=${ABI:-arm64-v8a}
case "$ABI" in
    arm64-v8a) EXPECTED_TARGET=aarch64-linux-android ;;
    armeabi-v7a) EXPECTED_TARGET=armv7-linux-androideabi ;;
    x86) EXPECTED_TARGET=i686-linux-android ;;
    x86_64) EXPECTED_TARGET=x86_64-linux-android ;;
    *) echo "unsupported ABI: $ABI" >&2; exit 1 ;;
esac
TARGET=${TARGET:-$EXPECTED_TARGET}
[ "$TARGET" = "$EXPECTED_TARGET" ] || { echo "ABI/TARGET mismatch" >&2; exit 1; }
DIST=dist
STAGE=$DIST/module
SDK_ROOT=${ANDROID_HOME:-${ANDROID_SDK_ROOT:-${ANDROID_NDK_HOME:-${NDK_HOME:-}}/../..}}
D8=${D8:-$(find "$SDK_ROOT/build-tools" -mindepth 2 -maxdepth 2 -name d8 2>/dev/null | sort -V | tail -1)}
[ -x "$D8" ] || { echo "Android SDK d8 is required for the usage-setting helper" >&2; exit 1; }

# The zip announces its version from module.prop and the binary reports its own from Cargo.toml.
# Nothing keeps the two in step, so a release can otherwise ship a module that calls itself one
# version while `imsforge --version` says another.
PROP_VERSION=$(sed -n 's/^version=//p' module/module.prop)
CARGO_VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' native/Cargo.toml | head -1)
[ "$PROP_VERSION" = "v$CARGO_VERSION" ] || {
    echo "version mismatch: module.prop says $PROP_VERSION, Cargo.toml says $CARGO_VERSION" >&2
    exit 1
}

command -v cargo-ndk >/dev/null || {
    echo "cargo-ndk is missing: cargo install cargo-ndk" >&2
    exit 1
}
[ -n "${ANDROID_NDK_HOME:-}${NDK_HOME:-}" ] || {
    echo "set ANDROID_NDK_HOME to your NDK" >&2
    exit 1
}

echo "==> building $TARGET"
(cd native && cargo ndk -t "$ABI" build --release --locked)

echo "==> assembling $STAGE"
rm -rf "$STAGE"
mkdir -p "$STAGE/bin"
install -m 755 "native/target/$TARGET/release/imsforge" "$STAGE/bin/imsforge"
mkdir -p "$DIST/java"
javac --release 8 -Xlint:-options -d "$DIST/java" native/java/UsageSetting.java
"$D8" --min-api 33 --output "$DIST/java" "$DIST/java/UsageSetting.class"
install -m 644 "$DIST/java/classes.dex" "$STAGE/bin/usage-setting.dex"
install -m 755 module/post-fs-data.sh module/service.sh module/action.sh \
    module/customize.sh module/uninstall.sh module/probe.sh "$STAGE/"
install -m 644 module/*.awk "$STAGE/"
install -m 644 module/module.prop "$STAGE/"
# A marker the manager will file wherever it keeps module content, so the module can see at boot
# which layout it got rather than guess from the root implementation.
mkdir -p "$STAGE/system/product/etc/CarrierSettings"
: > "$STAGE/system/product/etc/CarrierSettings/.keep"
mkdir -p "$STAGE/webroot"
install -m 644 module/webroot/* "$STAGE/webroot/"

ZIP=$DIST/imsforge.zip
rm -f "$ZIP"
(cd "$STAGE" && zip -qr "../${ZIP#"$DIST"/}" .)

# --apparent-size, or a compressing filesystem (zfs, btrfs) reports the blocks it managed to
# squeeze the zip into rather than the size the zip actually is.
echo "==> $ZIP ($(du -h --apparent-size "$ZIP" | cut -f1))"
unzip -l "$ZIP" | tail -n +4 | head -n -2
