//! The metadata (Exif) and the colour profile (ICC) put into PNG and WebP files, where WIC / libwebp leave them out:
//! PNG gets an `eXIf` chunk after its header, WebP is rebuilt as an extended file (VP8X) with `ICCP` and `EXIF`.

/// The PNG with the Exif block (TIFF bytes) as an `eXIf` chunk right after `IHDR`.
pub fn png_with_exif(png: &[u8], tiff: &[u8]) -> Vec<u8> {
    // signature (8) + IHDR: length (4) + "IHDR" (4) + 13 bytes + CRC (4)
    const AFTER_IHDR: usize = 8 + 4 + 4 + 13 + 4;
    if png.len() < AFTER_IHDR || &png[12..16] != b"IHDR" {
        return png.to_vec();
    }
    let mut out = Vec::with_capacity(png.len() + tiff.len() + 12);
    out.extend_from_slice(&png[..AFTER_IHDR]);
    out.extend_from_slice(&(tiff.len() as u32).to_be_bytes());
    let start = out.len();
    out.extend_from_slice(b"eXIf");
    out.extend_from_slice(tiff);
    let crc = crc32(&out[start..]);
    out.extend_from_slice(&crc.to_be_bytes());
    out.extend_from_slice(&png[AFTER_IHDR..]);
    out
}

/// The WebP of libwebp as an extended file with the colour profile and the Exif block (either may be missing).
pub fn webp_with(webp: &[u8], width: u32, height: u32, icc: Option<&[u8]>, exif: Option<&[u8]>) -> Vec<u8> {
    if icc.is_none() && exif.is_none() {
        return webp.to_vec();
    }
    if webp.len() < 12 || &webp[0..4] != b"RIFF" || &webp[8..12] != b"WEBP" {
        return webp.to_vec();
    }
    // the chunks of the file (a VP8X of libwebp – with alpha – is replaced by ours, its flags kept)
    let mut flags = 0u8;
    let mut chunks: Vec<(&[u8], &[u8])> = Vec::new();
    let mut at = 12;
    while at + 8 <= webp.len() {
        let id = &webp[at..at + 4];
        let size = u32::from_le_bytes([webp[at + 4], webp[at + 5], webp[at + 6], webp[at + 7]]) as usize;
        let Some(data) = webp.get(at + 8..at + 8 + size) else { return webp.to_vec() };
        match id {
            b"VP8X" => flags |= data.first().copied().unwrap_or(0),
            b"ALPH" => {
                flags |= 0x10;
                chunks.push((id, data));
            }
            _ => chunks.push((id, data)),
        }
        at += 8 + size + (size & 1);
    }
    if icc.is_some() {
        flags |= 0x20;
    }
    if exif.is_some() {
        flags |= 0x08;
    }
    let mut body = Vec::with_capacity(webp.len() + icc.map_or(0, |i| i.len()) + exif.map_or(0, |e| e.len()) + 64);
    body.extend_from_slice(b"WEBP");
    let mut vp8x = [0u8; 10];
    vp8x[0] = flags;
    vp8x[4..7].copy_from_slice(&(width.saturating_sub(1)).to_le_bytes()[..3]);
    vp8x[7..10].copy_from_slice(&(height.saturating_sub(1)).to_le_bytes()[..3]);
    chunk(&mut body, b"VP8X", &vp8x);
    if let Some(icc) = icc {
        chunk(&mut body, b"ICCP", icc);
    }
    for (id, data) in chunks {
        chunk(&mut body, id, data);
    }
    if let Some(exif) = exif {
        chunk(&mut body, b"EXIF", exif);
    }
    let mut out = Vec::with_capacity(body.len() + 8);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(body.len() as u32).to_le_bytes());
    out.extend_from_slice(&body);
    out
}

fn chunk(out: &mut Vec<u8>, id: &[u8], data: &[u8]) {
    out.extend_from_slice(id);
    out.extend_from_slice(&(data.len() as u32).to_le_bytes());
    out.extend_from_slice(data);
    if data.len() & 1 == 1 {
        out.push(0);
    }
}

/// CRC-32 of PNG chunks (ISO-HDLC).
fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0xEDB8_8320 } else { crc >> 1 };
        }
    }
    !crc
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc_of_iend() {
        assert_eq!(crc32(b"IEND"), 0xAE42_6082);
    }

    #[test]
    fn webp_extended() {
        // a simple lossy file: RIFF, WEBP, a "VP8 " chunk of 3 bytes (padded)
        let simple = [b"RIFF".as_slice(), &16u32.to_le_bytes(), b"WEBP", b"VP8 ", &3u32.to_le_bytes(), &[1, 2, 3, 0]].concat();
        let out = webp_with(&simple, 640, 480, Some(&[9, 9]), Some(b"MM"));
        assert_eq!(&out[12..16], b"VP8X");
        assert_eq!(out[20], 0x28);
        assert_eq!(&out[24..27], &[0x7F, 0x02, 0x00]);
        let size = u32::from_le_bytes([out[4], out[5], out[6], out[7]]) as usize;
        assert_eq!(size + 8, out.len());
        assert!(out.windows(4).any(|w| w == b"ICCP") && out.windows(4).any(|w| w == b"EXIF"));
    }
}
