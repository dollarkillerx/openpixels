//! Writes the EXIF block of exported files, and whole TIFF files: the capture details from
//! the source file, and a sRGB colour space tag, with orientation reset because pixels are
//! already upright.

use flashraw::FlashRaw;

const SOFTWARE: &str = "OpenPixels";
const TYPE_ASCII: u16 = 2;
const TYPE_SHORT: u16 = 3;
const TYPE_LONG: u16 = 4;
const TYPE_RATIONAL: u16 = 5;
const TYPE_UNDEFINED: u16 = 7;
const TYPE_SRATIONAL: u16 = 10;
const EXIF_IFD_POINTER: u16 = 34665;
const ICC_PROFILE: u16 = 34675;
const HEADER_SIZE: u32 = 8;
/// What precedes the TIFF structure in a JPEG's APP1 segment.
pub(crate) const EXIF_PREFIX: &[u8] = b"Exif\0\0";

const MAX_IFD_ENTRIES: usize = 1024;
const MAX_VALUE_BYTES: usize = 4096;
/// Make, model, capture time, artist and copyright.
const COPIED_MAIN_TAGS: [u16; 5] = [271, 272, 306, 315, 33432];
/// Exposure, aperture, ISO, capture times, exposure compensation, focal length and lens.
const COPIED_DETAIL_TAGS: [u16; 9] = [33434, 33437, 34855, 36867, 36868, 37380, 37386, 42035, 42036];

#[derive(Clone)]
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

fn long(tag: u16, value: u32) -> Entry {
    Entry { tag, kind: TYPE_LONG, count: 1, data: value.to_le_bytes().to_vec() }
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

/// Splits the box at `at` into its four-character type, body and the position after it.
fn heif_box(data: &[u8], at: usize) -> Option<(&[u8], &[u8], usize)> {
    let size = u32::from_be_bytes(data.get(at..at + 4)?.try_into().ok()?) as usize;
    let kind = data.get(at + 4..at + 8)?;
    let (body, end) = match size {
        // Runs to the end of the enclosing data.
        0 => (at + 8, data.len()),
        1 => (at + 16, at.checked_add(u64::from_be_bytes(data.get(at + 8..at + 16)?.try_into().ok()?) as usize)?),
        _ => (at + 8, at.checked_add(size)?),
    };
    Some((kind, data.get(body..end)?, end))
}

fn heif_child<'a>(data: &'a [u8], wanted: &[u8]) -> Option<&'a [u8]> {
    let mut at = 0;
    loop {
        let (kind, body, end) = heif_box(data, at)?;
        if kind == wanted {
            return Some(body);
        }
        at = end;
    }
}

fn be_uint(data: &[u8], at: usize, size: usize) -> Option<usize> {
    let bytes = data.get(at..at + size)?;
    Some(bytes.iter().fold(0usize, |value, &byte| (value << 8) | usize::from(byte)))
}

/// The item id of the first `Exif` item in an `iinf` box.
fn heif_exif_item(iinf: &[u8]) -> Option<usize> {
    let mut at = if *iinf.first()? == 0 { 6 } else { 8 };
    loop {
        let (kind, body, end) = heif_box(iinf, at)?;
        if kind == b"infe" {
            let version = *body.first()?;
            let id_size = match version {
                2 => 2,
                3 => 4,
                _ => return None,
            };
            if body.get(4 + id_size + 2..4 + id_size + 6)? == b"Exif" {
                return be_uint(body, 4, id_size);
            }
        }
        at = end;
    }
}

