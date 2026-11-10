//! ICC profiles, read far enough to simulate a print or another display on screen
//! (soft proofing). Handles matrix/TRC RGB, grey, and table-based profiles (lut8,
//! lut16, lutAtoB, lutBtoA), which covers display, working-space and printer profiles.
//!
//! Everything meets in the profile connection space as XYZ relative to D50.

use thiserror::Error;

const D50: [f32; 3] = [0.9642, 1.0, 0.8249];
const HEADER_SIZE: usize = 128;
const MAX_TAGS: usize = 200;
/// More inputs than CMYK are not proofed; it keeps the grid walk small.
const MAX_LUT_INPUTS: usize = 4;
/// Larger curve tables and colour grids than any real profile needs are refused, so a
/// hostile file cannot ask for gigabytes.
const MAX_CURVE_ENTRIES: usize = 1 << 16;
const MAX_CLUT_VALUES: usize = 1 << 24;
/// Steps when inverting a tone response curve by bisection.
const INVERSE_STEPS: usize = 24;

/// sRGB primaries adapted to D50, as in the ICC sRGB profile.
const SRGB_TO_XYZ: [[f32; 3]; 3] = [
    [0.436_074_7, 0.385_064_9, 0.143_080_4],
    [0.222_504_5, 0.716_878_6, 0.060_616_9],
    [0.013_932_2, 0.097_104_5, 0.714_173_3],
];

#[derive(Debug, Error, PartialEq)]
pub enum IccError {
    #[error("不是有效的 ICC 文件")]
    NotIcc,
    #[error("ICC 文件不完整")]
    Truncated,
    #[error("这个 ICC 文件的类型不能用于打样")]
    Unsupported,
}

/// How colours outside the destination's range are brought in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Intent {
    /// Compresses the whole range so relationships between colours survive.
    Perceptual,
    /// Keeps colours that fit exactly and clips the rest.
    Relative,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColourSpace {
    Rgb,
    Cmyk,
    Gray,
}

impl ColourSpace {
    pub fn channels(self) -> usize {
        match self {
            Self::Rgb => 3,
            Self::Cmyk => 4,
            Self::Gray => 1,
        }
    }
}

#[derive(Clone, Debug)]
enum Curve {
    Identity,
    Gamma(f32),
    Table(Vec<f32>),
    /// ICC parametric types 0 to 4: g, a, b, c, d, e, f.
    Parametric(u16, [f32; 7]),
}

impl Curve {
    fn eval(&self, x: f32) -> f32 {
        let x = x.clamp(0.0, 1.0);
        match self {
            Self::Identity => x,
            Self::Gamma(gamma) => x.powf(*gamma),
            Self::Table(table) => interpolate(table, x),
            Self::Parametric(kind, [g, a, b, c, d, e, f]) => match kind {
                0 => x.powf(*g),
                1 => if x >= -b / a { (a * x + b).max(0.0).powf(*g) } else { 0.0 },
                2 => if x >= -b / a { (a * x + b).max(0.0).powf(*g) + c } else { *c },
                3 => if x >= *d { (a * x + b).max(0.0).powf(*g) } else { c * x },
                _ => if x >= *d { (a * x + b).max(0.0).powf(*g) + e } else { c * x + f },
            },
        }
    }

    /// The input giving `y`, for the rising curves display profiles use.
    fn invert(&self, y: f32) -> f32 {
        let (mut low, mut high) = (0.0f32, 1.0f32);
        let rising = self.eval(1.0) >= self.eval(0.0);
        for _ in 0..INVERSE_STEPS {
            let middle = (low + high) / 2.0;
            if (self.eval(middle) < y) == rising {
                low = middle;
            } else {
                high = middle;
            }
        }
        (low + high) / 2.0
    }
}

fn interpolate(table: &[f32], x: f32) -> f32 {
    match table.len() {
        0 => x,
        1 => table[0],
        len => {
            let at = x.clamp(0.0, 1.0) * (len - 1) as f32;
            let index = (at as usize).min(len - 2);
            let t = at - index as f32;
            table[index] + (table[index + 1] - table[index]) * t
        }
    }
}

/// A multi-dimensional colour lookup table with multilinear interpolation.
#[derive(Clone, Debug)]
struct Clut {
    grid: Vec<usize>,
    outputs: usize,
    data: Vec<f32>,
}

impl Clut {
    fn eval(&self, input: &[f32]) -> Vec<f32> {
        let dims = self.grid.len();
        let mut base = Vec::with_capacity(dims);
        let mut fraction = Vec::with_capacity(dims);
        for (dim, &points) in self.grid.iter().enumerate() {
            let at = input[dim].clamp(0.0, 1.0) * (points - 1) as f32;
            let index = (at as usize).min(points.saturating_sub(2));
            base.push(index);
            fraction.push(if points > 1 { at - index as f32 } else { 0.0 });
        }
        let mut out = vec![0.0; self.outputs];
        for corner in 0..(1usize << dims) {
            let mut weight = 1.0;
            let mut offset = 0;
            for dim in 0..dims {
                let high = corner >> (dims - 1 - dim) & 1 == 1;
                weight *= if high { fraction[dim] } else { 1.0 - fraction[dim] };
                let index = (base[dim] + usize::from(high)).min(self.grid[dim] - 1);
                offset = offset * self.grid[dim] + index;
            }
            if weight == 0.0 {
                continue;
            }
            for (channel, value) in out.iter_mut().enumerate() {
                *value += weight * self.data[offset * self.outputs + channel];
            }
        }
        out
    }
}

