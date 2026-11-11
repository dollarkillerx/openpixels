//! The tools: one open photo at a time, edited, looked at and exported with the engine.

use std::path::{Path, PathBuf};

use op_native::NativeEngine;
use serde_json::{Map, Value, json};

use crate::edit::{self, Edit};
use crate::pixels::{Rgb, base64};

/// Long edge of the working copy that previews, measurements and pickers use. Exports
/// always develop the full-resolution file.
const PROXY_EDGE: u32 = 2048;
const DEFAULT_PREVIEW_EDGE: u32 = 1024;
const MEASURE_EDGE: u32 = 512;
const PREVIEW_QUALITY: u8 = 85;
const SIDECAR_SUFFIX: &str = ".openpixels.json";
const DEFAULT_HEAL_RADIUS: f64 = 0.015;

const RASTER_EXTENSIONS: [&str; 5] = ["jpg", "jpeg", "png", "tif", "tiff"];
const HEIF_EXTENSIONS: [&str; 4] = ["heic", "heif", "hif", "avif"];
const RAW_EXTENSIONS: [&str; 16] = ["dng", "cr2", "cr3", "crw", "nef", "nrw", "arw", "srf", "sr2", "raf", "rw2", "orf", "pef", "srw", "3fr", "iiq"];

/// What a tool hands back: MCP content items.
pub type Reply = Result<Vec<Value>, String>;

struct Photo {
    path: PathBuf,
    info: Value,
    edit: Edit,
    /// Healed spots as the engine returned them, so they can be saved and restored.
    heals: Vec<Value>,
}

#[derive(Default)]
pub struct Studio {
    engine: Option<NativeEngine>,
    photo: Option<Photo>,
}

fn text(value: impl Into<String>) -> Value {
    json!({ "type": "text", "text": value.into() })
}

fn data(value: &Value) -> Value {
    text(serde_json::to_string_pretty(value).unwrap_or_default())
}

fn image(rgb: &Rgb) -> Result<Value, String> {
    Ok(json!({ "type": "image", "data": base64(&rgb.jpeg(PREVIEW_QUALITY)?), "mimeType": "image/jpeg" }))
}

fn number(args: &Map<String, Value>, key: &str) -> Option<f64> {
    args.get(key).and_then(Value::as_f64).filter(|v| v.is_finite())
}

fn required_number(args: &Map<String, Value>, key: &str) -> Result<f64, String> {
    number(args, key).ok_or_else(|| format!("{key} is required and must be a number"))
}

fn flag(args: &Map<String, Value>, key: &str, fallback: bool) -> bool {
    args.get(key).and_then(Value::as_bool).unwrap_or(fallback)
}

fn string<'a>(args: &'a Map<String, Value>, key: &str) -> Option<&'a str> {
    args.get(key).and_then(Value::as_str).filter(|s| !s.trim().is_empty())
}

fn object<'a>(args: &'a Map<String, Value>, key: &str) -> Result<Option<&'a Map<String, Value>>, String> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Object(map)) => Ok(Some(map)),
        Some(_) => Err(format!("{key} must be an object")),
    }
}

/// A path from an agent: `~` is the home folder and relative paths start at the working folder.
pub fn resolve(path: &str) -> PathBuf {
    let path = path.trim();
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"));
    let expanded = match (path.strip_prefix("~/").or_else(|| path.strip_prefix("~\\")), home) {
        (Some(rest), Some(home)) => PathBuf::from(home).join(rest),
        _ if path == "~" => std::env::var_os("HOME").map_or_else(|| PathBuf::from(path), PathBuf::from),
        _ => PathBuf::from(path),
    };
    if expanded.is_absolute() { expanded } else { std::env::current_dir().map(|dir| dir.join(&expanded)).unwrap_or(expanded) }
}

fn extension(path: &Path) -> String {
    path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase()
}

pub fn is_photo(path: &Path) -> bool {
    let ext = extension(path);
    [&RASTER_EXTENSIONS[..], &HEIF_EXTENSIONS, &RAW_EXTENSIONS].iter().any(|list| list.contains(&ext.as_str()))
}

fn sidecar_of(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(SIDECAR_SUFFIX);
    PathBuf::from(name)
}

/// The edit and healed spots saved next to a photo, if any.
fn read_sidecar(path: &Path) -> Result<Option<(Edit, Vec<Value>)>, String> {
    let sidecar = sidecar_of(path);
    let Ok(text) = std::fs::read_to_string(&sidecar) else { return Ok(None) };
    let saved: Value = serde_json::from_str(&text).map_err(|e| format!("{} is not valid JSON: {e}", sidecar.display()))?;
    let edit = saved["edit"].as_object().map(|map| map.iter().filter_map(|(k, v)| Some((k.clone(), v.as_f64()?))).collect()).unwrap_or_default();
    let heals = saved["heals"].as_array().cloned().unwrap_or_default();
    Ok(Some((edit, heals)))
}

