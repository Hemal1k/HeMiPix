//! Converting one photo: decoded by the Windows Imaging Component (HEIC with the HEIF and HEVC extensions of
//! Windows, also AVIF, WebP, PNG, JPG, TIFF, BMP), turned upright, cropped and saved as JPG, PNG, WebP or TIFF with the
//! colour profile and – if wanted – the metadata (Exif) of the original.
//! Many photos at once: `run` converts a list on several threads.

use crate::{container, info};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use windows::Win32::Foundation::{GENERIC_READ, GENERIC_WRITE};
use windows::Win32::Graphics::Imaging::*;
use windows::Win32::System::Com::StructuredStorage::{PROPBAG2, PROPVARIANT};
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx};
use windows::Win32::System::Variant::{VARIANT, VT_R4, VT_UI1, VT_UI2};
use windows::core::{GUID, HSTRING, Interface, PWSTR};

/// The file types HeMiPix reads (lower case, without the dot).
pub const INPUTS: &[&str] = &["heic", "heif", "hif", "avif", "webp", "png", "jpg", "jpeg", "jfif", "tif", "tiff", "bmp"];

pub fn is_input(p: &Path) -> bool {
    p.extension().and_then(|e| e.to_str()).is_some_and(|e| INPUTS.contains(&e.to_ascii_lowercase().as_str()))
}

/// What the photos become.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Format {
    Jpg,
    Png,
    Webp,
    Tiff,
}

impl Format {
    pub const ALL: [Format; 4] = [Format::Jpg, Format::Png, Format::Webp, Format::Tiff];

    pub fn label(self) -> &'static str {
        match self {
            Format::Jpg => "JPG",
            Format::Png => "PNG",
            Format::Webp => "WebP",
            Format::Tiff => "TIFF",
        }
    }

    /// The extension of the new files.
    pub fn ext(self) -> &'static str {
        match self {
            Format::Jpg => "jpg",
            Format::Png => "png",
            Format::Webp => "webp",
            Format::Tiff => "tif",
        }
    }

    pub fn key(self) -> &'static str {
        self.ext()
    }

    pub fn from_key(k: &str) -> Format {
        Format::ALL.into_iter().find(|f| f.key() == k).unwrap_or(Format::Jpg)
    }

    /// A file already of this format (by its extension).
    pub fn is(self, p: &Path) -> bool {
        let e = p.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).unwrap_or_default();
        match self {
            Format::Jpg => matches!(e.as_str(), "jpg" | "jpeg" | "jfif"),
            Format::Png => e == "png",
            Format::Webp => e == "webp",
            Format::Tiff => matches!(e.as_str(), "tif" | "tiff"),
        }
    }

    /// Smaller files with a quality setting (JPG, WebP); PNG and TIFF keep every pixel.
    pub fn lossy(self) -> bool {
        matches!(self, Format::Jpg | Format::Webp)
    }

    /// The metadata (Exif) can be carried into it (not into TIFF).
    pub fn has_exif(self) -> bool {
        self != Format::Tiff
    }
}

/// The part of the photo to keep: x, y, width, height as fractions (0–1) of the upright photo.
pub type CropRect = [f32; 4];

#[derive(Clone, Copy, Debug)]
pub struct Options {
    pub format: Format,
    /// The quality of JPG and WebP, 1–100.
    pub quality: u8,
    /// The metadata (date, camera, location … – Exif) goes into the new file.
    pub keep_exif: bool,
    /// … but without the location.
    pub remove_gps: bool,
}

#[derive(Debug)]
pub enum Fail {
    /// Windows has no decoder for the file (HEIC: the HEIF / HEVC extensions are missing).
    NoCodec,
    Msg(String),
}

impl From<windows::core::Error> for Fail {
    fn from(e: windows::core::Error) -> Self {
        // WINCODEC_ERR_COMPONENTNOTFOUND: no decoder; 0xC00D5212 (MF_E_TOPO_CODEC_NOT_FOUND): no HEVC decoder
        if e.code().0 as u32 == 0x8898_2F50 || e.code().0 as u32 == 0xC00D_5212 {
            Fail::NoCodec
        } else {
            Fail::Msg(e.message().trim().to_string())
        }
    }
}

impl From<std::io::Error> for Fail {
    fn from(e: std::io::Error) -> Self {
        Fail::Msg(e.to_string())
    }
}

