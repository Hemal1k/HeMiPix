//! The photo information (Exif: date taken, camera, exposure, location …) of the original, carried into the JPG.
//! Read by kamadak-exif from HEIC / JPG / PNG / WebP / TIFF; then patched here: the orientation is set to
//! "upright" (HeMiPix turns the pixels itself) and the location (GPS) can be removed. Written into the JPG as
//! its APP1 segment.

use std::path::Path;

/// The Exif block (TIFF bytes) of the photo, ready for the JPG; None when it has none (or it cannot be cleaned).
pub fn read(src: &Path, remove_gps: bool) -> Option<Vec<u8>> {
    let file = std::fs::File::open(src).ok()?;
    let info = exif::Reader::new().read_from_container(&mut std::io::BufReader::new(file)).ok()?;
    let mut tiff = info.buf().to_vec();
    // the location must really be gone when asked: if the block cannot be patched, it is left out entirely
    match patch(&mut tiff, remove_gps) {
        Some(()) => Some(tiff),
        None if !remove_gps => Some(tiff),
        None => None,
    }
}

/// The JPG with the Exif block as APP1 right after its start (after JFIF, when there is one).
pub fn insert(jpg: &[u8], tiff: &[u8]) -> Vec<u8> {
    let len = tiff.len() + 6 + 2;
    if jpg.len() < 4 || jpg[0] != 0xFF || jpg[1] != 0xD8 || len > 0xFFFF {
        return jpg.to_vec();
    }
    let mut at = 2;
    // JFIF (APP0) stays first
    if jpg[2] == 0xFF && jpg[3] == 0xE0 && jpg.len() > 6 {
        at = 4 + u16::from_be_bytes([jpg[4], jpg[5]]) as usize;
    }
    let mut out = Vec::with_capacity(jpg.len() + len + 2);
    out.extend_from_slice(&jpg[..at]);
    out.extend_from_slice(&[0xFF, 0xE1]);
    out.extend_from_slice(&(len as u16).to_be_bytes());
    out.extend_from_slice(b"Exif\0\0");
    out.extend_from_slice(tiff);
    out.extend_from_slice(&jpg[at..]);
    out
}

struct Tiff<'a> {
    b: &'a mut [u8],
    le: bool,
}

impl Tiff<'_> {
    fn u16(&self, at: usize) -> Option<u16> {
        let s = self.b.get(at..at + 2)?;
        Some(if self.le { u16::from_le_bytes([s[0], s[1]]) } else { u16::from_be_bytes([s[0], s[1]]) })
    }
    fn u32(&self, at: usize) -> Option<u32> {
        let s = self.b.get(at..at + 4)?;
        let a = [s[0], s[1], s[2], s[3]];
        Some(if self.le { u32::from_le_bytes(a) } else { u32::from_be_bytes(a) })
    }
    fn set_u16(&mut self, at: usize, v: u16) -> Option<()> {
        let bytes = if self.le { v.to_le_bytes() } else { v.to_be_bytes() };
        self.b.get_mut(at..at + 2)?.copy_from_slice(&bytes);
        Some(())
    }
}

/// Bytes of one value of the TIFF field type.
fn type_size(t: u16) -> usize {
    match t {
        1 | 2 | 6 | 7 => 1,
        3 | 8 => 2,
        4 | 9 | 11 => 4,
        5 | 10 | 12 => 8,
        _ => 1,
    }
}

/// Orientation → 1; without GPS when asked (its directory is zeroed and its entry removed from IFD0).
fn patch(buf: &mut [u8], remove_gps: bool) -> Option<()> {
    let le = match buf.get(0..2)? {
        b"II" => true,
        b"MM" => false,
        _ => return None,
    };
    let mut t = Tiff { b: buf, le };
    let ifd0 = t.u32(4)? as usize;
    let n = t.u16(ifd0)? as usize;
    let entries = ifd0 + 2;
    let mut gps: Option<usize> = None;
    for i in 0..n {
        let e = entries + i * 12;
        match t.u16(e)? {
            0x0112 => t.set_u16(e + 8, 1)?,
            0x8825 => gps = Some(i),
            _ => {}
        }
    }
    if !remove_gps {
        return Some(());
    }
    let Some(i) = gps else { return Some(()) };
    // the GPS directory and its values: zeroed
    let e = entries + i * 12;
    let dir = t.u32(e + 8)? as usize;
    let gn = t.u16(dir)? as usize;
    for k in 0..gn {
        let ge = dir + 2 + k * 12;
        let size = type_size(t.u16(ge + 2)?) * t.u32(ge + 4)? as usize;
        if size > 4 {
            let at = t.u32(ge + 8)? as usize;
            t.b.get_mut(at..at + size)?.fill(0);
        }
    }
    t.b.get_mut(dir..dir + 2 + gn * 12 + 4)?.fill(0);
    // the entry pointing to it: removed from IFD0 (the later entries and the next-IFD offset move up)
    let end = entries + n * 12 + 4;
    t.b.copy_within(e + 12..end, e);
    t.b.get_mut(end - 12..end)?.fill(0);
    t.set_u16(ifd0, (n - 1) as u16)?;
    Some(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A small big-endian TIFF: IFD0 with Orientation = 6 and a GPS pointer, a GPS directory with a latitude.
    fn sample() -> Vec<u8> {
        let mut b = Vec::new();
        b.extend_from_slice(b"MM\0\x2A\0\0\0\x08");
        b.extend_from_slice(&2u16.to_be_bytes());
        b.extend_from_slice(&[0x01, 0x12, 0, 3, 0, 0, 0, 1, 0, 6, 0, 0]);
        b.extend_from_slice(&[0x88, 0x25, 0, 4, 0, 0, 0, 1, 0, 0, 0, 38]);
        b.extend_from_slice(&0u32.to_be_bytes());
        // GPS IFD at 38: one rational ×3 latitude stored at 56
        b.extend_from_slice(&1u16.to_be_bytes());
        b.extend_from_slice(&[0, 2, 0, 5, 0, 0, 0, 3, 0, 0, 0, 56]);
        b.extend_from_slice(&0u32.to_be_bytes());
        b.extend_from_slice(&[0x11; 24]);
        b
    }

    #[test]
    fn upright_and_without_gps() {
        let mut b = sample();
        patch(&mut b, false).unwrap();
        assert_eq!(&b[16..18], &[0, 1]);
        assert!(b[56..80].iter().all(|&x| x == 0x11));
        patch(&mut b, true).unwrap();
        assert_eq!(&b[8..10], &[0, 1]);
        assert!(b[56..80].iter().all(|&x| x == 0));
        assert!(!b.windows(2).any(|w| w == [0x88, 0x25]));
    }

    #[test]
    fn app1_after_jfif() {
        let jpg = [0xFF, 0xD8, 0xFF, 0xE0, 0, 4, 1, 2, 0xFF, 0xD9];
        let out = insert(&jpg, b"MM");
        assert_eq!(&out[..8], &[0xFF, 0xD8, 0xFF, 0xE0, 0, 4, 1, 2]);
        assert_eq!(&out[8..10], &[0xFF, 0xE1]);
        assert_eq!(&out[12..18], b"Exif\0\0");
    }
}
