//! Demosaicing for Fujifilm's 6x6 X-Trans sensors.
//!
//! Green is filled in first, from nearby greens weighted towards those that agree with
//! each other, so interpolation does not cross edges. Red and blue then follow as
//! colour differences from green, weighted towards neighbours of similar brightness.

/// Offsets of same-colour photosites, nearest first.
type Neighbours = Vec<(isize, isize, f32)>;

const GREEN: u8 = 1;
const GREEN_RADIUS: isize = 2;
const COLOUR_RADIUS: isize = 3;
const MAX_COLOUR_NEIGHBOURS: usize = 6;
/// Keeps weights finite on flat areas; a small fraction of full scale.
const SOFTENING: f32 = 0.01;

fn neighbours_for(pattern: &[[u8; 6]; 6], phase_x: usize, phase_y: usize, channel: u8, radius: isize) -> Neighbours {
    let mut found: Neighbours = Vec::new();
    for dy in -radius..=radius {
        for dx in -radius..=radius {
            if dx == 0 && dy == 0 {
                continue;
            }
            let x = (phase_x as isize + dx).rem_euclid(6) as usize;
            let y = (phase_y as isize + dy).rem_euclid(6) as usize;
            if pattern[y][x] == channel {
                found.push((dx, dy, ((dx * dx + dy * dy) as f32).sqrt()));
            }
        }
    }
    found.sort_by(|a, b| a.2.total_cmp(&b.2));
    found
}

/// Demosaics a block whose first sample is at `origin` in the sensor.
pub(crate) fn demosaic_xtrans_block(
    mosaic: &[f32],
    width: usize,
    height: usize,
    pattern: &[[u8; 6]; 6],
    origin: (usize, usize),
) -> [Vec<f32>; 3] {
    let phase_of = |x: usize, y: usize| ((origin.0 + x) % 6, (origin.1 + y) % 6);
    let channel_of = |x: usize, y: usize| {
        let (px, py) = phase_of(x, y);
        pattern[py][px]
    };
    // The neighbourhoods repeat with the pattern, so they are worked out once per phase.
    let table = |channel: u8, radius: isize, limit: usize| -> Vec<Neighbours> {
        let mut all = Vec::with_capacity(36);
        for py in 0..6 {
            for px in 0..6 {
                let mut list = neighbours_for(pattern, px, py, channel, radius);
                list.truncate(limit);
                all.push(list);
            }
        }
        all
    };
    let green_table = table(GREEN, GREEN_RADIUS, usize::MAX);
    let red_table = table(0, COLOUR_RADIUS, MAX_COLOUR_NEIGHBOURS);
    let blue_table = table(2, COLOUR_RADIUS, MAX_COLOUR_NEIGHBOURS);

    let inside = |x: isize, y: isize| x >= 0 && y >= 0 && x < width as isize && y < height as isize;
    let index = |x: isize, y: isize| y as usize * width + x as usize;

    let mut green = vec![0.0f32; width * height];
    for y in 0..height {
        for x in 0..width {
            let here = y * width + x;
            if channel_of(x, y) == GREEN {
                green[here] = mosaic[here];
                continue;
            }
            let (px, py) = phase_of(x, y);
            let list = &green_table[py * 6 + px];
            let (mut total, mut count) = (0.0, 0.0);
            for &(dx, dy, _) in list {
                let (nx, ny) = (x as isize + dx, y as isize + dy);
                if inside(nx, ny) {
                    total += mosaic[index(nx, ny)];
                    count += 1.0;
                }
            }
            if count == 0.0 {
                green[here] = mosaic[here];
                continue;
            }
            let mean = total / count;
            let (mut weighted, mut weights) = (0.0, 0.0);
            for &(dx, dy, distance) in list {
                let (nx, ny) = (x as isize + dx, y as isize + dy);
                if inside(nx, ny) {
                    let value = mosaic[index(nx, ny)];
                    let weight = 1.0 / (distance * (SOFTENING + (value - mean).abs()));
                    weighted += value * weight;
                    weights += weight;
                }
            }
            green[here] = weighted / weights;
        }
    }

    let fill = |channel: u8, lists: &[Neighbours]| -> Vec<f32> {
        let mut out = vec![0.0f32; width * height];
        for y in 0..height {
            for x in 0..width {
                let here = y * width + x;
                if channel_of(x, y) == channel {
                    out[here] = mosaic[here];
                    continue;
                }
                let (px, py) = phase_of(x, y);
                let (mut weighted, mut weights) = (0.0, 0.0);
                for &(dx, dy, distance) in &lists[py * 6 + px] {
                    let (nx, ny) = (x as isize + dx, y as isize + dy);
                    if inside(nx, ny) {
                        let at = index(nx, ny);
                        let weight = 1.0 / (distance * (SOFTENING + (green[at] - green[here]).abs()));
                        weighted += (mosaic[at] - green[at]) * weight;
                        weights += weight;
                    }
                }
                out[here] = if weights > 0.0 { green[here] + weighted / weights } else { green[here] };
            }
        }
        out
    };

    let red = fill(0, &red_table);
    let blue = fill(2, &blue_table);
    [red, green, blue]
}

