use crate::{contracts::*, preset::Frame};
use image::{DynamicImage, ImageDecoder, ImageEncoder, ImageReader, RgbaImage};
use serde::{Deserialize, Serialize};
use std::{
    collections::VecDeque,
    fs::File,
    io::{BufReader, BufWriter, Read, Seek, SeekFrom, Write},
    path::Path,
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Instant,
};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PhotoInfo {
    pub width: u32,
    pub height: u32,
    pub ppi: Option<[f64; 2]>,
    pub orientation: u8,
    pub has_profile: bool,
}
/// Automatic Quick Fix tone curve for one combination of Auto Levels, Auto Contrast and
/// Auto Color. Checked fixes run in that order and each analyzes the previous one's result,
/// as running the Elements commands in sequence would.
#[derive(Clone)]
pub struct Analysis {
    pub curve: Curve,
}
pub struct Prepared {
    pub thumbnail: RgbaImage,
    pub before_png: Vec<u8>,
    pub fingerprint: String,
    pub info: PhotoInfo,
}
fn any_auto(q: &QuickFix) -> bool {
    q.levels || q.contrast || q.color
}
// Only tiny parameter records are retained; no full-resolution image cache.
pub type AnalysisCache = Mutex<VecDeque<(String, Analysis)>>;
fn cached_analysis(
    image: &RgbaImage,
    fingerprint: &str,
    q: &QuickFix,
    cache: &AnalysisCache,
) -> Analysis {
    let key = format!(
        "{fingerprint}:{}{}{}",
        u8::from(q.levels),
        u8::from(q.contrast),
        u8::from(q.color)
    );
    let lock = || cache.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((_, a)) = lock().iter().find(|(k, _)| *k == key) {
        return a.clone();
    }
    // Analyze without holding the lock; concurrent duplicates compute identical results.
    let a = analyze(image, q);
    let mut entries = lock();
    if !entries.iter().any(|(k, _)| *k == key) {
        if entries.len() == 512 {
            entries.pop_front();
        }
        entries.push_back((key, a.clone()));
    }
    a
}
/// Removes every cached combination for a source whose contents changed.
pub fn forget_analysis(cache: &AnalysisCache, fingerprint: &str) {
    let prefix = format!("{fingerprint}:");
    cache
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .retain(|(k, _)| !k.starts_with(&prefix));
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Timings {
    pub decode_ms: u64,
    pub filter_ms: u64,
    pub encode_ms: u64,
}
pub fn cancelled(cancel: &AtomicBool) -> Result<(), String> {
    if cancel.load(Ordering::Relaxed) {
        Err("Cancelled".into())
    } else {
        Ok(())
    }
}

fn reader(path: &Path) -> Result<ImageReader<BufReader<File>>, String> {
    let mut r = ImageReader::open(path)
        .map_err(|e| format!("Open: {e}"))?
        .with_guessed_format()
        .map_err(|e| e.to_string())?;
    if !matches!(
        r.format(),
        Some(image::ImageFormat::Jpeg | image::ImageFormat::Png | image::ImageFormat::WebP)
    ) {
        return Err("Unsupported format. Use still 8-bit JPEG, PNG or WebP.".into());
    }
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(65535);
    limits.max_image_height = Some(65535);
    limits.max_alloc = Some(768 * 1024 * 1024);
    r.limits(limits);
    Ok(r)
}
fn exif_density(bytes: Option<Vec<u8>>) -> Option<[f64; 2]> {
    let b = bytes?;
    let e = exif::Reader::new()
        .read_raw(b.strip_prefix(b"Exif\0\0").unwrap_or(&b).to_vec())
        .ok()?;
    let unit = e
        .get_field(exif::Tag::ResolutionUnit, exif::In::PRIMARY)?
        .value
        .get_uint(0)?;
    let get = |t| {
        let f = e.get_field(t, exif::In::PRIMARY)?;
        if let exif::Value::Rational(v) = &f.value {
            v.first().map(|x| x.to_f64())
        } else {
            None
        }
    };
    let factor = match unit {
        2 => 1.,
        3 => 2.54,
        _ => return None,
    };
    let p = [
        get(exif::Tag::XResolution)? * factor,
        get(exif::Tag::YResolution)? * factor,
    ];
    if p.iter().all(|x| x.is_finite() && *x > 0. && *x <= 65535.) {
        Some(p)
    } else {
        None
    }
}
fn container_density(path: &Path) -> Option<[f64; 2]> {
    let mut f = BufReader::new(File::open(path).ok()?);
    let mut magic = [0; 8];
    f.read_exact(&mut magic).ok()?;
    if magic == *b"\x89PNG\r\n\x1a\n" {
        loop {
            let mut h = [0; 8];
            f.read_exact(&mut h).ok()?;
            let n = u32::from_be_bytes(h[..4].try_into().ok()?) as i64;
            if &h[4..] == b"pHYs" && n == 9 {
                let mut d = [0; 9];
                f.read_exact(&mut d).ok()?;
                if d[8] != 1 {
                    return None;
                }
                return Some([
                    u32::from_be_bytes(d[..4].try_into().ok()?) as f64 * 0.0254,
                    u32::from_be_bytes(d[4..8].try_into().ok()?) as f64 * 0.0254,
                ]);
            }
            if &h[4..] == b"IDAT" || &h[4..] == b"IEND" {
                return None;
            }
            f.seek(SeekFrom::Current(n + 4)).ok()?;
        }
    } else if magic[..2] == [255, 216] {
        f.seek(SeekFrom::Start(2)).ok()?;
        loop {
            let mut m = [0; 2];
            f.read_exact(&mut m).ok()?;
            if m[0] != 255 || m[1] == 218 || m[1] == 217 {
                return None;
            }
            let mut l = [0; 2];
            f.read_exact(&mut l).ok()?;
            let n = u16::from_be_bytes(l) as i64 - 2;
            if n < 0 {
                return None;
            }
            if m[1] == 224 && n >= 12 {
                let mut d = [0; 12];
                f.read_exact(&mut d).ok()?;
                if &d[..5] == b"JFIF\0" {
                    let factor = match d[7] {
                        1 => 1.,
                        2 => 2.54,
                        _ => return None,
                    };
                    let p = [
                        u16::from_be_bytes([d[8], d[9]]) as f64 * factor,
                        u16::from_be_bytes([d[10], d[11]]) as f64 * factor,
                    ];
                    if p[0] > 0. && p[1] > 0. {
                        return Some(p);
                    }
                    return None;
                }
                f.seek(SeekFrom::Current(n - 12)).ok()?;
            } else {
                f.seek(SeekFrom::Current(n)).ok()?;
            }
        }
    }
    None
}
pub fn probe(path: &Path) -> Result<PhotoInfo, String> {
    let r = reader(path)?;
    let animated = match r.format() {
        Some(image::ImageFormat::Png) => image::codecs::png::PngDecoder::new(BufReader::new(
            File::open(path).map_err(|e| e.to_string())?,
        ))
        .and_then(|d| d.is_apng())
        .map_err(|e| e.to_string())?,
        Some(image::ImageFormat::WebP) => image::codecs::webp::WebPDecoder::new(BufReader::new(
            File::open(path).map_err(|e| e.to_string())?,
        ))
        .map_err(|e| e.to_string())?
        .has_animation(),
        _ => false,
    };
    if animated {
        return Err("Animated images are unsupported. Export a still frame first.".into());
    }
    let mut d = r.into_decoder().map_err(|e| format!("Header: {e}"))?;
    let (mut w, mut h) = d.dimensions();
    if w as u64 * h as u64 > MAX_PIXELS {
        return Err("Source exceeds the 100 megapixel budget.".into());
    }
    if d.color_type().bits_per_pixel() / d.color_type().channel_count() as u16 > 8 {
        return Err("Unsupported bit depth: convert to 8-bit first.".into());
    }
    let metadata = d.exif_metadata().map_err(|e| e.to_string())?;
    let orientation = d.orientation().map_err(|e| e.to_string())?.to_exif();
    let mut ppi = exif_density(metadata).or_else(|| container_density(path));
    ppi = ppi.filter(|p| p.iter().all(|v| v.is_finite() && *v > 0. && *v <= 65535.));
    if orientation >= 5 {
        std::mem::swap(&mut w, &mut h);
        if let Some(p) = &mut ppi {
            p.swap(0, 1);
        }
    }
    Ok(PhotoInfo {
        width: w,
        height: h,
        ppi,
        orientation,
        has_profile: d.icc_profile().map_err(|e| e.to_string())?.is_some(),
    })
}
pub fn estimate(info: &PhotoInfo, size: &ResolvedSize, sharpen: bool) -> Result<u64, String> {
    // image's separable resize first allocates width(source) × height(target)
    // RGBA32F scratch. Account for extreme stretch ratios, not only pixel totals.
    let source = u64::from(info.width) * u64::from(info.height);
    let [_, _, crop_w, crop_h] = size.crop.map(u64::from);
    let [_, _, photo_w, photo_h] = size.photo.map(u64::from);
    let (photo, output) = (
        photo_w * photo_h,
        u64::from(size.width) * u64::from(size.height),
    );
    let cropped = if crop_w * crop_h == source {
        0
    } else {
        crop_w * crop_h * 4
    };
    let resize = if (crop_w, crop_h) == (photo_w, photo_h) {
        0
    } else {
        source * 4 + cropped + crop_w * photo_h * 16 + photo * 4
    };
    // A white border canvas is one more RGBA buffer.
    let canvas = if output == photo { 0 } else { output * 4 };
    // Decode/rotation: up to two RGBA buffers. Sharpen: RGBA32F + two RGBA8.
    source
        .checked_mul(8)
        .map(|decode| {
            decode
                .max(resize)
                .max(photo * if sharpen { 28 } else { 8 } + canvas)
        })
        .and_then(|v| v.checked_add(64 * 1024 * 1024))
        .ok_or("Memory estimate overflow".into())
}
fn decode(path: &Path, expected: &PhotoInfo, cancel: &AtomicBool) -> Result<RgbaImage, String> {
    cancelled(cancel)?;
    let mut d = reader(path)?.into_decoder().map_err(|e| e.to_string())?;
    let profile = d.icc_profile().map_err(|e| e.to_string())?;
    let orientation = d.orientation().map_err(|e| e.to_string())?;
    let (mut w, mut h) = d.dimensions();
    if orientation.to_exif() >= 5 {
        std::mem::swap(&mut w, &mut h);
    }
    if (w, h, orientation.to_exif()) != (expected.width, expected.height, expected.orientation)
        || d.color_type().bits_per_pixel() / u16::from(d.color_type().channel_count()) > 8
    {
        return Err(
            "Source changed after its memory reservation. Scan and retry this photo.".into(),
        );
    }
    let mut img = DynamicImage::from_decoder(d).map_err(|e| format!("Decode: {e}"))?;
    cancelled(cancel)?;
    img.apply_orientation(orientation);
    let mut rgba = img.into_rgba8();
    if let Some(icc) = profile {
        let p =
            lcms2::Profile::new_icc(&icc).map_err(|e| format!("Unsupported color profile: {e}"))?;
        if p.color_space() != lcms2::ColorSpaceSignature::RgbData {
            return Err("Unsupported non-RGB ICC profile. Convert to sRGB first.".into());
        }
        let transform = lcms2::Transform::<[u8; 3], [u8; 3]>::new(
            &p,
            lcms2::PixelFormat::RGB_8,
            &lcms2::Profile::new_srgb(),
            lcms2::PixelFormat::RGB_8,
            lcms2::Intent::Perceptual,
        )
        .map_err(|e| format!("Color conversion: {e}"))?;
        // Row-sized transforms avoid a second full image allocation.
        let stride = rgba.width() as usize * 4;
        for row in rgba.as_mut().chunks_mut(stride) {
            let mut rgb: Vec<[u8; 3]> = row
                .as_chunks::<4>()
                .0
                .iter()
                .map(|p| [p[0], p[1], p[2]])
                .collect();
            transform.transform_in_place(&mut rgb);
            for (p, c) in row.as_chunks_mut::<4>().0.iter_mut().zip(rgb) {
                p[..3].copy_from_slice(&c);
            }
        }
    }
    cancelled(cancel)?;
    Ok(rgba)
}
// Quick Fix behavior follows Adobe's documented Photoshop/Elements commands. Adobe's exact
// implementation is not public; clip fractions are Photoshop's documented defaults.
/// Auto Levels ("Enhance Per Channel Contrast"): ignore 0.1% at each end of every channel.
const LEVELS_CLIP: f64 = 0.001;
/// Auto Contrast ("Enhance Monochromatic Contrast"): 0.5%, one mapping for all channels.
const CONTRAST_CLIP: f64 = 0.005;
/// Auto Color ("Find Dark & Light Colors"): average the darkest/lightest 0.5% of pixels.
const COLOR_SAMPLE: f64 = 0.005;
/// Framebatch safeguard, not Adobe behavior: a dark/light color this saturated is probably
/// a colored subject rather than a cast, so it is neutralized only partially (full at or
/// below the first chroma, none at or above the second).
const CAST_CHROMA: (f32, f32) = (48., 112.);
/// Snap Neutral Midtones: near-neutral midtone pixels and the per-channel gamma range.
const NEUTRAL_CHROMA: f32 = 32.;
const NEUTRAL_MIN_SHARE: f64 = 0.005;
const MIDTONE_GAMMA: (f32, f32) = (0.75, 1.33);

type Curve = [[f32; 256]; 3];
fn luma(v: [f32; 3]) -> f32 {
    0.2126 * v[0] + 0.7152 * v[1] + 0.0722 * v[2]
}
fn level(v: f32) -> usize {
    v.round().clamp(0., 255.) as usize
}
fn map(curve: &Curve, p: &[u8; 3]) -> [f32; 3] {
    std::array::from_fn(|c| curve[c][p[c] as usize])
}
/// Histogram of one channel after its current curve.
fn mapped(curve: &[f32; 256], hist: &[u64; 256]) -> [u64; 256] {
    let mut out = [0; 256];
    for (i, n) in hist.iter().enumerate() {
        out[level(curve[i])] += n;
    }
    out
}
/// Darkest and lightest levels after ignoring `fraction` of the samples at each end.
fn clip_points(h: &[u64; 256], fraction: f64) -> Option<(f32, f32)> {
    let tail = (h.iter().sum::<u64>() as f64 * fraction) as u64;
    let mut s = 0;
    let lo = h.iter().position(|v| {
        s += v;
        s > tail
    })?;
    s = 0;
    let hi = 255
        - h.iter().rev().position(|v| {
            s += v;
            s > tail
        })?;
    Some((lo as f32, hi as f32))
}
/// Maps `lo` to 0 and `hi` to 255, clipping like a sequential 8-bit adjustment.
fn stretch(curve: &mut [f32; 256], lo: f32, hi: f32) {
    for v in curve.iter_mut() {
        *v = ((*v - lo) * 255. / (hi - lo)).clamp(0., 255.);
    }
}
fn auto_levels(curve: &mut Curve, hist: &[[u64; 256]; 3]) {
    for c in 0..3 {
        // Flat channels are left alone: stretching noise would invent a cast.
        if let Some((lo, hi)) = clip_points(&mapped(&curve[c], &hist[c]), LEVELS_CLIP)
            && hi > lo + 1.
        {
            stretch(&mut curve[c], lo, hi);
        }
    }
}
fn auto_contrast(curve: &mut Curve, hist: &[[u64; 256]; 3], luma_hist: &[u64; 256]) {
    // Adobe: Auto Contrast "does not improve flat-color images". Without this check a solid
    // color's channel differences would be mistaken for tonal range and oversaturated.
    let Some((ylo, yhi)) = clip_points(luma_hist, CONTRAST_CLIP) else {
        return;
    };
    if yhi <= ylo + 1. {
        return;
    }
    let (mut lo, mut hi) = (255f32, 0f32);
    for c in 0..3 {
        if let Some((l, h)) = clip_points(&mapped(&curve[c], &hist[c]), CONTRAST_CLIP) {
            (lo, hi) = (lo.min(l), hi.max(h));
        }
    }
    if hi > lo + 1. {
        for c in curve.iter_mut() {
            stretch(c, lo, hi);
        }
    }
}
/// Per-channel endpoints for a dark or light color. Full neutralization uses each channel's
/// own value. Strongly colored extremes (CAST_CHROMA) fall back toward one `shared`
/// endpoint (darkest channel for shadows, lightest for highlights), which keeps their color
/// like Auto Contrast and clips nothing extra.
fn cast_target(color: [f32; 3], shared: f32) -> [f32; 3] {
    let chroma = color[0].max(color[1]).max(color[2]) - color[0].min(color[1]).min(color[2]);
    let weight = ((CAST_CHROMA.1 - chroma) / (CAST_CHROMA.1 - CAST_CHROMA.0)).clamp(0., 1.);
    color.map(|v| shared + weight * (v - shared))
}
fn auto_color(curve: &mut Curve, pixels: &[[u8; 3]]) {
    let n = pixels.len();
    if n == 0 {
        return;
    }
    // Find Dark & Light Colors: the average darkest and lightest pixels by luminance
    // become the shadow and highlight colors, mapped to neutral black and white.
    let mut yh = [0u64; 256];
    for p in pixels {
        yh[level(luma(map(curve, p)))] += 1;
    }
    let k = ((n as f64 * COLOR_SAMPLE) as u64).max(1);
    let mut s = 0;
    let dark = yh.iter().position(|v| {
        s += v;
        s >= k
    });
    s = 0;
    let light = yh.iter().rev().position(|v| {
        s += v;
        s >= k
    });
    let (Some(dark), Some(light)) = (dark, light.map(|i| 255 - i)) else {
        return;
    };
    if light > dark + 1 {
        let (mut sd, mut nd, mut sl, mut nl) = ([0f64; 3], 0u32, [0f64; 3], 0u32);
        for p in pixels {
            let v = map(curve, p);
            let y = level(luma(v));
            if y <= dark {
                nd += 1;
                (0..3).for_each(|c| sd[c] += f64::from(v[c]));
            }
            if y >= light {
                nl += 1;
                (0..3).for_each(|c| sl[c] += f64::from(v[c]));
            }
        }
        let shadow = sd.map(|v| (v / f64::from(nd)) as f32);
        let highlight = sl.map(|v| (v / f64::from(nl)) as f32);
        let shadow = cast_target(shadow, shadow[0].min(shadow[1]).min(shadow[2]));
        let highlight = cast_target(highlight, highlight[0].max(highlight[1]).max(highlight[2]));
        for c in 0..3 {
            if highlight[c] > shadow[c] + 1. {
                stretch(&mut curve[c], shadow[c], highlight[c]);
            }
        }
    }
    // Snap Neutral Midtones: per-channel gamma makes the average near-neutral midtone gray,
    // keeping its brightness. Skipped when the photo has too few near-neutral pixels.
    let (mut sum, mut count) = ([0f64; 3], 0u32);
    for p in pixels {
        let v = map(curve, p);
        let y = luma(v);
        let chroma = v[0].max(v[1]).max(v[2]) - v[0].min(v[1]).min(v[2]);
        if (51. ..=204.).contains(&y) && chroma <= NEUTRAL_CHROMA {
            count += 1;
            (0..3).for_each(|c| sum[c] += f64::from(v[c]));
        }
    }
    if f64::from(count) < n as f64 * NEUTRAL_MIN_SHARE {
        return;
    }
    let mean = sum.map(|v| (v / f64::from(count)) as f32);
    let gray = mean.iter().sum::<f32>() / 3.;
    for c in 0..3 {
        let gamma =
            ((gray / 255.).ln() / (mean[c] / 255.).ln()).clamp(MIDTONE_GAMMA.0, MIDTONE_GAMMA.1);
        if gamma.is_finite() {
            for v in curve[c].iter_mut() {
                *v = 255. * (*v / 255.).powf(gamma);
            }
        }
    }
}
pub fn analyze(image: &RgbaImage, q: &QuickFix) -> Analysis {
    let mut curve: Curve = [std::array::from_fn(|i| i as f32); 3];
    if !any_auto(q) {
        return Analysis { curve };
    }
    let pixels: Vec<[u8; 3]> = image
        .pixels()
        .filter(|p| p[3] >= 128)
        .map(|p| [p[0], p[1], p[2]])
        .collect();
    let mut hist = [[0u64; 256]; 3];
    for p in &pixels {
        for c in 0..3 {
            hist[c][p[c] as usize] += 1;
        }
    }
    if q.levels {
        auto_levels(&mut curve, &hist);
    }
    if q.contrast {
        let mut luma_hist = [0u64; 256];
        for p in &pixels {
            luma_hist[level(luma(map(&curve, p)))] += 1;
        }
        auto_contrast(&mut curve, &hist, &luma_hist);
    }
    if q.color {
        auto_color(&mut curve, &pixels);
    }
    Analysis { curve }
}
pub fn prepare(path: &Path, fingerprint: &str, cancel: &AtomicBool) -> Result<Prepared, String> {
    let info = probe(path)?;
    let native = ResolvedSize {
        width: info.width,
        height: info.height,
        ppi: info.ppi,
        crop: [0, 0, info.width, info.height],
        photo: [0, 0, info.width, info.height],
    };
    if estimate(&info, &native, false)? > 768 * 1024 * 1024 {
        return Err("This image exceeds preview memory capacity; use a smaller source.".into());
    }
    let img = decode(path, &info, cancel)?;
    let thumb = fit_thumbnail(&img, 960, 960);
    Ok(Prepared {
        before_png: png_bytes(&thumb)?,
        thumbnail: thumb,
        fingerprint: fingerprint.into(),
        info,
    })
}
/// One aspect-preserving sampling path for analysis, preview and filmstrip.
pub fn fit_thumbnail(img: &RgbaImage, width: u32, height: u32) -> RgbaImage {
    let ratio = (width as f64 / img.width() as f64)
        .min(height as f64 / img.height() as f64)
        .min(1.);
    image::imageops::thumbnail(
        img,
        (img.width() as f64 * ratio).round().max(1.) as u32,
        (img.height() as f64 * ratio).round().max(1.) as u32,
    )
}
pub fn apply(
    img: &mut RgbaImage,
    a: &Analysis,
    r: &Recipe,
    frame: &crate::preset::Frame,
    cancel: &AtomicBool,
) -> Result<(), String> {
    let m = &r.manual;
    let auto = &a.curve;
    // Order: Quick Fix, preset, manual color/effect adjustments (same engine as presets),
    // then the basic manual sliders. Per-channel steps share one lookup table; per-pixel
    // steps follow in the same order.
    let looks: Vec<_> = r
        .preset
        .iter()
        .chain((!r.adjust.is_neutral()).then_some(&r.adjust))
        .map(crate::preset::look_for)
        .collect();
    let mut lut = [[0f32; 256]; 3];
    for (c, table) in lut.iter_mut().enumerate() {
        for (i, out) in table.iter_mut().enumerate() {
            let mut x = auto[c][i];
            for look in &looks {
                x = look.channel(c, x);
            }
            x = (x - m.black_point as f32) * 255. / (m.white_point - m.black_point) as f32;
            x = (x / 255.).clamp(0., 1.).powf(1. / m.gamma as f32) * 255.;
            x *= 2f32.powf(m.exposure as f32);
            x = (x - 127.5) * (1. + m.contrast as f32 / 100.).max(0.) + 127.5;
            *out = x;
        }
    }
    let preset_pixels = looks.iter().any(|l| l.has_pixel_steps());
    if m.shadows == 0. && m.highlights == 0. && m.saturation == 0. && !preset_pixels {
        let mut bytes = [[0u8; 256]; 3];
        for c in 0..3 {
            let warmth = if c == 0 {
                m.warmth as f32 * 0.35
            } else if c == 2 {
                -m.warmth as f32 * 0.35
            } else {
                0.
            };
            for i in 0..256 {
                bytes[c][i] = (lut[c][i] + warmth).round().clamp(0., 255.) as u8;
            }
        }
        for (i, p) in img.pixels_mut().enumerate() {
            if i % 65536 == 0 {
                cancelled(cancel)?;
            }
            for c in 0..3 {
                p[c] = bytes[c][p[c] as usize];
            }
        }
    } else {
        // Every per-pixel step depends only on the pixel's 8-bit color, and photos repeat
        // colors heavily, so results are memoized (same output, a fraction of the work).
        const MEMO: usize = 1 << 16;
        let mut memo = vec![(u32::MAX, [0u8; 3]); MEMO];
        for (i, p) in img.pixels_mut().enumerate() {
            if i % 65536 == 0 {
                cancelled(cancel)?;
            }
            let key = u32::from(p[0]) << 16 | u32::from(p[1]) << 8 | u32::from(p[2]);
            let slot = (key.wrapping_mul(0x9e37_79b1) >> 16) as usize;
            if memo[slot].0 == key {
                [p[0], p[1], p[2]] = memo[slot].1;
                continue;
            }
            let mut rgb = [
                lut[0][p[0] as usize],
                lut[1][p[1] as usize],
                lut[2][p[2] as usize],
            ];
            for look in &looks {
                look.pixel_fast(&mut rgb);
            }
            let y = (rgb[0] * 0.2126 + rgb[1] * 0.7152 + rgb[2] * 0.0722).clamp(0., 255.);
            let yn = y / 255.;
            let tone =
                m.shadows as f32 * 0.8 * (1. - yn).powi(2) + m.highlights as f32 * 0.8 * yn.powi(2);
            for value in &mut rgb {
                *value = y + (*value - y) * (1. + m.saturation as f32 / 100.) + tone;
            }
            rgb[0] += m.warmth as f32 * 0.35;
            rgb[2] -= m.warmth as f32 * 0.35;
            let out = rgb.map(to_u8);
            memo[slot] = (key, out);
            [p[0], p[1], p[2]] = out;
        }
    }
    // Glow (preset and manual amounts add up) softens before any sharpening.
    let glow = looks.iter().map(|l| l.glow).sum::<f32>().min(1.);
    if glow > 0. {
        crate::preset::glow_image(img, glow, frame, cancel)?;
    }
    let strength = m.sharpening as f32 / 100.
        + if r.quick.sharpen { 0.35 } else { 0. }
        + looks.iter().map(|l| l.sharpen).sum::<f32>();
    if strength > 0. {
        let mut weighted = img.clone();
        let alpha = weighted.pixels().any(|p| p[3] != 255);
        if alpha {
            for p in weighted.pixels_mut() {
                for c in 0..3 {
                    p[c] = ((u16::from(p[c]) * u16::from(p[3]) + 127) / 255) as u8;
                }
            }
        }
        let mut blurred = image::imageops::blur(&weighted, 1.);
        drop(weighted);
        if alpha {
            for p in blurred.pixels_mut() {
                if p[3] > 0 {
                    for c in 0..3 {
                        p[c] = ((u32::from(p[c]) * 255 + u32::from(p[3]) / 2) / u32::from(p[3]))
                            .min(255) as u8;
                    }
                }
            }
        }
        for (i, (p, b)) in img.pixels_mut().zip(blurred.pixels()).enumerate() {
            if i % 65536 == 0 {
                cancelled(cancel)?;
            }
            for c in 0..3 {
                p[c] = to_u8(p[c] as f32 + (p[c] as f32 - b[c] as f32) * strength);
            }
        }
    }
    // Preset vignette and grain come last, after sharpening, as in Lightroom.
    for look in looks.iter().filter(|l| l.has_effects()) {
        look.effects_image(img, frame, cancel)?;
    }
    Ok(())
}
/// Exactly `v.round().clamp(0., 255.) as u8`, without the C runtime `roundf` call that
/// baseline x86-64 builds make for every `round()` (the slowest part of per-pixel work).
pub fn to_u8(v: f32) -> u8 {
    if v.is_nan() || v <= 0. {
        return 0;
    }
    if v >= 255. {
        return 255;
    }
    let whole = v as u32;
    (whole + u32::from(v - whole as f32 >= 0.5)) as u8
}
/// Crops the source for "fill" and scales it to the photo's size on the output.
fn fit_photo(img: RgbaImage, size: &ResolvedSize) -> RgbaImage {
    let [x, y, w, h] = size.crop;
    let img = if img.dimensions() == (w, h) {
        img
    } else {
        image::imageops::crop_imm(&img, x, y, w, h).to_image()
    };
    resize(img, size.photo[2], size.photo[3])
}
/// Centers a "border" photo on its white canvas.
fn place(photo: RgbaImage, size: &ResolvedSize) -> RgbaImage {
    if photo.dimensions() == (size.width, size.height) {
        return photo;
    }
    let mut canvas = RgbaImage::from_pixel(size.width, size.height, image::Rgba([255; 4]));
    image::imageops::replace(
        &mut canvas,
        &photo,
        size.photo[0].into(),
        size.photo[1].into(),
    );
    canvas
}
/// The same geometry for a downsampled copy of the source, scaled by `ratio` on output.
fn scaled(size: &ResolvedSize, source: (u32, u32), sample: (u32, u32), ratio: f64) -> ResolvedSize {
    let (fx, fy) = (
        sample.0 as f64 / source.0 as f64,
        sample.1 as f64 / source.1 as f64,
    );
    let [x, y, w, h] = size.crop;
    let cx = ((x as f64 * fx).round() as u32).min(sample.0 - 1);
    let cy = ((y as f64 * fy).round() as u32).min(sample.1 - 1);
    let cw = ((w as f64 * fx).round() as u32).clamp(1, sample.0 - cx);
    let ch = ((h as f64 * fy).round() as u32).clamp(1, sample.1 - cy);
    let s = |v: u32| ((v as f64 * ratio).round() as u32).max(1);
    let (width, height) = (s(size.width), s(size.height));
    let (pw, ph) = (s(size.photo[2]).min(width), s(size.photo[3]).min(height));
    ResolvedSize {
        width,
        height,
        ppi: size.ppi,
        crop: [cx, cy, cw, ch],
        photo: [(width - pw) / 2, (height - ph) / 2, pw, ph],
    }
}
fn resize(img: RgbaImage, w: u32, h: u32) -> RgbaImage {
    if img.dimensions() == (w, h) {
        return img;
    }
    let mut img = img;
    let alpha = img.pixels().any(|p| p[3] != 255);
    if alpha {
        for p in img.pixels_mut() {
            for c in 0..3 {
                p[c] = ((p[c] as u16 * p[3] as u16 + 127) / 255) as u8;
            }
        }
    }
    let mut out = image::imageops::resize(&img, w, h, image::imageops::FilterType::Lanczos3);
    if alpha {
        for p in out.pixels_mut() {
            if p[3] > 0 {
                for c in 0..3 {
                    p[c] = ((p[c] as u32 * 255 + p[3] as u32 / 2) / p[3] as u32).min(255) as u8;
                }
            }
        }
    }
    out
}
pub fn density_exif(ppi: Option<[f64; 2]>) -> Vec<u8> {
    let mut b = b"II\x2a\0\x08\0\0\0".to_vec();
    let count = if ppi.is_some() { 4u16 } else { 1 };
    b.extend(count.to_le_bytes());
    let mut entry = |tag: u16, typ: u16, n: u32, value: u32| {
        b.extend(tag.to_le_bytes());
        b.extend(typ.to_le_bytes());
        b.extend(n.to_le_bytes());
        b.extend(value.to_le_bytes());
    };
    entry(274, 3, 1, 1);
    if ppi.is_some() {
        entry(282, 5, 1, 62);
        entry(283, 5, 1, 70);
        entry(296, 3, 1, 2);
    }
    b.extend(0u32.to_le_bytes());
    if let Some(p) = ppi {
        for v in p {
            b.extend(((v * 10000.).round() as u32).to_le_bytes());
            b.extend(10000u32.to_le_bytes());
        }
    }
    b
}
pub fn encode<W: Write>(
    writer: W,
    img: &RgbaImage,
    r: &Recipe,
    size: &ResolvedSize,
    metadata: &[u8],
) -> Result<(), String> {
    // JPEG emits small writes. Buffer all codecs without swallowing flush errors.
    let mut writer = BufWriter::with_capacity(256 * 1024, writer);
    encode_buffered(&mut writer, img, r, size, metadata)?;
    writer.flush().map_err(|e| format!("Destination: {e}"))
}
fn encode_buffered<W: Write>(
    writer: W,
    img: &RgbaImage,
    r: &Recipe,
    size: &ResolvedSize,
    metadata: &[u8],
) -> Result<(), String> {
    // A fixed standard sRGB profile keeps identical exports byte-identical.
    let profile: &[u8] = include_bytes!("../assets/srgb.icc");
    match r.format.as_str() {
        "jpeg" => {
            let mut rgb = image::RgbImage::new(img.width(), img.height());
            for (d, s) in rgb.pixels_mut().zip(img.pixels()) {
                for c in 0..3 {
                    d[c] =
                        ((s[c] as u32 * s[3] as u32 + 255 * (255 - s[3] as u32) + 127) / 255) as u8;
                }
            }
            let mut e = jpeg_encoder::Encoder::new(writer, r.quality);
            if let Some(p) = size.ppi {
                e.set_density(jpeg_encoder::PixelDensity {
                    density: (p[0].round() as u16, p[1].round() as u16),
                    unit: jpeg_encoder::PixelDensityUnit::Inches,
                });
            }
            // Keep 4:4:4 at every quality, matching the initial encoder's actual output.
            e.set_sampling_factor(jpeg_encoder::SamplingFactor::F_1_1);
            e.add_icc_profile(profile).map_err(|e| e.to_string())?;
            e.add_exif_metadata(metadata).map_err(|e| e.to_string())?;
            e.encode(
                rgb.as_raw(),
                rgb.width() as u16,
                rgb.height() as u16,
                jpeg_encoder::ColorType::Rgb,
            )
            .map_err(|e| e.to_string())
        }

        "png" => {
            let mut info = png::Info::with_size(img.width(), img.height());
            info.color_type = png::ColorType::Rgba;
            info.bit_depth = png::BitDepth::Eight;
            info.icc_profile = Some(std::borrow::Cow::Borrowed(profile));
            info.exif_metadata = Some(std::borrow::Cow::Borrowed(metadata));
            if let Some(p) = size.ppi {
                info.pixel_dims = Some(png::PixelDimensions {
                    xppu: (p[0] / 0.0254).round() as u32,
                    yppu: (p[1] / 0.0254).round() as u32,
                    unit: png::Unit::Meter,
                });
            }
            let e = png::Encoder::with_info(writer, info).map_err(|e| e.to_string())?;
            let mut w = e.write_header().map_err(|e| e.to_string())?;
            w.write_image_data(img.as_raw())
                .map_err(|e| e.to_string())?;
            w.finish().map_err(|e| e.to_string())
        }
        "webp" => {
            let mut e = image::codecs::webp::WebPEncoder::new_lossless(writer);
            e.set_icc_profile(profile.to_vec())
                .map_err(|e| e.to_string())?;
            e.set_exif_metadata(metadata.to_vec())
                .map_err(|e| e.to_string())?;
            e.write_image(
                img.as_raw(),
                img.width(),
                img.height(),
                image::ExtendedColorType::Rgba8,
            )
            .map_err(|e| e.to_string())
        }
        _ => Err("Unsupported format".into()),
    }
}
pub fn process<W: Write>(
    path: &Path,
    writer: W,
    r: &Recipe,
    cancel: &AtomicBool,
    reserved_bytes: u64,
    fingerprint: &str,
    cache: &AnalysisCache,
) -> Result<(ResolvedSize, Timings), String> {
    r.validate()?;
    let info = probe(path)?;
    let size = r.size.resolve(info.width, info.height, info.ppi)?;
    if estimate(&info, &size, r.sharpens())? > reserved_bytes {
        return Err("Photo exceeds the configured memory budget. Reduce output size or raise the memory limit.".into());
    }
    let start = Instant::now();
    let metadata = output_metadata(path, size.ppi, r.retain_capture)?;
    let img = decode(path, &info, cancel)?;
    let decode_ms = start.elapsed().as_millis() as u64;
    let t = Instant::now();
    let a = if any_auto(&r.quick) {
        cached_analysis(&fit_thumbnail(&img, 960, 960), fingerprint, &r.quick, cache)
    } else {
        analyze(&img, &r.quick)
    };
    let mut out = fit_photo(img, &size);
    cancelled(cancel)?;
    apply(
        &mut out,
        &a,
        r,
        &Frame::whole(size.photo[2], size.photo[3]),
        cancel,
    )?;
    let out = place(out, &size);
    let filter_ms = t.elapsed().as_millis() as u64;
    let t = Instant::now();
    cancelled(cancel)?;
    encode(writer, &out, r, &size, &metadata).map_err(|e| format!("Destination: {e}"))?;
    cancelled(cancel)?;
    Ok((
        size,
        Timings {
            decode_ms,
            filter_ms,
            encode_ms: t.elapsed().as_millis() as u64,
        },
    ))
}
pub fn preview(
    p: &Prepared,
    r: &Recipe,
    cache: &AnalysisCache,
    cancel: &AtomicBool,
) -> Result<(Vec<u8>, Vec<u8>, ResolvedSize), String> {
    r.validate()?;
    let a = cached_analysis(&p.thumbnail, &p.fingerprint, &r.quick, cache);
    let size = r.size.resolve(p.info.width, p.info.height, p.info.ppi)?;
    let ratio = (960. / size.width as f64)
        .min(960. / size.height as f64)
        .min(1.);
    let small = scaled(
        &size,
        (p.info.width, p.info.height),
        p.thumbnail.dimensions(),
        ratio,
    );
    let before = fit_photo(p.thumbnail.clone(), &small);
    let mut after = before.clone();
    let frame = Frame {
        scale: size.photo[2] as f32 / small.photo[2] as f32,
        ..Frame::whole(size.photo[2], size.photo[3])
    };
    apply(&mut after, &a, r, &frame, cancel)?;
    let (before, after) = (place(before, &small), place(after, &small));
    let before_png =
        if before.dimensions() == p.thumbnail.dimensions() && small.crop[2..] == small.photo[2..] {
            p.before_png.clone()
        } else {
            png_bytes(&before)?
        };
    Ok((before_png, png_bytes(&after)?, size))
}
fn png_bytes(im: &RgbaImage) -> Result<Vec<u8>, String> {
    let mut bytes = vec![];
    image::codecs::png::PngEncoder::new_with_quality(
        &mut bytes,
        image::codecs::png::CompressionType::Fast,
        image::codecs::png::FilterType::NoFilter,
    )
    .write_image(
        im.as_raw(),
        im.width(),
        im.height(),
        image::ExtendedColorType::Rgba8,
    )
    .map_err(|e| e.to_string())?;
    Ok(bytes)
}

pub fn detail(
    path: &Path,
    p: &Prepared,
    r: &Recipe,
    cache: &AnalysisCache,
    cancel: &AtomicBool,
) -> Result<(Vec<u8>, Vec<u8>, ResolvedSize), String> {
    let a = cached_analysis(&p.thumbnail, &p.fingerprint, &r.quick, cache);
    let size = r.size.resolve(p.info.width, p.info.height, p.info.ppi)?;
    if estimate(&p.info, &size, false)? > 768 * 1024 * 1024 {
        return Err("100% detail exceeds preview capacity. Reduce the output size.".into());
    }
    // 100% detail shows the center of the photo itself, not a border.
    let full = fit_photo(decode(path, &p.info, cancel)?, &size);
    let w = full.width().min(640);
    let h = full.height().min(640);
    let x = (full.width() - w) / 2;
    let y = (full.height() - h) / 2;
    // Include a halo so sharpening (and glow's wider blur) at the displayed crop edge
    // matches export.
    let glows = r.preset.as_ref().is_some_and(|p| p.glow > 0.) || r.adjust.glow > 0.;
    let halo = if glows {
        8 + (3. * crate::preset::glow_sigma(&Frame::whole(full.width(), full.height()))).ceil()
            as u32
    } else {
        8
    };
    let left = x.saturating_sub(halo);
    let top = y.saturating_sub(halo);
    let right = (x + w + halo).min(full.width());
    let bottom = (y + h + halo).min(full.height());
    let mut after =
        image::imageops::crop_imm(&full, left, top, right - left, bottom - top).to_image();
    let before = image::imageops::crop_imm(&full, x, y, w, h).to_image();
    let frame = Frame {
        x0: left,
        y0: top,
        ..Frame::whole(full.width(), full.height())
    };
    drop(full);
    apply(&mut after, &a, r, &frame, cancel)?;
    let after = image::imageops::crop_imm(&after, x - left, y - top, w, h).to_image();
    Ok((png_bytes(&before)?, png_bytes(&after)?, size))
}

fn output_metadata(path: &Path, ppi: Option<[f64; 2]>, retain: bool) -> Result<Vec<u8>, String> {
    let basic = density_exif(ppi);
    if !retain {
        return Ok(basic);
    }
    let mut decoder = reader(path)?.into_decoder().map_err(|e| e.to_string())?;
    let Some(raw) = decoder.exif_metadata().map_err(|e| e.to_string())? else {
        return Ok(basic);
    };
    let Ok(source) =
        exif::Reader::new().read_raw(raw.strip_prefix(b"Exif\0\0").unwrap_or(&raw).to_vec())
    else {
        // Damaged optional capture tags must not prevent exporting valid pixels.
        return Ok(basic);
    };
    let base = exif::Reader::new()
        .read_raw(basic)
        .map_err(|e| e.to_string())?;
    use exif::Tag;
    let allowed = [
        Tag::Make,
        Tag::Model,
        Tag::DateTimeOriginal,
        Tag::DateTimeDigitized,
        Tag::SubSecTimeOriginal,
        Tag::OffsetTimeOriginal,
        Tag::ExposureTime,
        Tag::FNumber,
        Tag::PhotographicSensitivity,
        Tag::FocalLength,
        Tag::LensMake,
        Tag::LensModel,
        Tag::ExposureBiasValue,
        Tag::ExposureProgram,
        Tag::Flash,
        Tag::WhiteBalance,
        Tag::MeteringMode,
    ];
    let mut writer = exif::experimental::Writer::new();
    for field in base.fields().chain(
        source
            .fields()
            .filter(|f| f.ifd_num == exif::In::PRIMARY && allowed.contains(&f.tag)),
    ) {
        writer.push_field(field);
    }
    let mut result = std::io::Cursor::new(Vec::new());
    writer.write(&mut result, true).map_err(|e| e.to_string())?;
    Ok(result.into_inner())
}

#[cfg(test)]
mod rounding_tests {
    #[test]
    fn to_u8_matches_round_clamp() {
        let mut v = -3f32;
        while v < 260. {
            for x in [
                v,
                f32::from_bits(v.to_bits().wrapping_sub(1)),
                f32::from_bits(v.to_bits().wrapping_add(1)),
            ] {
                assert_eq!(super::to_u8(x), x.round().clamp(0., 255.) as u8, "{x}");
            }
            v += 0.125;
        }
        assert_eq!(super::to_u8(f32::NAN), 0);
        assert_eq!(super::to_u8(0.49999997), 0);
    }
}
#[cfg(test)]
mod quick_fix_tests {
    use super::*;
    fn image(f: impl Fn(u32, u32) -> [u8; 3]) -> RgbaImage {
        RgbaImage::from_fn(256, 64, |x, y| {
            let [r, g, b] = f(x, y);
            image::Rgba([r, g, b, 255])
        })
    }
    fn quick(levels: bool, contrast: bool, color: bool) -> QuickFix {
        QuickFix {
            levels,
            contrast,
            color,
            sharpen: false,
        }
    }
    fn out(a: &RgbaImage, q: &QuickFix, p: [u8; 3]) -> [f32; 3] {
        map(&analyze(a, q).curve, &p)
    }
    fn chroma(v: [f32; 3]) -> f32 {
        v[0].max(v[1]).max(v[2]) - v[0].min(v[1]).min(v[2])
    }
    #[test]
    fn flat_images_are_unchanged_by_every_quick_fix() {
        for color in [[128, 128, 128], [60, 100, 140], [230, 40, 30]] {
            let a = image(|_, _| color);
            for index in 1..8 {
                let q = quick(index & 1 != 0, index & 2 != 0, index & 4 != 0);
                let v = out(&a, &q, color);
                for c in 0..3 {
                    assert!(
                        (v[c] - f32::from(color[c])).abs() < 0.5,
                        "{index}: {color:?} -> {v:?}"
                    );
                }
            }
        }
    }
    #[test]
    fn auto_levels_stretches_each_channel() {
        let a = image(|x, _| {
            [
                40 + (x * 160 / 255) as u8,
                30 + (x * 190 / 255) as u8,
                60 + (x * 120 / 255) as u8,
            ]
        });
        let q = quick(true, false, false);
        assert!(out(&a, &q, [40, 30, 60]).iter().all(|v| *v < 1.));
        assert!(out(&a, &q, [200, 220, 180]).iter().all(|v| *v > 254.));
    }
    #[test]
    fn auto_contrast_uses_one_mapping_and_keeps_color() {
        // A warm cast: Auto Contrast must keep it (Adobe: no casts introduced or removed).
        let a = image(|x, _| {
            let v = 40 + (x * 160 / 255) as u8;
            [v + 20, v, v - 20]
        });
        let q = quick(false, true, false);
        let [r, g, b] = out(&a, &q, [140, 120, 100]);
        assert!((r - g - (g - b)).abs() < 1., "{r} {g} {b}");
        assert!(r - g > 20., "cast preserved and scaled, got {r} {g} {b}");
        assert!(out(&a, &q, [60, 40, 20])[2] < 1. && out(&a, &q, [220, 200, 180])[0] > 254.);
    }
    #[test]
    fn auto_color_removes_cast_but_keeps_saturated_subject() {
        // Neutral gradient with a warm cast, plus a saturated red subject on 10% of the frame.
        let a = image(|x, y| {
            if y < 6 {
                [220, 40, 40]
            } else {
                let v = 30 + (x * 190 / 255) as u8;
                [v + 15, v, v - 15]
            }
        });
        let q = quick(false, false, true);
        for v in [60u8, 125, 190] {
            let o = out(&a, &q, [v + 15, v, v - 15]);
            assert!(chroma(o) <= 3., "gray {v} still cast: {o:?}");
        }
        let red = out(&a, &q, [220, 40, 40]);
        assert!(
            red[0] - red[1] > 120. && red[0] - red[2] > 120.,
            "subject lost color: {red:?}"
        );
        assert!(out(&a, &q, [45, 30, 15]).iter().all(|v| *v < 6.));
    }
    #[test]
    fn auto_color_leaves_photos_without_neutrals_alone_in_midtones() {
        // Sunset-like: everything orange. Must not be pulled toward gray.
        let a = image(|x, _| {
            let v = x * 200 / 255;
            [(55 + v) as u8, (25 + v * 3 / 5) as u8, (5 + v / 4) as u8]
        });
        let o = out(&a, &quick(false, false, true), [155, 85, 30]);
        assert!(o[0] - o[2] > 90., "warm scene neutralized: {o:?}");
        assert!(o[0] < 250., "red channel clipped: {o:?}");
    }
    #[test]
    fn checked_fixes_run_in_sequence() {
        let a = image(|x, _| {
            let v = 40 + (x * 160 / 255) as u8;
            [v + 20, v, v - 20]
        });
        // After per-channel Auto Levels every channel spans 0..255, so a following Auto
        // Contrast has nothing left to stretch.
        for p in [[60, 40, 20], [140, 120, 100], [220, 200, 180]] {
            let l = out(&a, &quick(true, false, false), p);
            let lc = out(&a, &quick(true, true, false), p);
            assert!((0..3).all(|c| (l[c] - lc[c]).abs() < 1.5), "{l:?} {lc:?}");
        }
    }
}
#[cfg(test)]
mod regression_tests {
    use super::*;
    #[test]
    fn fill_and_border_exports_match_their_previews() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("landscape.png");
        // A landscape photo with a dark red left edge and a dark green top band.
        RgbaImage::from_fn(1200, 800, |x, y| {
            image::Rgba(if x < 40 {
                [120, 20, 20, 255]
            } else if y < 40 {
                [20, 120, 20, 255]
            } else {
                [90, 90, 160, 255]
            })
        })
        .save(&path)
        .unwrap();
        let mut r = Recipe {
            format: "png".into(),
            ..Default::default()
        };
        r.quick = QuickFix::default();
        r.size = SizeSpec {
            resize: true,
            width: Some(500.),
            height: Some(700.),
            unit: "pixels".into(),
            ..Default::default()
        };
        let cancel = AtomicBool::new(false);
        let fingerprint = crate::batch::hash(&path).unwrap();
        let cache = AnalysisCache::default();
        let prepared = prepare(&path, &fingerprint, &cancel).unwrap();
        let export = |r: &Recipe| {
            let mut bytes = Vec::new();
            let (size, _) =
                process(&path, &mut bytes, r, &cancel, 1 << 30, &fingerprint, &cache).unwrap();
            let (_, after, _) = preview(&prepared, r, &cache, &cancel).unwrap();
            let out = image::load_from_memory(&bytes).unwrap().to_rgba8();
            let small = image::load_from_memory(&after).unwrap().to_rgba8();
            assert_eq!(out.dimensions(), (size.width, size.height));
            (out, small)
        };
        // Border: the turned 700 × 500 box, the whole photo at 700 × 467, white above and below.
        r.size.fit = "border".into();
        let (out, small) = export(&r);
        assert_eq!(out.dimensions(), (700, 500));
        assert_eq!(out.get_pixel(350, 5).0, [255; 4]);
        assert_eq!(out.get_pixel(350, 494).0, [255; 4]);
        assert!(
            out.get_pixel(350, 22).0[1] > 100,
            "green band kept just inside the border"
        );
        assert!(out.get_pixel(5, 250).0[0] > 100, "red edge kept");
        assert_eq!(small.dimensions(), (700, 500));
        assert_eq!(small.get_pixel(350, 5).0, [255; 4]);
        // Fill: exactly 700 × 500 with the left and right edges cropped.
        r.size.fit = "fill".into();
        let (out, small) = export(&r);
        assert_eq!(out.dimensions(), (700, 500));
        assert!(out.get_pixel(5, 250).0[0] < 100, "red edge cropped off");
        assert!(out.get_pixel(350, 10).0[1] > 100, "green band kept");
        assert_eq!(small.dimensions(), (700, 500));
        assert!(small.get_pixel(3, 250).0[0] < 100);
    }
    #[test]
    fn analysis_is_independent_of_preview_cache_order() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("uneven-landscape.png");
        let img = RgbaImage::from_fn(1231, 777, |x, y| {
            image::Rgba([
                ((x * 17 + y * 3) % 256) as u8,
                ((x / 9 + y * 11) % 256) as u8,
                if x < 310 { 25 } else { ((y * 7) % 256) as u8 },
                255,
            ])
        });
        img.save(&path).unwrap();
        let mut r = Recipe {
            format: "png".into(),
            ..Default::default()
        };
        r.quick.color = true;
        r.size.resize = false;
        let cancel = AtomicBool::new(false);
        let fingerprint = crate::batch::hash(&path).unwrap();
        let export_cache = AnalysisCache::default();
        let preview_cache = AnalysisCache::default();
        let prepared = prepare(&path, &fingerprint, &cancel).unwrap();
        // A preview fills the shared cache that the second export then reuses.
        preview(&prepared, &r, &preview_cache, &cancel).unwrap();
        assert_eq!(preview_cache.lock().unwrap().len(), 1);
        assert_eq!(prepared.thumbnail.dimensions(), (960, 606));
        let mut direct = Vec::new();
        let mut after_preview = Vec::new();
        process(
            &path,
            &mut direct,
            &r,
            &cancel,
            1024 * 1024 * 1024,
            &fingerprint,
            &export_cache,
        )
        .unwrap();
        process(
            &path,
            &mut after_preview,
            &r,
            &cancel,
            1024 * 1024 * 1024,
            &fingerprint,
            &preview_cache,
        )
        .unwrap();
        assert_eq!(
            direct, after_preview,
            "Preview must not change the exported file"
        );
        // A Lightroom preset changes the export, preview and export stay in step, and the
        // preset survives the recipe's journal round trip.
        r.preset = Some(Preset {
            exposure: 0.4,
            contrast: 20.,
            vibrance: 30.,
            shadows: 25.,
            curves: [vec![[0., 16.], [255., 240.]], vec![], vec![], vec![]],
            hsl: [[10., -30., 5.]; 8],
            grading: Grading {
                shadow_hue: 210.,
                shadow_sat: 20.,
                highlight_hue: 50.,
                highlight_sat: 15.,
                ..Grading::default()
            },
            calibration: [5., 20., 10., 0., -10., 0., 0.],
            vignette: [-30., 50., 50., 0.],
            grain: [30., 25., 50.],
            sharpness: 40.,
            ..Preset::default()
        });
        let back: Recipe = serde_json::from_str(&serde_json::to_string(&r).unwrap()).unwrap();
        assert_eq!(back.preset, r.preset);
        let (mut cold, mut warm) = (Vec::new(), Vec::new());
        process(
            &path,
            &mut cold,
            &r,
            &cancel,
            1 << 30,
            &fingerprint,
            &AnalysisCache::default(),
        )
        .unwrap();
        preview(&prepared, &r, &preview_cache, &cancel).unwrap();
        process(
            &path,
            &mut warm,
            &r,
            &cancel,
            1 << 30,
            &fingerprint,
            &preview_cache,
        )
        .unwrap();
        assert_eq!(cold, warm, "preview must not change a preset export");
        assert_ne!(cold, direct, "the preset changes the export");
        // Manual color/effect adjustments use the same engine; neutral ones change nothing.
        let mut manual = r.clone();
        manual.preset = None;
        manual.adjust = Preset {
            hsl: [[0., -40., 0.]; 8],
            vignette: [-40., 50., 50., 0.],
            ..Preset::default()
        };
        let mut adjusted = Vec::new();
        process(
            &path,
            &mut adjusted,
            &manual,
            &cancel,
            1 << 30,
            &fingerprint,
            &export_cache,
        )
        .unwrap();
        assert_ne!(adjusted, direct, "manual adjustments change the export");
        // The 100% detail crop shows exactly the export's pixels, vignette and grain included.
        let (_, detail_after, _) = detail(&path, &prepared, &r, &preview_cache, &cancel).unwrap();
        let crop = image::load_from_memory(&detail_after).unwrap().to_rgba8();
        let full = image::load_from_memory(&cold).unwrap().to_rgba8();
        let (x, y) = (
            (full.width() - crop.width()) / 2,
            (full.height() - crop.height()) / 2,
        );
        assert!(
            crop == image::imageops::crop_imm(&full, x, y, crop.width(), crop.height()).to_image(),
            "detail crop must match the export"
        );
        assert_eq!(
            fit_thumbnail(&RgbaImage::new(300, 400), 120, 80).dimensions(),
            (60, 80)
        );
        assert_eq!(
            fit_thumbnail(&RgbaImage::new(400, 300), 120, 80).dimensions(),
            (107, 80)
        );
    }
}
