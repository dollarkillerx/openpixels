//! The edit model agents work with: the same flat `key: number` map the desktop and web
//! editions store, so an edit made here means the same there. This module knows each
//! key's range and turns an edit into the engine's JSON, as `Edits.ToEngineJson` does.

use std::collections::BTreeMap;

use serde_json::{Map, Value, json};

pub type Edit = BTreeMap<String, f64>;

/// One setting an agent can change.
pub struct Setting {
    pub key: String,
    pub group: &'static str,
    pub min: f64,
    pub max: f64,
    /// The value that changes nothing.
    pub neutral: f64,
    pub whole: bool,
    pub about: String,
}

fn setting(key: &str, group: &'static str, min: f64, max: f64, about: &str) -> Setting {
    Setting { key: key.into(), group, min, max, neutral: 0.0, whole: false, about: about.into() }
}

/// Colour bands of the HSL sliders, by index.
pub const HSL_BANDS: [&str; 8] = ["red", "orange", "yellow", "green", "aqua", "blue", "purple", "magenta"];
const CURVE_CHANNELS: [(&str, &str); 4] = [("M", "master"), ("R", "red"), ("G", "green"), ("B", "blue")];
const GRADE_RANGES: [(&str, &str); 3] = [("Shadow", "shadows"), ("Mid", "midtones"), ("High", "highlights")];
const MAX_CURVE_POINTS: usize = 16;

