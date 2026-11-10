#!/usr/bin/env bash
# Builds a self-contained OpenPixels for 64-bit Linux into
# artifacts/OpenPixels-<version>-linux-x64.tar.gz. Run it on Linux: the Rust engine is
# built for the machine doing the build.
#
# The archive unpacks to OpenPixels/, which runs as ./OpenPixels. It needs a
# Vulkan driver for the GPU, and X11 or XWayland. openpixels.desktop and
# openpixels.png are included for adding it to a desktop menu.
set -euo pipefail
cd "$(dirname "$0")/.."

PROJECT=desktop/OpenPixels.Desktop
VERSION=$(sed -n 's:.*<Version>\(.*\)</Version>.*:\1:p' "$PROJECT/OpenPixels.Desktop.csproj")
OUT=artifacts/linux-x64
APP=$OUT/OpenPixels
ARCHIVE=artifacts/OpenPixels-$VERSION-linux-x64.tar.gz

rm -rf "$OUT" "$ARCHIVE"
dotnet publish "$PROJECT" -c Release -r linux-x64 --self-contained true -o "$APP"
# ONNX Runtime's package carries its Windows DLLs to every platform.
rm -f "$APP/onnxruntime.dll" "$APP/onnxruntime_providers_shared.dll"
cp desktop/packaging/linux/openpixels.desktop "$APP/"
cp "$PROJECT/Assets/app-icon-1024.png" "$APP/openpixels.png"
cp LICENSE "$APP/"
tar -czf "$ARCHIVE" -C "$OUT" OpenPixels
echo "$ARCHIVE ($(du -h "$ARCHIVE" | cut -f1))"
