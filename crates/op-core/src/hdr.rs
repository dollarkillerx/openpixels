//! HDR delivery: Ultra HDR JPEGs, which carry an SDR picture and a gain map that lifts it
//! to HDR on screens that can show it, and OpenEXR files of linear light.

use std::io::Write;

use image::ImageEncoder;
use image::codecs::jpeg::JpegEncoder;

/// The gain map is stored at this fraction of the picture's size in each direction.
pub const GAIN_MAP_REDUCTION: u32 = 2;
const GAIN_MAP_QUALITY: u8 = 90;

/// One channel of gain codes, block-averaged by `factor`, from the three equal channels
/// the export produces. Returns `(codes, width, height)`.
pub fn gain_map_plane(codes: &[u8], width: u32, height: u32, factor: u32) -> (Vec<u8>, u32, u32) {
    let factor = factor.max(1);
    let (out_w, out_h) = (width.div_ceil(factor), height.div_ceil(factor));
    let mut out = Vec::with_capacity(out_w as usize * out_h as usize);
    for by in 0..out_h {
        for bx in 0..out_w {
            let (mut total, mut count) = (0u32, 0u32);
            for y in by * factor..((by + 1) * factor).min(height) {
                for x in bx * factor..((bx + 1) * factor).min(width) {
                    total += u32::from(codes[(y as usize * width as usize + x as usize) * 3]);
                    count += 1;
                }
            }
            out.push(((total + count / 2) / count.max(1)) as u8);
        }
    }
    (out, out_w, out_h)
}

/// Encodes a gain map plane as a greyscale JPEG.
pub fn encode_gain_map(plane: &[u8], width: u32, height: u32) -> image::ImageResult<Vec<u8>> {
    let mut out = Vec::new();
    JpegEncoder::new_with_quality(&mut out, GAIN_MAP_QUALITY).write_image(plane, width, height, image::ExtendedColorType::L8)?;
    Ok(out)
}

fn segment(marker: u8, payload: &[u8]) -> Vec<u8> {
    let length = u16::try_from(payload.len() + 2).expect("a JPEG segment holds under 64 KB");
    let mut out = vec![0xFF, marker];
    out.extend_from_slice(&length.to_be_bytes());
    out.extend_from_slice(payload);
    out
}

fn xmp_segment(xml: &str) -> Vec<u8> {
    let mut payload = b"http://ns.adobe.com/xap/1.0/\0".to_vec();
    payload.extend_from_slice(xml.as_bytes());
    segment(0xE1, &payload)
}

/// Where segments can be added to a JPEG: after SOI and an Exif APP1, which must come first.
fn insertion_point(jpeg: &[u8]) -> usize {
    let mut at = 2;
    if jpeg.len() > at + 10 && jpeg[at] == 0xFF && jpeg[at + 1] == 0xE1 && &jpeg[at + 4..at + 10] == b"Exif\0\0" {
        at += 2 + usize::from(u16::from_be_bytes([jpeg[at + 2], jpeg[at + 3]]));
    }
    at
}

fn number(value: f32) -> String {
    format!("{value:.6}").trim_end_matches('0').trim_end_matches('.').to_string()
}

/// The Multi-Picture Format index naming the primary picture and the gain map, as an APP2
/// segment. Offsets count from the TIFF header inside the segment.
fn mpf_segment(primary_length: u32, gain_map_length: u32, gain_map_offset: u32) -> Vec<u8> {
    const ENTRIES_AT: u32 = 8 + 2 + 3 * 12 + 4;
    let mut tiff = b"MM\0\x2A".to_vec();
    tiff.extend_from_slice(&8u32.to_be_bytes());
    tiff.extend_from_slice(&3u16.to_be_bytes());
    let mut entry = |tag: u16, kind: u16, count: u32, value: [u8; 4]| {
        tiff.extend_from_slice(&tag.to_be_bytes());
        tiff.extend_from_slice(&kind.to_be_bytes());
        tiff.extend_from_slice(&count.to_be_bytes());
        tiff.extend_from_slice(&value);
    };
    entry(0xB000, 7, 4, *b"0100");
    entry(0xB001, 4, 1, 2u32.to_be_bytes());
    entry(0xB002, 7, 32, ENTRIES_AT.to_be_bytes());
    tiff.extend_from_slice(&0u32.to_be_bytes());
    debug_assert_eq!(tiff.len() as u32, ENTRIES_AT);
    // The primary picture, a baseline JPEG, then the gain map, which depends on nothing.
    for (attribute, length, offset) in [(0x0003_0000u32, primary_length, 0u32), (0, gain_map_length, gain_map_offset)] {
        tiff.extend_from_slice(&attribute.to_be_bytes());
        tiff.extend_from_slice(&length.to_be_bytes());
        tiff.extend_from_slice(&offset.to_be_bytes());
        tiff.extend_from_slice(&[0; 4]);
    }
    let mut payload = b"MPF\0".to_vec();
    payload.extend_from_slice(&tiff);
    segment(0xE2, &payload)
}

