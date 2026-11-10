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
    /// Fine local contrast, -1..=1.
    pub texture: f32,
    /// Haze removal when positive, added haze when negative.
    pub dehaze: f32,
    /// Film grain strength, 0..=1.
    pub grain: f32,
    /// Film grain coarseness, 0..=1.
    pub grain_size: f32,
    /// Keystone correction, vertical then horizontal, each -1..=1.
    pub perspective: [f32; 2],
    /// Non-zero zooms out to show the whole photo after a correction, leaving black
    /// edges, rather than zooming in to hide them.
    pub perspective_keep: f32,
    #[serde(skip)]
    pub _padding: f32,
    /// Per local layer: lightest-to-darkest limits, then hue in degrees and hue half
    /// width. A zero upper limit means the layer has no range.
    pub local_range: [[f32; 4]; 4],
    /// How much of the background to replace; 0 is off.
    pub replace_background: f32,
    /// Brightness of the new background, in EV.
    pub replace_background_exposure: f32,
    /// Moves the new background up (negative) or down (positive), as a fraction of its height.
    pub replace_background_shift: f32,
    pub _replace_padding: f32,
    /// How much of the sky to replace; 0 is off.
    pub replace_sky: f32,
    pub replace_sky_exposure: f32,
    pub replace_sky_shift: f32,
    /// How far the rest of the photo takes on the new sky's colour.
    pub replace_sky_relight: f32,
    /// Sharpening radius in pixels, 1..=3; below 1 means 1.
    pub sharpen_radius: f32,
    /// 0 sharpens at the radius; 1 at the finest detail.
    pub sharpen_detail: f32,
    /// How much low-contrast areas are left unsharpened, 0..=1.
    pub sharpen_masking: f32,
    /// Darkens (negative) or lightens the corners of the cropped frame.
    pub post_crop_vignette: f32,
    /// Hue and saturation of the red, green and blue primaries, each -1..=1.
    pub calibration_red_hue: f32,
    pub calibration_red_sat: f32,
    pub calibration_green_hue: f32,
    pub calibration_green_sat: f32,
    pub calibration_blue_hue: f32,
    pub calibration_blue_sat: f32,
    #[serde(skip)]
    pub _calibration_padding: [f32; 2],
    /// A picked colour's hue in degrees, and how far around it to reach; 0 is off.
    pub point_hue: f32,
    pub point_width: f32,
    pub point_hue_shift: f32,
    pub point_sat_shift: f32,
    pub point_lum_shift: f32,
    #[serde(skip)]
    pub _point_padding: [f32; 3],
}

/// The white balance sliders' travel either way.
const WHITE_BALANCE_RANGE: f32 = 2.0;

/// Temperature and tint that make a scene-linear colour neutral, or `None` if it is
/// too dark or clipped to say anything about the light.
pub fn neutral_white_balance(rgb: [f32; 3]) -> Option<(f32, f32)> {
    const DARKEST: f32 = 1e-3;
    const BRIGHTEST: f32 = 0.98;
    if rgb.iter().any(|&channel| !(DARKEST..=BRIGHTEST).contains(&channel)) {
        return None;
    }
    let [red, green, blue] = rgb.map(f32::log2);
    let temperature = blue - red;
    let tint = 4.0 / 3.0 * (green - (red + blue) / 2.0);
    Some((temperature.clamp(-WHITE_BALANCE_RANGE, WHITE_BALANCE_RANGE), tint.clamp(-WHITE_BALANCE_RANGE, WHITE_BALANCE_RANGE)))
}

impl Adjustments {
    /// The colour matrix for the calibration sliders, as rows. Greys stay grey.
    pub fn calibration_matrix(&self) -> [[f32; 3]; 3] {
        const HUE_REACH: f32 = 0.25;
        const SATURATION_REACH: f32 = 0.2;
        let primaries = [
            (self.calibration_red_hue, self.calibration_red_sat),
            (self.calibration_green_hue, self.calibration_green_sat),
            (self.calibration_blue_hue, self.calibration_blue_sat),
        ];
        let mut matrix = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
        for (primary, (hue, saturation)) in primaries.into_iter().enumerate() {
            // Positive hue moves red towards green, green towards blue, blue towards red.
            let (next, previous) = ((primary + 1) % 3, (primary + 2) % 3);
            let target = if hue >= 0.0 { next } else { previous };
            matrix[target][primary] += hue.abs() * HUE_REACH;
            matrix[primary][primary] += saturation * SATURATION_REACH;
            matrix[next][primary] -= saturation * SATURATION_REACH / 2.0;
            matrix[previous][primary] -= saturation * SATURATION_REACH / 2.0;
        }
        matrix.map(|row| {
            let total: f32 = row.iter().sum();
            row.map(|value| value / total)
        })
    }
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
    fn neutral_calibration_changes_nothing_and_any_calibration_keeps_greys() {
        let identity = Adjustments::default().calibration_matrix();
        assert_eq!(identity, [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]);
        let adj = Adjustments { calibration_red_hue: 0.8, calibration_green_sat: -0.6, calibration_blue_hue: -0.4, ..Default::default() };
        for row in adj.calibration_matrix() {
            assert!((row.iter().sum::<f32>() - 1.0).abs() < 1e-5, "{row:?}");
        }
    }

    #[test]
    fn a_positive_red_hue_moves_red_towards_green() {
        let matrix = Adjustments { calibration_red_hue: 1.0, ..Default::default() }.calibration_matrix();
        assert!(matrix[1][0] > 0.1, "{matrix:?}");
        assert_eq!(matrix[2][0], 0.0);
    }

    #[test]
    fn gpu_layout_is_tightly_packed() {
        assert_eq!(std::mem::size_of::<Adjustments>(), 20 * 4 + 6 * 16 + 4 * 16 + 3 * 16 + 16 + 16 + 16 + 16 + 4 * 16 + 32 + 80);
    }

    #[test]
    fn the_neutral_balance_cancels_a_colour_cast() {
        let rgb = [0.3f32, 0.25, 0.2];
        let (t, tint) = neutral_white_balance(rgb).unwrap();
        // The gains of `white_balance` in develop.wgsl.
        let gain = [(t * 0.5 + tint * 0.25).exp2(), (-tint * 0.5).exp2(), (-t * 0.5 + tint * 0.25).exp2()];
        let out = [rgb[0] * gain[0], rgb[1] * gain[1], rgb[2] * gain[2]];
        assert!((out[0] - out[1]).abs() < 1e-3 && (out[1] - out[2]).abs() < 1e-3, "{out:?}");
    }

    #[test]
    fn clipped_or_black_colours_cannot_set_the_balance() {
        assert_eq!(neutral_white_balance([1.0, 0.5, 0.5]), None);
        assert_eq!(neutral_white_balance([0.0, 0.5, 0.5]), None);
    }
}