#[derive(Clone, Debug)]
enum Stage {
    Curves(Vec<Curve>),
    /// A 3x3 matrix with an offset, on normalised values.
    Matrix([[f32; 3]; 3], [f32; 3]),
    Clut(Clut),
}

/// How a table encodes the connection space.
#[derive(Clone, Copy, Debug, PartialEq)]
enum PcsEncoding {
    /// lut8: L 0..100 and a, b -128..127 over 0..1.
    Eight,
    /// lut16, also in version 4 profiles: L tops out at 0xFF00.
    Legacy16,
    /// lutAtoB and lutBtoA.
    Version4,
}

#[derive(Clone, Debug)]
struct Pipeline {
    stages: Vec<Stage>,
    encoding: PcsEncoding,
    inputs: usize,
    outputs: usize,
}

impl Pipeline {
    fn eval(&self, input: &[f32]) -> Vec<f32> {
        let mut values = input.to_vec();
        for stage in &self.stages {
            values = match stage {
                Stage::Curves(curves) => values.iter().zip(curves).map(|(&v, curve)| curve.eval(v)).collect(),
                Stage::Matrix(m, offset) => (0..3).map(|row| m[row][0] * values[0] + m[row][1] * values[1] + m[row][2] * values[2] + offset[row]).collect(),
                Stage::Clut(clut) => clut.eval(&values),
            };
        }
        values
    }
}

#[derive(Clone, Debug)]
struct MatrixShaper {
    curves: [Curve; 3],
    to_xyz: [[f32; 3]; 3],
    from_xyz: [[f32; 3]; 3],
}

/// What a profile turns colours into and back from.
#[derive(Clone, Debug)]
enum Model {
    Matrix(MatrixShaper),
    Gray(Curve),
    Tables { to_pcs: [Option<Pipeline>; 2], from_pcs: [Option<Pipeline>; 2], gamut: Option<Pipeline> },
}

#[derive(Clone, Debug)]
pub struct IccProfile {
    pub name: String,
    pub space: ColourSpace,
    /// True for printers and other output devices.
    pub is_output: bool,
    pcs_is_lab: bool,
    media_white: [f32; 3],
    model: Model,
}

struct Reader<'a> {
    bytes: &'a [u8],
}

impl Reader<'_> {
    fn slice(&self, offset: usize, len: usize) -> Result<&[u8], IccError> {
        self.bytes.get(offset..offset.checked_add(len).ok_or(IccError::Truncated)?).ok_or(IccError::Truncated)
    }
    fn u8(&self, offset: usize) -> Result<u8, IccError> {
        Ok(self.slice(offset, 1)?[0])
    }
    fn u16(&self, offset: usize) -> Result<u16, IccError> {
        let b = self.slice(offset, 2)?;
        Ok(u16::from_be_bytes([b[0], b[1]]))
    }
    fn u32(&self, offset: usize) -> Result<u32, IccError> {
        let b = self.slice(offset, 4)?;
        Ok(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }
    fn s15(&self, offset: usize) -> Result<f32, IccError> {
        Ok(self.u32(offset)? as i32 as f32 / 65536.0)
    }
    fn sig(&self, offset: usize) -> Result<[u8; 4], IccError> {
        let b = self.slice(offset, 4)?;
        Ok([b[0], b[1], b[2], b[3]])
    }
}