/// Writes through a temporary file, so a failed write never leaves half a file.
fn write_file(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("could not create {}: {e}", parent.display()))?;
    }
    let mut temporary = path.as_os_str().to_owned();
    temporary.push(".partial");
    std::fs::write(&temporary, bytes).map_err(|e| format!("could not write {}: {e}", path.display()))?;
    std::fs::rename(&temporary, path).map_err(|e| {
        let _ = std::fs::remove_file(&temporary);
        format!("could not write {}: {e}", path.display())
    })
}

/// HEIC and AVIF go through the system's decoder: `sips` on macOS, libheif's tools elsewhere.
fn decode_heif(path: &Path) -> Result<Vec<u8>, String> {
    let out = std::env::temp_dir().join(format!("openpixels-mcp-{}-{}.png", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos())));
    let attempts: Vec<(&str, Vec<&std::ffi::OsStr>)> = if cfg!(target_os = "macos") {
        vec![("sips", vec!["-s".as_ref(), "format".as_ref(), "png".as_ref(), path.as_os_str(), "--out".as_ref(), out.as_os_str()])]
    } else {
        vec![("heif-dec", vec![path.as_os_str(), out.as_os_str()]), ("heif-convert", vec![path.as_os_str(), out.as_os_str()])]
    };
    let mut failures = Vec::new();
    for (program, arguments) in attempts {
        match std::process::Command::new(program).args(&arguments).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::piped()).output() {
            Ok(result) if result.status.success() => {
                let bytes = std::fs::read(&out).map_err(|e| format!("{program} did not write the image: {e}"));
                let _ = std::fs::remove_file(&out);
                return bytes;
            }
            Ok(result) => failures.push(format!("{program}: {}", String::from_utf8_lossy(&result.stderr).trim())),
            Err(e) => failures.push(format!("{program}: {e}")),
        }
    }
    let _ = std::fs::remove_file(&out);
    Err(format!(
        "could not decode {}. {}{}",
        path.display(),
        if cfg!(target_os = "macos") { "" } else { "Install libheif's tools (for example `apt install libheif-examples`). " },
        failures.join("; ")
    ))
}

struct ExportSettings {
    format: &'static str,
    quality: u32,
    long_edge: u32,
    bit_depth: u32,
    color_space: String,
    overwrite: bool,
}

impl ExportSettings {
    fn read(args: &Map<String, Value>, path: Option<&Path>) -> Result<Self, String> {
        let named = string(args, "format").map(str::to_ascii_lowercase);
        let from_path = path.map(extension);
        let format = match named.as_deref().or(from_path.as_deref()) {
            Some("jpg" | "jpeg") | None | Some("") => "jpeg",
            Some("png") => "png",
            Some("tif" | "tiff") => "tiff",
            Some(other) => return Err(format!("format {other} is not supported; use jpeg, png or tiff")),
        };
        let bit_depth = number(args, "bit_depth").map_or(8, |v| v as u32);
        if bit_depth != 8 && bit_depth != 16 {
            return Err("bit_depth is 8 or 16".into());
        }
        if bit_depth == 16 && format == "jpeg" {
            return Err("JPEG is always 8-bit; use png or tiff for 16-bit".into());
        }
        let color_space = string(args, "color_space").unwrap_or("srgb").to_ascii_lowercase();
        if !["srgb", "p3", "adobe", "prophoto"].contains(&color_space.as_str()) {
            return Err("color_space is srgb, p3, adobe or prophoto".into());
        }
        Ok(Self {
            format,
            quality: number(args, "quality").map_or(92, |v| v.clamp(1.0, 100.0) as u32),
            long_edge: number(args, "long_edge").map_or(0, |v| v.max(0.0) as u32),
            bit_depth,
            color_space,
            overwrite: flag(args, "overwrite", false),
        })
    }

    fn extension(&self) -> &'static str {
        match self.format {
            "jpeg" => "jpg",
            "png" => "png",
            _ => "tif",
        }
    }
}

impl Studio {
    fn engine(&mut self) -> Result<&mut NativeEngine, String> {
        if self.engine.is_none() {
            self.engine = Some(NativeEngine::new().map_err(|e| format!("the OpenPixels engine could not start a GPU device: {e}"))?);
        }
        Ok(self.engine.as_mut().expect("created above"))
    }