/// Joins an SDR JPEG and a gain map JPEG into an Ultra HDR JPEG (Google's Ultra HDR 1.0,
/// with Adobe's `hdrgm` gain map metadata). The gain map codes are `log2(gain)` over
/// `log2(headroom)`, with no offsets and a gamma of 1, so HDR = SDR x gain. Viewers that
/// do not know gain maps show the SDR picture.
pub fn ultra_hdr_jpeg(primary: &[u8], gain_map: &[u8], headroom: f32) -> Vec<u8> {
    let stops = number(headroom.max(1.0).log2());
    let gain_xmp = xmp_segment(&format!(
        concat!(
            r#"<x:xmpmeta xmlns:x="adobe:ns:meta/" x:xmptk="OpenPixels">"#,
            r#"<rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">"#,
            r#"<rdf:Description rdf:about="" xmlns:hdrgm="http://ns.adobe.com/hdr-gain-map/1.0/""#,
            r#" hdrgm:Version="1.0" hdrgm:GainMapMin="0" hdrgm:GainMapMax="{stops}" hdrgm:Gamma="1""#,
            r#" hdrgm:OffsetSDR="0" hdrgm:OffsetHDR="0" hdrgm:HDRCapacityMin="0" hdrgm:HDRCapacityMax="{stops}""#,
            r#" hdrgm:BaseRenditionIsHDR="False"/></rdf:RDF></x:xmpmeta>"#,
        ),
        stops = stops
    ));
    let at = insertion_point(gain_map);
    let mut gain = gain_map[..at].to_vec();
    gain.extend_from_slice(&gain_xmp);
    gain.extend_from_slice(&gain_map[at..]);

    let primary_xmp = xmp_segment(&format!(
        concat!(
            r#"<x:xmpmeta xmlns:x="adobe:ns:meta/" x:xmptk="OpenPixels">"#,
            r#"<rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">"#,
            r#"<rdf:Description rdf:about="" xmlns:Container="http://ns.google.com/photos/1.0/container/""#,
            r#" xmlns:Item="http://ns.google.com/photos/1.0/container/item/" xmlns:hdrgm="http://ns.adobe.com/hdr-gain-map/1.0/""#,
            r#" hdrgm:Version="1.0"><Container:Directory><rdf:Seq>"#,
            r#"<rdf:li rdf:parseType="Resource"><Container:Item Item:Semantic="Primary" Item:Mime="image/jpeg"/></rdf:li>"#,
            r#"<rdf:li rdf:parseType="Resource"><Container:Item Item:Semantic="GainMap" Item:Mime="image/jpeg" Item:Length="{length}"/></rdf:li>"#,
            r#"</rdf:Seq></Container:Directory></rdf:Description></rdf:RDF></x:xmpmeta>"#,
        ),
        length = gain.len()
    ));
    let at = insertion_point(primary);
    // The MPF segment's size does not depend on the numbers in it, so lay it out once to measure.
    let mpf_length = mpf_segment(0, 0, 0).len();
    let primary_length = primary.len() + primary_xmp.len() + mpf_length;
    let tiff_at = at + primary_xmp.len() + 4 + 4;
    let mpf = mpf_segment(primary_length as u32, gain.len() as u32, (primary_length - tiff_at) as u32);

    let mut out = Vec::with_capacity(primary_length + gain.len());
    out.extend_from_slice(&primary[..at]);
    out.extend_from_slice(&primary_xmp);
    out.extend_from_slice(&mpf);
    out.extend_from_slice(&primary[at..]);
    debug_assert_eq!(out.len(), primary_length);
    out.extend_from_slice(&gain);
    out
}