impl IccProfile {
    pub fn parse(bytes: &[u8]) -> Result<Self, IccError> {
        let r = Reader { bytes };
        if bytes.len() < HEADER_SIZE || &r.sig(36)? != b"acsp" {
            return Err(IccError::NotIcc);
        }
        let space = match &r.sig(16)? {
            b"RGB " => ColourSpace::Rgb,
            b"CMYK" => ColourSpace::Cmyk,
            b"GRAY" => ColourSpace::Gray,
            _ => return Err(IccError::Unsupported),
        };
        let pcs_is_lab = match &r.sig(20)? {
            b"Lab " => true,
            b"XYZ " => false,
            _ => return Err(IccError::Unsupported),
        };
        let is_output = &r.sig(12)? == b"prtr";
        let count = r.u32(HEADER_SIZE)? as usize;
        if count > MAX_TAGS {
            return Err(IccError::NotIcc);
        }
        let mut tags = Vec::with_capacity(count);
        for i in 0..count {
            let at = HEADER_SIZE + 4 + i * 12;
            let offset = r.u32(at + 4)? as usize;
            if offset >= bytes.len() {
                return Err(IccError::Truncated);
            }
            tags.push((r.sig(at)?, offset));
        }
        let find = |name: &[u8; 4]| tags.iter().find(|(sig, _)| sig == name).map(|&(_, offset)| offset);

        let name = find(b"desc").and_then(|offset| read_text(&r, offset).ok()).unwrap_or_default();
        let media_white = match find(b"wtpt") {
            Some(offset) => [r.s15(offset + 8)?, r.s15(offset + 12)?, r.s15(offset + 16)?],
            None => D50,
        };

        let table = |name: &[u8; 4]| find(name).map(|offset| read_pipeline(&r, offset)).transpose();
        let to_pcs = [table(b"A2B0")?, table(b"A2B1")?];
        let from_pcs = [table(b"B2A0")?, table(b"B2A1")?];
        let model = if to_pcs.iter().any(Option::is_some) && from_pcs.iter().any(Option::is_some) {
            // The evaluators index by these counts, so a table of another shape is refused.
            let device = space.channels();
            let fits = |pipelines: &[Option<Pipeline>], inputs: usize, outputs: usize| {
                pipelines.iter().flatten().all(|p| p.inputs == inputs && p.outputs == outputs)
            };
            let gamut = table(b"gamt")?.filter(|p| p.inputs == 3 && p.outputs >= 1);
            if !fits(&to_pcs, device, 3) || !fits(&from_pcs, 3, device) {
                return Err(IccError::Unsupported);
            }
            Model::Tables { to_pcs, from_pcs, gamut }
        } else if space == ColourSpace::Gray {
            Model::Gray(read_curve(&r, find(b"kTRC").ok_or(IccError::Unsupported)?)?.0)
        } else if space == ColourSpace::Rgb {
            let colorant = |name: &[u8; 4]| -> Result<[f32; 3], IccError> {
                let offset = find(name).ok_or(IccError::Unsupported)?;
                Ok([r.s15(offset + 8)?, r.s15(offset + 12)?, r.s15(offset + 16)?])
            };
            let [red, green, blue] = [colorant(b"rXYZ")?, colorant(b"gXYZ")?, colorant(b"bXYZ")?];
            let to_xyz = std::array::from_fn(|row| [red[row], green[row], blue[row]]);
            let curve = |name: &[u8; 4]| -> Result<Curve, IccError> { Ok(read_curve(&r, find(name).ok_or(IccError::Unsupported)?)?.0) };
            Model::Matrix(MatrixShaper {
                curves: [curve(b"rTRC")?, curve(b"gTRC")?, curve(b"bTRC")?],
                to_xyz,
                from_xyz: invert3(&to_xyz).ok_or(IccError::Unsupported)?,
            })
        } else {
            return Err(IccError::Unsupported);
        };
        Ok(Self { name, space, is_output, pcs_is_lab, media_white, model })
    }

    /// Device values for a PCS colour, and whether it had to be clipped to fit.
    fn to_device(&self, xyz: [f32; 3], intent: Intent) -> (Vec<f32>, bool) {
        match &self.model {
            Model::Matrix(shaper) => {
                let linear = mul3(&shaper.from_xyz, xyz);
                let clipped = linear.iter().any(|&v| !(-1e-3..=1.0 + 1e-3).contains(&v));
                (linear.iter().zip(&shaper.curves).map(|(&v, curve)| curve.invert(v.clamp(0.0, 1.0))).collect(), clipped)
            }
            Model::Gray(curve) => {
                let y = xyz[1];
                (vec![curve.invert(y.clamp(0.0, 1.0))], !(-1e-3..=1.0 + 1e-3).contains(&y))
            }
            Model::Tables { from_pcs, .. } => {
                let pipeline = pick(from_pcs, intent);
                let device = pipeline.eval(&self.encode_pcs(xyz, pipeline.encoding));
                (device.into_iter().map(|v| v.clamp(0.0, 1.0)).collect(), false)
            }
        }
    }

    /// The D50 XYZ colour of device values in 0..=1, relative colorimetric.
    pub fn xyz_of(&self, device: &[f32]) -> [f32; 3] {
        self.to_pcs(device)
    }

    /// The PCS colour of device values, relative colorimetric.
    fn to_pcs(&self, device: &[f32]) -> [f32; 3] {
        match &self.model {
            Model::Matrix(shaper) => mul3(&shaper.to_xyz, std::array::from_fn(|i| shaper.curves[i].eval(device[i]))),
            Model::Gray(curve) => D50.map(|white| white * curve.eval(device[0])),
            Model::Tables { to_pcs, .. } => {
                let pipeline = pick(to_pcs, Intent::Relative);
                let out = pipeline.eval(device);
                self.decode_pcs(&out, pipeline.encoding)
            }
        }
    }

    fn out_of_gamut(&self, xyz: [f32; 3]) -> Option<bool> {
        match &self.model {
            Model::Tables { gamut: Some(gamut), .. } => Some(gamut.eval(&self.encode_pcs(xyz, gamut.encoding))[0] > 0.5),
            _ => None,
        }
    }