    fn call(&mut self, method: &str, args: Value, bytes: &[u8]) -> Result<(Value, Vec<u8>), String> {
        self.engine()?.call(method, &args, bytes).map_err(|e| format!("{method}: {e}"))
    }

    fn photo(&self) -> Result<&Photo, String> {
        self.photo.as_ref().ok_or_else(|| "no photo is open; call open_photo first".to_string())
    }

    fn photo_mut(&mut self) -> Result<&mut Photo, String> {
        self.photo.as_mut().ok_or_else(|| "no photo is open; call open_photo first".to_string())
    }

    /// Opens a file into the engine with an edit and healed spots, replacing the open photo.
    fn load(&mut self, path: PathBuf, edit: Edit, heals: Vec<Value>) -> Result<(), String> {
        self.photo = None;
        let bytes = if HEIF_EXTENSIONS.contains(&extension(&path).as_str()) {
            decode_heif(&path)?
        } else {
            std::fs::read(&path).map_err(|e| format!("could not read {}: {e}", path.display()))?
        };
        let (info, _) = self.call("decode", Value::Null, &bytes)?;
        drop(bytes);
        self.call("buildProxy", json!({ "maxLongEdge": PROXY_EDGE }), &[])?;
        self.call("setView", json!({ "zoom": 1, "x": 0.5, "y": 0.5 }), &[])?;
        self.call("setAdjustments", edit::engine_json(&edit), &[])?;
        if !heals.is_empty() {
            self.call("setHeals", Value::Array(heals.clone()), &[])?;
        }
        self.photo = Some(Photo { path, info, edit, heals });
        Ok(())
    }

    fn push_edit(&mut self) -> Result<(), String> {
        let json = edit::engine_json(&self.photo()?.edit);
        self.call("setAdjustments", json, &[]).map(|_| ())
    }

    fn summary(&self) -> Result<Value, String> {
        let photo = self.photo()?;
        Ok(json!({
            "path": photo.path,
            "edit": edit::without_curve_points(&photo.edit),
            "curves": edit::curves_of(&photo.edit),
            "healedSpots": photo.heals.len(),
        }))
    }

    /// The framed photo with the current edit, as RGB8.
    fn render(&mut self, long_edge: u32) -> Result<Rgb, String> {
        let (size, _) = self.call("framedThumbnailSize", json!({ "maxLongEdge": long_edge }), &[])?;
        let (_, data) = self.call("framedThumbnail", json!({ "maxLongEdge": long_edge }), &[])?;
        let (width, height) = (size["width"].as_u64().unwrap_or(0) as u32, size["height"].as_u64().unwrap_or(0) as u32);
        if width == 0 || height == 0 || data.len() != (width * height * 3) as usize {
            return Err("the preview came back empty".into());
        }
        Ok(Rgb { width, height, data })
    }

    /// The stage the pickers measure positions on: the framed working copy.
    fn stage(&mut self) -> Result<(u64, u64), String> {
        let (size, _) = self.call("framedThumbnailSize", json!({ "maxLongEdge": PROXY_EDGE }), &[])?;
        Ok((size["width"].as_u64().unwrap_or(1), size["height"].as_u64().unwrap_or(1)))
    }

    pub fn call_tool(&mut self, name: &str, args: &Map<String, Value>) -> Reply {
        match name {
            "list_settings" => Ok(vec![data(&edit::catalogue_json())]),
            "open_photo" => self.open_photo(args),
            "get_edit" => Ok(vec![data(&self.summary()?)]),
            "set_edit" => self.set_edit(args),
            "auto_tone" => self.auto_tone(args),
            "pick_white_balance" => self.pick_white_balance(args),
            "heal_spot" => self.heal_spot(args),
            "clear_heals" => {
                self.photo()?;
                self.call("setHeals", json!([]), &[])?;
                self.photo_mut()?.heals.clear();
                Ok(vec![text("All healed spots removed.")])
            }
            "preview" => self.preview(args),
            "analyze" => self.analyze(),
            "save_edit" => self.save_edit(args),
            "export_photo" => self.export_photo(args),
            "batch_export" => self.batch_export(args),
            _ => Err(format!("unknown tool {name}")),
        }
    }

    fn open_photo(&mut self, args: &Map<String, Value>) -> Reply {
        let path = resolve(string(args, "path").ok_or("path is required")?);
        if !path.is_file() {
            return Err(format!("{} is not a file", path.display()));
        }
        let saved = if flag(args, "use_sidecar", true) { read_sidecar(&path)? } else { None };
        let from_sidecar = saved.is_some();
        let (mut edit, heals) = saved.unwrap_or_default();
        let mut notes = Vec::new();
        if let Some(changes) = object(args, "edit")? {
            notes = edit::apply_changes(&mut edit, changes)?;
        }
        self.load(path, edit, heals)?;
        let photo = self.photo()?;
        let mut reply = json!({ "opened": photo.path, "info": photo.info, "edit": edit::without_curve_points(&photo.edit), "curves": edit::curves_of(&photo.edit), "healedSpots": photo.heals.len() });
        if from_sidecar {
            reply["loadedFrom"] = json!(sidecar_of(&photo.path));
        }
        if !notes.is_empty() {
            reply["notes"] = json!(notes);
        }
        Ok(vec![data(&reply)])
    }