/// Every setting, in the order a photo editor's panels show them.
pub fn catalogue() -> Vec<Setting> {
    let mut all = vec![
        setting("temperature", "white balance", -2.0, 2.0, "Negative is cooler (bluer), positive warmer (yellower)."),
        setting("tint", "white balance", -2.0, 2.0, "Negative is greener, positive more magenta."),
        setting("exposure", "tone", -4.0, 4.0, "Brightness in stops (EV)."),
        setting("contrast", "tone", -1.0, 1.0, ""),
        setting("highlights", "tone", -1.0, 1.0, "Negative recovers bright areas."),
        setting("shadows", "tone", -1.0, 1.0, "Positive opens up dark areas."),
        setting("whites", "tone", -1.0, 1.0, "Sets the white point."),
        setting("blacks", "tone", -1.0, 1.0, "Sets the black point."),
        setting("curveHighlights", "tone curve", -1.0, 1.0, "Parametric curve region around 80% brightness."),
        setting("curveLights", "tone curve", -1.0, 1.0, "Parametric curve region around 60%."),
        setting("curveDarks", "tone curve", -1.0, 1.0, "Parametric curve region around 40%."),
        setting("curveShadows", "tone curve", -1.0, 1.0, "Parametric curve region around 20%."),
        setting("vibrance", "presence", -1.0, 1.0, "Saturation that spares already saturated colours and skin."),
        setting("saturation", "presence", -1.0, 1.0, ""),
        setting("dehaze", "presence", -1.0, 1.0, "Positive removes haze, negative adds it."),
        setting("clarity", "presence", -1.0, 1.0, "Large-radius local contrast."),
        setting("texture", "presence", -1.0, 1.0, "Fine local contrast."),
        setting("sharpen", "detail", 0.0, 1.0, "Sharpening amount."),
        Setting { neutral: 1.0, ..setting("sharpenRadius", "detail", 1.0, 3.0, "Sharpening radius in pixels.") },
        setting("sharpenDetail", "detail", 0.0, 1.0, "0 sharpens at the radius, 1 at the finest detail."),
        setting("sharpenMasking", "detail", 0.0, 1.0, "Leaves smooth areas unsharpened."),
        setting("denoiseLuma", "detail", 0.0, 1.0, "Luminance noise reduction."),
        setting("denoiseChroma", "detail", 0.0, 1.0, "Colour noise reduction."),
        setting("vignette", "effects", -1.0, 1.0, "Negative darkens the edges of the whole photo."),
        setting("postCropVignette", "effects", -1.0, 1.0, "Negative darkens the corners of the cropped frame."),
        setting("grain", "effects", 0.0, 1.0, "Film grain strength."),
        setting("grainSize", "effects", 0.0, 1.0, "Film grain coarseness."),
    ];
    for (index, band) in HSL_BANDS.iter().enumerate() {
        for (prefix, what) in [("hslHue", "hue shift"), ("hslSat", "saturation"), ("hslLum", "luminance")] {
            all.push(setting(&format!("{prefix}{index}"), "colour mixer (HSL)", -1.0, 1.0, &format!("{band} {what}.")));
        }
    }
    for (range, name) in GRADE_RANGES {
        all.push(setting(&format!("grade{range}Hue"), "colour grading", 0.0, 360.0, &format!("Hue in degrees of the tint added to the {name}.")));
        all.push(setting(&format!("grade{range}Amount"), "colour grading", 0.0, 1.0, &format!("Strength of the {name} tint.")));
    }
    all.extend([
        setting("pointHue", "point colour", 0.0, 360.0, "Hue in degrees of the colour to change; 0 with pointWidth unset is off."),
        Setting { neutral: 30.0, ..setting("pointWidth", "point colour", 10.0, 90.0, "How far around pointHue to reach, in degrees.") },
        setting("pointHueShift", "point colour", -1.0, 1.0, ""),
        setting("pointSatShift", "point colour", -1.0, 1.0, ""),
        setting("pointLumShift", "point colour", -1.0, 1.0, ""),
        setting("calibrationRedHue", "calibration", -1.0, 1.0, "Red primary hue."),
        setting("calibrationRedSat", "calibration", -1.0, 1.0, "Red primary saturation."),
        setting("calibrationGreenHue", "calibration", -1.0, 1.0, ""),
        setting("calibrationGreenSat", "calibration", -1.0, 1.0, ""),
        setting("calibrationBlueHue", "calibration", -1.0, 1.0, ""),
        setting("calibrationBlueSat", "calibration", -1.0, 1.0, ""),
        Setting { whole: true, ..setting("cameraProfile", "profile", 0.0, 1.0, "1 uses the colour fitted to the camera's own JPEG (RAW files the engine knows).") },
        Setting { whole: true, ..setting("displayTransform", "profile", 0.0, 2.0, "How a RAW's highlights are rendered: 0 standard, 1 AgX (bright saturated colours fade to white), 2 neutral (midtones untouched, only highlights compressed). No effect on JPEG, PNG or TIFF.") },
        Setting { whole: true, ..setting("lensAuto", "lens", 0.0, 1.0, "1 applies the lens corrections stored in the file.") },
        setting("lensDistortion", "lens", -1.0, 1.0, "Manual distortion correction."),
        setting("lensVignette", "lens", -1.0, 1.0, "Manual lens vignetting correction."),
        setting("lensCaRed", "lens", -1.0, 1.0, "Red/cyan fringe."),
        setting("lensCaBlue", "lens", -1.0, 1.0, "Blue/yellow fringe."),
        setting("perspectiveV", "perspective", -1.0, 1.0, "Vertical keystone."),
        setting("perspectiveH", "perspective", -1.0, 1.0, "Horizontal keystone."),
        Setting { whole: true, ..setting("perspectiveKeep", "perspective", 0.0, 1.0, "1 shows the whole corrected photo with empty edges rather than zooming in.") },
        setting("cropX", "framing", 0.0, 1.0, "Left edge of the crop, as a fraction of the rotated frame's width."),
        setting("cropY", "framing", 0.0, 1.0, "Top edge of the crop, as a fraction of the height."),
        Setting { neutral: 1.0, ..setting("cropW", "framing", 0.01, 1.0, "Crop width, as a fraction of the frame.") },
        Setting { neutral: 1.0, ..setting("cropH", "framing", 0.01, 1.0, "Crop height, as a fraction of the frame.") },
        setting("rotate", "framing", -45.0, 45.0, "Straighten, in degrees; positive turns clockwise."),
        Setting { whole: true, ..setting("quarter", "framing", -3.0, 3.0, "Quarter turns clockwise.") },
    ]);
    all
}

/// Framing belongs to one photo, so batches and presets leave it alone, as the other editions do.
pub fn belongs_to_one_photo(key: &str) -> bool {
    (key.starts_with("crop") && key.len() == 5) || key == "quarter" || key == "rotate"
}

/// True for the keys that hold tone curve points: `curveMN`, `curveMX0`, `curveMY0`, ….
fn is_curve_point_key(key: &str) -> Option<&str> {
    let rest = key.strip_prefix("curve")?;
    let channel = CURVE_CHANNELS.iter().find(|(code, _)| rest.starts_with(code))?.0;
    let tail = &rest[1..];
    let valid = tail == "N" || (tail.len() > 1 && (tail.starts_with('X') || tail.starts_with('Y')) && tail[1..].chars().all(|c| c.is_ascii_digit()));
    valid.then_some(channel)
}

