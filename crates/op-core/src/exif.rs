//! Writes an EXIF block for exported JPEGs: the capture details from the source file,
//! and a sRGB colour space tag, with orientation reset because pixels are already upright.

use flashraw::FlashRaw;

const SOFTWARE: &str = "OpenPixels";
const TYPE_ASCII: u16 = 2;
const TYPE_SHORT: u16 = 3;
const TYPE_LONG: u16 = 4;
const TYPE_RATIONAL: u16 = 5;
const TYPE_SRATIONAL: u16 = 10;
const EXIF_IFD_POINTER: u16 = 34665;

const MAX_IFD_ENTRIES: usize = 1024;
const MAX_VALUE_BYTES: usize = 4096;
/// Make, model, capture time, artist and copyright.
const COPIED_MAIN_TAGS: [u16; 5] = [271, 272, 306, 315, 33432];
/// Exposure, aperture, ISO, capture times, exposure compensation, focal length and lens.
const COPIED_DETAIL_TAGS: [u16; 9] = [33434, 33437, 34855, 36867, 36868, 37380, 37386, 42035, 42036];

struct Entry {
    tag: u16,
    kind: u16,
    count: u32,
    data: Vec<u8>,
}

fn ascii(tag: u16, text: &str) -> Entry {
    let mut data = text.as_bytes().to_vec();
    data.push(0);
    Entry { tag, kind: TYPE_ASCII, count: data.len() as u32, data }
}

fn short(tag: u16, value: u16) -> Entry {
    Entry { tag, kind: TYPE_SHORT, count: 1, data: value.to_le_bytes().to_vec() }
}

fn rational(tag: u16, numerator: u32, denominator: u32) -> Entry {
    let mut data = numerator.to_le_bytes().to_vec();
    data.extend(denominator.to_le_bytes());
    Entry { tag, kind: TYPE_RATIONAL, count: 1, data }
}

/// Serialises an IFD at `offset`, returning its bytes, with long values placed after it.
fn write_ifd(mut entries: Vec<Entry>, offset: u32) -> Vec<u8> {
    entries.sort_by_key(|entry| entry.tag);
    let table_size = 2 + entries.len() * 12 + 4;
    let mut table = (entries.len() as u16).to_le_bytes().to_vec();
    let mut extra = Vec::new();
    for entry in entries {
        table.extend(entry.tag.to_le_bytes());
        table.extend(entry.kind.to_le_bytes());
        table.extend(entry.count.to_le_bytes());
        if entry.data.len() <= 4 {
            let mut inline = entry.data;
            inline.resize(4, 0);
            table.extend(inline);
        } else {
            table.extend((offset + table_size as u32 + extra.len() as u32).to_le_bytes());
            extra.extend(&entry.data);
            if extra.len() % 2 == 1 {
                extra.push(0);
            }
        }
    }
    table.extend(0u32.to_le_bytes());
    table.extend(extra);
    table
}

/// `YYYY:MM:DD HH:MM:SS`, as EXIF requires, or `None` if the text is not a timestamp.
fn exif_time(text: &str) -> Option<String> {
    let head: String = text.chars().take(19).collect();
    let normal: String = head
        .chars()
        .enumerate()
        .map(|(index, c)| match (index, c) {
            (4 | 7, '-') => ':',
            (10, 'T') => ' ',
            _ => c,
        })
        .collect();
    let bytes = normal.as_bytes();
    let valid = bytes.len() == 19
        && bytes[4] == b':'
        && bytes[7] == b':'
        && bytes[10] == b' '
        && bytes[13] == b':'
        && bytes[16] == b':';
    valid.then_some(normal)
}

/// The TIFF structure holding a JPEG's EXIF, or a whole TIFF file.
fn tiff_block(source: &[u8]) -> Option<&[u8]> {
    if matches!(source.get(..2)?, b"II" | b"MM") {
        return Some(source);
    }
    if source.get(..2)? != [0xFF, 0xD8] {
        return None;
    }
    let mut at = 2;
    loop {
        let (marker, kind) = (*source.get(at)?, *source.get(at + 1)?);
        // The image data starts at SOS, and EXIF always comes before it.
        if marker != 0xFF || kind == 0xDA {
            return None;
        }
        let size = usize::from(u16::from_be_bytes([*source.get(at + 2)?, *source.get(at + 3)?]));
        let body = source.get(at + 4..at + 2 + size)?;
        if kind == 0xE1 && body.starts_with(b"Exif\0\0") {
            return Some(&body[6..]);
        }
        at += 2 + size;
    }
}

/// Reads tags from one IFD of a TIFF structure of either byte order.
struct TiffReader<'a> {
    data: &'a [u8],
    big_endian: bool,
}