    fn set_edit(&mut self, args: &Map<String, Value>) -> Reply {
        let mut next = if flag(args, "replace", false) { Edit::new() } else { self.photo()?.edit.clone() };
        let mut notes = Vec::new();
        if let Some(changes) = object(args, "changes")? {
            notes = edit::apply_changes(&mut next, changes)?;
        }
        if let Some(curves) = object(args, "curves")? {
            edit::apply_curves(&mut next, curves)?;
        }
        let previous = std::mem::replace(&mut self.photo_mut()?.edit, next);
        if let Err(error) = self.push_edit() {
            self.photo_mut()?.edit = previous;
            return Err(error);
        }
        let mut summary = self.summary()?;
        if !notes.is_empty() {
            summary["notes"] = json!(notes);
        }
        let mut content = vec![data(&summary)];
        if flag(args, "preview", false) {
            content.push(image(&self.render(DEFAULT_PREVIEW_EDGE)?)?);
        }
        Ok(content)
    }

    fn auto_tone(&mut self, args: &Map<String, Value>) -> Reply {
        self.photo()?;
        let (suggestion, _) = self.call("autoTone", Value::Null, &[])?;
        if !flag(args, "apply", true) {
            return Ok(vec![data(&json!({ "suggestion": suggestion, "applied": false }))]);
        }
        let changes = suggestion.as_object().cloned().unwrap_or_default();
        let mut next = self.photo()?.edit.clone();
        edit::apply_changes(&mut next, &changes)?;
        self.photo_mut()?.edit = next;
        self.push_edit()?;
        Ok(vec![data(&json!({ "suggestion": suggestion, "applied": true, "edit": edit::without_curve_points(&self.photo()?.edit) }))])
    }

    fn pick_white_balance(&mut self, args: &Map<String, Value>) -> Reply {
        self.photo()?;
        let (x, y) = (required_number(args, "x")?, required_number(args, "y")?);
        let (width, height) = self.stage()?;
        let (picked, _) = self.call("whiteBalanceAt", json!({ "x": x, "y": y, "stageWidth": width, "stageHeight": height }), &[])?;
        if picked.is_null() {
            return Err("that spot is too dark, clipped or outside the photo; pick something light grey or white".into());
        }
        if !flag(args, "apply", true) {
            return Ok(vec![data(&json!({ "whiteBalance": picked, "applied": false }))]);
        }
        let mut next = self.photo()?.edit.clone();
        edit::apply_changes(&mut next, &Map::from_iter([("temperature".into(), picked["temperature"].clone()), ("tint".into(), picked["tint"].clone())]))?;
        self.photo_mut()?.edit = next;
        self.push_edit()?;
        Ok(vec![data(&json!({ "whiteBalance": picked, "applied": true }))])
    }

    fn heal_spot(&mut self, args: &Map<String, Value>) -> Reply {
        self.photo()?;
        let (x, y) = (required_number(args, "x")?, required_number(args, "y")?);
        let radius = number(args, "radius").unwrap_or(DEFAULT_HEAL_RADIUS).clamp(0.002, 0.2);
        let (width, height) = self.stage()?;
        let (spot, _) = self.call("healAt", json!({ "x": x, "y": y, "stageWidth": width, "stageHeight": height, "radius": radius }), &[])?;
        if spot.is_null() {
            return Err("no spot healed: the position is outside the photo or no clean source area was found nearby".into());
        }
        let photo = self.photo_mut()?;
        photo.heals.push(spot);
        Ok(vec![text(format!("Healed the spot at ({x:.3}, {y:.3}) with radius {radius:.3}; {} spot(s) in total.", photo.heals.len()))])
    }