    fn encode_pcs(&self, xyz: [f32; 3], encoding: PcsEncoding) -> Vec<f32> {
        if self.pcs_is_lab {
            let [l, a, b] = xyz_to_lab(xyz);
            match encoding {
                PcsEncoding::Legacy16 => vec![l / 100.0 * 65280.0 / 65535.0, (a + 128.0) * 256.0 / 65535.0, (b + 128.0) * 256.0 / 65535.0],
                _ => vec![l / 100.0, (a + 128.0) / 255.0, (b + 128.0) / 255.0],
            }
        } else {
            xyz.iter().map(|v| v * 32768.0 / 65535.0).collect()
        }
    }

    fn decode_pcs(&self, values: &[f32], encoding: PcsEncoding) -> [f32; 3] {
        if self.pcs_is_lab {
            let lab = match encoding {
                PcsEncoding::Legacy16 => [values[0] * 65535.0 / 65280.0 * 100.0, values[1] * 65535.0 / 256.0 - 128.0, values[2] * 65535.0 / 256.0 - 128.0],
                _ => [values[0] * 100.0, values[1] * 255.0 - 128.0, values[2] * 255.0 - 128.0],
            };
            lab_to_xyz(lab)
        } else {
            [0, 1, 2].map(|i| values[i] * 65535.0 / 32768.0)
        }
    }
}

fn pick(pipelines: &[Option<Pipeline>; 2], intent: Intent) -> &Pipeline {
    let wanted = usize::from(intent == Intent::Relative);
    pipelines[wanted].as_ref().or(pipelines[1 - wanted].as_ref()).expect("parse keeps only profiles with both directions")
}

fn read_text(r: &Reader, offset: usize) -> Result<String, IccError> {
    match &r.sig(offset)? {
        b"desc" => {
            let len = r.u32(offset + 8)? as usize;
            let raw = r.slice(offset + 12, len.min(4096))?;
            Ok(String::from_utf8_lossy(raw).trim_end_matches('\0').trim().to_string())
        }
        b"mluc" => {
            let len = r.u32(offset + 20)? as usize;
            let start = offset + r.u32(offset + 24)? as usize;
            let raw = r.slice(start, len.min(4096))?;
            let units: Vec<u16> = raw.chunks_exact(2).map(|c| u16::from_be_bytes([c[0], c[1]])).collect();
            Ok(String::from_utf16_lossy(&units).trim_end_matches('\0').trim().to_string())
        }
        _ => Err(IccError::Unsupported),
    }
}

/// A curve and the number of bytes it takes, padded to four.
fn read_curve(r: &Reader, offset: usize) -> Result<(Curve, usize), IccError> {
    let pad = |len: usize| len.div_ceil(4) * 4;
    match &r.sig(offset)? {
        b"curv" => {
            let count = r.u32(offset + 8)? as usize;
            if count > MAX_CURVE_ENTRIES {
                return Err(IccError::Unsupported);
            }
            let curve = match count {
                0 => Curve::Identity,
                1 => Curve::Gamma(f32::from(r.u16(offset + 12)?) / 256.0),
                _ => {
                    let raw = r.slice(offset + 12, count * 2)?;
                    Curve::Table(raw.chunks_exact(2).map(|c| f32::from(u16::from_be_bytes([c[0], c[1]])) / 65535.0).collect())
                }
            };
            Ok((curve, pad(12 + 2 * count)))
        }
        b"para" => {
            let kind = r.u16(offset + 8)?;
            let count = match kind {
                0 => 1,
                1 => 3,
                2 => 4,
                3 => 5,
                4 => 7,
                _ => return Err(IccError::Unsupported),
            };
            let mut params = [0.0; 7];
            for (i, param) in params.iter_mut().enumerate().take(count) {
                *param = r.s15(offset + 12 + 4 * i)?;
            }
            Ok((Curve::Parametric(kind, params), 12 + 4 * count))
        }
        _ => Err(IccError::Unsupported),
    }
}

fn read_curves(r: &Reader, mut offset: usize, count: usize) -> Result<Vec<Curve>, IccError> {
    let mut curves = Vec::with_capacity(count);
    for _ in 0..count {
        let (curve, size) = read_curve(r, offset)?;
        curves.push(curve);
        offset += size;
    }
    Ok(curves)
}

fn grid_size(grid: &[usize], outputs: usize) -> Result<usize, IccError> {
    let total = grid.iter().try_fold(outputs, |total, &points| if points < 2 { Err(IccError::Unsupported) } else { total.checked_mul(points).ok_or(IccError::Unsupported) })?;
    if total > MAX_CLUT_VALUES { Err(IccError::Unsupported) } else { Ok(total) }
}

