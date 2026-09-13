#!/usr/bin/env bash
# Builds the module zip. The result is device-independent: it carries no carrier data, only the
# patcher, which derives everything on the phone at boot.
set -euo pipefail

cd "$(dirname "$0")"
ABI=${ABI:-arm64-v8a}
TARGET=${TARGET:-aarch64-linux-android}
DIST=dist
STAGE=$DIST/module

command -v cargo-ndk >/dev/null || {
    echo "cargo-ndk is missing: cargo install cargo-ndk" >&2
    exit 1
}
[ -n "${ANDROID_NDK_HOME:-}${NDK_HOME:-}" ] || {
    echo "set ANDROID_NDK_HOME to your NDK" >&2
    exit 1
}

echo "==> building $TARGET"
(cd native && cargo ndk -t "$ABI" build --release)

echo "==> assembling $STAGE"
rm -rf "$STAGE"
mkdir -p "$STAGE/bin"
install -m 755 "native/target/$TARGET/release/imsforge" "$STAGE/bin/imsforge"
install -m 755 module/post-fs-data.sh module/action.sh module/customize.sh \
    module/uninstall.sh "$STAGE/"
install -m 644 module/module.prop "$STAGE/"
mkdir -p "$STAGE/webroot"
install -m 644 module/webroot/* "$STAGE/webroot/"

ZIP=$DIST/imsforge.zip
rm -f "$ZIP"
(cd "$STAGE" && zip -qr "../${ZIP#$DIST/}" .)

echo "==> $ZIP ($(du -h "$ZIP" | cut -f1))"
unzip -l "$ZIP" | tail -n +4 | head -n -2
