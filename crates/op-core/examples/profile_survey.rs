//! Checks camera-derived corrections against the JPEG embedded in the same file.
//!
//!   cargo run --release -p op-core --example profile_survey -- a.ARW b.CR3 ...
//!
//! For colour: mean Lab error of our render against the JPEG, with the standard matrix
//! and the fitted one. For lenses: how well each candidate correction lines up with the
//! JPEG in the outer part of the frame, and how the light falloff compares.

use flashraw::{Demosaic, FlashRaw, Orientation};
use half::f16;
use image::RgbImage;
use image::imageops::{self, FilterType};
use op_core::look::{base_look as render, fit_camera_matrix};
use op_core::{Adjustments, LensMap, LensProfile, LinearImage, camera_matched_exposure, decode_linear};

const COMPARE_LONG_EDGE: u32 = 1200;

fn main() {
    for path in std::env::args().skip(1) {
        println!("== {path}");
        let bytes = std::fs::read(&path).expect("read file");
        survey(&bytes);
    }
}

fn survey(bytes: &[u8]) {
    let raw = FlashRaw::open(bytes).expect("open RAW");
    let working = decode_linear(&raw, Demosaic::HalfSize).expect("decode");
    let exposure = camera_matched_exposure(&raw, &working).expect("exposure");
    let linear = working.proxy(COMPARE_LONG_EDGE);

    let info = raw.previews().iter().max_by_key(|p| p.width.max(p.height)).expect("preview");
    println!("preview {}x{}, working {}x{}, exposure {exposure:.2}", info.width, info.height, working.width, working.height);
    let mut shown = image::load_from_memory(raw.preview_bytes(info).expect("bytes")).expect("jpeg").into_rgb8();
    if (shown.width() > shown.height()) != (working.width > working.height) {
        shown = match raw.metadata().orientation {
            Orientation::Rotate270 => imageops::rotate270(&shown),
            _ => imageops::rotate90(&shown),
        };
    }
    let shown = imageops::resize(&shown, linear.width, linear.height, FilterType::Triangle);

    colour(&linear, &shown, exposure);
    match LensProfile::from_file(bytes) {
        Some(profile) => lens(&linear, &shown, exposure, &profile),
        None => println!("no lens data"),
    }
}

fn pixel(linear: &LinearImage, index: usize) -> [f32; 3] {
    [0, 1, 2].map(|c| f16::from_bits(linear.pixels[index * 3 + c]).to_f32())
}

fn apply(matrix: &[[f32; 3]; 3], [r, g, b]: [f32; 3]) -> [f32; 3] {
    matrix.map(|row| row[0] * r + row[1] * g + row[2] * b)
}

fn decode_srgb(c: f32) -> f32 {
    if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}

fn lab(rgb: [f32; 3]) -> [f32; 3] {
    let [r, g, b] = rgb.map(|v| v.clamp(0.0, 1.0));
    let xyz = [
        (0.4124 * r + 0.3576 * g + 0.1805 * b) / 0.9505,
        0.2126 * r + 0.7152 * g + 0.0722 * b,
        (0.0193 * r + 0.1192 * g + 0.9505 * b) / 1.089,
    ];
    let f = xyz.map(|t| if t > 0.008856 { t.cbrt() } else { 7.787 * t + 16.0 / 116.0 });
    [116.0 * f[1] - 16.0, 500.0 * (f[0] - f[1]), 200.0 * (f[1] - f[2])]
}

fn colour(linear: &LinearImage, shown: &RgbImage, exposure: f32) {
    let matrix = fit_camera_matrix(linear, shown.as_raw(), exposure);
    println!("fitted matrix {matrix:?}");
    let identity = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
    for (name, m) in [("standard", Some(identity)), ("fitted", matrix)] {
        let Some(m) = m else { continue };
        let (mut total, mut chroma, mut count) = (0.0, 0.0, 0.0);
        for (index, jpeg) in shown.pixels().enumerate() {
            let target = lab(jpeg.0.map(|v| decode_srgb(f32::from(v) / 255.0)));
            let ours = lab(render(apply(&m, pixel(linear, index)), exposure));
            total += (0..3).map(|c| (target[c] - ours[c]).powi(2)).sum::<f32>().sqrt();
            chroma += (1..3).map(|c| (target[c] - ours[c]).powi(2)).sum::<f32>().sqrt();
            count += 1.0;
        }
        println!("{name:>9}: mean dE {:.2}, chroma error {:.2}", total / count, chroma / count);
    }
}

fn luma_of(linear: &LinearImage, exposure: f32) -> Vec<f32> {
    (0..linear.width as usize * linear.height as usize)
        .map(|i| {
            let [r, g, b] = render(pixel(linear, i), exposure);
            0.2126 * r + 0.7152 * g + 0.0722 * b
        })
        .collect()
}