    fn preview(&mut self, args: &Map<String, Value>) -> Reply {
        let name = self.photo()?.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let max_edge = number(args, "max_size").map_or(DEFAULT_PREVIEW_EDGE, |v| v.clamp(64.0, f64::from(PROXY_EDGE)) as u32);
        let region = match object(args, "region")? {
            Some(r) => Some([required_number(r, "x")?, required_number(r, "y")?, required_number(r, "width")?, required_number(r, "height")?]),
            None => None,
        };
        // Render sizes count the uncropped photo's long edge, so scale up for a crop, and
        // more for a region, which then shows more detail; never past the working copy.
        // The working copy can be smaller than PROXY_EDGE, which then limits the detail.
        let (framed_w, framed_h) = self.stage()?;
        let (whole, _) = self.call("thumbnailSize", json!({ "maxLongEdge": PROXY_EDGE }), &[])?;
        let whole_long = whole["width"].as_f64().unwrap_or(1.0).max(whole["height"].as_f64().unwrap_or(1.0));
        let [_, _, w, h] = region.unwrap_or([0.0, 0.0, 1.0, 1.0]);
        let shown_long = (w.clamp(0.01, 1.0) * framed_w as f64).max(h.clamp(0.01, 1.0) * framed_h as f64).max(1.0);
        let render_edge = (whole_long * f64::from(max_edge) / shown_long).clamp(16.0, whole_long) as u32;
        let draw = |studio: &mut Studio| -> Result<Rgb, String> {
            let full = studio.render(render_edge)?;
            Ok(match region {
                Some([x, y, w, h]) => full.region(x, y, w, h),
                None => full,
            })
        };
        let after = draw(self)?;
        let mode = string(args, "show").unwrap_or("after");
        let shown = match mode {
            "after" => after,
            "before" | "compare" => {
                // The original keeps the framing, so both show the same part of the photo.
                let framing: Edit = self.photo()?.edit.iter().filter(|(k, _)| edit::belongs_to_one_photo(k)).map(|(k, v)| (k.clone(), *v)).collect();
                self.call("setAdjustments", edit::engine_json(&framing), &[])?;
                let before = draw(self);
                self.push_edit()?;
                let before = before?;
                if mode == "before" { before } else { before.beside(&after) }
            }
            other => return Err(format!("show is after, before or compare, not {other}")),
        };
        let label = match mode {
            "compare" => "before (left) and after (right)",
            other => other,
        };
        Ok(vec![text(format!("{name}, {label}: {}x{} pixels", shown.width, shown.height)), image(&shown)?])
    }

    fn analyze(&mut self) -> Reply {
        let info = self.photo()?.info.clone();
        let stats = self.render(MEASURE_EDGE)?.measure();
        Ok(vec![data(&json!({ "photo": info, "withCurrentEdit": stats, "edit": edit::without_curve_points(&self.photo()?.edit), "curves": edit::curves_of(&self.photo()?.edit) }))])
    }

    fn save_edit(&mut self, args: &Map<String, Value>) -> Reply {
        let photo = self.photo()?;
        let target = string(args, "path").map_or_else(|| sidecar_of(&photo.path), resolve);
        let saved = json!({ "openpixels": 1, "photo": photo.path.file_name().map(|n| n.to_string_lossy()), "edit": photo.edit, "heals": photo.heals });
        write_file(&target, serde_json::to_string_pretty(&saved).unwrap_or_default().as_bytes())?;
        Ok(vec![text(format!("Saved the edit to {}. open_photo reads it back automatically when it sits next to the photo.", target.display()))])
    }

    /// Writes the open photo, developed at full resolution, to `path`.
    fn write_export(&mut self, path: &Path, settings: &ExportSettings) -> Result<Value, String> {
        if path.exists() && !settings.overwrite {
            return Err(format!("{} already exists; pass overwrite: true to replace it", path.display()));
        }
        let (full, _) = self.call("hasFullImage", Value::Null, &[])?;
        if full == json!(false) {
            self.call("developFull", Value::Null, &[])?;
        }
        let (size, _) = self.call("exportSize", json!({ "longEdge": settings.long_edge }), &[])?;
        let request = json!({ "settings": { "format": settings.format, "quality": settings.quality, "longEdge": settings.long_edge, "bitDepth": settings.bit_depth, "colorSpace": settings.color_space } });
        let (_, bytes) = self.call("exportImage", request, &[])?;
        write_file(path, &bytes)?;
        Ok(json!({ "path": path, "width": size["width"], "height": size["height"], "bytes": bytes.len(), "format": settings.format, "bitDepth": settings.bit_depth, "colorSpace": settings.color_space }))
    }

    fn export_photo(&mut self, args: &Map<String, Value>) -> Reply {
        self.photo()?;
        let mut path = resolve(string(args, "path").ok_or("path is required")?);
        let settings = ExportSettings::read(args, Some(&path))?;
        if extension(&path).is_empty() {
            path.set_extension(settings.extension());
        }
        if path == self.photo()?.path {
            return Err("refusing to write over the original photo".into());
        }
        Ok(vec![data(&self.write_export(&path, &settings)?)])
    }