/// File range of an item from an `iloc` box, if it is one extent in the file itself.
fn heif_item_range(iloc: &[u8], item: usize) -> Option<(usize, usize)> {
    let version = *iloc.first()?;
    let (offset_size, length_size) = (usize::from(iloc.get(4)? >> 4), usize::from(iloc.get(4)? & 15));
    let base_size = usize::from(iloc.get(5)? >> 4);
    let index_size = if version > 0 { usize::from(iloc.get(5)? & 15) } else { 0 };
    let (id_size, count_size) = if version < 2 { (2, 2) } else { (4, 4) };
    let items = be_uint(iloc, 6, count_size)?.min(MAX_IFD_ENTRIES);
    let mut at = 6 + count_size;
    for _ in 0..items {
        let id = be_uint(iloc, at, id_size)?;
        at += id_size;
        let in_file = version == 0 || be_uint(iloc, at, 2)? & 15 == 0;
        at += if version == 0 { 0 } else { 2 } + 2;
        let base = be_uint(iloc, at, base_size)?;
        at += base_size;
        let extents = be_uint(iloc, at, 2)?;
        at += 2;
        if id == item {
            if !in_file || extents != 1 {
                return None;
            }
            let offset = base.checked_add(be_uint(iloc, at + index_size, offset_size)?)?;
            return Some((offset, be_uint(iloc, at + index_size + offset_size, length_size)?));
        }
        at += extents * (index_size + offset_size + length_size);
    }
    None
}

/// The TIFF structure of a HEIF file's EXIF item.
fn heif_tiff_block(source: &[u8]) -> Option<&[u8]> {
    let meta = heif_child(source, b"meta")?.get(4..)?;
    let item = heif_exif_item(heif_child(meta, b"iinf")?)?;
    let (offset, length) = heif_item_range(heif_child(meta, b"iloc")?, item)?;
    let data = source.get(offset..offset.checked_add(length)?)?;
    // The item starts with the distance from its fifth byte to the TIFF header.
    data.get(4 + be_uint(data, 0, 4)?..)
}