/// Applies `changes` to `edit`: numbers set a value, `null` resets it. Values are clamped to
/// their range; anything that is not a known setting is refused before anything changes.
/// Returns notes about clamped values.
pub fn apply_changes(edit: &mut Edit, changes: &Map<String, Value>) -> Result<Vec<String>, String> {
    let settings = catalogue();
    let mut notes = Vec::new();
    let mut next = edit.clone();
    let mut unknown = Vec::new();
    for (key, value) in changes {
        let Some(spec) = settings.iter().find(|s| &s.key == key) else {
            if is_curve_point_key(key).is_some() {
                return Err(format!("{key}: set tone curves with the `curves` argument instead"));
            }
            unknown.push(key.clone());
            continue;
        };
        match value {
            Value::Null => {
                next.remove(key);
            }
            Value::Number(number) => {
                let raw = number.as_f64().filter(|v| v.is_finite()).ok_or_else(|| format!("{key}: not a finite number"))?;
                let mut value = raw.clamp(spec.min, spec.max);
                if spec.whole {
                    value = value.round();
                }
                if value != raw {
                    notes.push(format!("{key} {raw} became {value} (range {} to {})", spec.min, spec.max));
                }
                if value == spec.neutral && spec.neutral == 0.0 {
                    next.remove(key);
                } else {
                    next.insert(key.clone(), value);
                }
            }
            Value::Bool(flag) if spec.whole && spec.max == 1.0 => {
                if *flag {
                    next.insert(key.clone(), 1.0);
                } else {
                    next.remove(key);
                }
            }
            other => return Err(format!("{key}: expected a number or null, got {other}")),
        }
    }
    if !unknown.is_empty() {
        return Err(format!(
            "unknown setting{} {}. Call list_settings for the names; looks, local masks, sky and background replacement and skin retouching are only in the desktop app.",
            if unknown.len() > 1 { "s" } else { "" },
            unknown.join(", ")
        ));
    }
    if next.get("cropX").copied().unwrap_or(0.0) + next.get("cropW").copied().unwrap_or(1.0) > 1.0 + 1e-9
        || next.get("cropY").copied().unwrap_or(0.0) + next.get("cropH").copied().unwrap_or(1.0) > 1.0 + 1e-9
    {
        return Err("the crop reaches past the frame: cropX + cropW and cropY + cropH must be at most 1".into());
    }
    *edit = next;
    Ok(notes)
}

/// Replaces tone curves. `curves` maps "master", "red", "green" or "blue" to a list of
/// `[input, output]` points in 0..=1, or to `null` or `[]` for a straight line.
pub fn apply_curves(edit: &mut Edit, curves: &Map<String, Value>) -> Result<(), String> {
    let mut next = edit.clone();
    for (name, points) in curves {
        let code = CURVE_CHANNELS.iter().find(|(_, n)| n == name).map(|(c, _)| *c).ok_or_else(|| format!("curves: unknown channel {name}; use master, red, green or blue"))?;
        next.retain(|key, _| is_curve_point_key(key) != Some(code));
        let list = match points {
            Value::Null => continue,
            Value::Array(list) => list,
            _ => return Err(format!("curves.{name}: expected a list of [input, output] points")),
        };
        let mut parsed = list
            .iter()
            .map(|point| {
                let pair = point.as_array().filter(|p| p.len() == 2).ok_or_else(|| format!("curves.{name}: each point is [input, output]"))?;
                let get = |i: usize| pair[i].as_f64().filter(|v| v.is_finite()).map(|v| v.clamp(0.0, 1.0)).ok_or_else(|| format!("curves.{name}: points are numbers"));
                Ok((get(0)?, get(1)?))
            })
            .collect::<Result<Vec<_>, String>>()?;
        parsed.sort_by(|a, b| a.0.total_cmp(&b.0));
        parsed.dedup_by(|a, b| (a.0 - b.0).abs() < 1e-6);
        if parsed.is_empty() {
            continue;
        }
        if parsed.len() > MAX_CURVE_POINTS {
            return Err(format!("curves.{name}: at most {MAX_CURVE_POINTS} points"));
        }
        // The other editions always keep the end points.
        if parsed[0].0 > 0.0 {
            parsed.insert(0, (0.0, 0.0));
        }
        if parsed.last().is_some_and(|p| p.0 < 1.0) {
            parsed.push((1.0, 1.0));
        }
        if parsed == [(0.0, 0.0), (1.0, 1.0)] {
            continue;
        }
        next.insert(format!("curve{code}N"), parsed.len() as f64);
        for (i, (x, y)) in parsed.iter().enumerate() {
            next.insert(format!("curve{code}X{i}"), *x);
            next.insert(format!("curve{code}Y{i}"), *y);
        }
    }
    *edit = next;
    Ok(())
}