impl TiffReader<'_> {
    fn u16(&self, at: usize) -> Option<u16> {
        let bytes = self.data.get(at..at + 2)?.try_into().ok()?;
        Some(if self.big_endian { u16::from_be_bytes(bytes) } else { u16::from_le_bytes(bytes) })
    }

    fn u32(&self, at: usize) -> Option<u32> {
        let bytes = self.data.get(at..at + 4)?.try_into().ok()?;
        Some(if self.big_endian { u32::from_be_bytes(bytes) } else { u32::from_le_bytes(bytes) })
    }

    /// The entry for `tag` in the IFD at `ifd`, with its value in little-endian order.
    fn entry(&self, ifd: usize, tag: u16) -> Option<Entry> {
        let count = usize::from(self.u16(ifd)?).min(MAX_IFD_ENTRIES);
        let at = (0..count).map(|i| ifd + 2 + i * 12).find(|&at| self.u16(at) == Some(tag))?;
        let kind = self.u16(at + 2)?;
        let items = self.u32(at + 4)?;
        let (word, size) = match kind {
            TYPE_ASCII => (1, 1),
            TYPE_SHORT => (2, 2),
            TYPE_LONG => (4, 4),
            TYPE_RATIONAL | TYPE_SRATIONAL => (4, 8),
            _ => return None,
        };
        let total = (items as usize).checked_mul(size).filter(|total| *total <= MAX_VALUE_BYTES)?;
        let start = if total <= 4 { at + 8 } else { self.u32(at + 8)? as usize };
        let mut data = self.data.get(start..start.checked_add(total)?)?.to_vec();
        if self.big_endian && word > 1 {
            data.chunks_exact_mut(word).for_each(<[u8]>::reverse);
        }
        Some(Entry { tag, kind, count: items, data })
    }
}

/// Copies capture details from a JPEG or TIFF source. Location is deliberately left out.
fn copy_source_tags(source: &[u8], main: &mut Vec<Entry>, details: &mut Vec<Entry>) -> Option<()> {
    let data = tiff_block(source)?;
    let big_endian = match data.get(..2)? {
        b"II" => false,
        b"MM" => true,
        _ => return None,
    };
    let reader = TiffReader { data, big_endian };
    let first = reader.u32(4)? as usize;
    main.extend(COPIED_MAIN_TAGS.iter().filter_map(|&tag| reader.entry(first, tag)));
    let pointer = reader.entry(first, EXIF_IFD_POINTER)?;
    let details_at = u32::from_le_bytes(pointer.data.get(..4)?.try_into().ok()?) as usize;
    details.extend(COPIED_DETAIL_TAGS.iter().filter_map(|&tag| reader.entry(details_at, tag)));
    Some(())
}

