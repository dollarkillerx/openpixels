#!/usr/bin/env bash
# Downloads MODNet (Apache-2.0) into web/public/models, pinned to a revision and
# verified by checksum. Re-running resumes a partial download.
set -euo pipefail
cd "$(dirname "$0")/.."

REPO=Xenova/modnet
REVISION=fa2fa546052fba4c08921230a26cc69a333fca12
SHA256=07c308cf0fc7e6e8b2065a12ed7fc07e1de8febb7dc7839d7b7f15dd66584df9
OUTPUT=web/public/models/modnet.onnx

mkdir -p "$(dirname "$OUTPUT")"
if [ -f "$OUTPUT" ] && [ "$(shasum -a 256 "$OUTPUT" | cut -d' ' -f1)" = "$SHA256" ]; then
  echo "already downloaded: $OUTPUT"
  exit 0
fi

curl -sS -L --fail -C - --retry 5 --retry-delay 3 -A 'openpixels-model-fetch' -o "$OUTPUT" \
  "https://huggingface.co/$REPO/resolve/$REVISION/onnx/model.onnx"

ACTUAL="$(shasum -a 256 "$OUTPUT" | cut -d' ' -f1)"
if [ "$ACTUAL" != "$SHA256" ]; then
  echo "checksum mismatch: expected $SHA256, got $ACTUAL" >&2
  exit 1
fi
echo "ok: $OUTPUT"
