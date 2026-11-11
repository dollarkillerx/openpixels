#!/usr/bin/env bash
# Regenerates the screenshots in docs/screenshots from the CC0 photos in the git-ignored
# web/public/fixtures. The desktop ones come from the headless app; the Lite one needs
# the Lite dev server (pnpm --dir web dev:lite --port 5174) and Google Chrome.
set -euo pipefail
cd "$(dirname "$0")/.."
out=docs/screenshots
work="$(mktemp -d)"
OPENPIXELS_README_SHOTS="$work" dotnet test desktop/OpenPixels.slnx --filter "FullyQualifiedName~Readme_screenshots"
chrome="${CHROME:-/Applications/Google Chrome.app/Contents/MacOS/Google Chrome}"
if curl -sf http://localhost:5174/ >/dev/null && [ -x "$chrome" ]; then
  node scripts/web-screenshot.mjs "$chrome" "http://localhost:5174/?fixture=/fixtures/people/06-outdoor.jpg" "$work/web-lite.png" 1
else
  echo "skipping the Lite screenshot: start its dev server on port 5174 and install Chrome" >&2
fi
mkdir -p "$out"
for png in "$work"/*.png; do
  sips -s format jpeg -s formatOptions 88 "$png" --out "$out/$(basename "${png%.png}").jpg" >/dev/null
done
ls -l "$out"