/// Builds an APP1 EXIF payload for a JPEG exported from `source`.
pub fn export_exif(source: &[u8]) -> Vec<u8> {
    let mut main = vec![ascii(305, SOFTWARE), short(274, 1)];
    let mut details = vec![short(40961, 1)];

    if flashraw::supports(source).is_some()
        && let Ok(raw) = FlashRaw::open(source)
    {
        let metadata = raw.metadata();
        let make = metadata.make.trim();
        let model = metadata.model.trim();
        if !make.is_empty() {
            main.push(ascii(271, make));
        }
        if !model.is_empty() {
            main.push(ascii(272, model));
        }
        if let Some(time) = metadata.capture_time.as_deref().and_then(exif_time) {
            main.push(ascii(306, &time));
            details.push(ascii(36867, &time));
        }
        let exposure = &metadata.exposure;
        if let Some(iso) = exposure.iso {
            details.push(short(34855, iso.min(65535) as u16));
        }
        if let Some(seconds) = exposure.exposure_time.filter(|t| *t > 0.0) {
            let (numerator, denominator) = if seconds >= 1.0 {
                ((seconds * 10.0).round() as u32, 10)
            } else {
                (1, (1.0 / seconds).round() as u32)
            };
            details.push(rational(33434, numerator, denominator));
        }
        if let Some(aperture) = exposure.f_number {
            details.push(rational(33437, (aperture * 100.0).round() as u32, 100));
        }
        if let Some(focal) = exposure.focal_length {
            details.push(rational(37386, (focal * 10.0).round() as u32, 10));
        }
    } else {
        // Whatever could not be read is simply left out.
        let _ = copy_source_tags(source, &mut main, &mut details);
    }

    // The sub-IFD follows the main one, so reserve the main IFD's size first.
    main.push(Entry { tag: EXIF_IFD_POINTER, kind: TYPE_LONG, count: 1, data: vec![0; 4] });
    const HEADER: u32 = 8;
    let placeholder = write_ifd(
        main.iter()
            .map(|e| Entry { tag: e.tag, kind: e.kind, count: e.count, data: e.data.clone() })
            .collect(),
        HEADER,
    );
    let details_offset = HEADER + placeholder.len() as u32;
    if let Some(pointer) = main.iter_mut().find(|e| e.tag == EXIF_IFD_POINTER) {
        pointer.data = details_offset.to_le_bytes().to_vec();
    }

    let mut out = b"Exif\0\0II\x2a\0".to_vec();
    out.extend(HEADER.to_le_bytes());
    out.extend(write_ifd(main, HEADER));
    out.extend(write_ifd(details, details_offset));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read_u16(tiff: &[u8], at: usize) -> u16 {
        u16::from_le_bytes([tiff[at], tiff[at + 1]])
    }

    fn read_u32(tiff: &[u8], at: usize) -> u32 {
        u32::from_le_bytes([tiff[at], tiff[at + 1], tiff[at + 2], tiff[at + 3]])
    }

    /// (tag, value bytes offset) of each entry in the IFD at `offset`.
    fn entries(tiff: &[u8], offset: usize) -> Vec<(u16, usize)> {
        (0..read_u16(tiff, offset) as usize)
            .map(|i| {
                let at = offset + 2 + i * 12;
                (read_u16(tiff, at), at + 8)
            })
            .collect()
    }

    #[test]
    fn a_non_raw_source_still_declares_srgb_and_upright() {
        let exif = export_exif(b"not raw");
        assert_eq!(&exif[..6], b"Exif\0\0");
        let tiff = &exif[6..];
        let main = entries(tiff, read_u32(tiff, 4) as usize);
        let orientation = main.iter().find(|(tag, _)| *tag == 274).unwrap().1;
        assert_eq!(read_u16(tiff, orientation), 1);

        let pointer = main.iter().find(|(tag, _)| *tag == EXIF_IFD_POINTER).unwrap().1;
        let details = entries(tiff, read_u32(tiff, pointer) as usize);
        let color = details.iter().find(|(tag, _)| *tag == 40961).unwrap().1;
        assert_eq!(read_u16(tiff, color), 1);
    }

    #[test]
    fn tags_are_written_in_ascending_order() {
        let exif = export_exif(b"not raw");
        let tiff = &exif[6..];
        let tags: Vec<u16> = entries(tiff, read_u32(tiff, 4) as usize).iter().map(|e| e.0).collect();
        assert!(tags.windows(2).all(|pair| pair[0] < pair[1]), "{tags:?}");
    }

    #[test]
    fn long_values_are_stored_at_their_offset() {
        let exif = export_exif(b"not raw");
        let tiff = &exif[6..];
        let main = entries(tiff, read_u32(tiff, 4) as usize);
        let software = read_u32(tiff, main.iter().find(|(tag, _)| *tag == 305).unwrap().1) as usize;
        assert_eq!(&tiff[software..software + SOFTWARE.len()], SOFTWARE.as_bytes());
    }

    /// A JPEG whose EXIF is big-endian and holds a model name and an ISO.
    fn jpeg_with_exif() -> Vec<u8> {
        let mut tiff = b"MM\0\x2a\0\0\0\x08".to_vec();
        tiff.extend([0, 3]);
        tiff.extend([0x01, 0x10, 0, 2, 0, 0, 0, 5, 0, 0, 0, 68]); // Model, at offset 68.
        tiff.extend([0x01, 0x12, 0, 3, 0, 0, 0, 1, 0, 6, 0, 0]); // Orientation 6.
        tiff.extend([0x87, 0x69, 0, 4, 0, 0, 0, 1, 0, 0, 0, 50]); // Exif IFD, at offset 50.
        tiff.extend([0; 4]);
        tiff.extend([0, 1]);
        tiff.extend([0x88, 0x27, 0, 3, 0, 0, 0, 1, 0x01, 0x90, 0, 0]); // ISO 400.
        tiff.extend([0; 4]);
        tiff.extend(b"ABCD\0");
        let mut jpeg = vec![0xFF, 0xD8, 0xFF, 0xE1];
        jpeg.extend(((tiff.len() + 8) as u16).to_be_bytes());
        jpeg.extend(b"Exif\0\0");
        jpeg.extend(tiff);
        jpeg.extend([0xFF, 0xDA]);
        jpeg
    }

    #[test]
    fn a_jpeg_source_keeps_its_capture_details_and_is_marked_upright() {
        let exif = export_exif(&jpeg_with_exif());
        let tiff = &exif[6..];
        let main = entries(tiff, read_u32(tiff, 4) as usize);
        let model = read_u32(tiff, main.iter().find(|(tag, _)| *tag == 272).unwrap().1) as usize;
        assert_eq!(&tiff[model..model + 4], b"ABCD");
        let orientation = main.iter().find(|(tag, _)| *tag == 274).unwrap().1;
        assert_eq!(read_u16(tiff, orientation), 1);

        let pointer = main.iter().find(|(tag, _)| *tag == EXIF_IFD_POINTER).unwrap().1;
        let details = entries(tiff, read_u32(tiff, pointer) as usize);
        assert_eq!(read_u16(tiff, details.iter().find(|(tag, _)| *tag == 34855).unwrap().1), 400);
    }

    #[test]
    fn a_truncated_jpeg_is_not_a_problem() {
        let jpeg = jpeg_with_exif();
        for length in 0..jpeg.len() {
            let _ = export_exif(&jpeg[..length]);
        }
    }

    #[test]
    fn timestamps_are_normalised() {
        assert_eq!(exif_time("2024-03-05T10:20:30Z").as_deref(), Some("2024:03:05 10:20:30"));
        assert_eq!(exif_time("2024:03:05 10:20:30").as_deref(), Some("2024:03:05 10:20:30"));
        assert_eq!(exif_time("yesterday"), None);
    }
}
