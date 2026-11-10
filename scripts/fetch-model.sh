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

# LaMa inpainting, OpenCV's export (Apache-2.0).
fetch "https://huggingface.co/opencv/inpainting_lama/resolve/aee6d22f0a13e5e35af1c9a1c3afd62841fc6f3f/inpainting_lama_2025jan.onnx" \
  7df918ac3921d3daf0aae1d219776cf0dc4e4935f035af81841b40adcf74fdf2 \
  web/public/models/lama.onnx

# Sky, water and person segmentation (MIT; trained on ADE20K).
fetch "https://huggingface.co/Realcat/skywater_seg/resolve/dac255883ec5faf508561a47172096bfd8708db0/skywater_segformer_b2_fp16.onnx" \
  a4f518e6ace87b3ee473ea2578d00f0fb5f7bf8e4e6a3a49ae368c048c4e47c3 \
  web/public/models/skywater.onnx

# The desktop edition's face finder: MediaPipe's BlazeFace short-range detector and Face
# Mesh V2 landmarks from the Face Landmarker bundle above, converted to ONNX
# (Apache-2.0, code and weights) by the QtMeshEditor project with a parity check.
fetch "https://huggingface.co/fernandotonon/QtMeshEditor-blazeface-onnx/resolve/50f2c66ffbdf84beae8c267df2b49e5c5a5162e9/face_detector.onnx" \
  02a04d5d37c3558dc4d5274f7f8f0f0f01ac94e46c5ffb2cee82395d47e23181 \
  web/public/models/face_detector.onnx

fetch "https://huggingface.co/fernandotonon/QtMeshEditor-facemesh-onnx/resolve/32714088ed9830a98df6ee653ad406a3b83ef014/face_landmarks.onnx" \
  d16e5a55e6a480284d468ee32469692464049518c311662ab3956681de31e3e9 \
  web/public/models/face_landmarks.onnx
