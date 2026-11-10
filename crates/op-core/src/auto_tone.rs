//! Automatic tone: reads what a photo's tones look like and suggests slider values.
//!
//! It works in steps because the sliders interact: the caller renders with neutral
//! sliders for [`auto_exposure`], again with that exposure for [`auto_finish`], and
//! once more for [`trim_exposure`]. Saturation is the user's to choose, so it is
//! neither measured away nor suggested.

use serde::Serialize;

/// The display tone a typical photo's middle should land on.
const TARGET_MIDDLE: f32 = 0.5;
/// A photo's middle this close to the target is left alone.
const EXPOSURE_DEAD_ZONE: f32 = 0.06;
/// How much of the way to the target to go when brightening, so a moody photo keeps
/// some of its mood.
const LIFT_FOLLOW: f32 = 0.85;
/// Darkening a photo that looks fine is the costlier mistake, so go less far.
const DARKEN_FOLLOW: f32 = 0.5;
/// How much a deliberately bright or dark scene keeps its own level.
const KEY_KEPT: f32 = 0.6;
const MAX_LIFT: f32 = 1.2;
const MAX_DARKEN: f32 = 1.0;
/// The last exposure correction only fine-tunes.
const MAX_TRIM: f32 = 0.2;
/// Exposure stops short of pushing most of the photo up to here.
const HIGH_TONE_LIMIT: f32 = 0.93;
const DISPLAY_GAMMA: f32 = 2.2;

const HIGHLIGHT_START: f32 = 0.8;
const MAX_HIGHLIGHT_PULL: f32 = 0.55;
const SHADOW_START: f32 = 0.22;
const MAX_SHADOW_LIFT: f32 = 0.45;
const WHITE_TARGET: f32 = 0.96;
const MAX_WHITES: f32 = 0.5;
const BLACK_TARGET: f32 = 0.03;
const CONTRAST_TARGET: f32 = 0.3;
const VIBRANCE_TARGET: f32 = 0.4;
const MAX_VIBRANCE: f32 = 0.3;

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize)]
pub struct AutoTone {
    pub exposure: f32,
    pub contrast: f32,
    pub highlights: f32,
    pub shadows: f32,
    pub whites: f32,
    pub blacks: f32,
    pub vibrance: f32,
}

impl AutoTone {
    /// Rounded to the steps the sliders move in.
    pub fn rounded(self) -> Self {
        let round = |value: f32| (value * 100.0).round() / 100.0;
        Self {
            exposure: round(self.exposure),
            contrast: round(self.contrast),
            highlights: round(self.highlights),
            shadows: round(self.shadows),
            whites: round(self.whites),
            blacks: round(self.blacks),
            vibrance: round(self.vibrance),
        }
    }
}

/// Where a rendered photo's tones sit.
pub struct ToneStats {
    histogram: [u32; 256],
    total: u32,
    mean_saturation: f32,
}

impl ToneStats {
    pub fn from_rgb8(rgb: &[u8]) -> Self {
        let mut histogram = [0; 256];
        let mut saturation_sum = 0.0;
        let mut total = 0;
        for pixel in rgb.chunks_exact(3) {
            let [r, g, b] = [f32::from(pixel[0]), f32::from(pixel[1]), f32::from(pixel[2])];
            histogram[(0.2126 * r + 0.7152 * g + 0.0722 * b).round().min(255.0) as usize] += 1;
            let peak = r.max(g).max(b);
            if peak > 0.0 {
                saturation_sum += (peak - r.min(g).min(b)) / peak;
            }
            total += 1;
        }
        Self { histogram, total: total.max(1), mean_saturation: saturation_sum / total.max(1) as f32 }
    }

    /// The tone in 0..=1 that `share` of the pixels are at or below.
    fn percentile(&self, share: f32) -> f32 {
        let wanted = (share * self.total as f32).ceil() as u32;
        let mut seen = 0;
        for (level, &count) in self.histogram.iter().enumerate() {
            seen += count;
            if seen >= wanted {
                return level as f32 / 255.0;
            }
        }
        1.0
    }
}

fn to_linear(tone: f32) -> f32 {
    tone.max(0.01).powf(DISPLAY_GAMMA)
}

fn lerp(from: f32, to: f32, amount: f32) -> f32 {
    from + (to - from) * amount
}

/// The exposure, in stops, that brings the photo's middle to a good level.
pub fn auto_exposure(stats: &ToneStats) -> f32 {
    let (p10, p25, p50, p75, p90, p95) = (
        stats.percentile(0.1),
        stats.percentile(0.25),
        stats.percentile(0.5),
        stats.percentile(0.75),
        stats.percentile(0.9),
        stats.percentile(0.95),
    );
    let middle = 0.5 * p50 + 0.25 * (p25 + p75);
    let high_key = p10 > 0.25 && p50 > 0.55;
    let low_key = p50 < 0.22 && p95 > 0.6;
    let target = if high_key || low_key { lerp(TARGET_MIDDLE, middle, KEY_KEPT) } else { TARGET_MIDDLE };
    if (target - middle).abs() < EXPOSURE_DEAD_ZONE {
        return 0.0;
    }
    let stops = (to_linear(target) / to_linear(middle)).log2();
    if stops < 0.0 {
        return (stops * DARKEN_FOLLOW).max(-MAX_DARKEN);
    }
    let room = (to_linear(HIGH_TONE_LIMIT) / to_linear(p90)).log2().max(0.0);
    (stops * LIFT_FOLLOW).min(room).min(MAX_LIFT)
}