#[derive(Clone, Debug)]
pub struct Done {
    pub out: PathBuf,
    pub in_size: u64,
    pub out_size: u64,
    pub width: u32,
    pub height: u32,
}

/// The imaging factory of this thread (COM is started for the thread first).
pub fn factory() -> Result<IWICImagingFactory, Fail> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        Ok(CoCreateInstance(&CLSID_WICImagingFactory2, None, CLSCTX_INPROC_SERVER)?)
    }
}

/// The Exif orientation (1–8) as a turn / flip of WIC that puts the picture upright.
fn transform_of(orientation: u16) -> WICBitmapTransformOptions {
    match orientation {
        2 => WICBitmapTransformFlipHorizontal,
        3 => WICBitmapTransformRotate180,
        4 => WICBitmapTransformFlipVertical,
        5 => WICBitmapTransformOptions(WICBitmapTransformRotate270.0 | WICBitmapTransformFlipHorizontal.0),
        6 => WICBitmapTransformRotate90,
        7 => WICBitmapTransformOptions(WICBitmapTransformRotate90.0 | WICBitmapTransformFlipHorizontal.0),
        8 => WICBitmapTransformRotate270,
        _ => WICBitmapTransformRotate0,
    }
}

/// The orientation the decoder reports for the frame (System.Photo.Orientation), 1 when there is none.
fn orientation(frame: &IWICBitmapFrameDecode) -> u16 {
    unsafe {
        let Ok(reader) = frame.GetMetadataQueryReader() else { return 1 };
        let mut v = PROPVARIANT::default();
        if reader.GetMetadataByName(&HSTRING::from("System.Photo.Orientation"), &mut v).is_err() {
            return 1;
        }
        let inner = &v.Anonymous.Anonymous;
        if inner.vt == VT_UI2 { inner.Anonymous.uiVal } else { 1 }
    }
}

/// The colour profile (ICC) of the original, to be written into the new file unchanged.
fn color_profile(factory: &IWICImagingFactory, frame: &IWICBitmapFrameDecode) -> Option<IWICColorContext> {
    unsafe {
        let mut count = 0u32;
        if frame.GetColorContexts(&mut [], &mut count).is_err() || count == 0 {
            return None;
        }
        let mut list: Vec<Option<IWICColorContext>> = (0..count).filter_map(|_| factory.CreateColorContext().ok().map(Some)).collect();
        if list.len() as u32 != count || frame.GetColorContexts(&mut list, &mut count).is_err() {
            return None;
        }
        list.into_iter().flatten().find(|c| c.GetType().ok() == Some(WICColorContextProfile))
    }
}

/// The bytes of a colour profile (for WebP, which gets it by hand).
fn profile_bytes(c: &IWICColorContext) -> Option<Vec<u8>> {
    unsafe {
        let mut size = 0u32;
        c.GetProfileBytes(&mut [], &mut size).ok()?;
        let mut buf = vec![0u8; size as usize];
        c.GetProfileBytes(&mut buf, &mut size).ok()?;
        (size > 0).then_some(buf)
    }
}

/// The picture has transparent parts (its pixel format has alpha).
fn has_alpha(factory: &IWICImagingFactory, source: &IWICBitmapSource) -> bool {
    unsafe {
        let Ok(format) = source.GetPixelFormat() else { return false };
        factory
            .CreateComponentInfo(&format)
            .ok()
            .and_then(|i| i.cast::<IWICPixelFormatInfo2>().ok())
            .and_then(|i| i.SupportsTransparency().ok())
            .is_some_and(|b| b.as_bool())
    }
}

/// The most pixels of a picture HeMiPix converts (200 MP: the photos of today's phones fit). A bigger one – often a
/// small file that claims a huge size ("decompression bomb") – is an error of its row, not a crash of the app.
pub const MAX_PIXELS: u64 = 200_000_000;
/// The widest and highest picture WebP can hold (WEBP_MAX_DIMENSION of libwebp); a bigger one is made smaller.
pub const WEBP_MAX: u32 = 16_383;

/// The picture is not too big to convert (and not empty).
fn check_size(w: u32, h: u32) -> Result<(), Fail> {
    if w == 0 || h == 0 || u64::from(w) * u64::from(h) > MAX_PIXELS {
        return Err(too_big());
    }
    Ok(())
}