/// Scanlines per OpenEXR chunk with ZIP compression.
const EXR_ZIP_LINES: u32 = 16;

fn exr_attribute(header: &mut Vec<u8>, name: &str, kind: &str, value: &[u8]) {
    header.extend_from_slice(name.as_bytes());
    header.push(0);
    header.extend_from_slice(kind.as_bytes());
    header.push(0);
    header.extend_from_slice(&(value.len() as i32).to_le_bytes());
    header.extend_from_slice(value);
}

/// OpenEXR's ZIP filter: bytes split into odd and even halves, then each stored as its
/// difference from the one before, then deflated.
fn exr_zip(raw: &[u8]) -> Vec<u8> {
    let half = raw.len().div_ceil(2);
    let mut filtered = vec![0u8; raw.len()];
    for (index, &byte) in raw.iter().enumerate() {
        filtered[if index % 2 == 0 { index / 2 } else { half + index / 2 }] = byte;
    }
    let mut previous = filtered.first().copied().unwrap_or(0);
    for byte in filtered.iter_mut().skip(1) {
        let current = *byte;
        *byte = current.wrapping_sub(previous).wrapping_add(128);
        previous = current;
    }
    let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(&filtered).expect("writing to memory cannot fail");
    encoder.finish().expect("writing to memory cannot fail")
}

