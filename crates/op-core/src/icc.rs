//! A minimal sRGB ICC v2 display profile, embedded in exported JPEGs so colour-managed
//! applications do not have to guess the colour space.

const CURVE_ENTRIES: usize = 1024;
const DESCRIPTION: &str = "sRGB (OpenPixels)";
const COPYRIGHT: &str = "No copyright, use freely";

fn fixed(value: f64) -> [u8; 4] {
    ((value * 65536.0).round() as i32).to_be_bytes()
}

fn xyz(values: [f64; 3]) -> Vec<u8> {
    let mut out = b"XYZ \0\0\0\0".to_vec();
    for value in values {
        out.extend(fixed(value));
    }
    out
}

fn text(content: &str) -> Vec<u8> {
    let mut out = b"text\0\0\0\0".to_vec();
    out.extend(content.as_bytes());
    out.push(0);
    out
}

/// The v2 `desc` layout: ASCII text, then empty Unicode and ScriptCode sections.
fn description(content: &str) -> Vec<u8> {
    let mut out = b"desc\0\0\0\0".to_vec();
    out.extend((content.len() as u32 + 1).to_be_bytes());
    out.extend(content.as_bytes());
    out.push(0);
    out.extend([0u8; 8]); // Unicode language code and length.
    out.extend([0u8; 3]); // ScriptCode code and length.
    out.extend([0u8; 67]);
    out
}

fn srgb_curve() -> Vec<u8> {
    let mut out = b"curv\0\0\0\0".to_vec();
    out.extend((CURVE_ENTRIES as u32).to_be_bytes());
    for index in 0..CURVE_ENTRIES {
        let encoded = index as f64 / (CURVE_ENTRIES - 1) as f64;
        let linear = if encoded <= 0.04045 { encoded / 12.92 } else { ((encoded + 0.055) / 1.055).powf(2.4) };
        out.extend(((linear * 65535.0).round() as u16).to_be_bytes());
    }
    out
}

/// Builds the profile.
pub fn srgb_icc_profile() -> Vec<u8> {
    let curve = srgb_curve();
    // XYZ values are the sRGB primaries adapted to the D50 profile connection space.
    let tags: Vec<(&[u8; 4], Vec<u8>)> = vec![
        (b"desc", description(DESCRIPTION)),
        (b"cprt", text(COPYRIGHT)),
        (b"wtpt", xyz([0.9642, 1.0, 0.8249])),
        (b"rXYZ", xyz([0.436_074_7, 0.222_504_5, 0.013_932_2])),
        (b"gXYZ", xyz([0.385_064_9, 0.716_878_6, 0.097_104_5])),
        (b"bXYZ", xyz([0.143_080_4, 0.060_616_9, 0.714_173_3])),
        (b"rTRC", curve.clone()),
        (b"gTRC", curve.clone()),
        (b"bTRC", curve),
    ];

    const HEADER_SIZE: usize = 128;
    let table_size = 4 + tags.len() * 12;
    let mut table = (tags.len() as u32).to_be_bytes().to_vec();
    let mut body = Vec::new();
    for (signature, data) in &tags {
        table.extend(*signature);
        table.extend(((HEADER_SIZE + table_size + body.len()) as u32).to_be_bytes());
        table.extend((data.len() as u32).to_be_bytes());
        body.extend(data);
        while body.len() % 4 != 0 {
            body.push(0);
        }
    }

    let mut header = vec![0u8; HEADER_SIZE];
    header[0..4].copy_from_slice(&((HEADER_SIZE + table.len() + body.len()) as u32).to_be_bytes());
    header[8..12].copy_from_slice(&[0x02, 0x10, 0, 0]);
    header[12..16].copy_from_slice(b"mntr");
    header[16..20].copy_from_slice(b"RGB ");
    header[20..24].copy_from_slice(b"XYZ ");
    header[24..36].copy_from_slice(&[0x07, 0xE6, 0, 1, 0, 1, 0, 0, 0, 0, 0, 0]);
    header[36..40].copy_from_slice(b"acsp");
    header[68..72].copy_from_slice(&fixed(0.9642));
    header[72..76].copy_from_slice(&fixed(1.0));
    header[76..80].copy_from_slice(&fixed(0.8249));

    let mut out = header;
    out.extend(table);
    out.extend(body);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read_u32(data: &[u8], at: usize) -> usize {
        u32::from_be_bytes([data[at], data[at + 1], data[at + 2], data[at + 3]]) as usize
    }

    #[test]
    fn the_declared_size_matches_the_data() {
        let profile = srgb_icc_profile();
        assert_eq!(read_u32(&profile, 0), profile.len());
        assert_eq!(&profile[36..40], b"acsp");
    }

    #[test]
    fn every_tag_points_inside_the_profile_on_a_four_byte_boundary() {
        let profile = srgb_icc_profile();
        let count = read_u32(&profile, 128);
        assert_eq!(count, 9);
        for index in 0..count {
            let at = 132 + index * 12;
            let (offset, size) = (read_u32(&profile, at + 4), read_u32(&profile, at + 8));
            assert_eq!(offset % 4, 0, "tag {index}");
            assert!(offset + size <= profile.len(), "tag {index}");
        }
    }

    #[test]
    fn the_curve_runs_from_black_to_white() {
        let curve = srgb_curve();
        let entry = |index: usize| u16::from_be_bytes([curve[12 + index * 2], curve[13 + index * 2]]);
        assert_eq!(entry(0), 0);
        assert_eq!(entry(CURVE_ENTRIES - 1), 65535);
        assert!(entry(512) < 65535 / 2, "an sRGB curve lies below linear");
    }
}