/// The TIFF structure holding the EXIF of a JPEG or HEIF, or a whole TIFF file.
fn tiff_block(source: &[u8]) -> Option<&[u8]> {
    if matches!(source.get(..2)?, b"II" | b"MM") {
        return Some(source);
    }
    if source.get(4..8)? == b"ftyp" {
        return heif_tiff_block(source);
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

/// The embedded JPEG of a Fujifilm RAF, whose EXIF has the lens name.
fn raf_preview(source: &[u8]) -> Option<&[u8]> {
    let offset = u32::from_be_bytes(source.get(84..88)?.try_into().ok()?) as usize;
    let length = u32::from_be_bytes(source.get(88..92)?.try_into().ok()?) as usize;
    source.get(offset..offset.checked_add(length)?)
}

/// The EXIF sub-IFD of a Canon CR3, stored as its own TIFF structure in a `CMT2` box.
fn cr3_details(source: &[u8]) -> Option<&[u8]> {
    let at = source.windows(4).position(|window| window == b"CMT2")?;
    let size = u32::from_be_bytes(source.get(at.checked_sub(4)?..at)?.try_into().ok()?) as usize;
    source.get(at + 4..(at - 4).checked_add(size)?)
}

fn ascii_tag(reader: &TiffReader<'_>, ifd: usize, tag: u16) -> Option<String> {
    let entry = reader.entry(ifd, tag)?;
    let end = entry.data.iter().position(|&byte| byte == 0).unwrap_or(entry.data.len());
    let text = std::str::from_utf8(&entry.data[..end]).ok()?.trim();
    (!text.is_empty()).then(|| text.to_string())
}

fn tiff_reader(data: &[u8]) -> Option<TiffReader<'_>> {
    let big_endian = match data.get(..2)? {
        b"II" => false,
        b"MM" => true,
        _ => return None,
    };
    Some(TiffReader { data, big_endian })
}

fn details_ifd(reader: &TiffReader<'_>) -> Option<usize> {
    let pointer = reader.entry(reader.u32(4)? as usize, EXIF_IFD_POINTER)?;
    Some(u32::from_le_bytes(pointer.data.get(..4)?.try_into().ok()?) as usize)
}

/// The name of the lens that took a RAW file.
pub(crate) fn lens_model(source: &[u8]) -> Option<String> {
    const LENS_MODEL: u16 = 42036;
    if source.get(8..12)? == b"crx " {
        let reader = tiff_reader(cr3_details(source)?)?;
        return ascii_tag(&reader, reader.u32(4)? as usize, LENS_MODEL);
    }
    let data = if source.starts_with(b"FUJIFILMCCD-RAW") { tiff_block(raf_preview(source)?)? } else { tiff_block(source)? };
    let reader = tiff_reader(data)?;
    ascii_tag(&reader, details_ifd(&reader)?, LENS_MODEL)
}

/// When a photo was taken, as `YYYY:MM:DD HH:MM:SS`, from an APP1 payload made by [`export_exif`].
pub fn capture_time(exif: &[u8]) -> Option<String> {
    const DATE_TIME: u16 = 306;
    const DATE_TIME_ORIGINAL: u16 = 36867;
    let reader = tiff_reader(exif.strip_prefix(EXIF_PREFIX)?)?;
    details_ifd(&reader)
        .and_then(|ifd| ascii_tag(&reader, ifd, DATE_TIME_ORIGINAL))
        .or_else(|| ascii_tag(&reader, reader.u32(4)? as usize, DATE_TIME))
}

/// Copies capture details from a JPEG, HEIF or TIFF source. Location is deliberately left out.
fn copy_source_tags(source: &[u8], main: &mut Vec<Entry>, details: &mut Vec<Entry>) -> Option<()> {
    let reader = tiff_reader(tiff_block(source)?)?;
    let first = reader.u32(4)? as usize;
    main.extend(COPIED_MAIN_TAGS.iter().filter_map(|&tag| reader.entry(first, tag)));
    let pointer = reader.entry(first, EXIF_IFD_POINTER)?;
    let details_at = u32::from_le_bytes(pointer.data.get(..4)?.try_into().ok()?) as usize;
    details.extend(COPIED_DETAIL_TAGS.iter().filter_map(|&tag| reader.entry(details_at, tag)));
    Some(())
}

/// Marks an [`export_exif`] payload as not sRGB, as EXIF asks for other colour spaces
/// (the embedded ICC profile then says which).
pub fn mark_uncalibrated(exif: &mut [u8]) {
    // The ColorSpace entry as `export_exif` writes it: tag 0xA001, SHORT, count 1, value 1.
    const SRGB_ENTRY: [u8; 10] = [0x01, 0xA0, 0x03, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x00];
    if let Some(at) = exif.windows(SRGB_ENTRY.len()).position(|window| window == SRGB_ENTRY) {
        exif[at + 8..at + 10].copy_from_slice(&[0xFF, 0xFF]);
    }
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

    let mut out = EXIF_PREFIX.to_vec();
    out.extend(tiff_structure(main, details, |_, _| {}));
    out
}

/// Header, main IFD and EXIF IFD. `finish` can fill in values that depend on where
/// the structure ends, given the main IFD's entries and that end.
fn tiff_structure(mut main: Vec<Entry>, details: Vec<Entry>, finish: impl FnOnce(&mut Vec<Entry>, u32)) -> Vec<u8> {
    // The sub-IFD follows the main one, so reserve the main IFD's size first.
    main.push(long(EXIF_IFD_POINTER, 0));
    let details_offset = HEADER_SIZE + write_ifd(main.clone(), HEADER_SIZE).len() as u32;
    let details = write_ifd(details, details_offset);
    if let Some(pointer) = main.iter_mut().find(|e| e.tag == EXIF_IFD_POINTER) {
        pointer.data = details_offset.to_le_bytes().to_vec();
    }
    finish(&mut main, details_offset + details.len() as u32);

    let mut out = b"II\x2a\0".to_vec();
    out.extend(HEADER_SIZE.to_le_bytes());
    out.extend(write_ifd(main, HEADER_SIZE));
    out.extend(details);
    out
}

/// Builds an uncompressed RGB TIFF from little-endian samples, with a colour profile
/// and, from an APP1 payload made by [`export_exif`], the capture details.
pub(crate) fn write_tiff(pixels: &[u8], width: u32, height: u32, bits: u16, icc: &[u8], exif: Option<&[u8]>) -> Vec<u8> {
    const STRIP_OFFSETS: u16 = 273;
    let mut main = vec![
        long(256, width),
        long(257, height),
        Entry { tag: 258, kind: TYPE_SHORT, count: 3, data: [bits; 3].iter().flat_map(|b| b.to_le_bytes()).collect() },
        short(259, 1),
        short(262, 2),
        long(STRIP_OFFSETS, 0),
        short(274, 1),
        short(277, 3),
        long(278, height),
        long(279, pixels.len() as u32),
        rational(282, 72, 1),
        rational(283, 72, 1),
        short(284, 1),
        short(296, 2),
        ascii(305, SOFTWARE),
        Entry { tag: ICC_PROFILE, kind: TYPE_UNDEFINED, count: icc.len() as u32, data: icc.to_vec() },
    ];
    // EXIF only names sRGB; anything else is "uncalibrated", described by the ICC profile.
    let colour_space = if icc == crate::srgb_icc_profile().as_slice() { 1 } else { 0xFFFF };
    let mut details = vec![short(40961, colour_space)];
    if let Some(block) = exif.and_then(|block| block.strip_prefix(EXIF_PREFIX)) {
        // Whatever could not be read is simply left out.
        let _ = copy_source_tags(block, &mut main, &mut details);
    }

    let mut out = tiff_structure(main, details, |main, end| {
        if let Some(entry) = main.iter_mut().find(|e| e.tag == STRIP_OFFSETS) {
            entry.data = end.to_le_bytes().to_vec();
        }
    });
    out.extend_from_slice(pixels);
    out
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    #[test]
    fn a_tiff_in_another_colour_space_is_not_labelled_srgb() {
        let srgb_entry = [0x01, 0xA0, 0x03, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x00];
        let has_srgb = |tiff: &[u8]| tiff.windows(10).any(|w| w == srgb_entry);
        assert!(has_srgb(&write_tiff(&[0; 12], 2, 2, 8, &crate::srgb_icc_profile(), None)));
        assert!(!has_srgb(&write_tiff(&[0; 12], 2, 2, 8, &crate::OutputSpace::P3.icc_profile(), None)));
    }

    #[test]
    fn a_non_srgb_export_says_so() {
        let mut exif = export_exif(b"not a raw file");
        let before = exif.clone();
        mark_uncalibrated(&mut exif);
        assert_ne!(exif, before);
        assert_eq!(exif.len(), before.len());
        assert!(exif.windows(2).any(|w| w == [0xFF, 0xFF]));
    }

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
    pub(crate) fn jpeg_with_exif() -> Vec<u8> {
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

    /// A HEIF holding the same TIFF structure as [`jpeg_with_exif`] as its EXIF item.
    fn heif_with_exif() -> Vec<u8> {
        fn boxed(kind: &[u8], body: &[u8]) -> Vec<u8> {
            let mut out = ((body.len() + 8) as u32).to_be_bytes().to_vec();
            out.extend(kind);
            out.extend(body);
            out
        }
        let jpeg = jpeg_with_exif();
        let tiff = &jpeg[12..jpeg.len() - 2];
        let mut item = vec![0; 4];
        item.extend(tiff);

        let build = |offset: u32| {
            // Two items, so finding the right one matters.
            let mut infos = vec![0, 0, 0, 0, 0, 2];
            infos.extend(boxed(b"infe", &[2, 0, 0, 0, 0, 1, 0, 0, b'h', b'v', b'c', b'1']));
            infos.extend(boxed(b"infe", &[2, 0, 0, 0, 0, 2, 0, 0, b'E', b'x', b'i', b'f']));
            let mut locations = vec![0, 0, 0, 0, 0x44, 0, 0, 2];
            locations.extend([0, 1, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 9]);
            locations.extend([0, 2, 0, 0, 0, 1]);
            locations.extend(offset.to_be_bytes());
            locations.extend((item.len() as u32).to_be_bytes());
            let mut meta = vec![0; 4];
            meta.extend(boxed(b"iinf", &infos));
            meta.extend(boxed(b"iloc", &locations));
            let mut file = boxed(b"ftyp", b"heic\0\0\0\0mif1");
            file.extend(boxed(b"meta", &meta));
            file
        };
        let offset = build(0).len() as u32;
        let mut file = build(offset);
        file.extend(&item);
        file
    }

    #[test]
    fn a_heif_source_keeps_its_capture_details() {
        let exif = export_exif(&heif_with_exif());
        let tiff = &exif[6..];
        let main = entries(tiff, read_u32(tiff, 4) as usize);
        let model = read_u32(tiff, main.iter().find(|(tag, _)| *tag == 272).unwrap().1) as usize;
        assert_eq!(&tiff[model..model + 4], b"ABCD");
    }

    #[test]
    fn a_truncated_heif_is_not_a_problem() {
        let heif = heif_with_exif();
        for length in 0..heif.len() {
            let _ = export_exif(&heif[..length]);
        }
    }

    #[test]
    fn a_tiff_has_a_valid_structure_with_its_details_in_order() {
        let exif = export_exif(&jpeg_with_exif());
        let pixels = [7u8; 3 * 2 * 2];
        let tiff = write_tiff(&pixels, 2, 2, 8, b"profile-bytes", Some(&exif));
        let main = entries(&tiff, read_u32(&tiff, 4) as usize);
        let tags: Vec<u16> = main.iter().map(|e| e.0).collect();
        assert!(tags.windows(2).all(|pair| pair[0] < pair[1]), "{tags:?}");

        let strip = read_u32(&tiff, main.iter().find(|(tag, _)| *tag == 273).unwrap().1) as usize;
        assert_eq!(&tiff[strip..], &pixels);
        let icc = read_u32(&tiff, main.iter().find(|(tag, _)| *tag == ICC_PROFILE).unwrap().1) as usize;
        assert_eq!(&tiff[icc..icc + 13], b"profile-bytes");
        let model = read_u32(&tiff, main.iter().find(|(tag, _)| *tag == 272).unwrap().1) as usize;
        assert_eq!(&tiff[model..model + 4], b"ABCD");
        let pointer = main.iter().find(|(tag, _)| *tag == EXIF_IFD_POINTER).unwrap().1;
        let details = entries(&tiff, read_u32(&tiff, pointer) as usize);
        assert_eq!(read_u16(&tiff, details.iter().find(|(tag, _)| *tag == 34855).unwrap().1), 400);
    }

    #[test]
    fn capture_time_comes_from_the_details_then_the_main_directory() {
        let mut main = vec![ascii(306, "2020:01:02 03:04:05")];
        let mut block = EXIF_PREFIX.to_vec();
        block.extend(tiff_structure(main.clone(), vec![ascii(36867, "2021:06:07 08:09:10")], |_, _| {}));
        assert_eq!(capture_time(&block).as_deref(), Some("2021:06:07 08:09:10"));

        main.push(short(274, 1));
        let mut block = EXIF_PREFIX.to_vec();
        block.extend(tiff_structure(main, vec![short(40961, 1)], |_, _| {}));
        assert_eq!(capture_time(&block).as_deref(), Some("2020:01:02 03:04:05"));
        assert_eq!(capture_time(b"not exif"), None);
    }

    #[test]
    fn a_cr3_gives_its_lens_name_from_the_cmt2_box() {
        let lens = tiff_structure(vec![ascii(42036, "RF50mm F1.8 STM")], vec![], |_, _| {});
        // The lens tag sits in the first directory of this structure.
        let mut file = b"\0\0\0\x18ftypcrx \0\0\0\x01crx isom".to_vec();
        file.extend(((lens.len() + 8) as u32).to_be_bytes());
        file.extend(b"CMT2");
        file.extend(&lens);
        assert_eq!(lens_model(&file).as_deref(), Some("RF50mm F1.8 STM"));
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
