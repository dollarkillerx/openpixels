#!/usr/bin/env bash
# Downloads the models used in the browser into web/public/models, verified by
# checksum. Re-running resumes a partial download.
set -euo pipefail
cd "$(dirname "$0")/.."

fetch() {
  local url=$1 sha256=$2 output=$3
  mkdir -p "$(dirname "$output")"
  if [ -f "$output" ] && [ "$(shasum -a 256 "$output" | cut -d' ' -f1)" = "$sha256" ]; then
    echo "already downloaded: $output"
    return
  fi
  curl -sS -L --fail -C - --retry 5 --retry-delay 3 -A 'openpixels-model-fetch' -o "$output" "$url"
  local actual
  actual="$(shasum -a 256 "$output" | cut -d' ' -f1)"
  if [ "$actual" != "$sha256" ]; then
    echo "checksum mismatch for $output: expected $sha256, got $actual" >&2
    exit 1
  fi
  echo "ok: $output"
}

# MODNet portrait matting (Apache-2.0).
fetch "https://huggingface.co/Xenova/modnet/resolve/fa2fa546052fba4c08921230a26cc69a333fca12/onnx/model.onnx" \
  07c308cf0fc7e6e8b2065a12ed7fc07e1de8febb7dc7839d7b7f15dd66584df9 \
  web/public/models/modnet.onnx

# MediaPipe Face Landmarker (Apache-2.0).
fetch "https://storage.googleapis.com/mediapipe-models/face_landmarker/face_landmarker/float16/1/face_landmarker.task" \
  64184e229b263107bc2b804c6625db1341ff2bb731874b0bcc2fe6544e0bc9ff \
  web/public/models/face_landmarker.task
