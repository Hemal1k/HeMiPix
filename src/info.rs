//! The photo information (Exif: date taken, camera, exposure, location …) of the original, carried into the new JPG, PNG or WebP.
//! Read by kamadak-exif from HEIC / JPG / PNG / WebP / TIFF; then patched here: the orientation is set to
//! "upright" (HeMiPix turns the pixels itself) and the location (GPS) can be removed. Written into a JPG as
//! its APP1 segment (PNG and WebP: container.rs).

use std::path::Path;

/// The Exif block (TIFF bytes) of the photo, ready for the JPG; None when it has none (or it cannot be cleaned).
pub fn read(src: &Path, remove_gps: bool) -> Option<Vec<u8>> {
    let file = std::fs::File::open(src).ok()?;
    let info = exif::Reader::new().read_from_container(&mut std::io::BufReader::new(file)).ok()?;
    let mut tiff = info.buf().to_vec();
    // the location must really be gone when asked, and the thumbnail of the original always: if the block cannot be
    // patched, it is left out entirely
    patch(&mut tiff, remove_gps).map(|()| tiff)
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

/// Zeroes `len` bytes at `at`, as far as they are inside the block.
fn zero(b: &mut [u8], at: usize, len: usize) {
    let end = at.saturating_add(len).min(b.len());
    if at < end {
        b[at..end].fill(0);
    }
}

/// The first value of an IFD entry that has one value (SHORT or LONG), as an offset or a length.
fn single_value(t: &Tiff, e: usize) -> Option<usize> {
    if t.u32(e + 4)? != 1 {
        return None;
    }
    match t.u16(e + 2)? {
        3 => t.u16(e + 8).map(usize::from),
        4 => t.u32(e + 8).map(|v| v as usize),
        _ => None,
    }
}

/// The thumbnail (IFD1, and any IFD after it): never carried, whatever is chosen. It shows the whole original – not
/// cropped and not turned – so it would undo a crop. IFD0 no longer points to it, and its directory, its values and
/// the small picture itself are zeroed (so they cannot be dug out of the block either).
fn remove_thumbnail(t: &mut Tiff, next_at: usize) -> Option<()> {
    let mut next = t.u32(next_at)? as usize;
    t.b.get_mut(next_at..next_at + 4)?.fill(0);
    // a chain of a few IFDs at most (also a broken one, which points back, ends here)
    for _ in 0..4 {
        if next == 0 {
            break;
        }
        let Some(n) = t.u16(next).map(usize::from) else { break };
        let (mut jpeg_at, mut jpeg_len, mut strip_at, mut strip_len) = (None, None, None, None);
        for k in 0..n {
            let e = next + 2 + k * 12;
            let (Some(tag), Some(typ), Some(count)) = (t.u16(e), t.u16(e + 2), t.u32(e + 4)) else { break };
            match tag {
                0x0201 => jpeg_at = single_value(t, e),
                0x0202 => jpeg_len = single_value(t, e),
                0x0111 => strip_at = single_value(t, e),
                0x0117 => strip_len = single_value(t, e),
                _ => {}
            }
            // values that do not fit into the entry
            let size = type_size(typ).saturating_mul(count as usize);
            if size > 4
                && let Some(at) = t.u32(e + 8)
            {
                zero(t.b, at as usize, size);
            }
        }
        // the small picture: JPEG, or uncompressed in one strip
        for (at, len) in [(jpeg_at, jpeg_len), (strip_at, strip_len)] {
            if let (Some(at), Some(len)) = (at, len) {
                zero(t.b, at, len);
            }
        }
        let after = t.u32(next + 2 + n * 12).map_or(0, |v| v as usize);
        zero(t.b, next, 2 + n * 12 + 4);
        next = after;
    }
    Some(())
}

/// Orientation → 1; without the thumbnail (IFD1) always; without GPS when asked (its directory is zeroed and its
/// entry removed from IFD0).
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
    // first, before the GPS entry moves the end of IFD0 (with the offset of the next IFD) up
    remove_thumbnail(&mut t, entries + n * 12)?;
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

    /// A big-endian TIFF as a camera writes it: IFD0 (Orientation = 6, a GPS pointer) → IFD1 with a JPEG thumbnail.
    fn with_thumbnail() -> Vec<u8> {
        let mut b = Vec::new();
        b.extend_from_slice(b"MM\0\x2A\0\0\0\x08");
        b.extend_from_slice(&2u16.to_be_bytes());
        b.extend_from_slice(&[0x01, 0x12, 0, 3, 0, 0, 0, 1, 0, 6, 0, 0]);
        b.extend_from_slice(&[0x88, 0x25, 0, 4, 0, 0, 0, 1, 0, 0, 0, 38]);
        b.extend_from_slice(&80u32.to_be_bytes()); // the next IFD: IFD1 at 80
        // GPS IFD at 38: a latitude (3 rationals) at 56
        b.extend_from_slice(&1u16.to_be_bytes());
        b.extend_from_slice(&[0, 2, 0, 5, 0, 0, 0, 3, 0, 0, 0, 56]);
        b.extend_from_slice(&0u32.to_be_bytes());
        b.extend_from_slice(&[0x11; 24]);
        // IFD1 at 80: the JPEG thumbnail at 110, 6 bytes long
        b.extend_from_slice(&2u16.to_be_bytes());
        b.extend_from_slice(&[0x02, 0x01, 0, 4, 0, 0, 0, 1, 0, 0, 0, 110]);
        b.extend_from_slice(&[0x02, 0x02, 0, 4, 0, 0, 0, 1, 0, 0, 0, 6]);
        b.extend_from_slice(&0u32.to_be_bytes());
        assert_eq!(b.len(), 110);
        b.extend_from_slice(&[0xFF, 0xD8, 0x77, 0x77, 0xFF, 0xD9]);
        b
    }

    #[test]
    fn thumbnail_never_carried() {
        for remove_gps in [false, true] {
            let mut b = with_thumbnail();
            patch(&mut b, remove_gps).unwrap();
            // the thumbnail and IFD1 are zeroed, nothing points to them
            assert!(b[80..116].iter().all(|&x| x == 0), "remove_gps {remove_gps}");
            let next_at = if remove_gps { 22 } else { 34 };
            assert_eq!(&b[next_at..next_at + 4], &[0, 0, 0, 0]);
            // still a valid Exif block: upright, the location as asked, no field of the thumbnail
            let info = exif::Reader::new().read_raw(b).unwrap();
            assert!(info.fields().all(|f| f.ifd_num == exif::In::PRIMARY));
            let o = info.get_field(exif::Tag::Orientation, exif::In::PRIMARY).and_then(|f| f.value.get_uint(0));
            assert_eq!(o, Some(1));
            assert_eq!(info.get_field(exif::Tag::GPSLatitude, exif::In::PRIMARY).is_some(), !remove_gps);
        }
    }

    #[test]
    fn broken_block_left_out() {
        // IFD0 cut off before its end: not carried at all (neither the location nor the thumbnail could be removed)
        let mut b = with_thumbnail();
        b.truncate(30);
        assert!(patch(&mut b, false).is_none());
        assert!(patch(&mut b, true).is_none());
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