/// The edit's tone curves as named point lists, for showing back to an agent.
pub fn curves_of(edit: &Edit) -> Map<String, Value> {
    let mut out = Map::new();
    for (code, name) in CURVE_CHANNELS {
        let points = curve_points(edit, code);
        if points.len() > 2 || points != [(0.0, 0.0), (1.0, 1.0)] {
            out.insert(name.into(), json!(points.iter().map(|(x, y)| [x, y]).collect::<Vec<_>>()));
        }
    }
    out
}

/// The edit without the stored curve points, which `curves_of` shows more readably.
pub fn without_curve_points(edit: &Edit) -> Edit {
    edit.iter().filter(|(key, _)| is_curve_point_key(key).is_none()).map(|(k, v)| (k.clone(), *v)).collect()
}

fn get(edit: &Edit, key: &str, fallback: f64) -> f64 {
    edit.get(key).copied().filter(|v| v.is_finite()).unwrap_or(fallback)
}

fn curve_points(edit: &Edit, code: &str) -> Vec<(f64, f64)> {
    let count = get(edit, &format!("curve{code}N"), 0.0) as usize;
    if count > 0 {
        return (0..count).map(|i| (get(edit, &format!("curve{code}X{i}"), 0.0), get(edit, &format!("curve{code}Y{i}"), 0.0))).collect();
    }
    // Older edits stored offsets at five fixed inputs.
    let legacy: Vec<f64> = (0..5).map(|i| get(edit, &format!("curve{code}{i}"), 0.0)).collect();
    if legacy.iter().all(|&v| v == 0.0) {
        return vec![(0.0, 0.0), (1.0, 1.0)];
    }
    legacy.iter().enumerate().map(|(i, offset)| (i as f64 / 4.0, i as f64 / 4.0 + offset)).collect()
}

/// The engine's adjustment JSON for an edit, matching `Edits.ToEngineJson` on the desktop
/// and `engineJson` on the web.
pub fn engine_json(edit: &Edit) -> Value {
    let mut out = Map::new();
    for (key, value) in edit {
        out.insert(key.clone(), json!(if value.is_finite() { *value } else { 0.0 }));
    }
    let bands = |prefix: &str| json!([(0..4).map(|i| get(edit, &format!("{prefix}{i}"), 0.0)).collect::<Vec<_>>(), (4..8).map(|i| get(edit, &format!("{prefix}{i}"), 0.0)).collect::<Vec<_>>()]);
    out.insert("quarter".into(), json!(get(edit, "quarter", 0.0) as i32));
    out.insert("hslHue".into(), bands("hslHue"));
    out.insert("hslSaturation".into(), bands("hslSat"));
    out.insert("hslLuminance".into(), bands("hslLum"));
    out.insert("curve".into(), json!(CURVE_CHANNELS.iter().map(|(code, _)| curve_points(edit, code).iter().map(|(x, y)| [*x, *y]).collect::<Vec<_>>()).collect::<Vec<_>>()));
    out.insert("parametric".into(), json!(["curveHighlights", "curveLights", "curveDarks", "curveShadows"].map(|key| get(edit, key, 0.0))));
    for key in ["replaceBackground", "replaceBackgroundExposure", "replaceBackgroundShift", "replaceSky", "replaceSkyExposure", "replaceSkyShift", "replaceSkyRelight"] {
        out.insert(key.into(), json!(0.0));
    }
    out.insert("lensManual".into(), json!(["lensDistortion", "lensVignette", "lensCaRed", "lensCaBlue"].map(|key| get(edit, key, 0.0))));
    out.insert("perspective".into(), json!([get(edit, "perspectiveV", 0.0), get(edit, "perspectiveH", 0.0)]));
    out.insert("grade".into(), json!(GRADE_RANGES.map(|(range, _)| [get(edit, &format!("grade{range}Hue"), 0.0), get(edit, &format!("grade{range}Amount"), 0.0), 0.0, 0.0])));
    // No local layers: each has the full brightness range and no change.
    let (ranges, layers) = ([[0.0, 1.0, 0.0, 0.0]; 4], [[0.0; 4]; 4]);
    out.insert("localRange".into(), json!(ranges));
    out.insert("local".into(), json!(layers));
    // Skin retouching needs the desktop app's face finder; without a face mask it would
    // smooth the whole frame, so it never reaches the engine from here.
    out.remove("skinSmooth");
    out.remove("skinBlemish");
    Value::Object(out)
}