fn too_big() -> Fail {
    Fail::Msg(crate::i18n::tr("The picture is too big (at most 200 megapixels).", "Obrázek je příliš velký (nejvýš 200 megapixelů).").into())
}

/// The size of a w × h picture made smaller to fit into `max` × `max` (the same when it fits).
pub fn fit_within(w: u32, h: u32, max: u32) -> (u32, u32) {
    let long = w.max(h);
    if long <= max {
        return (w, h);
    }
    let scale = |v: u32| ((u64::from(v) * u64::from(max) + u64::from(long) / 2) / u64::from(long)).clamp(1, u64::from(max)) as u32;
    (scale(w), scale(h))
}

/// The pixels of the picture in the given format (RGBA / BGRA: 4 bytes, RGB / BGR: 3 bytes per pixel).
fn pixels(factory: &IWICImagingFactory, source: &IWICBitmapSource, format: &GUID, bytes: u32, w: u32, h: u32) -> Result<Vec<u8>, Fail> {
    // the sizes in usize and checked: they never wrap around
    let stride = w.checked_mul(bytes).ok_or_else(too_big)?;
    let len = (stride as usize).checked_mul(h as usize).ok_or_else(too_big)?;
    let mut buf = Vec::new();
    buf.try_reserve_exact(len)
        .map_err(|_| Fail::Msg(crate::i18n::tr("Not enough memory for this picture.", "Na tento obrázek nestačí paměť.").into()))?;
    buf.resize(len, 0);
    unsafe {
        let converter = factory.CreateFormatConverter()?;
        converter.Initialize(source, format, WICBitmapDitherTypeNone, None, 0.0, WICBitmapPaletteTypeCustom)?;
        converter.CopyPixels(std::ptr::null(), stride, &mut buf)?;
        Ok(buf)
    }
}

/// The crop rectangle in pixels of a w × h picture (at least 1 × 1, inside the picture).
pub fn crop_pixels(w: u32, h: u32, c: CropRect) -> (u32, u32, u32, u32) {
    let x = ((c[0].clamp(0.0, 1.0) * w as f32).round() as u32).min(w - 1);
    let y = ((c[1].clamp(0.0, 1.0) * h as f32).round() as u32).min(h - 1);
    let cw = ((c[2].clamp(0.0, 1.0) * w as f32).round() as u32).clamp(1, w - x);
    let ch = ((c[3].clamp(0.0, 1.0) * h as f32).round() as u32).clamp(1, h - y);
    (x, y, cw, ch)
}

/// Converts `src` into `dst` in the chosen format (written through a temporary file, so a half-written file never stays).
pub fn convert(factory: &IWICImagingFactory, src: &Path, dst: &Path, o: &Options, crop: Option<CropRect>) -> Result<Done, Fail> {
    let in_size = std::fs::metadata(src)?.len();
    let tmp = dst.with_extension("hemipix-tmp");
    // the metadata of the original, upright and without the location if wanted
    let exif = if o.keep_exif && o.format.has_exif() { info::read(src, o.remove_gps) } else { None };
    let r = encode(factory, src, &tmp, o, crop, exif.as_deref());
    let (width, height) = match r {
        Ok(size) => size,
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            return Err(e);
        }
    };
    // the date of the original stays, so the photos sort as before
    if let Ok(modified) = std::fs::metadata(src).and_then(|m| m.modified()) {
        let _ = std::fs::File::options().write(true).open(&tmp).and_then(|f| f.set_modified(modified));
    }
    std::fs::rename(&tmp, dst).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })?;
    let out_size = std::fs::metadata(dst)?.len();
    Ok(Done { out: dst.to_path_buf(), in_size, out_size, width, height })
}

/// A property of an encoder: a float (quality) or a byte (TIFF compression).
unsafe fn set_option(props: &windows::Win32::System::Com::StructuredStorage::IPropertyBag2, name: &str, value: VARIANT) -> Result<(), Fail> {
    let mut name: Vec<u16> = name.encode_utf16().chain([0]).collect();
    let bag = PROPBAG2 { pstrName: PWSTR(name.as_mut_ptr()), ..Default::default() };
    unsafe { props.Write(1, &bag, &value)? };
    Ok(())
}

