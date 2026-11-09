//! Writes an EXIF block for exported JPEGs: the capture details from a RAW file, and a
//! sRGB colour space tag, with orientation reset because pixels are already upright.

use flashraw::FlashRaw;

const SOFTWARE: &str = "OpenPixels";
const TYPE_ASCII: u16 = 2;
const TYPE_SHORT: u16 = 3;
const TYPE_LONG: u16 = 4;
const TYPE_RATIONAL: u16 = 5;
const EXIF_IFD_POINTER: u16 = 34665;

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

    #[test]
    fn timestamps_are_normalised() {
        assert_eq!(exif_time("2024-03-05T10:20:30Z").as_deref(), Some("2024:03:05 10:20:30"));
        assert_eq!(exif_time("2024:03:05 10:20:30").as_deref(), Some("2024:03:05 10:20:30"));
        assert_eq!(exif_time("yesterday"), None);
    }
}
