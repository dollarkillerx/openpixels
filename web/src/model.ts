/** MODNet (Apache-2.0) preprocessing, from the model's preprocessor_config.json. */
export const MODEL = {
  url: "/models/modnet.onnx",
  shortestEdge: 512,
  divisibleBy: 32,
  mean: [0.5, 0.5, 0.5],
  std: [0.5, 0.5, 0.5],
};

/** SkyWater-Seg (MIT), which labels background, sky, water and person. */
export const SKY_MODEL = {
  url: "/models/skywater.onnx",
  // Above the 384 it was trained at, for finer edges around branches and rooftops.
  size: 640,
  mean: [0.485, 0.456, 0.406],
  std: [0.229, 0.224, 0.225],
  classes: 4,
  skyClass: 1,
};

/** LaMa inpainting (Apache-2.0). This export only accepts one input size. */
export const INPAINT_MODEL = {
  url: "/models/lama.onnx",
  size: 512,
};

/** Scales so the shortest edge matches the model, keeping aspect ratio. */
export function modelInputSize(width: number, height: number) {
  const scale = MODEL.shortestEdge / Math.min(width, height);
  const fit = (edge: number) =>
    Math.max(MODEL.divisibleBy, Math.round((edge * scale) / MODEL.divisibleBy) * MODEL.divisibleBy);
  return { width: fit(width), height: fit(height) };
}