fn encode(factory: &IWICImagingFactory, src: &Path, tmp: &Path, o: &Options, crop: Option<CropRect>, exif: Option<&[u8]>) -> Result<(u32, u32), Fail> {
    unsafe {
        let decoder = factory.CreateDecoderFromFilename(&HSTRING::from(src.as_os_str()), None, GENERIC_READ, WICDecodeMetadataCacheOnDemand)?;
        let frame = decoder.GetFrame(0)?;
        let profile = color_profile(factory, &frame);

        // upright: what the camera recorded as its orientation is applied to the pixels
        let mut source: IWICBitmapSource = frame.clone().into();
        let turn = transform_of(orientation(&frame));
        if turn != WICBitmapTransformRotate0 {
            let rotator = factory.CreateBitmapFlipRotator()?;
            rotator.Initialize(&source, turn)?;
            source = rotator.into();
        }
        let (mut w, mut h) = (0u32, 0u32);
        source.GetSize(&mut w, &mut h)?;
        check_size(w, h)?;

        // cropped to the rectangle chosen in the crop editor
        if let Some(c) = crop {
            let (x, y, cw, ch) = crop_pixels(w, h, c);
            if (cw, ch) != (w, h) {
                let clipper = factory.CreateBitmapClipper()?;
                let rect = WICRect { X: x as i32, Y: y as i32, Width: cw as i32, Height: ch as i32 };
                clipper.Initialize(&source, &rect)?;
                source = clipper.into();
                (w, h) = (cw, ch);
            }
        }
        let alpha = has_alpha(factory, &source);

        // WebP: libwebp, then the profile and the metadata put in by hand
        if o.format == Format::Webp {
            // WebP holds at most 16 383 × 16 383 pixels: a wider panorama is made smaller to fit
            let (nw, nh) = fit_within(w, h, WEBP_MAX);
            if (nw, nh) != (w, h) {
                let scaler = factory.CreateBitmapScaler()?;
                scaler.Initialize(&source, nw, nh, WICBitmapInterpolationModeFant)?;
                source = scaler.into();
                (w, h) = (nw, nh);
            }
            let quality = o.quality.clamp(1, 100) as f32;
            // encode_simple returns the error of libwebp (encode would end the app on it)
            let encoded = if alpha {
                let rgba = pixels(factory, &source, &GUID_WICPixelFormat32bppRGBA, 4, w, h)?;
                webp::Encoder::from_rgba(&rgba, w, h).encode_simple(false, quality).map(|m| m.to_vec())
            } else {
                let rgb = pixels(factory, &source, &GUID_WICPixelFormat24bppRGB, 3, w, h)?;
                webp::Encoder::from_rgb(&rgb, w, h).encode_simple(false, quality).map(|m| m.to_vec())
            };
            let data = encoded.map_err(|e| Fail::Msg(format!("WebP: {e:?}")))?;
            let icc = profile.as_ref().and_then(profile_bytes);
            std::fs::write(tmp, container::webp_with(&data, w, h, icc.as_deref(), exif))?;
            return Ok((w, h));
        }

        // JPG has no transparency: transparent parts are put on white (not black)
        if o.format == Format::Jpg && alpha {
            let bgra = pixels(factory, &source, &GUID_WICPixelFormat32bppBGRA, 4, w, h)?;
            let mut bgr = Vec::with_capacity(bgra.len() / 4 * 3);
            for p in bgra.chunks_exact(4) {
                let a = p[3] as u32;
                for c in &p[..3] {
                    bgr.push(((*c as u32 * a + 255 * (255 - a)) / 255) as u8);
                }
            }
            source = factory.CreateBitmapFromMemory(w, h, &GUID_WICPixelFormat24bppBGR, w * 3, &bgr)?.into();
        }
        let (container_format, mut pixel_format) = match o.format {
            Format::Jpg => (GUID_ContainerFormatJpeg, GUID_WICPixelFormat24bppBGR),
            Format::Png => (GUID_ContainerFormatPng, if alpha { GUID_WICPixelFormat32bppBGRA } else { GUID_WICPixelFormat24bppBGR }),
            _ => (GUID_ContainerFormatTiff, if alpha { GUID_WICPixelFormat32bppBGRA } else { GUID_WICPixelFormat24bppBGR }),
        };
        let converter = factory.CreateFormatConverter()?;
        converter.Initialize(&source, &pixel_format, WICBitmapDitherTypeNone, None, 0.0, WICBitmapPaletteTypeCustom)?;

        let stream = factory.CreateStream()?;
        stream.InitializeFromFilename(&HSTRING::from(tmp.as_os_str()), GENERIC_WRITE.0)?;
        let encoder = factory.CreateEncoder(&container_format, std::ptr::null())?;
        encoder.Initialize(&stream, WICBitmapEncoderNoCache)?;
        let mut out: Option<IWICBitmapFrameEncode> = None;
        let mut props = None;
        encoder.CreateNewFrame(&mut out, &mut props)?;
        let out = out.ok_or_else(|| Fail::Msg(crate::i18n::tr("Windows could not write the picture.", "Windows obrázek nedokázal zapsat.").into()))?;
        if let Some(props) = &props {
            let mut value = VARIANT::default();
            match o.format {
                Format::Jpg => {
                    (*value.Anonymous.Anonymous).vt = VT_R4;
                    (*value.Anonymous.Anonymous).Anonymous.fltVal = o.quality.clamp(1, 100) as f32 / 100.0;
                    set_option(props, "ImageQuality", value)?;
                }
                Format::Tiff => {
                    // lossless and read everywhere
                    (*value.Anonymous.Anonymous).vt = VT_UI1;
                    (*value.Anonymous.Anonymous).Anonymous.bVal = WICTiffCompressionLZW.0 as u8;
                    let _ = set_option(props, "TiffCompressionMethod", value);
                }
                _ => {}
            }
        }
        out.Initialize(props.as_ref())?;
        out.SetSize(w, h)?;
        out.SetPixelFormat(&mut pixel_format)?;
        if let Some(p) = profile {
            let _ = out.SetColorContexts(&[Some(p)]);
        }
        out.WriteSource(&converter, std::ptr::null())?;
        out.Commit()?;
        encoder.Commit()?;
        drop(stream);

        // the metadata: JPG an APP1 segment, PNG an eXIf chunk
        if let Some(tiff) = exif {
            let data = std::fs::read(tmp)?;
            let with = match o.format {
                Format::Jpg => info::insert(&data, tiff),
                Format::Png => container::png_with_exif(&data, tiff),
                _ => data,
            };
            std::fs::write(tmp, with)?;
        }
        Ok((w, h))
    }
}
/// A small upright picture of the photo for the crop editor.
pub struct Preview {
    pub rgba: Vec<u8>,
    pub width: u32,
    pub height: u32,
    /// The size of the upright photo itself.
    pub full: (u32, u32),
}