fn read_pipeline(r: &Reader, offset: usize) -> Result<Pipeline, IccError> {
    let kind = r.sig(offset)?;
    let (inputs, outputs) = (usize::from(r.u8(offset + 8)?), usize::from(r.u8(offset + 9)?));
    if inputs == 0 || inputs > MAX_LUT_INPUTS || outputs == 0 || outputs > MAX_LUT_INPUTS {
        return Err(IccError::Unsupported);
    }
    match &kind {
        b"mft1" | b"mft2" => {
            let wide = &kind == b"mft2";
            let points = usize::from(r.u8(offset + 10)?);
            let matrix: [[f32; 3]; 3] = {
                let mut m = [[0.0; 3]; 3];
                for (i, value) in m.iter_mut().flatten().enumerate() {
                    *value = r.s15(offset + 12 + 4 * i)?;
                }
                m
            };
            let (in_entries, out_entries, mut at) =
                if wide { (usize::from(r.u16(offset + 48)?), usize::from(r.u16(offset + 50)?), offset + 52) } else { (256, 256, offset + 48) };
            let (size, max) = if wide { (2, 65535.0) } else { (1, 255.0) };
            let value = |at: usize| -> Result<f32, IccError> { Ok(if wide { f32::from(r.u16(at)?) } else { f32::from(r.u8(at)?) } / max) };
            let table = |entries: usize, at: &mut usize| -> Result<Curve, IccError> {
                r.slice(*at, entries * size)?;
                let values = (0..entries).map(|i| value(*at + i * size)).collect::<Result<Vec<_>, _>>()?;
                *at += entries * size;
                Ok(Curve::Table(values))
            };
            let input = (0..inputs).map(|_| table(in_entries, &mut at)).collect::<Result<Vec<_>, _>>()?;
            let grid = vec![points; inputs];
            let total = grid_size(&grid, outputs)?;
            r.slice(at, total * size)?;
            let data = (0..total).map(|i| value(at + i * size)).collect::<Result<Vec<_>, _>>()?;
            at += total * size;
            let output = (0..outputs).map(|_| table(out_entries, &mut at)).collect::<Result<Vec<_>, _>>()?;
            let mut stages = Vec::new();
            let identity = matrix == [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
            if inputs == 3 && !identity {
                stages.push(Stage::Matrix(matrix, [0.0; 3]));
            }
            stages.extend([Stage::Curves(input), Stage::Clut(Clut { grid, outputs, data }), Stage::Curves(output)]);
            Ok(Pipeline { stages, encoding: if wide { PcsEncoding::Legacy16 } else { PcsEncoding::Eight }, inputs, outputs })
        }
        b"mAB " | b"mBA " => {
            let to_pcs = &kind == b"mAB ";
            let element = |at: usize| -> Result<Option<usize>, IccError> { Ok(match r.u32(offset + at)? { 0 => None, relative => Some(offset + relative as usize) }) };
            let (b, matrix, m, clut, a) = (element(12)?, element(16)?, element(20)?, element(24)?, element(28)?);
            // B curves sit on the PCS side; A curves on the device side.
            let (pcs_channels, device_channels) = if to_pcs { (outputs, inputs) } else { (inputs, outputs) };
            let b = b.map(|at| read_curves(r, at, pcs_channels)).transpose()?;
            let m = m.map(|at| read_curves(r, at, pcs_channels)).transpose()?;
            let a = a.map(|at| read_curves(r, at, device_channels)).transpose()?;
            let matrix = matrix
                .map(|at| -> Result<Stage, IccError> {
                    let mut m = [[0.0; 3]; 3];
                    for (i, value) in m.iter_mut().flatten().enumerate() {
                        *value = r.s15(at + 4 * i)?;
                    }
                    Ok(Stage::Matrix(m, [r.s15(at + 36)?, r.s15(at + 40)?, r.s15(at + 44)?]))
                })
                .transpose()?;
            let clut = clut
                .map(|at| -> Result<Clut, IccError> {
                    let grid = (0..inputs).map(|i| r.u8(at + i).map(usize::from)).collect::<Result<Vec<_>, _>>()?;
                    let precision = usize::from(r.u8(at + 16)?);
                    if precision != 1 && precision != 2 {
                        return Err(IccError::Unsupported);
                    }
                    let total = grid_size(&grid, outputs)?;
                    let start = at + 20;
                    r.slice(start, total * precision)?;
                    let data = (0..total)
                        .map(|i| if precision == 1 { r.u8(start + i).map(|v| f32::from(v) / 255.0) } else { r.u16(start + 2 * i).map(|v| f32::from(v) / 65535.0) })
                        .collect::<Result<Vec<_>, _>>()?;
                    Ok(Clut { grid, outputs, data })
                })
                .transpose()?;
            let mut stages = Vec::new();
            let mut push = |stage: Option<Stage>| stages.extend(stage);
            if to_pcs {
                push(a.map(Stage::Curves));
                push(clut.map(Stage::Clut));
                push(m.map(Stage::Curves));
                push(matrix);
                push(b.map(Stage::Curves));
            } else {
                push(b.map(Stage::Curves));
                push(matrix);
                push(m.map(Stage::Curves));
                push(clut.map(Stage::Clut));
                push(a.map(Stage::Curves));
            }
            Ok(Pipeline { stages, encoding: PcsEncoding::Version4, inputs, outputs })
        }
        _ => Err(IccError::Unsupported),
    }
}

fn mul3(m: &[[f32; 3]; 3], v: [f32; 3]) -> [f32; 3] {
    std::array::from_fn(|row| m[row][0] * v[0] + m[row][1] * v[1] + m[row][2] * v[2])
}

fn invert3(m: &[[f32; 3]; 3]) -> Option<[[f32; 3]; 3]> {
    let [[a, b, c], [d, e, f], [g, h, i]] = *m;
    let det = a * (e * i - f * h) - b * (d * i - f * g) + c * (d * h - e * g);
    if det.abs() < 1e-9 {
        return None;
    }
    Some([
        [(e * i - f * h) / det, (c * h - b * i) / det, (b * f - c * e) / det],
        [(f * g - d * i) / det, (a * i - c * g) / det, (c * d - a * f) / det],
        [(d * h - e * g) / det, (b * g - a * h) / det, (a * e - b * d) / det],
    ])
}

pub fn xyz_to_lab(xyz: [f32; 3]) -> [f32; 3] {
    let f = |t: f32| if t > 216.0 / 24389.0 { t.cbrt() } else { (24389.0 / 27.0 * t + 16.0) / 116.0 };
    let [x, y, z] = [0, 1, 2].map(|i| f(xyz[i] / D50[i]));
    [116.0 * y - 16.0, 500.0 * (x - y), 200.0 * (y - z)]
}

pub fn lab_to_xyz([l, a, b]: [f32; 3]) -> [f32; 3] {
    let y = (l + 16.0) / 116.0;
    let finv = |t: f32| if t.powi(3) > 216.0 / 24389.0 { t.powi(3) } else { (116.0 * t - 16.0) * 27.0 / 24389.0 };
    [finv(y + a / 500.0) * D50[0], finv(y) * D50[1], finv(y - b / 200.0) * D50[2]]
}

fn srgb_decode(v: f32) -> f32 {
    if v <= 0.040_45 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
}

fn srgb_encode(v: f32) -> f32 {
    let v = v.clamp(0.0, 1.0);
    if v <= 0.003_130_8 { v * 12.92 } else { 1.055 * v.powf(1.0 / 2.4) - 0.055 }
}

/// How the proof is shown.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ProofOptions {
    pub intent: Intent,
    /// Shows the paper's white and the ink's black as they would print, rather than
    /// stretching them to the screen's white and black.
    pub simulate_paper: bool,
}

