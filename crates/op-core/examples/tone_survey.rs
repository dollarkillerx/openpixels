//! Measures the tone curve a camera applies, by comparing its RAW data with the JPEG
//! embedded in the same file. Used to calibrate the default look of RAW files.
//!
//!   cargo run --release -p op-core --example tone_survey -- a.CR3 b.NEF ...
//!
//! Prints one row per file: the median sRGB output of the camera JPEG for each
//! half-stop of scene-linear luminance, relative to sensor saturation.

use flashraw::{Demosaic, FlashRaw, Orientation};
use half::f16;
use image::imageops::{self, FilterType};
use op_core::look::look_curve;
use op_core::{camera_matched_exposure, decode_linear};

const COMPARE_LONG_EDGE: u32 = 480;
const FIRST_STOP: f32 = -9.0;
const STOPS_PER_BIN: f32 = 0.5;
const BINS: usize = 19;
const MIN_SAMPLES: usize = 40;

fn main() {
    print!("{:<34}", "EV");
    for bin in 0..BINS {
        print!("{:>6.1}", FIRST_STOP + bin as f32 * STOPS_PER_BIN);
    }
    println!();
    for path in std::env::args().skip(1) {
        survey(&path);
    }
}

fn survey(path: &str) {
    let bytes = std::fs::read(path).expect("read file");
    let raw = FlashRaw::open(&bytes[..]).expect("open RAW");
    let linear = decode_linear(&raw, Demosaic::HalfSize).expect("decode").proxy(COMPARE_LONG_EDGE);

    let preview_info = raw.preview().expect("embedded preview");
    let preview_bytes = raw.preview_bytes(preview_info).expect("preview bytes");
    let mut preview = image::load_from_memory(preview_bytes).expect("decode preview").into_rgb8();
    match raw.metadata().orientation {
        Orientation::Rotate90 if preview.width() > preview.height() => preview = imageops::rotate90(&preview),
        Orientation::Rotate270 if preview.width() > preview.height() => preview = imageops::rotate270(&preview),
        _ => {}
    }
    let preview = imageops::resize(&preview, linear.width, linear.height, FilterType::Triangle);

    let mut bins: Vec<Vec<u8>> = vec![Vec::new(); BINS];
    for (rgb, jpeg) in linear.pixels.chunks_exact(3).zip(preview.pixels()) {
        let [r, g, b] = [rgb[0], rgb[1], rgb[2]].map(|bits| f16::from_bits(bits).to_f32());
        let luma = 0.2126 * r + 0.7152 * g + 0.0722 * b;
        if luma <= 0.0 {
            continue;
        }
        let bin = ((luma.log2() - FIRST_STOP) / STOPS_PER_BIN).round();
        if (0.0..BINS as f32).contains(&bin) {
            let [jr, jg, jb] = jpeg.0.map(f32::from);
            bins[bin as usize].push((0.2126 * jr + 0.7152 * jg + 0.0722 * jb).round() as u8);
        }
    }

    let name = std::path::Path::new(path).file_name().unwrap().to_string_lossy();
    print!("{:<34}", name.chars().take(32).collect::<String>());
    for bin in &mut bins {
        if bin.len() < MIN_SAMPLES {
            print!("{:>6}", "-");
        } else {
            bin.sort_unstable();
            print!("{:>6}", bin[bin.len() / 2]);
        }
    }
    println!();

    let half = decode_linear(&raw, Demosaic::HalfSize).expect("decode");
    let exposure = camera_matched_exposure(&raw, &half).expect("estimate exposure");
    print!("{:<34}", format!("  model at {exposure:+.2} EV"));
    let white = look_curve(2.0f32.powf(exposure));
    for (index, bin) in bins.iter().enumerate() {
        if bin.len() < MIN_SAMPLES {
            print!("{:>6}", "-");
            continue;
        }
        let stops = FIRST_STOP + index as f32 * STOPS_PER_BIN + exposure;
        let shown = (look_curve(2.0f32.powf(stops)) / white).clamp(0.0, 1.0);
        let encoded = if shown <= 0.003_130_8 { shown * 12.92 } else { 1.055 * shown.powf(1.0 / 2.4) - 0.055 };
        print!("{:>6.0}", encoded * 255.0);
    }
    println!();
}