#[cfg(test)]
mod tests {
    use super::*;

    const PATTERN: [[u8; 6]; 6] = [
        [1, 2, 1, 1, 0, 1],
        [0, 1, 0, 2, 1, 2],
        [1, 2, 1, 1, 0, 1],
        [1, 0, 1, 1, 2, 1],
        [2, 1, 2, 0, 1, 0],
        [1, 0, 1, 1, 2, 1],
    ];

    fn scene(x: usize, y: usize) -> [f32; 3] {
        let (fx, fy) = (x as f32, y as f32);
        let luma = 0.5 + 0.3 * ((fx * 0.3).sin() * (fy * 0.05).cos());
        let edge = if (fx * 0.7 + fy * 0.3) as i32 % 60 < 30 { 1.0 } else { 0.5 };
        [luma * edge, luma * edge * 0.8, luma * edge * 0.6]
    }

    fn mosaic_of(width: usize, height: usize) -> Vec<f32> {
        let mut out = Vec::with_capacity(width * height);
        for y in 0..height {
            for x in 0..width {
                out.push(scene(x, y)[PATTERN[y % 6][x % 6] as usize]);
            }
        }
        out
    }

    /// The decoder's approach: average nearby samples of the same colour.
    fn same_colour_average(mosaic: &[f32], width: usize, height: usize) -> [Vec<f32>; 3] {
        let plane = |channel: u8| {
            let mut out = vec![0.0; width * height];
            for y in 3..height - 3 {
                for x in 3..width - 3 {
                    let (mut total, mut count) = (0.0, 0.0);
                    for ny in y - 3..=y + 3 {
                        for nx in x - 3..=x + 3 {
                            if PATTERN[ny % 6][nx % 6] == channel {
                                total += mosaic[ny * width + nx];
                                count += 1.0;
                            }
                        }
                    }
                    out[y * width + x] = total / count;
                }
            }
            out
        };
        [plane(0), plane(1), plane(2)]
    }

    fn psnr(result: &[Vec<f32>; 3], width: usize, height: usize) -> f32 {
        let (mut error, mut samples) = (0.0f64, 0.0f64);
        for y in 8..height - 8 {
            for x in 8..width - 8 {
                let truth = scene(x, y);
                for channel in 0..3 {
                    let difference = result[channel][y * width + x] - truth[channel];
                    error += f64::from(difference * difference);
                    samples += 1.0;
                }
            }
        }
        (-10.0 * (error / samples).log10()) as f32
    }

    #[test]
    fn beats_same_colour_averaging() {
        let (width, height) = (240, 240);
        let mosaic = mosaic_of(width, height);
        let ours = demosaic_xtrans_block(&mosaic, width, height, &PATTERN, (0, 0));
        let baseline = same_colour_average(&mosaic, width, height);
        let (ours_db, baseline_db) = (psnr(&ours, width, height), psnr(&baseline, width, height));
        assert!(ours_db > baseline_db + 2.0, "ours {ours_db:.2} dB, average {baseline_db:.2} dB");
    }

    #[test]
    fn flat_colour_is_reconstructed_exactly() {
        let colour = [0.6f32, 0.3, 0.15];
        let (width, height) = (36, 36);
        let mosaic: Vec<f32> = (0..height)
            .flat_map(|y| (0..width).map(move |x| colour[PATTERN[y % 6][x % 6] as usize]))
            .collect();
        let result = demosaic_xtrans_block(&mosaic, width, height, &PATTERN, (0, 0));
        for (channel, plane) in result.iter().enumerate() {
            for value in plane {
                assert!((value - colour[channel]).abs() < 1e-4, "channel {channel}: {value}");
            }
        }
    }

    #[test]
    fn the_origin_keeps_the_pattern_phase() {
        let (width, height) = (60, 60);
        let mosaic = mosaic_of(width, height);
        let full = demosaic_xtrans_block(&mosaic, width, height, &PATTERN, (0, 0));

        let (x0, y0, w, h) = (7, 13, 40, 40);
        let block: Vec<f32> = (0..h)
            .flat_map(|row| mosaic[(y0 + row) * width + x0..(y0 + row) * width + x0 + w].to_vec())
            .collect();
        let part = demosaic_xtrans_block(&block, w, h, &PATTERN, (x0, y0));
        // Away from the block's edges the neighbourhoods are identical.
        let inner = |x: usize, y: usize| part[1][y * w + x];
        assert!((inner(20, 20) - full[1][(y0 + 20) * width + x0 + 20]).abs() < 1e-6);
    }
}
