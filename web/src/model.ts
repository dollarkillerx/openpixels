/** MODNet (Apache-2.0) preprocessing, from the model's preprocessor_config.json. */
export const MODEL = {
  url: "/models/modnet.onnx",
  shortestEdge: 512,
  divisibleBy: 32,
  mean: [0.5, 0.5, 0.5],
  std: [0.5, 0.5, 0.5],
};

/** Scales so the shortest edge matches the model, keeping aspect ratio. */
export function modelInputSize(width: number, height: number) {
  const scale = MODEL.shortestEdge / Math.min(width, height);
  const fit = (edge: number) =>
    Math.max(MODEL.divisibleBy, Math.round((edge * scale) / MODEL.divisibleBy) * MODEL.divisibleBy);
  return { width: fit(width), height: fit(height) };
}