/// The settings table as JSON, for `list_settings`.
pub fn catalogue_json() -> Value {
    let settings: Vec<Value> = catalogue()
        .into_iter()
        .map(|s| {
            let mut entry = json!({ "key": s.key, "group": s.group, "min": s.min, "max": s.max, "neutral": s.neutral });
            if s.whole {
                entry["integer"] = json!(true);
            }
            if !s.about.is_empty() {
                entry["about"] = json!(s.about);
            }
            entry
        })
        .collect();
    json!({
        "settings": settings,
        "hslBands": HSL_BANDS,
        "curves": "Pass `curves` to set_edit: {\"master\"|\"red\"|\"green\"|\"blue\": [[input, output], ...]} with values in 0..1; null or [] resets a channel.",
        "notes": [
            "Positions (x, y, crop) are fractions of the framed photo: 0,0 is the top-left corner, 1,1 the bottom-right.",
            "Edits use the same keys as the OpenPixels desktop and web editions.",
            "Looks (LUT presets), local masks, AI subject/sky masks, sky and background replacement and skin retouching are only in the desktop app."
        ]
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn changes(value: Value) -> Map<String, Value> {
        value.as_object().unwrap().clone()
    }

    #[test]
    fn values_are_clamped_and_neutral_values_are_dropped() {
        let mut edit = Edit::new();
        let notes = apply_changes(&mut edit, &changes(json!({ "exposure": 9, "contrast": 0.2, "quarter": 1.4 }))).unwrap();
        assert_eq!(edit["exposure"], 4.0);
        assert_eq!(edit["quarter"], 1.0);
        assert_eq!(notes.len(), 2);
        apply_changes(&mut edit, &changes(json!({ "contrast": 0, "exposure": null }))).unwrap();
        assert!(!edit.contains_key("contrast") && !edit.contains_key("exposure"));
    }

    #[test]
    fn unknown_keys_change_nothing() {
        let mut edit = Edit::from([("exposure".to_string(), 1.0)]);
        let error = apply_changes(&mut edit, &changes(json!({ "exposure": 2, "lutId": 3 }))).unwrap_err();
        assert!(error.contains("lutId"), "{error}");
        assert_eq!(edit["exposure"], 1.0);
    }

    #[test]
    fn a_crop_past_the_frame_is_refused() {
        let mut edit = Edit::new();
        assert!(apply_changes(&mut edit, &changes(json!({ "cropX": 0.5, "cropW": 0.8 }))).is_err());
        apply_changes(&mut edit, &changes(json!({ "cropX": 0.2, "cropW": 0.8 }))).unwrap();
    }

    #[test]
    fn curves_round_trip_through_the_stored_keys() {
        let mut edit = Edit::new();
        apply_curves(&mut edit, &changes(json!({ "master": [[0.25, 0.2], [0.75, 0.8]] }))).unwrap();
        assert_eq!(edit["curveMN"], 4.0);
        let shown = curves_of(&edit);
        assert_eq!(shown["master"], json!([[0.0, 0.0], [0.25, 0.2], [0.75, 0.8], [1.0, 1.0]]));
        let engine = engine_json(&edit);
        assert_eq!(engine["curve"][0].as_array().unwrap().len(), 4);
        assert_eq!(engine["curve"][1], json!([[0.0, 0.0], [1.0, 1.0]]));
        apply_curves(&mut edit, &changes(json!({ "master": null }))).unwrap();
        assert!(edit.is_empty());
    }

    #[test]
    fn the_engine_json_has_the_grouped_fields() {
        let edit = Edit::from([("hslSat5".to_string(), 0.4), ("gradeMidHue".to_string(), 30.0), ("gradeMidAmount".to_string(), 0.2), ("lensCaRed".to_string(), 0.1)]);
        let json = engine_json(&edit);
        assert_eq!(json["hslSaturation"], json!([[0.0, 0.0, 0.0, 0.0], [0.0, 0.4, 0.0, 0.0]]));
        assert_eq!(json["grade"][1], json!([30.0, 0.2, 0.0, 0.0]));
        assert_eq!(json["lensManual"][1], json!(0.0));
        assert_eq!(json["lensManual"][2], json!(0.1));
        assert_eq!(json["quarter"], json!(0));
    }

    #[test]
    fn every_setting_reaches_an_engine_field_or_a_grouped_one() {
        // A typo in the table would otherwise be accepted and silently do nothing.
        let adjustments = serde_json::to_value(op_core::Adjustments::default()).unwrap();
        let grouped = ["hslHue", "hslSat", "hslLum", "grade", "curve", "lens", "perspective", "crop", "rotate", "quarter"];
        for setting in catalogue() {
            let direct = adjustments.get(&setting.key).is_some();
            assert!(direct || grouped.iter().any(|g| setting.key.starts_with(g)), "{} goes nowhere", setting.key);
        }
    }
}