pub fn preview(src: &Path, max: u32) -> Result<Preview, Fail> {
    let factory = factory()?;
    unsafe {
        let decoder = factory.CreateDecoderFromFilename(&HSTRING::from(src.as_os_str()), None, GENERIC_READ, WICDecodeMetadataCacheOnDemand)?;
        let frame = decoder.GetFrame(0)?;
        let mut source: IWICBitmapSource = frame.clone().into();
        let turn = transform_of(orientation(&frame));
        if turn != WICBitmapTransformRotate0 {
            let rotator = factory.CreateBitmapFlipRotator()?;
            rotator.Initialize(&source, turn)?;
            source = rotator.into();
        }
        let (mut w, mut h) = (0u32, 0u32);
        source.GetSize(&mut w, &mut h)?;
        let full = (w, h);
        if w.max(h) > max {
            let k = max as f64 / w.max(h) as f64;
            let (nw, nh) = (((w as f64 * k).round() as u32).max(1), ((h as f64 * k).round() as u32).max(1));
            let scaler = factory.CreateBitmapScaler()?;
            scaler.Initialize(&source, nw, nh, WICBitmapInterpolationModeFant)?;
            source = scaler.into();
            (w, h) = (nw, nh);
        }
        let converter = factory.CreateFormatConverter()?;
        converter.Initialize(&source, &GUID_WICPixelFormat32bppRGBA, WICBitmapDitherTypeNone, None, 0.0, WICBitmapPaletteTypeCustom)?;
        let mut rgba = vec![0u8; (w * h * 4) as usize];
        converter.CopyPixels(std::ptr::null(), w * 4, &mut rgba)?;
        Ok(Preview { rgba, width: w, height: h, full })
    }
}

pub struct Job {
    pub id: usize,
    pub src: PathBuf,
    pub dst: PathBuf,
    pub crop: Option<CropRect>,
}

pub enum Event {
    Started(usize),
    Finished(usize, Result<Done, Fail>),
    /// Everything is converted (or the rest was stopped).
    AllDone,
}