/// The remaining sliders, from a render that already has `exposure` applied.
pub fn auto_finish(stats: &ToneStats, exposure: f32) -> AutoTone {
    let (p005, p10, p25, p75, p95, p995) = (
        stats.percentile(0.005),
        stats.percentile(0.1),
        stats.percentile(0.25),
        stats.percentile(0.75),
        stats.percentile(0.95),
        stats.percentile(0.995),
    );
    AutoTone {
        exposure,
        contrast: ((CONTRAST_TARGET - (p75 - p25)) * 0.9).clamp(-0.15, 0.25),
        highlights: -(((p95 - HIGHLIGHT_START) / 0.15).clamp(0.0, 1.0) * MAX_HIGHLIGHT_PULL),
        shadows: ((SHADOW_START - p10) / 0.18).clamp(0.0, 1.0) * MAX_SHADOW_LIFT,
        whites: ((WHITE_TARGET - p995) / 0.18).clamp(-MAX_WHITES, MAX_WHITES),
        blacks: ((BLACK_TARGET - p005) / 0.12).clamp(-0.6, 0.4),
        vibrance: ((VIBRANCE_TARGET - stats.mean_saturation) * 0.7).clamp(0.0, MAX_VIBRANCE),
    }
}

/// `exposure` fine-tuned from a render with every suggestion applied, still within the
/// limits [`auto_exposure`] keeps to.
pub fn trim_exposure(exposure: f32, stats: &ToneStats) -> f32 {
    (exposure + auto_exposure(stats).clamp(-MAX_TRIM, MAX_TRIM)).clamp(-MAX_DARKEN, MAX_LIFT)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A photo whose tones spread evenly from `low` to `high`, greyscale.
    fn spread(low: u8, high: u8) -> ToneStats {
        let rgb: Vec<u8> = (0..=1000u32)
            .flat_map(|i| {
                let v = (f32::from(low) + (f32::from(high) - f32::from(low)) * i as f32 / 1000.0).round() as u8;
                [v, v, v]
            })
            .collect();
        ToneStats::from_rgb8(&rgb)
    }

    #[test]
    fn a_dark_photo_is_brightened_and_a_bright_one_darkened() {
        assert!(auto_exposure(&spread(5, 90)) > 0.8);
        assert!(auto_exposure(&spread(150, 250)) < -0.1);
    }

    #[test]
    fn a_well_exposed_photo_is_left_where_it_is() {
        assert_eq!(auto_exposure(&spread(10, 225)), 0.0);
    }

    #[test]
    fn neither_direction_goes_too_far() {
        assert!(auto_exposure(&spread(0, 8)) <= MAX_LIFT);
        assert!(auto_exposure(&spread(240, 255)) >= -MAX_DARKEN);
    }

    #[test]
    fn exposure_does_not_blow_out_a_photo_with_bright_highlights() {
        let mut rgb: Vec<u8> = vec![30; 3 * 800];
        rgb.extend(vec![235; 3 * 200]);
        let exposure = auto_exposure(&ToneStats::from_rgb8(&rgb));
        assert!(exposure < 0.2, "{exposure}");
    }

    #[test]
    fn a_flat_photo_gets_contrast_and_a_punchy_one_does_not() {
        assert!(auto_finish(&spread(100, 150), 0.0).contrast > 0.1);
        assert!(auto_finish(&spread(0, 255), 0.0).contrast <= 0.0);
    }

    #[test]
    fn hot_highlights_are_pulled_and_deep_shadows_are_lifted() {
        let tone = auto_finish(&spread(0, 255), 0.0);
        assert!(tone.highlights < -0.3, "{}", tone.highlights);
        assert!(tone.shadows > 0.0, "{}", tone.shadows);
    }

    #[test]
    fn a_range_short_of_white_and_black_is_stretched() {
        let tone = auto_finish(&spread(40, 200), 0.0);
        assert!(tone.whites > 0.3, "{}", tone.whites);
        assert!(tone.blacks < -0.3, "{}", tone.blacks);
    }

    #[test]
    fn muted_colours_get_vibrance_and_vivid_ones_do_not() {
        let vivid: Vec<u8> = [240u8, 20, 20].repeat(100);
        assert_eq!(auto_finish(&ToneStats::from_rgb8(&vivid), 0.0).vibrance, 0.0);
        assert!(auto_finish(&spread(20, 220), 0.0).vibrance > 0.2);
    }

    #[test]
    fn every_suggestion_stays_within_slider_range() {
        for (low, high) in [(0, 0), (255, 255), (0, 255), (128, 128), (1, 2)] {
            let stats = spread(low, high);
            let tone = auto_finish(&stats, auto_exposure(&stats));
            for value in [tone.exposure, tone.contrast, tone.highlights, tone.shadows, tone.whites, tone.blacks, tone.vibrance] {
                assert!(value.is_finite() && value.abs() <= 2.0, "{value} for {low}..{high}");
            }
        }
    }

    #[test]
    fn trimming_never_passes_the_limits() {
        let dark = spread(0, 30);
        assert_eq!(trim_exposure(MAX_LIFT, &dark), MAX_LIFT);
        assert_eq!(trim_exposure(-MAX_DARKEN, &spread(245, 255)), -MAX_DARKEN);
        let trimmed = trim_exposure(0.5, &dark);
        assert!(trimmed > 0.5 && trimmed <= 0.5 + MAX_TRIM, "{trimmed}");
    }

    #[test]
    fn rounding_matches_the_slider_step() {
        let tone = AutoTone { exposure: 0.1234, whites: -0.456, ..Default::default() }.rounded();
        assert_eq!((tone.exposure, tone.whites), (0.12, -0.46));
    }
}