/// Writes interleaved linear RGB as a scanline OpenEXR file with ZIP compression, in half
/// (16-bit) or full (32-bit) floats, tagged with the colour space's chromaticities.
pub fn encode_exr(rgb: &[f32], width: u32, height: u32, full_float: bool, chromaticities: [f32; 8]) -> Vec<u8> {
    assert_eq!(rgb.len(), width as usize * height as usize * 3);
    let mut out = vec![0x76, 0x2F, 0x31, 0x01, 2, 0, 0, 0];
    let mut channels = Vec::new();
    // Channels are listed, and stored, in name order.
    for name in ["B", "G", "R"] {
        channels.extend_from_slice(name.as_bytes());
        channels.push(0);
        channels.extend_from_slice(&(if full_float { 2i32 } else { 1 }).to_le_bytes());
        channels.extend_from_slice(&[0, 0, 0, 0]);
        channels.extend_from_slice(&1i32.to_le_bytes());
        channels.extend_from_slice(&1i32.to_le_bytes());
    }
    channels.push(0);
    let window: Vec<u8> = [0i32, 0, width as i32 - 1, height as i32 - 1].iter().flat_map(|v| v.to_le_bytes()).collect();
    exr_attribute(&mut out, "channels", "chlist", &channels);
    exr_attribute(&mut out, "chromaticities", "chromaticities", &chromaticities.iter().flat_map(|v| v.to_le_bytes()).collect::<Vec<_>>());
    exr_attribute(&mut out, "compression", "compression", &[3]);
    exr_attribute(&mut out, "dataWindow", "box2i", &window);
    exr_attribute(&mut out, "displayWindow", "box2i", &window);
    exr_attribute(&mut out, "lineOrder", "lineOrder", &[0]);
    exr_attribute(&mut out, "pixelAspectRatio", "float", &1f32.to_le_bytes());
    exr_attribute(&mut out, "screenWindowCenter", "v2f", &[0u8; 8]);
    exr_attribute(&mut out, "screenWindowWidth", "float", &1f32.to_le_bytes());
    out.push(0);

    let chunks = height.div_ceil(EXR_ZIP_LINES) as usize;
    let table_at = out.len();
    out.resize(table_at + chunks * 8, 0);
    let mut raw = Vec::new();
    for chunk in 0..chunks {
        let first = chunk as u32 * EXR_ZIP_LINES;
        raw.clear();
        for y in first..(first + EXR_ZIP_LINES).min(height) {
            let row = &rgb[y as usize * width as usize * 3..][..width as usize * 3];
            for channel in [2, 1, 0] {
                for pixel in row.chunks_exact(3) {
                    if full_float {
                        raw.extend_from_slice(&pixel[channel].to_le_bytes());
                    } else {
                        raw.extend_from_slice(&half::f16::from_f32(pixel[channel]).to_le_bytes());
                    }
                }
            }
        }
        let packed = exr_zip(&raw);
        // A chunk that would not shrink is stored as it is, which readers recognise by its size.
        let data = if packed.len() < raw.len() { &packed } else { &raw };
        let at = out.len() as u64;
        out[table_at + chunk * 8..][..8].copy_from_slice(&at.to_le_bytes());
        out.extend_from_slice(&(first as i32).to_le_bytes());
        out.extend_from_slice(&(data.len() as i32).to_le_bytes());
        out.extend_from_slice(data);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn jpeg(width: u32, height: u32) -> Vec<u8> {
        let mut out = Vec::new();
        JpegEncoder::new_with_quality(&mut out, 90).write_image(&vec![128u8; (width * height * 3) as usize], width, height, image::ExtendedColorType::Rgb8).unwrap();
        out
    }

    #[test]
    fn an_ultra_hdr_jpeg_indexes_its_gain_map() {
        let primary = jpeg(16, 8);
        let (plane, w, h) = gain_map_plane(&[200u8; 16 * 8 * 3], 16, 8, GAIN_MAP_REDUCTION);
        assert_eq!((w, h, plane[0]), (8, 4, 200));
        let gain = encode_gain_map(&plane, w, h).unwrap();
        let file = ultra_hdr_jpeg(&primary, &gain, 4.0);
        // Still an ordinary JPEG to anything that only reads the first picture.
        assert!(image::load_from_memory(&file).is_ok());
        let text = String::from_utf8_lossy(&file);
        assert!(text.contains(r#"hdrgm:GainMapMax="2""#));
        // The MPF entry for the gain map points at its SOI.
        let mpf = file.windows(4).position(|w| w == b"MPF\0").unwrap() + 4;
        let entry = mpf + 50 + 16;
        let offset = u32::from_be_bytes(file[entry + 8..entry + 12].try_into().unwrap()) as usize;
        let length = u32::from_be_bytes(file[entry + 4..entry + 8].try_into().unwrap()) as usize;
        assert_eq!(&file[mpf + offset..mpf + offset + 2], &[0xFF, 0xD8]);
        assert_eq!(mpf + offset + length, file.len());
        let primary_length = u32::from_be_bytes(file[mpf + 50 + 4..mpf + 50 + 8].try_into().unwrap()) as usize;
        assert_eq!(primary_length, mpf + offset);
        assert!(text.contains(&format!(r#"Item:Length="{length}""#)));
    }

    #[test]
    fn an_exr_file_has_its_header_and_one_chunk_per_sixteen_lines() {
        let rgb: Vec<f32> = (0..20 * 33 * 3).map(|i| i as f32 / 100.0).collect();
        let file = encode_exr(&rgb, 20, 33, false, crate::OutputSpace::Srgb.chromaticities());
        assert_eq!(&file[..4], &[0x76, 0x2F, 0x31, 0x01]);
        let text = String::from_utf8_lossy(&file);
        assert!(text.contains("chromaticities") && text.contains("dataWindow"));
        // The offset table follows the header; the first chunk starts with scanline 0, the third with 32.
        let table = file.windows(14).position(|w| w == b"screenWindowWidth\0float\0"[..14].as_ref()).unwrap() + 18 + 6 + 4 + 4 + 1;
        let third = u64::from_le_bytes(file[table + 16..table + 24].try_into().unwrap()) as usize;
        assert_eq!(i32::from_le_bytes(file[third..third + 4].try_into().unwrap()), 32);
    }

    #[test]
    fn the_zip_filter_round_trips() {
        let raw: Vec<u8> = (0..1000u32).map(|i| (i * 7 % 251) as u8).collect();
        let packed = exr_zip(&raw);
        let mut inflated = Vec::new();
        flate2::read::ZlibDecoder::new(&packed[..]).read_to_end(&mut inflated).unwrap();
        for i in 1..inflated.len() {
            inflated[i] = inflated[i].wrapping_add(inflated[i - 1]).wrapping_sub(128);
        }
        let half = raw.len().div_ceil(2);
        let restored: Vec<u8> = (0..raw.len()).map(|i| if i % 2 == 0 { inflated[i / 2] } else { inflated[half + i / 2] }).collect();
        assert_eq!(restored, raw);
    }

    use std::io::Read;
}
