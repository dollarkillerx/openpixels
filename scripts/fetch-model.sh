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

# The desktop edition's point and box masks: Meta's SAM 2.1 Hiera Small (Apache-2.0, code
# and weights), as exported to ONNX for Transformers.js by onnx-community. Each graph keeps
# its weights in a .onnx_data file beside it, under the name the graph refers to.
SAM=https://huggingface.co/onnx-community/sam2.1-hiera-small-ONNX/resolve/a7df49d8de14b9d2e4504d1687b0d568f905fd8d/onnx
fetch "$SAM/vision_encoder.onnx" \
  aacf1f7137bb6fffcf6bf166abcfabe28f57a76059254f3fb611c4a64a208119 \
  web/public/models/sam/vision_encoder.onnx
fetch "$SAM/vision_encoder.onnx_data" \
  260fd1f0a34e72a3dc79a739e563b4facc0ba75504818b433a1f808e66637456 \
  web/public/models/sam/vision_encoder.onnx_data
fetch "$SAM/prompt_encoder_mask_decoder.onnx" \
  079c59b261f723ff5c6a125e69b0170a957b21c58738c28d2b0394ecd0587d7f \
  web/public/models/sam/prompt_encoder_mask_decoder.onnx
fetch "$SAM/prompt_encoder_mask_decoder.onnx_data" \
  f9e59a584ab8ced21fa812c211bc01084204db1c9e92a5ef4fb3a49972b4e864 \
  web/public/models/sam/prompt_encoder_mask_decoder.onnx_data

# The desktop library's search and auto-tags: Google's SigLIP 2 base, patch 16 at 224
# (Apache-2.0), as exported to ONNX for Transformers.js by onnx-community, int8 weights,
# with the Gemma SentencePiece tokenizer it reads text with.
SIGLIP=https://huggingface.co/onnx-community/siglip2-base-patch16-224-ONNX/resolve/ba1f3b0843f24bc5417d38e19c37b287d719b2f4
fetch "$SIGLIP/onnx/vision_model_int8.onnx" \
  0dd31785a2713f1113ef2272472165c69d580473dae38d7b47568ac587795e70 \
  web/public/models/siglip/vision_model.onnx
fetch "$SIGLIP/onnx/text_model_int8.onnx" \
  3a0603d3a00c05a80a6ded4743c16aaac7b1e62cdcc7e362e7ce418659b96400 \
  web/public/models/siglip/text_model.onnx
fetch "$SIGLIP/tokenizer.model" \
  61a7b147390c64585d6c3543dd6fc636906c9af3865a5548f27f31aee1d4c8e2 \
  web/public/models/siglip/tokenizer.model
