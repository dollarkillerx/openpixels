use bytemuck::{Pod, Zeroable};
use serde::{Deserialize, Serialize};

/// Global develop parameters. Uploaded to the GPU as-is, so the field order must
/// match `Adjustments` in `op-gpu/src/develop.wgsl`.
///
/// `exposure` is in EV. Everything else is normalised to -1..=1, with 0 meaning
/// "no change" (`vignette` is negative for a darker edge).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Adjustments {
    pub temperature: f32,
    pub tint: f32,
    pub exposure: f32,
    pub contrast: f32,
    pub highlights: f32,
    pub shadows: f32,
    pub whites: f32,
    pub blacks: f32,
    pub vibrance: f32,
    pub saturation: f32,
    pub vignette: f32,
    /// EV applied only outside the subject mask.
    pub background_exposure: f32,
    /// 0..=1 opacity of the on-screen mask tint. Never included in exports.
    pub mask_overlay: f32,
    /// Large-radius local contrast.
    pub clarity: f32,
    /// Small-radius edge contrast, 0..=1.
    pub sharpen: f32,
    /// Strength of mid-frequency skin smoothing, 0..=1.
    pub skin_smooth: f32,
    /// Strength of blemish removal, 0..=1.
    pub skin_blemish: f32,
    /// Luminance noise reduction, 0..=1.
    pub denoise_luma: f32,
    /// Colour noise reduction, 0..=1.
    pub denoise_chroma: f32,
    /// Set by the engine: non-zero when a tone curve is in effect.
    #[serde(skip)]
    pub curve_active: f32,
    /// Hue, saturation and luminance shift per colour band (red, orange, yellow,
    /// green, aqua, blue, purple, magenta), in two groups of four for the GPU.
    pub hsl_hue: [[f32; 4]; 2],
    pub hsl_saturation: [[f32; 4]; 2],
    pub hsl_luminance: [[f32; 4]; 2],
    /// Per local layer: exposure in EV, contrast, saturation and temperature.
    pub local: [[f32; 4]; 4],
    /// Colour toning for shadows, midtones and highlights: hue in degrees, then amount.
    pub grade: [[f32; 4]; 3],
    /// 1-based local layer whose mask is tinted on screen, or 0. Never exported.
    pub local_overlay: f32,
    /// Blend of the 3D lookup table, 0..=1.
    pub lut_amount: f32,
    /// Blend towards the colour matrix fitted to the camera's own JPEG, 0..=1.
    pub camera_profile: f32,
    /// Non-zero to apply the lens corrections stored in the file.
    pub lens_auto: f32,
    /// Manual distortion, vignetting, red fringe and blue fringe, each -1..=1.
    pub lens_manual: [f32; 4],
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_fields_default_to_neutral() {
        let adj: Adjustments = serde_json::from_str(r#"{"exposure": 1.5}"#).unwrap();
        assert_eq!(adj, Adjustments { exposure: 1.5, ..Default::default() });
    }

    #[test]
    fn gpu_layout_is_tightly_packed() {
        assert_eq!(std::mem::size_of::<Adjustments>(), 20 * 4 + 6 * 16 + 4 * 16 + 3 * 16 + 16 + 16);
    }
}