/// Colour differences, in ΔE, beyond which a colour counts as out of gamut when the
/// profile does not say.
const GAMUT_TOLERANCE: f32 = 3.0;

/// A `size`^3 RGBA table, red fastest, from an sRGB screen colour to how it would look
/// through `profile`. Alpha is 1 where the colour is outside the profile's gamut.
pub fn proof_lut(profile: &IccProfile, options: ProofOptions, size: u32) -> Vec<f32> {
    let from_xyz = invert3(&SRGB_TO_XYZ).expect("sRGB primaries are independent");
    let paper = [0, 1, 2].map(|i| profile.media_white[i] / D50[i]);
    // Only printers have paper; a display profile's white point is its illuminant.
    let simulate_paper = options.simulate_paper && profile.is_output;
    // Black point compensation: the deepest black the device makes maps to the screen's.
    let ink_black = profile.to_pcs(&profile.to_device([0.0; 3], Intent::Relative).0)[1].clamp(0.0, 0.5);
    let steps = (size.max(2) - 1) as f32;
    let mut out = Vec::with_capacity((size * size * size * 4) as usize);
    for blue in 0..size {
        for green in 0..size {
            for red in 0..size {
                let screen = [red, green, blue].map(|v| srgb_decode(v as f32 / steps));
                let xyz = mul3(&SRGB_TO_XYZ, screen);
                let (device, clipped) = profile.to_device(xyz, options.intent);
                let printed = profile.to_pcs(&device);
                let shown = if simulate_paper {
                    [0, 1, 2].map(|i| printed[i] * paper[i])
                } else {
                    [0, 1, 2].map(|i| (printed[i] - ink_black * D50[i]) / (1.0 - ink_black))
                };
                let outside = profile.out_of_gamut(xyz).unwrap_or_else(|| {
                    clipped || {
                        let exact = profile.to_pcs(&profile.to_device(xyz, Intent::Relative).0);
                        delta_e(xyz_to_lab(xyz), xyz_to_lab(exact)) > GAMUT_TOLERANCE
                    }
                });
                let rgb = mul3(&from_xyz, shown);
                out.extend([srgb_encode(rgb[0]), srgb_encode(rgb[1]), srgb_encode(rgb[2]), f32::from(u8::from(outside))]);
            }
        }
    }
    out
}

/// Puts RGB8 through a table from [`proof_lut`], as the preview does, so a histogram
/// can show the proof.
pub fn apply_proof_lut(lut: &[f32], size: u32, rgb: &mut [u8]) {
    let size = size as usize;
    let last = (size - 1) as f32;
    let at = |r: usize, g: usize, b: usize| ((b * size + g) * size + r) * 4;
    for pixel in rgb.chunks_exact_mut(3) {
        let position = [0, 1, 2].map(|c| f32::from(pixel[c]) / 255.0 * last);
        let low = position.map(|p| (p as usize).min(size - 2));
        let t = [0, 1, 2].map(|c| position[c] - low[c] as f32);
        let mut out = [0.0f32; 3];
        for corner in 0..8 {
            let high = [corner & 1, corner >> 1 & 1, corner >> 2 & 1];
            let weight: f32 = (0..3).map(|c| if high[c] == 1 { t[c] } else { 1.0 - t[c] }).product();
            let index = at(low[0] + high[0], low[1] + high[1], low[2] + high[2]);
            for (channel, value) in out.iter_mut().enumerate() {
                *value += weight * lut[index + channel];
            }
        }
        for (target, value) in pixel.iter_mut().zip(out) {
            *target = (value.clamp(0.0, 1.0) * 255.0).round() as u8;
        }
    }
}

