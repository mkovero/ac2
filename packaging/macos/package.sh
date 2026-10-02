#!/usr/bin/env bash
# macOS release artifacts from the two per-architecture builds:
#   ac2-<version>-macos-universal.dmg   ac2.app (universal2) + command-line tools
#   ac2-<version>-macos-universal.zip   the same content as a zip
#
#   packaging/macos/package.sh <version> <aarch64 bin dir> <x86_64 bin dir> <out dir>
#
# Signing is optional and driven by the environment, so the same script makes unsigned
# dry-run builds and signed releases:
#   MACOS_SIGN_IDENTITY   "Developer ID Application: …" in an unlocked keychain → hardened
#                         runtime signature with entitlements.plist; unset → ad-hoc
#                         signature (runs after the user allows it in Privacy & Security)
#   APPLE_ID, APPLE_TEAM_ID, APPLE_APP_PASSWORD
#                         all set (and an identity) → notarize the DMG and staple it
set -euo pipefail

version=$1
arm=$2
x86=$3
out=$4
root=$(cd "$(dirname "$0")/../.." && pwd)
mkdir -p "$out"
out=$(cd "$out" && pwd)
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

name="ac2-$version-macos-universal"
stage="$work/$name"
mkdir -p "$stage/bin" "$stage/launchd" "$stage/docs"

for b in ac2 ac2d ac2-ui; do
    lipo -create -output "$work/$b" "$arm/$b" "$x86/$b"
    lipo -verify_arch "$work/$b" arm64 x86_64
done

app="$stage/ac2.app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
install -m 0755 "$work/ac2-ui" "$app/Contents/MacOS/ac2-ui"
sed "s|@VERSION@|$version|g" "$root/packaging/macos/Info.plist" > "$app/Contents/Info.plist"
plutil -lint "$app/Contents/Info.plist"
install -m 0644 "$root/packaging/icon/generated/ac2.icns" "$app/Contents/Resources/ac2.icns"
printf 'APPL????' > "$app/Contents/PkgInfo"
install -m 0755 "$work/ac2" "$work/ac2d" "$stage/bin/"
install -m 0644 "$root/packaging/macos/io.github.mkovero.ac2d.plist" "$stage/launchd/"
install -m 0644 "$root/README.md" "$root/LICENSE" "$stage/"
install -m 0644 "$root/docs/install.md" "$root/docs/user-guide.md" "$root/docs/protocol.md" "$stage/docs/"

ent="$root/packaging/macos/entitlements.plist"
if [ -n "${MACOS_SIGN_IDENTITY:-}" ]; then
    sign=(codesign --force --timestamp --options runtime --entitlements "$ent" --sign "$MACOS_SIGN_IDENTITY")
    echo "signing with $MACOS_SIGN_IDENTITY"
else
    sign=(codesign --force --sign -)
    echo "no MACOS_SIGN_IDENTITY: ad-hoc signature (unsigned release)"
fi
"${sign[@]}" "$stage/bin/ac2" "$stage/bin/ac2d"
"${sign[@]}" "$app"
codesign --verify --strict --verbose=2 "$app"

dmg="$out/$name.dmg"
ln -s /Applications "$stage/Applications"
hdiutil create -quiet -volname "ac2 $version" -srcfolder "$stage" -ov -format UDZO "$dmg"
rm "$stage/Applications"
if [ -n "${MACOS_SIGN_IDENTITY:-}" ]; then
    codesign --force --timestamp --sign "$MACOS_SIGN_IDENTITY" "$dmg"
    if [ -n "${APPLE_ID:-}" ] && [ -n "${APPLE_TEAM_ID:-}" ] && [ -n "${APPLE_APP_PASSWORD:-}" ]; then
        xcrun notarytool submit "$dmg" --apple-id "$APPLE_ID" --team-id "$APPLE_TEAM_ID" \
            --password "$APPLE_APP_PASSWORD" --wait
        xcrun stapler staple "$dmg"
    else
        echo "notarization skipped: APPLE_ID / APPLE_TEAM_ID / APPLE_APP_PASSWORD not set"
    fi
fi
echo "wrote $dmg"

(cd "$work" && ditto -c -k --keepParent "$name" "$out/$name.zip")
echo "wrote $out/$name.zip"