/// Correlation of horizontal and vertical gradients in the outer part of the frame.
fn edge_match(ours: &[f32], jpeg: &[f32], width: usize, height: usize, map: &LensMap) -> f32 {
    let sample = |x: f32, y: f32| -> f32 {
        let (x, y) = (x.clamp(0.0, (width - 2) as f32), y.clamp(0.0, (height - 2) as f32));
        let (x0, y0) = (x as usize, y as usize);
        let (tx, ty) = (x - x0 as f32, y - y0 as f32);
        let at = |px: usize, py: usize| ours[py * width + px];
        let top = at(x0, y0) * (1.0 - tx) + at(x0 + 1, y0) * tx;
        let bottom = at(x0, y0 + 1) * (1.0 - tx) + at(x0 + 1, y0 + 1) * tx;
        top * (1.0 - ty) + bottom * ty
    };
    let (mut ab, mut aa, mut bb) = (0.0f64, 0.0f64, 0.0f64);
    for y in 2..height - 2 {
        for x in 2..width - 2 {
            let (nx, ny) = ((x as f32 + 0.5) / width as f32 - 0.5, (y as f32 + 0.5) / height as f32 - 0.5);
            let (px, py) = (nx * 2.0 * map.aspect[0], ny * 2.0 * map.aspect[1]);
            if px * px + py * py < 0.5 {
                continue;
            }
            let (u, v) = map.source_uv(nx + 0.5, ny + 0.5, 1);
            let (sx, sy) = (u * width as f32 - 0.5, v * height as f32 - 0.5);
            let ours_x = f64::from(sample(sx + 1.0, sy) - sample(sx - 1.0, sy));
            let ours_y = f64::from(sample(sx, sy + 1.0) - sample(sx, sy - 1.0));
            let jpeg_x = f64::from(jpeg[y * width + x + 1] - jpeg[y * width + x - 1]);
            let jpeg_y = f64::from(jpeg[(y + 1) * width + x] - jpeg[(y - 1) * width + x]);
            ab += ours_x * jpeg_x + ours_y * jpeg_y;
            aa += ours_x * ours_x + ours_y * ours_y;
            bb += jpeg_x * jpeg_x + jpeg_y * jpeg_y;
        }
    }
    (ab / (aa * bb).sqrt()) as f32
}

fn lens(linear: &LinearImage, shown: &RgbImage, exposure: f32, profile: &LensProfile) {
    println!("{profile:?}");
    let (width, height) = (linear.width as usize, linear.height as usize);
    let ours = luma_of(linear, exposure);
    let jpeg: Vec<f32> = shown
        .pixels()
        .map(|p| {
            let [r, g, b] = p.0.map(|v| decode_srgb(f32::from(v) / 255.0));
            0.2126 * r + 0.7152 * g + 0.0722 * b
        })
        .collect();

    let enabled = Adjustments { lens_auto: 1.0, ..Default::default() };
    let base = LensMap::new(Some(profile), &enabled, linear.width, linear.height);
    let none = LensMap { distortion: [0.0; 4], scale: 1.0, chroma: [0.0; 4], ..base };
    println!("{:>12} {:.4}", "uncorrected", edge_match(&ours, &jpeg, width, height, &none));
    println!("{:>12} {:.4}  (scale {:.4})", "as read", edge_match(&ours, &jpeg, width, height, &base), base.scale);
    let inverted = LensMap { distortion: base.distortion.map(|v| -v), ..base };
    println!("{:>12} {:.4}", "inverted", edge_match(&ours, &jpeg, width, height, &inverted));

    // The JPEG's crop is unknown, so search for the scale that aligns best.
    for (name, template) in [("as read", base), ("inverted", inverted), ("uncorrected", none)] {
        let best = (-60..=60)
            .map(|step| {
                let scale = 1.0 + step as f32 * 0.001;
                let map = LensMap { scale, ..template };
                (scale, edge_match(&ours, &jpeg, width, height, &map))
            })
            .max_by(|a, b| a.1.total_cmp(&b.1))
            .unwrap();
        println!("{name:>12} best scale {:.3} -> {:.4}", best.0, best.1);
    }

    // Light falloff of the JPEG relative to our uncorrected render, against what we'd apply.
    println!("  r   jpeg/ours   ours gain");
    for band in 2..10 {
        let (low, high) = (band as f32 * 0.1, (band + 1) as f32 * 0.1);
        let (mut a, mut b, mut n) = (0.0f32, 0.0f32, 0.0f32);
        for y in 0..height {
            for x in 0..width {
                let px = ((x as f32 + 0.5) / width as f32 - 0.5) * 2.0 * base.aspect[0];
                let py = ((y as f32 + 0.5) / height as f32 - 0.5) * 2.0 * base.aspect[1];
                let r = px.hypot(py);
                if r >= low && r < high && (0.02..0.8).contains(&jpeg[y * width + x]) && ours[y * width + x] > 0.01 {
                    a += jpeg[y * width + x].ln();
                    b += ours[y * width + x].ln();
                    n += 1.0;
                }
            }
        }
        let r2 = ((low + high) * 0.5).powi(2);
        let [v0, v1, v2, v3] = base.vignette;
        let gain = 1.0 + r2 * (v0 + r2 * (v1 + r2 * (v2 + r2 * v3)));
        if n > 0.0 {
            println!("{:.2}  {:>8.3}  {gain:>8.3}", (low + high) * 0.5, ((a - b) / n).exp());
        }
    }
}