    fn batch_export(&mut self, args: &Map<String, Value>) -> Reply {
        let mut inputs: Vec<PathBuf> = match args.get("inputs") {
            Some(Value::Array(list)) => list.iter().filter_map(Value::as_str).map(resolve).collect(),
            _ => Vec::new(),
        };
        if let Some(folder) = string(args, "folder") {
            let folder = resolve(folder);
            let mut found: Vec<PathBuf> = std::fs::read_dir(&folder)
                .map_err(|e| format!("could not read {}: {e}", folder.display()))?
                .filter_map(|entry| entry.ok().map(|e| e.path()))
                .filter(|path| path.is_file() && is_photo(path))
                .collect();
            found.sort();
            inputs.extend(found);
        }
        if inputs.is_empty() {
            return Err("give inputs (a list of photo paths) or folder".into());
        }
        let output = resolve(string(args, "output_folder").ok_or("output_folder is required")?);
        let settings = ExportSettings::read(args, None)?;
        let changes = object(args, "edit")?.cloned();
        let curves = object(args, "curves")?.cloned();
        let use_sidecars = flag(args, "use_sidecars", true);
        let auto = flag(args, "auto_tone", false);
        let suffix = string(args, "suffix").unwrap_or("");
        // Validated once up front, so a bad setting fails before any file is written.
        if let Some(changes) = &changes {
            edit::apply_changes(&mut Edit::new(), changes)?;
        }

        let previous = self.photo.take().map(|p| (p.path, p.edit, p.heals));
        let mut results = Vec::new();
        let (mut written, mut failed) = (0, 0);
        for input in &inputs {
            let outcome = (|| -> Result<Value, String> {
                let stem = input.file_stem().ok_or("no file name")?.to_string_lossy();
                let target = output.join(format!("{stem}{suffix}.{}", settings.extension()));
                if target == *input {
                    return Err("the output would replace the original".into());
                }
                if target.exists() && !settings.overwrite {
                    return Err(format!("{} already exists; pass overwrite: true to replace it", target.display()));
                }
                let (mut edit, heals) = if use_sidecars { read_sidecar(input)?.unwrap_or_default() } else { Default::default() };
                if let Some(changes) = &changes {
                    edit::apply_changes(&mut edit, changes)?;
                }
                if let Some(curves) = &curves {
                    edit::apply_curves(&mut edit, curves)?;
                }
                self.load(input.clone(), edit, heals)?;
                if auto {
                    let (suggestion, _) = self.call("autoTone", Value::Null, &[])?;
                    let photo = self.photo_mut()?;
                    edit::apply_changes(&mut photo.edit, suggestion.as_object().ok_or("auto tone gave nothing")?)?;
                    self.push_edit()?;
                }
                self.write_export(&target, &settings)
            })();
            match outcome {
                Ok(done) => {
                    written += 1;
                    results.push(json!({ "input": input, "output": done["path"], "width": done["width"], "height": done["height"] }));
                }
                Err(error) => {
                    failed += 1;
                    results.push(json!({ "input": input, "error": error }));
                }
            }
        }
        self.photo = None;
        let mut reopened = Value::Null;
        if let Some((path, edit, heals)) = previous {
            reopened = json!(path);
            if let Err(error) = self.load(path, edit, heals) {
                reopened = json!(format!("could not reopen the photo that was open: {error}"));
            }
        }
        Ok(vec![data(&json!({ "written": written, "failed": failed, "outputFolder": output, "results": results, "reopened": reopened }))])
    }
}