/// Converts the jobs on several threads (as many as the processor has cores, at most 8); each result is sent
/// as soon as it is ready. `stop` ends the work after the photos being converted.
pub fn run(jobs: Vec<Job>, o: Options, stop: Arc<AtomicBool>, tx: Sender<Event>, wake: impl Fn() + Send + Sync + 'static) {
    let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).clamp(1, 8).min(jobs.len().max(1));
    let queue = Arc::new(Mutex::new(VecDeque::from(jobs)));
    let wake = Arc::new(wake);
    std::thread::spawn(move || {
        let workers: Vec<_> = (0..threads)
            .map(|_| {
                let (queue, stop, tx, wake) = (queue.clone(), stop.clone(), tx.clone(), wake.clone());
                std::thread::spawn(move || {
                    let factory = factory();
                    loop {
                        if stop.load(Ordering::Relaxed) {
                            break;
                        }
                        let Some(job) = queue.lock().unwrap().pop_front() else { break };
                        let _ = tx.send(Event::Started(job.id));
                        wake();
                        let r = match &factory {
                            Ok(f) => convert(f, &job.src, &job.dst, &o, job.crop),
                            Err(Fail::NoCodec) => Err(Fail::NoCodec),
                            Err(Fail::Msg(m)) => Err(Fail::Msg(m.clone())),
                        };
                        let _ = tx.send(Event::Finished(job.id, r));
                        wake();
                    }
                })
            })
            .collect();
        for w in workers {
            let _ = w.join();
        }
        let _ = tx.send(Event::AllDone);
        wake();
    });
}

/// Real conversions of sample photos: `HEMIPIX_SAMPLES=<folder with .heic> cargo test -- --include-ignored`.
#[cfg(test)]
mod tests {
    use super::*;

    fn size_of(f: &IWICImagingFactory, p: &Path) -> (u32, u32) {
        unsafe {
            let d = f.CreateDecoderFromFilename(&HSTRING::from(p.as_os_str()), None, GENERIC_READ, WICDecodeMetadataCacheOnDemand).unwrap();
            let (mut w, mut h) = (0, 0);
            d.GetFrame(0).unwrap().GetSize(&mut w, &mut h).unwrap();
            (w, h)
        }
    }

    fn samples() -> Option<PathBuf> {
        std::env::var_os("HEMIPIX_SAMPLES").map(PathBuf::from)
    }

    fn opts() -> Options {
        Options { format: Format::Jpg, quality: 85, keep_exif: true, remove_gps: false }
    }

    #[test]
    fn crop_in_pixels() {
        assert_eq!(crop_pixels(4000, 3000, [0.125, 0.0, 0.75, 1.0]), (500, 0, 3000, 3000));
        assert_eq!(crop_pixels(100, 100, [0.9, 0.9, 0.5, 0.5]), (90, 90, 10, 10));
        assert_eq!(crop_pixels(100, 100, [0.0, 0.0, 0.0, 0.0]), (0, 0, 1, 1));
    }

    #[test]
    fn size_limits() {
        // a 200 MP photo of a phone passes, bigger or empty pictures are an error (never a crash)
        assert!(check_size(16_320, 12_240).is_ok());
        assert!(check_size(30_000, 30_000).is_err());
        assert!(check_size(u32::MAX, u32::MAX).is_err());
        assert!(check_size(0, 100).is_err());
        // WebP: a panorama is made smaller to 16 383 px, the side ratio stays; smaller pictures stay as they are
        assert_eq!(fit_within(20_000, 4_000, WEBP_MAX), (16_383, 3_277));
        assert_eq!(fit_within(4_000, 20_000, WEBP_MAX), (3_277, 16_383));
        assert_eq!(fit_within(16_383, 16_383, WEBP_MAX), (16_383, 16_383));
        assert_eq!(fit_within(200_000, 1, WEBP_MAX), (16_383, 1));
        assert_eq!(fit_within(4_000, 3_000, WEBP_MAX), (4_000, 3_000));
    }
    #[test]
    #[ignore]
    fn heic_to_jpg() {
        let Some(dir) = samples() else { return };
        let f = factory().unwrap();
        let out = std::env::temp_dir().join("hemipix-test");
        let _ = std::fs::create_dir_all(&out);
        for e in std::fs::read_dir(&dir).unwrap().flatten() {
            let src = e.path();
            if src.extension().is_none_or(|x| x != "heic") {
                continue;
            }
            let (w, h) = size_of(&f, &src);
            let dst = out.join(src.file_stem().unwrap()).with_extension("jpg");
            let done = convert(&f, &src, &dst, &opts(), None).unwrap();
            assert_eq!((done.width, done.height), (w, h));
            assert_eq!(size_of(&f, &dst), (w, h));
            // smaller: lower quality and the middle quarter only
            let small = Options { quality: 70, ..opts() };
            let dst2 = out.join(format!("{}-small.jpg", src.file_stem().unwrap().to_string_lossy()));
            let d2 = convert(&f, &src, &dst2, &small, Some([0.25, 0.25, 0.5, 0.5])).unwrap();
            assert_eq!((d2.width, d2.height), { let (_, _, cw, ch) = crop_pixels(w, h, [0.25, 0.25, 0.5, 0.5]); (cw, ch) });
            assert!(d2.out_size < done.out_size);
            println!("{}: {}x{} {} B -> {} B, small {} B", src.display(), w, h, done.in_size, done.out_size, d2.out_size);
        }
    }

