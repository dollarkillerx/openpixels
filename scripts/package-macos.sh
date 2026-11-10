#!/usr/bin/env bash
# Builds OpenPixels.app and a .dmg for this Mac's architecture (or the one given:
# arm64 or x64) into artifacts/.
#
# Signing: ad hoc by default, which runs on this Mac. For a build other people can
# open without warnings, set OPENPIXELS_SIGN_IDENTITY to a "Developer ID Application"
# certificate in the keychain, and OPENPIXELS_NOTARY_PROFILE to a profile saved with
# `xcrun notarytool store-credentials`, and the .dmg is notarised and stapled too.
#
# Needs: .NET 10 SDK, Rust, and the models and lens database fetched by
# scripts/fetch-model.sh and scripts/fetch-lensfun.sh (the app works without them,
# minus AI tools and automatic lens correction).
set -euo pipefail
cd "$(dirname "$0")/.."

case "${1:-$(uname -m)}" in
  arm64 | aarch64) ARCH=arm64 ;;
  x64 | x86_64) ARCH=x64 ;;
  *) echo "unknown architecture: $1" >&2; exit 1 ;;
esac
if [ "$ARCH" = arm64 ] && [ "$(uname -m)" != arm64 ] || [ "$ARCH" = x64 ] && [ "$(uname -m)" != x86_64 ]; then
  # The Rust engine is built for the machine doing the build.
  echo "build the $ARCH package on a $ARCH Mac" >&2
  exit 1
fi

PROJECT=desktop/OpenPixels.Desktop
VERSION=$(sed -n 's:.*<Version>\(.*\)</Version>.*:\1:p' "$PROJECT/OpenPixels.Desktop.csproj")
OUT=artifacts/macos-$ARCH
APP=$OUT/OpenPixels.app
DMG=artifacts/OpenPixels-$VERSION-macos-$ARCH.dmg
IDENTITY=${OPENPIXELS_SIGN_IDENTITY:--}

rm -rf "$OUT" "$DMG"
dotnet publish "$PROJECT" -c Release -r "osx-$ARCH" --self-contained true -p:UseAppHost=true -o "$OUT/publish"
# ONNX Runtime's package carries its Windows DLLs to every platform.
rm -f "$OUT/publish/onnxruntime.dll" "$OUT/publish/onnxruntime_providers_shared.dll"

mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp -R "$OUT/publish/." "$APP/Contents/MacOS/"
sed "s/__VERSION__/$VERSION/g" desktop/packaging/macos/Info.plist > "$APP/Contents/Info.plist"

ICONSET=$OUT/AppIcon.iconset
mkdir -p "$ICONSET"
for size in 16 32 128 256 512; do
  sips -z $size $size "$PROJECT/Assets/app-icon-1024.png" --out "$ICONSET/icon_${size}x${size}.png" >/dev/null
  sips -z $((size * 2)) $((size * 2)) "$PROJECT/Assets/app-icon-1024.png" --out "$ICONSET/icon_${size}x${size}@2x.png" >/dev/null
done
iconutil -c icns "$ICONSET" -o "$APP/Contents/Resources/AppIcon.icns"

if [ "$IDENTITY" = "-" ]; then
  codesign --force --deep --sign - "$APP"
else
  # Libraries first, then the app with the hardened runtime notarisation requires.
  find "$APP/Contents/MacOS" -type f \( -name '*.dylib' -o -perm -u+x \) -print0 |
    xargs -0 codesign --force --timestamp --options runtime --entitlements desktop/packaging/macos/entitlements.plist --sign "$IDENTITY"
  codesign --force --timestamp --options runtime --entitlements desktop/packaging/macos/entitlements.plist --sign "$IDENTITY" "$APP"
fi
codesign --verify --deep --strict "$APP"

STAGING=$OUT/dmg
mkdir -p "$STAGING"
cp -R "$APP" "$STAGING/"
ln -s /Applications "$STAGING/Applications"
hdiutil create -volname "OpenPixels $VERSION" -srcfolder "$STAGING" -ov -format UDZO "$DMG" >/dev/null

if [ "$IDENTITY" != "-" ]; then
  codesign --force --timestamp --sign "$IDENTITY" "$DMG"
  if [ -n "${OPENPIXELS_NOTARY_PROFILE:-}" ]; then
    xcrun notarytool submit "$DMG" --keychain-profile "$OPENPIXELS_NOTARY_PROFILE" --wait
    xcrun stapler staple "$DMG"
  fi
fi

echo "$APP"
echo "$DMG ($(du -h "$DMG" | cut -f1))"