/// The tools as MCP describes them.
pub fn tool_list() -> Value {
    let edit_object = json!({ "type": "object", "description": "Settings to change, as {key: number}; null resets a key. Keys and ranges come from list_settings.", "additionalProperties": { "type": ["number", "boolean", "null"] } });
    let curves_object = json!({ "type": "object", "description": "Tone curves to replace: {\"master\"|\"red\"|\"green\"|\"blue\": [[input, output], ...]} with values in 0..1; null or [] makes a channel straight.", "additionalProperties": { "type": ["array", "null"] } });
    let export_properties = json!({
        "format": { "type": "string", "enum": ["jpeg", "png", "tiff"], "description": "Defaults to the file extension, else jpeg." },
        "quality": { "type": "number", "minimum": 1, "maximum": 100, "description": "JPEG quality, default 92." },
        "long_edge": { "type": "number", "minimum": 0, "description": "Resize so the long edge is at most this many pixels; 0 (default) keeps the full size." },
        "bit_depth": { "type": "number", "enum": [8, 16], "description": "16 needs png or tiff. Default 8." },
        "color_space": { "type": "string", "enum": ["srgb", "p3", "adobe", "prophoto"], "description": "Default srgb, right for the web. Wide spaces keep colours outside sRGB." },
        "overwrite": { "type": "boolean", "description": "Replace existing files. Default false." }
    });
    let position = |what: &str| json!({ "type": "number", "minimum": 0, "maximum": 1, "description": format!("{what} as a fraction of the framed photo as preview shows it (0 is the left or top edge, 1 the right or bottom).") });
    let read_only = json!({ "readOnlyHint": true, "openWorldHint": false });
    let editing = json!({ "readOnlyHint": false, "destructiveHint": false, "idempotentHint": true, "openWorldHint": false });
    let writing = json!({ "readOnlyHint": false, "destructiveHint": true, "idempotentHint": true, "openWorldHint": false });
    let mut batch_properties = json!({
        "inputs": { "type": "array", "items": { "type": "string" }, "description": "Photo paths." },
        "folder": { "type": "string", "description": "A folder whose photos (JPEG, PNG, TIFF, HEIC, RAW) are all exported; not recursive." },
        "output_folder": { "type": "string", "description": "Where the files go; created if needed. Names are the photo's name plus suffix." },
        "suffix": { "type": "string", "description": "Added to each output name, for example \"-web\"." },
        "edit": edit_object,
        "curves": curves_object,
        "use_sidecars": { "type": "boolean", "description": "Start from each photo's saved edit (save_edit), if it has one. Default true." },
        "auto_tone": { "type": "boolean", "description": "Run automatic tone on each photo after the edit. Default false." }
    });
    for (key, value) in export_properties.as_object().expect("an object") {
        batch_properties[key] = value.clone();
    }
    let mut export = json!({ "path": { "type": "string", "description": "Output file. The extension picks the format when format is not given." } });
    for (key, value) in export_properties.as_object().expect("an object") {
        export[key] = value.clone();
    }

    json!([
        {
            "name": "list_settings",
            "title": "List edit settings",
            "description": "Lists every edit setting set_edit accepts, with its range, neutral value and meaning. Call this once before editing.",
            "inputSchema": { "type": "object", "properties": {} },
            "annotations": read_only
        },
        {
            "name": "open_photo",
            "title": "Open photo",
            "description": "Opens a photo for editing (JPEG, PNG, TIFF, HEIC/AVIF, or a camera RAW such as DNG, CR3, NEF, ARW, RAF). Loads the edit saved next to it by save_edit, if any. Returns its size and camera details. Only one photo is open at a time.",
            "inputSchema": { "type": "object", "properties": {
                "path": { "type": "string", "description": "The photo's path; ~ and relative paths work." },
                "edit": edit_object,
                "use_sidecar": { "type": "boolean", "description": "Load the saved edit next to the photo. Default true." }
            }, "required": ["path"] },
            "annotations": read_only
        },
        {
            "name": "get_edit",
            "title": "Get edit",
            "description": "Shows the open photo's current edit: changed settings, tone curves and the number of healed spots.",
            "inputSchema": { "type": "object", "properties": {} },
            "annotations": read_only
        },
        {
            "name": "set_edit",
            "title": "Change edit",
            "description": "Changes the open photo's edit, like moving sliders in a photo editor. Only the keys given change unless replace is true. Out-of-range values are clamped and reported. Nothing is written to disk; use export_photo or save_edit for that.",
            "inputSchema": { "type": "object", "properties": {
                "changes": edit_object,
                "curves": curves_object,
                "replace": { "type": "boolean", "description": "Start from an untouched photo instead of the current edit. Default false." },
                "preview": { "type": "boolean", "description": "Also return a preview image of the result. Default false." }
            } },
            "annotations": editing
        },
        {
            "name": "auto_tone",
            "title": "Automatic tone",
            "description": "Suggests exposure, contrast, highlights, shadows, whites, blacks and vibrance for the photo as framed, and applies them unless apply is false.",
            "inputSchema": { "type": "object", "properties": { "apply": { "type": "boolean", "description": "Default true." } } },
            "annotations": editing
        },
        {
            "name": "pick_white_balance",
            "title": "Pick white balance",
            "description": "Sets temperature and tint so the colour at a position becomes neutral, like a white balance eyedropper. Pick something that should be white or light grey, not clipped.",
            "inputSchema": { "type": "object", "properties": { "x": position("Horizontal position"), "y": position("Vertical position"), "apply": { "type": "boolean", "description": "Default true." } }, "required": ["x", "y"] },
            "annotations": editing
        },
        {
            "name": "heal_spot",
            "title": "Heal spot",
            "description": "Removes a small blemish, dust spot or distraction at a position by copying a matching nearby area over it. Look at a close preview first to aim.",
            "inputSchema": { "type": "object", "properties": {
                "x": position("Horizontal centre"),
                "y": position("Vertical centre"),
                "radius": { "type": "number", "minimum": 0.002, "maximum": 0.2, "description": "Radius as a fraction of the photo's long edge. Default 0.015." }
            }, "required": ["x", "y"] },
            "annotations": { "readOnlyHint": false, "destructiveHint": false, "idempotentHint": false, "openWorldHint": false }
        },
        {
            "name": "clear_heals",
            "title": "Clear healed spots",
            "description": "Removes every healed spot from the open photo.",
            "inputSchema": { "type": "object", "properties": {} },
            "annotations": editing
        },
        {
            "name": "preview",
            "title": "Preview",
            "description": "Returns a JPEG of the open photo with the current edit, cropped and rotated as it will export. Use show: compare to see the original and the edit side by side, and region to look closely at part of the photo.",
            "inputSchema": { "type": "object", "properties": {
                "max_size": { "type": "number", "minimum": 64, "maximum": 2048, "description": "Long edge in pixels. Default 1024." },
                "show": { "type": "string", "enum": ["after", "before", "compare"], "description": "Default after." },
                "region": { "type": "object", "description": "A part of the framed photo to show larger, as fractions.", "properties": { "x": { "type": "number" }, "y": { "type": "number" }, "width": { "type": "number" }, "height": { "type": "number" } }, "required": ["x", "y", "width", "height"] }
            } },
            "annotations": read_only
        },
        {
            "name": "analyze",
            "title": "Measure photo",
            "description": "Measures the photo with the current edit: brightness percentiles, clipped highlights and crushed shadows, colour cast (warmth, magenta), saturation and a 16-bin histogram. Use it to judge exposure and colour numerically.",
            "inputSchema": { "type": "object", "properties": {} },
            "annotations": read_only
        },
        {
            "name": "save_edit",
            "title": "Save edit",
            "description": "Saves the open photo's edit and healed spots as JSON, by default next to the photo as <photo>.openpixels.json, which open_photo and batch_export pick up again. The photo itself is never changed.",
            "inputSchema": { "type": "object", "properties": { "path": { "type": "string", "description": "Somewhere else to save it, for example to reuse it as a preset." } } },
            "annotations": writing
        },
        {
            "name": "export_photo",
            "title": "Export photo",
            "description": "Develops the open photo at full resolution with the current edit and writes it as JPEG, PNG or TIFF. The original is never changed.",
            "inputSchema": { "type": "object", "properties": export, "required": ["path"] },
            "annotations": writing
        },
        {
            "name": "batch_export",
            "title": "Batch export",
            "description": "Applies one edit to many photos and exports each: every photo starts from its saved edit (if any), then gets edit and curves, then optional auto tone. Failures are reported per photo and do not stop the rest. Framing keys in edit (crop, rotate) apply to every photo too.",
            "inputSchema": { "type": "object", "properties": batch_properties, "required": ["output_folder"] },
            "annotations": writing
        }
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_resolve_against_home_and_the_working_folder() {
        if let Some(home) = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")) {
            assert_eq!(resolve("~/Pictures/a.jpg"), PathBuf::from(home).join("Pictures/a.jpg"));
        }
        assert!(resolve("a.jpg").is_absolute());
        assert_eq!(sidecar_of(Path::new("/x/IMG_1.CR3")), PathBuf::from("/x/IMG_1.CR3.openpixels.json"));
    }

    #[test]
    fn export_settings_follow_the_extension_and_refuse_impossible_files() {
        let none = Map::new();
        assert_eq!(ExportSettings::read(&none, Some(Path::new("a.TIF"))).unwrap().format, "tiff");
        assert_eq!(ExportSettings::read(&none, Some(Path::new("a"))).unwrap().format, "jpeg");
        let deep_jpeg = Map::from_iter([("bit_depth".to_string(), json!(16))]);
        assert!(ExportSettings::read(&deep_jpeg, Some(Path::new("a.jpg"))).is_err());
        assert!(ExportSettings::read(&none, Some(Path::new("a.webp"))).is_err());
    }

    #[test]
    fn every_tool_has_a_schema_and_is_handled() {
        let mut studio = Studio::default();
        for tool in tool_list().as_array().unwrap() {
            let name = tool["name"].as_str().unwrap();
            assert_eq!(tool["inputSchema"]["type"], "object", "{name}");
            // Without a photo every tool answers with an error or a result, never "unknown tool".
            if let Err(error) = studio.call_tool(name, &Map::new()) {
                assert!(!error.contains("unknown tool"), "{name}");
            }
        }
    }
}