    /// An upright HEIC (made from a sample by the HEIF encoder of Windows with "turn 90°", without recompressing)
    /// must come out upright – turned once, not twice and not never.
    #[test]
    #[ignore]
    fn turned_heic() {
        let Some(dir) = samples() else { return };
        let f = factory().unwrap();
        let src = dir.join("sample1.heic");
        let turned = std::env::temp_dir().join("hemipix-test").join("turned.heic");
        let _ = std::fs::create_dir_all(turned.parent().unwrap());
        unsafe {
            let d = f.CreateDecoderFromFilename(&HSTRING::from(src.as_os_str()), None, GENERIC_READ, WICDecodeMetadataCacheOnDemand).unwrap();
            let frame = d.GetFrame(0).unwrap();
            let stream = f.CreateStream().unwrap();
            stream.InitializeFromFilename(&HSTRING::from(turned.as_os_str()), GENERIC_WRITE.0).unwrap();
            let enc = f.CreateEncoder(&GUID_ContainerFormatHeif, std::ptr::null()).unwrap();
            enc.Initialize(&stream, WICBitmapEncoderNoCache).unwrap();
            let (mut out, mut props) = (None, None);
            enc.CreateNewFrame(&mut out, &mut props).unwrap();
            let out: IWICBitmapFrameEncode = out.unwrap();
            let props = props.unwrap();
            let mut name: Vec<u16> = "BitmapTransform".encode_utf16().chain([0]).collect();
            let bag = PROPBAG2 { pstrName: PWSTR(name.as_mut_ptr()), ..Default::default() };
            let mut v = VARIANT::default();
            (*v.Anonymous.Anonymous).vt = windows::Win32::System::Variant::VT_UI1;
            (*v.Anonymous.Anonymous).Anonymous.bVal = WICBitmapTransformRotate90.0 as u8;
            props.Write(1, &bag, &v).unwrap();
            out.Initialize(&props).unwrap();
            out.WriteSource(&frame, std::ptr::null()).unwrap();
            out.Commit().unwrap();
            enc.Commit().unwrap();
        }
        let (w, h) = size_of(&f, &src);
        println!("decoder sees the turned file as {:?}, orientation reported: {}", size_of(&f, &turned), {
            let d = unsafe { f.CreateDecoderFromFilename(&HSTRING::from(turned.as_os_str()), None, GENERIC_READ, WICDecodeMetadataCacheOnDemand).unwrap() };
            orientation(&unsafe { d.GetFrame(0).unwrap() })
        });
        let dst = turned.with_extension("jpg");
        let done = convert(&f, &turned, &dst, &opts(), None).unwrap();
        assert_eq!((done.width, done.height), (h, w), "the photo must be turned exactly once");
    }