fn delta_e(a: [f32; 3], b: [f32; 3]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal matrix/TRC RGB profile with the given D50 colorants and gamma.
    fn matrix_profile(colorants: [[f32; 3]; 3], gamma: f32) -> Vec<u8> {
        let fixed = |v: f32| ((v * 65536.0).round() as i32).to_be_bytes();
        let xyz_tag = |xyz: [f32; 3]| -> Vec<u8> { [b"XYZ ".to_vec(), vec![0; 4], fixed(xyz[0]).to_vec(), fixed(xyz[1]).to_vec(), fixed(xyz[2]).to_vec()].concat() };
        let curve_tag = [b"curv".to_vec(), vec![0; 4], 1u32.to_be_bytes().to_vec(), ((gamma * 256.0) as u16).to_be_bytes().to_vec(), vec![0; 2]].concat();
        let desc = [b"desc".to_vec(), vec![0; 4], 5u32.to_be_bytes().to_vec(), b"Test\0".to_vec(), vec![0; 3]].concat();
        let tags: Vec<(&[u8; 4], Vec<u8>)> = vec![
            (b"desc", desc),
            (b"wtpt", xyz_tag(D50)),
            (b"rXYZ", xyz_tag(colorants[0])),
            (b"gXYZ", xyz_tag(colorants[1])),
            (b"bXYZ", xyz_tag(colorants[2])),
            (b"rTRC", curve_tag.clone()),
            (b"gTRC", curve_tag.clone()),
            (b"bTRC", curve_tag),
        ];
        let mut header = vec![0u8; HEADER_SIZE];
        header[12..16].copy_from_slice(b"mntr");
        header[16..20].copy_from_slice(b"RGB ");
        header[20..24].copy_from_slice(b"XYZ ");
        header[36..40].copy_from_slice(b"acsp");
        let mut table = (tags.len() as u32).to_be_bytes().to_vec();
        let mut data = Vec::new();
        let start = HEADER_SIZE + 4 + 12 * tags.len();
        for (sig, body) in &tags {
            table.extend_from_slice(*sig);
            table.extend(((start + data.len()) as u32).to_be_bytes());
            table.extend((body.len() as u32).to_be_bytes());
            data.extend(body);
            while data.len() % 4 != 0 {
                data.push(0);
            }
        }
        [header, table, data].concat()
    }

    fn srgb_like() -> Vec<u8> {
        matrix_profile([[0.4361, 0.2225, 0.0139], [0.3851, 0.7169, 0.0971], [0.1431, 0.0606, 0.7142]], 2.2)
    }

    /// Primaries halfway to grey: a deliberately small gamut.
    fn narrow() -> Vec<u8> {
        let srgb = [[0.4361, 0.2225, 0.0139], [0.3851, 0.7169, 0.0971], [0.1431, 0.0606, 0.7142]];
        let third = D50.map(|v| v / 3.0);
        matrix_profile(srgb.map(|c| std::array::from_fn(|i| (c[i] + third[i]) / 2.0)), 2.2)
    }

    fn at(lut: &[f32], size: u32, r: u32, g: u32, b: u32) -> [f32; 4] {
        let i = (((b * size + g) * size + r) * 4) as usize;
        [lut[i], lut[i + 1], lut[i + 2], lut[i + 3]]
    }

    #[test]
    fn rejects_files_that_are_not_profiles() {
        assert_eq!(IccProfile::parse(b"not a profile").unwrap_err(), IccError::NotIcc);
        let mut truncated = srgb_like();
        truncated.truncate(200);
        assert!(IccProfile::parse(&truncated).is_err());
    }

    #[test]
    fn hostile_sizes_are_refused_rather_than_panicking() {
        // A curve claiming two billion entries.
        let mut huge = srgb_like();
        let at = huge.windows(4).position(|w| w == b"curv").unwrap();
        huge[at + 8..at + 12].copy_from_slice(&0x8000_0002u32.to_be_bytes());
        assert!(IccProfile::parse(&huge).is_err());
        // A tag pointing past the end of the file.
        let mut past = srgb_like();
        past[HEADER_SIZE + 4 + 4..HEADER_SIZE + 4 + 8].copy_from_slice(&u32::MAX.to_be_bytes());
        assert!(IccProfile::parse(&past).is_err());
    }

    #[test]
    fn table_profiles_of_the_wrong_shape_are_refused() {
        let Ok(bytes) = std::fs::read("/System/Library/ColorSync/Profiles/Generic CMYK Profile.icc") else { return };
        // Declare the CMYK profile as RGB: its 4-input tables no longer fit.
        let mut wrong = bytes.clone();
        wrong[16..20].copy_from_slice(b"RGB ");
        assert_eq!(IccProfile::parse(&wrong).unwrap_err(), IccError::Unsupported);
    }

    #[test]
    fn paper_simulation_leaves_display_profiles_alone() {
        let profile = IccProfile::parse(&srgb_like()).unwrap();
        let plain = proof_lut(&profile, ProofOptions { intent: Intent::Relative, simulate_paper: false }, 5);
        let paper = proof_lut(&profile, ProofOptions { intent: Intent::Relative, simulate_paper: true }, 5);
        assert_eq!(plain, paper);
    }

    #[test]
    fn reads_the_name_and_kind() {
        let profile = IccProfile::parse(&srgb_like()).unwrap();
        assert_eq!(profile.name, "Test");
        assert_eq!(profile.space, ColourSpace::Rgb);
        assert!(!profile.is_output);
    }

    #[test]
    fn lab_and_xyz_round_trip() {
        for lab in [[50.0, 20.0, -30.0], [90.0, -5.0, 60.0], [5.0, 1.0, 1.0]] {
            let back = xyz_to_lab(lab_to_xyz(lab));
            assert!(delta_e(lab, back) < 1e-3, "{lab:?} -> {back:?}");
        }
    }

    #[test]
    fn proofing_to_an_srgb_like_screen_changes_nothing_and_warns_about_nothing() {
        let profile = IccProfile::parse(&srgb_like()).unwrap();
        let options = ProofOptions { intent: Intent::Relative, simulate_paper: false };
        let lut = proof_lut(&profile, options, 9);
        for (r, g, b) in [(0, 0, 0), (8, 8, 8), (8, 0, 0), (2, 5, 7), (4, 4, 4)] {
            let [pr, pg, pb, warn] = at(&lut, 9, r, g, b);
            for (got, want) in [pr, pg, pb].iter().zip([r, g, b]) {
                assert!((got - want as f32 / 8.0).abs() < 0.01, "({r},{g},{b}): {pr} {pg} {pb}");
            }
            assert_eq!(warn, 0.0, "({r},{g},{b})");
        }
    }

    #[test]
    fn a_small_gamut_dulls_and_flags_saturated_colours_only() {
        let profile = IccProfile::parse(&narrow()).unwrap();
        let lut = proof_lut(&profile, ProofOptions { intent: Intent::Relative, simulate_paper: false }, 9);
        let red = at(&lut, 9, 8, 0, 0);
        assert_eq!(red[3], 1.0, "pure red is out of gamut");
        assert!(red[1] > 0.1, "and comes out less pure: {red:?}");
        let grey = at(&lut, 9, 4, 4, 4);
        assert_eq!(grey[3], 0.0, "grey fits");
        assert!((grey[0] - 0.5).abs() < 0.02 && (grey[1] - 0.5).abs() < 0.02, "{grey:?}");
    }

    #[test]
    fn the_histogram_lookup_matches_the_table() {
        let profile = IccProfile::parse(&narrow()).unwrap();
        let lut = proof_lut(&profile, ProofOptions { intent: Intent::Relative, simulate_paper: false }, 9);
        let mut rgb = [255u8, 0, 0, 128, 128, 128];
        apply_proof_lut(&lut, 9, &mut rgb);
        let red = at(&lut, 9, 8, 0, 0);
        assert_eq!(rgb[..3], [0, 1, 2].map(|c| (red[c] * 255.0).round() as u8));
        assert!(rgb[3].abs_diff(128) <= 3 && rgb[4].abs_diff(128) <= 3, "grey stays grey: {rgb:?}");
    }

    /// Checked against the profile macOS ships, where it exists.
    #[test]
    fn a_system_printer_profile_proofs_plausibly() {
        let Ok(bytes) = std::fs::read("/System/Library/ColorSync/Profiles/Generic CMYK Profile.icc") else { return };
        let profile = IccProfile::parse(&bytes).unwrap();
        assert_eq!(profile.space, ColourSpace::Cmyk);
        assert!(profile.is_output);
        for intent in [Intent::Perceptual, Intent::Relative] {
            let lut = proof_lut(&profile, ProofOptions { intent, simulate_paper: false }, 9);
            let white = at(&lut, 9, 8, 8, 8);
            assert!(white[..3].iter().all(|v| *v > 0.95), "{intent:?} white: {white:?}");
            let black = at(&lut, 9, 0, 0, 0);
            assert!(black[..3].iter().all(|v| *v < 0.08), "{intent:?} black: {black:?}");
            let blue = at(&lut, 9, 0, 0, 8);
            assert_eq!(blue[3], 1.0, "screen blue cannot be printed");
            let skin = at(&lut, 9, 7, 5, 4);
            assert_eq!(skin[3], 0.0, "a skin tone can: {skin:?}");
        }
        let paper = at(&proof_lut(&profile, ProofOptions { intent: Intent::Relative, simulate_paper: true }, 9), 9, 8, 8, 8);
        assert!(paper[..3].iter().any(|v| *v < 0.99), "paper white is not screen white: {paper:?}");
    }
}