    /// A JPG with Exif orientation 6 (turn 90°) and a location: upright pixels, orientation 1, location removable.
    #[test]
    #[ignore]
    fn jpg_with_exif() {
        let Some(dir) = samples() else { return };
        let f = factory().unwrap();
        let tmp = std::env::temp_dir().join("hemipix-test");
        let _ = std::fs::create_dir_all(&tmp);
        // a plain JPG of a sample, then a hand-made Exif with orientation 6 and GPS put into it
        let plain = tmp.join("plain.jpg");
        convert(&f, &dir.join("sample1.heic"), &plain, &Options { keep_exif: false, ..opts() }, None).unwrap();
        let mut tiff = Vec::new();
        tiff.extend_from_slice(b"MM\0\x2A\0\0\0\x08");
        tiff.extend_from_slice(&2u16.to_be_bytes());
        tiff.extend_from_slice(&[0x01, 0x12, 0, 3, 0, 0, 0, 1, 0, 6, 0, 0]);
        tiff.extend_from_slice(&[0x88, 0x25, 0, 4, 0, 0, 0, 1, 0, 0, 0, 38]);
        tiff.extend_from_slice(&0u32.to_be_bytes());
        tiff.extend_from_slice(&1u16.to_be_bytes());
        tiff.extend_from_slice(&[0, 2, 0, 5, 0, 0, 0, 3, 0, 0, 0, 56]);
        tiff.extend_from_slice(&0u32.to_be_bytes());
        for v in [49u32, 1, 11, 1, 30, 1] {
            tiff.extend_from_slice(&v.to_be_bytes());
        }
        let src = tmp.join("rotated.jpg");
        std::fs::write(&src, crate::info::insert(&std::fs::read(&plain).unwrap(), &tiff)).unwrap();
        let (w, h) = size_of(&f, &src);
        for remove_gps in [false, true] {
            let dst = tmp.join(format!("upright-{remove_gps}.jpg"));
            let done = convert(&f, &src, &dst, &Options { remove_gps, ..opts() }, None).unwrap();
            assert_eq!((done.width, done.height), (h, w));
            let info = exif::Reader::new().read_from_container(&mut std::io::BufReader::new(std::fs::File::open(&dst).unwrap())).unwrap();
            let o = info.get_field(exif::Tag::Orientation, exif::In::PRIMARY).and_then(|f| f.value.get_uint(0));
            assert_eq!(o, Some(1));
            let gps = info.get_field(exif::Tag::GPSLatitude, exif::In::PRIMARY).is_some();
            assert_eq!(gps, !remove_gps);
        }
    }

    /// PNG, WebP and TIFF: the right size, readable by Windows, the metadata in PNG and WebP.
    #[test]
    #[ignore]
    fn other_formats() {
        let Some(dir) = samples() else { return };
        let f = factory().unwrap();
        let tmp = std::env::temp_dir().join("hemipix-test-formats");
        let _ = std::fs::create_dir_all(&tmp);
        // a JPG with metadata (orientation 1, a model name) as the source
        let plain = tmp.join("plain.jpg");
        convert(&f, &dir.join("sample1.heic"), &plain, &Options { keep_exif: false, ..opts() }, None).unwrap();
        let mut tiff = Vec::new();
        tiff.extend_from_slice(b"MM\0\x2A\0\0\0\x08");
        tiff.extend_from_slice(&1u16.to_be_bytes());
        tiff.extend_from_slice(&[0x01, 0x10, 0, 2, 0, 0, 0, 4, b'P', b'i', b'x', 0]);
        tiff.extend_from_slice(&0u32.to_be_bytes());
        let src = tmp.join("with-exif.jpg");
        std::fs::write(&src, crate::info::insert(&std::fs::read(&plain).unwrap(), &tiff)).unwrap();
        let (w, h) = size_of(&f, &src);
        for format in [Format::Png, Format::Webp, Format::Tiff] {
            let dst = tmp.join(format!("out.{}", format.ext()));
            let done = convert(&f, &src, &dst, &Options { format, ..opts() }, Some([0.0, 0.0, 0.5, 1.0])).unwrap();
            assert_eq!((done.width, done.height), (w / 2, h), "{format:?}");
            assert_eq!(size_of(&f, &dst), (w / 2, h), "{format:?} readable by Windows");
            if format.has_exif() {
                let info = exif::Reader::new().read_from_container(&mut std::io::BufReader::new(std::fs::File::open(&dst).unwrap())).unwrap();
                let model = info.get_field(exif::Tag::Model, exif::In::PRIMARY).map(|f| f.display_value().to_string());
                assert_eq!(model.as_deref(), Some("\"Pix\""), "{format:?} keeps the metadata");
            }
            println!("{format:?}: {} B", done.out_size);
        }
    }
}