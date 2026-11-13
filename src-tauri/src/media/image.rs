//! Image conversion: WebP, PNG, JPEG, GIF, BMP and TIFF into JPG or PNG.
//!
//! Done in-process with the `image` crate, whose decoders are memory-safe Rust, so images need neither FFmpeg
//! nor any other helper. Images are recognised by their contents rather than their names (a ".webp" saved
//! from a browser is sometimes really a JPEG), decoded within size limits, turned the right way up, and
//! written with their colour profile. JPG has no transparency, so transparent areas become white. Camera
//! details (EXIF) aren't carried into a re-encoded file. An image already in the chosen format is kept as it
//! is: re-encoding it could only lose quality.

use std::fs::File;
use std::io::{BufReader, BufWriter, Cursor, Read, Write};
use std::path::Path;

use image::codecs::jpeg::JpegEncoder;
use image::codecs::png::{CompressionType, FilterType, PngEncoder};
use image::metadata::Orientation;
use image::{ColorType, DynamicImage, ExtendedColorType, ImageDecoder, ImageEncoder, ImageError, ImageReader, Limits};

use crate::engine::output_files::is_out_of_space;
use crate::error::MediaForgeError;
use crate::models::{ImageFormat, ImageOptions};

/// Widest or tallest image MediaForge will decode.
pub const MAX_SIDE: u32 = 32_768;
/// Largest image, in pixels, MediaForge will decode: a little over a 100-megapixel camera's output.
pub const MAX_PIXELS: u64 = 120_000_000;
/// Largest image decoded just to show a thumbnail.
const THUMBNAIL_MAX_PIXELS: u64 = 60_000_000;
const MAX_DECODED_BYTES: u64 = 1 << 30;
const MAX_FILE_BYTES: u64 = 512 * 1024 * 1024;

/// The image formats MediaForge reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceKind {
    Webp,
    Png,
    Jpeg,
    Gif,
    Bmp,
    Tiff,
}

impl SourceKind {
    fn from_codec(format: image::ImageFormat) -> Option<Self> {
        Some(match format {
            image::ImageFormat::WebP => Self::Webp,
            image::ImageFormat::Png => Self::Png,
            image::ImageFormat::Jpeg => Self::Jpeg,
            image::ImageFormat::Gif => Self::Gif,
            image::ImageFormat::Bmp => Self::Bmp,
            image::ImageFormat::Tiff => Self::Tiff,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Webp => "WebP",
            Self::Png => "PNG",
            Self::Jpeg => "JPEG",
            Self::Gif => "GIF",
            Self::Bmp => "BMP",
            Self::Tiff => "TIFF",
        }
    }

    pub fn extension(self) -> &'static str {
        match self {
            Self::Webp => "webp",
            Self::Png => "png",
            Self::Jpeg => "jpg",
            Self::Gif => "gif",
            Self::Bmp => "bmp",
            Self::Tiff => "tiff",
        }
    }

    /// Whether a file of this kind already is what `format` would produce.
    fn is(self, format: ImageFormat) -> bool {
        matches!((self, format), (Self::Jpeg, ImageFormat::Jpg) | (Self::Png, ImageFormat::Png))
    }
}

/// What the first bytes of a file or download turn out to be.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Sniffed {
    Image(SourceKind),
    /// An image MediaForge can't read, by name ("AVIF", "HEIC" and so on).
    Unsupported(String),
    NotImage,
}

/// Recognises an image from its first few bytes (16 are enough).
pub fn sniff(data: &[u8]) -> Sniffed {
    // HEIF and AVIF are ISO media files, like MP4, told apart by their brand.
    if data.len() >= 12 && &data[4..8] == b"ftyp" {
        return match &data[8..12] {
            b"avif" | b"avis" => Sniffed::Unsupported("AVIF".into()),
            b"heic" | b"heix" | b"heim" | b"heis" | b"hevc" | b"hevx" => Sniffed::Unsupported("HEIC".into()),
            b"mif1" | b"msf1" => Sniffed::Unsupported("HEIF".into()),
            _ => Sniffed::NotImage,
        };
    }
    match image::guess_format(data) {
        Ok(format) => match SourceKind::from_codec(format) {
            Some(kind) => Sniffed::Image(kind),
            None => Sniffed::Unsupported(format.extensions_str().first().map(|e| e.to_uppercase()).unwrap_or_default()),
        },
        Err(_) => Sniffed::NotImage,
    }
}

/// An image's size, from as much of the start of the file as has arrived, when that's enough to tell.
pub fn dimensions(data: &[u8]) -> Option<(u32, u32)> {
    ImageReader::new(Cursor::new(data)).with_guessed_format().ok()?.into_dimensions().ok()
}

fn file_name(path: &Path) -> String {
    path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

fn unreadable() -> MediaForgeError {
    MediaForgeError::UnreadableFile("The image is damaged or incomplete, so it couldn't be read.".into())
}

fn too_large(width: u32, height: u32) -> MediaForgeError {
    MediaForgeError::ConversionFailed(format!(
        "This image is too large to convert ({width} × {height} pixels). MediaForge converts images of up to {} megapixels.",
        MAX_PIXELS / 1_000_000
    ))
}

fn read_error(error: ImageError) -> MediaForgeError {
    match error {
        ImageError::Limits(_) => MediaForgeError::ConversionFailed(format!(
            "This image is too large to convert. MediaForge converts images of up to {} megapixels.",
            MAX_PIXELS / 1_000_000
        )),
        _ => unreadable(),
    }
}

fn write_error(error: std::io::Error) -> MediaForgeError {
    if is_out_of_space(&error) { MediaForgeError::InsufficientDiskSpace } else { MediaForgeError::Unknown(error.to_string()) }
}

fn encode_error(error: ImageError, format: ImageFormat) -> MediaForgeError {
    match error {
        ImageError::IoError(error) => write_error(error),
        _ => MediaForgeError::ConversionFailed(format!("The image couldn't be saved as {}.", format.display_name())),
    }
}

/// Identifies a file from its contents.
pub fn identify(path: &Path) -> Result<SourceKind, MediaForgeError> {
    let mut head = [0u8; 32];
    let mut file = File::open(path).map_err(|_| MediaForgeError::SourceFileMissing(file_name(path)))?;
    let mut filled = 0;
    while filled < head.len() {
        match file.read(&mut head[filled..]) {
            Ok(0) => break,
            Ok(count) => filled += count,
            Err(_) => return Err(unreadable()),
        }
    }
    match sniff(&head[..filled]) {
        Sniffed::Image(kind) => Ok(kind),
        Sniffed::Unsupported(name) => Err(MediaForgeError::UnsupportedImage(name)),
        Sniffed::NotImage => Err(unreadable()),
    }
}

fn limits() -> Limits {
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_SIDE);
    limits.max_image_height = Some(MAX_SIDE);
    limits.max_alloc = Some(MAX_DECODED_BYTES);
    limits
}

fn reader(path: &Path) -> Result<ImageReader<BufReader<File>>, MediaForgeError> {
    let file = File::open(path).map_err(|_| MediaForgeError::SourceFileMissing(file_name(path)))?;
    let mut reader = ImageReader::new(BufReader::new(file)).with_guessed_format().map_err(|_| unreadable())?;
    reader.limits(limits());
    Ok(reader)
}

struct Decoded {
    image: DynamicImage,
    icc_profile: Option<Vec<u8>>,
}

/// Decodes an image (the first frame of an animation), the right way up.
fn decode(path: &Path, max_pixels: u64) -> Result<Decoded, MediaForgeError> {
    let reader = reader(path)?;
    if reader.format().and_then(SourceKind::from_codec).is_none() {
        return Err(unreadable());
    }
    let mut decoder = reader.into_decoder().map_err(read_error)?;
    let (width, height) = decoder.dimensions();
    if width as u64 * height as u64 > max_pixels || decoder.total_bytes() > MAX_DECODED_BYTES {
        return Err(too_large(width, height));
    }
    let orientation = decoder.orientation().unwrap_or(Orientation::NoTransforms);
    let icc_profile = decoder.icc_profile().ok().flatten();
    let mut image = DynamicImage::from_decoder(decoder).map_err(read_error)?;
    image.apply_orientation(orientation);
    Ok(Decoded { image, icc_profile })
}

/// Converts `input` into `output` (a new file) in the chosen format.
pub fn convert(input: &Path, output: &Path, options: ImageOptions) -> Result<(), MediaForgeError> {
    let metadata = std::fs::metadata(input).map_err(|_| MediaForgeError::SourceFileMissing(file_name(input)))?;
    if !metadata.is_file() {
        return Err(MediaForgeError::SourceFileMissing(file_name(input)));
    }
    if metadata.len() > MAX_FILE_BYTES {
        return Err(MediaForgeError::ConversionFailed("This image file is too large to convert.".into()));
    }
    let kind = identify(input)?;
    if kind.is(options.format) {
        // Already a JPG (or PNG): keep it exactly as it is, once it's known to be a readable image.
        let (width, height) = reader(input)?.into_dimensions().map_err(read_error)?;
        if width as u64 * height as u64 > MAX_PIXELS {
            return Err(too_large(width, height));
        }
        std::fs::copy(input, output).map_err(write_error)?;
        return Ok(());
    }

    let decoded = decode(input, MAX_PIXELS)?;
    let file = File::create(output).map_err(write_error)?;
    let mut writer = BufWriter::new(file);
    let written = match options.format {
        ImageFormat::Jpg => write_jpeg(&mut writer, decoded.image, decoded.icc_profile, options.quality.value() as u8),
        ImageFormat::Png => write_png(&mut writer, decoded.image, decoded.icc_profile),
    };
    written.map_err(|e| encode_error(e, options.format))?;
    writer.flush().map_err(write_error)?;
    Ok(())
}

/// Whether an ICC profile describes the colour space the pixels are written in. A CMYK JPEG is decoded to
/// RGB, for instance, and its CMYK profile would then turn every colour wrong.
fn profile_fits(profile: &[u8], grey: bool) -> bool {
    match profile.get(16..20) {
        Some(space) if grey => space == b"GRAY",
        Some(space) => space == b"RGB ",
        None => false,
    }
}

/// Straight (unpremultiplied) alpha over white.
fn over_white(value: u8, alpha: u8) -> u8 {
    let (value, alpha) = (value as u32, alpha as u32);
    ((value * alpha + 255 * (255 - alpha) + 127) / 255) as u8
}

fn write_jpeg(writer: impl Write, image: DynamicImage, icc_profile: Option<Vec<u8>>, quality: u8) -> image::ImageResult<()> {
    let (width, height) = (image.width(), image.height());
    let grey = !image.color().has_color();
    let mut encoder = JpegEncoder::new_with_quality(writer, quality);
    if let Some(profile) = icc_profile.filter(|p| profile_fits(p, grey)) {
        let _ = encoder.set_icc_profile(profile);
    }
    match (grey, image.color().has_alpha()) {
        (false, false) => encoder.write_image(image.into_rgb8().as_raw(), width, height, ExtendedColorType::Rgb8),
        (true, false) => encoder.write_image(image.into_luma8().as_raw(), width, height, ExtendedColorType::L8),
        (false, true) => {
            let pixels: Vec<u8> = image
                .into_rgba8()
                .pixels()
                .flat_map(|p| [over_white(p[0], p[3]), over_white(p[1], p[3]), over_white(p[2], p[3])])
                .collect();
            encoder.write_image(&pixels, width, height, ExtendedColorType::Rgb8)
        }
        (true, true) => {
            let pixels: Vec<u8> = image.into_luma_alpha8().pixels().map(|p| over_white(p[0], p[1])).collect();
            encoder.write_image(&pixels, width, height, ExtendedColorType::L8)
        }
    }
}

fn write_png(writer: impl Write, image: DynamicImage, icc_profile: Option<Vec<u8>>) -> image::ImageResult<()> {
    // PNG holds up to 16 bits per channel; an alpha channel that's entirely opaque only makes the file bigger.
    let image = match image.color() {
        ColorType::Rgb32F => DynamicImage::ImageRgb16(image.into_rgb16()),
        ColorType::Rgba32F => DynamicImage::ImageRgba16(image.into_rgba16()),
        ColorType::Rgba8 if image.as_rgba8().is_some_and(|i| i.pixels().all(|p| p[3] == u8::MAX)) => {
            DynamicImage::ImageRgb8(image.into_rgb8())
        }
        ColorType::La8 if image.as_luma_alpha8().is_some_and(|i| i.pixels().all(|p| p[1] == u8::MAX)) => {
            DynamicImage::ImageLuma8(image.into_luma8())
        }
        _ => image,
    };
    let mut encoder = PngEncoder::new_with_quality(writer, CompressionType::Default, FilterType::Adaptive);
    if let Some(profile) = icc_profile.filter(|p| profile_fits(p, !image.color().has_color())) {
        let _ = encoder.set_icc_profile(profile);
    }
    image.write_with_encoder(encoder)
}

/// A small preview of an image file for the UI: JPEG, or PNG when it has transparency.
pub fn thumbnail(path: &Path, size: u32) -> Option<Vec<u8>> {
    let small = decode(path, THUMBNAIL_MAX_PIXELS).ok()?.image.thumbnail(size, size);
    let mut out = Vec::new();
    if small.color().has_alpha() {
        DynamicImage::ImageRgba8(small.into_rgba8()).write_with_encoder(PngEncoder::new(&mut out)).ok()?;
    } else {
        let rgb = small.into_rgb8();
        JpegEncoder::new_with_quality(&mut out, 82).write_image(rgb.as_raw(), rgb.width(), rgb.height(), ExtendedColorType::Rgb8).ok()?;
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::ImageQuality;
    use image::codecs::webp::WebPEncoder;
    use image::{Rgb, RgbImage, Rgba, RgbaImage};

    fn fixture(name: &str) -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/images").join(name)
    }

    fn jpg() -> ImageOptions {
        ImageOptions { format: ImageFormat::Jpg, quality: ImageQuality::High }
    }

    fn png() -> ImageOptions {
        ImageOptions { format: ImageFormat::Png, quality: ImageQuality::High }
    }

    fn open(path: &Path) -> DynamicImage {
        ImageReader::open(path).unwrap().with_guessed_format().unwrap().decode().unwrap()
    }

    fn close(a: u8, b: u8, tolerance: u8) -> bool {
        a.abs_diff(b) <= tolerance
    }

    /// Half transparent, half opaque red: a lossless WebP made here.
    fn write_transparent_webp(path: &Path) {
        let picture = RgbaImage::from_fn(40, 20, |x, _| if x < 20 { Rgba([0, 0, 0, 0]) } else { Rgba([220, 20, 30, 255]) });
        let file = File::create(path).unwrap();
        WebPEncoder::new_lossless(file).write_image(picture.as_raw(), 40, 20, ExtendedColorType::Rgba8).unwrap();
    }

    #[test]
    fn recognises_images_by_their_contents() {
        assert_eq!(sniff(b"RIFF\0\0\0\0WEBPVP8 "), Sniffed::Image(SourceKind::Webp));
        assert_eq!(sniff(b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR"), Sniffed::Image(SourceKind::Png));
        assert_eq!(sniff(&[0xFF, 0xD8, 0xFF, 0xE0, 0, 0x10, b'J', b'F']), Sniffed::Image(SourceKind::Jpeg));
        assert_eq!(sniff(b"GIF89a\x01\0\x01\0"), Sniffed::Image(SourceKind::Gif));
        assert_eq!(sniff(b"\0\0\0\x1cftypavif\0\0\0\0"), Sniffed::Unsupported("AVIF".into()));
        assert_eq!(sniff(b"\0\0\0\x18ftypheic\0\0\0\0"), Sniffed::Unsupported("HEIC".into()));
        assert_eq!(sniff(b"\0\0\0\x18ftypisom\0\0\0\0"), Sniffed::NotImage);
        assert_eq!(sniff(b"<!DOCTYPE html><html>"), Sniffed::NotImage);
        assert_eq!(sniff(b""), Sniffed::NotImage);
    }

    #[test]
    fn converts_lossy_webp_to_jpg_and_png() {
        let dir = tempfile::tempdir().unwrap();
        let source = fixture("photo-lossy.webp");
        assert_eq!(identify(&source).unwrap(), SourceKind::Webp);
        for (options, kind) in [(jpg(), SourceKind::Jpeg), (png(), SourceKind::Png)] {
            let output = dir.path().join(format!("out.{}", options.format.extension()));
            convert(&source, &output, options).unwrap();
            assert_eq!(identify(&output).unwrap(), kind);
            let picture = open(&output).into_rgb8();
            assert_eq!(picture.dimensions(), (64, 48));
            // Left half red-ish, right half not: the picture survived the trip.
            assert!(picture.get_pixel(8, 8)[0] > 200 && picture.get_pixel(56, 8)[0] < 60, "{:?}", picture.get_pixel(8, 8));
        }
    }

    #[test]
    fn transparency_becomes_white_in_jpg_and_stays_in_png() {
        let dir = tempfile::tempdir().unwrap();
        for source in [dir.path().join("made.webp"), fixture("logo-alpha.webp")] {
            if !source.exists() {
                write_transparent_webp(&source);
            }
            let jpeg = dir.path().join("logo.jpg");
            let png_path = dir.path().join("logo.png");
            convert(&source, &jpeg, jpg()).unwrap();
            convert(&source, &png_path, png()).unwrap();

            let flat = open(&jpeg).into_rgb8();
            let Rgb([r, g, b]) = *flat.get_pixel(2, 2);
            assert!(close(r, 255, 3) && close(g, 255, 3) && close(b, 255, 3), "transparent area should be white, got {r} {g} {b}");
            let Rgb([r, g, b]) = *flat.get_pixel(flat.width() - 3, 2);
            assert!(r > 180 && g < 70 && b < 80, "opaque area should stay red, got {r} {g} {b}");

            let kept = open(&png_path);
            assert!(kept.color().has_alpha());
            assert_eq!(kept.into_rgba8().get_pixel(2, 2)[3], 0);
            std::fs::remove_file(&jpeg).unwrap();
            std::fs::remove_file(&png_path).unwrap();
        }
    }

    #[test]
    fn an_animation_becomes_its_first_frame() {
        let dir = tempfile::tempdir().unwrap();
        let output = dir.path().join("first.png");
        convert(&fixture("animated.webp"), &output, png()).unwrap();
        let picture = open(&output).into_rgb8();
        let Rgb([r, g, b]) = *picture.get_pixel(4, 4);
        assert!(r > 200 && g < 40 && b < 40, "the first frame is red, got {r} {g} {b}");
    }

    #[test]
    fn an_image_already_in_the_chosen_format_is_kept_byte_for_byte() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("photo.jpg");
        let picture = RgbImage::from_pixel(16, 16, Rgb([10, 200, 90]));
        JpegEncoder::new_with_quality(File::create(&source).unwrap(), 60)
            .write_image(picture.as_raw(), 16, 16, ExtendedColorType::Rgb8)
            .unwrap();
        let output = dir.path().join("copy.jpg");
        convert(&source, &output, ImageOptions { format: ImageFormat::Jpg, quality: ImageQuality::Maximum }).unwrap();
        assert_eq!(std::fs::read(&source).unwrap(), std::fs::read(&output).unwrap());
    }

    #[test]
    fn a_misnamed_file_is_read_for_what_it_is() {
        // Browsers often save JPEGs with a .webp name, and the other way round.
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("really a jpeg.webp");
        let picture = RgbImage::from_pixel(12, 8, Rgb([0, 0, 255]));
        JpegEncoder::new_with_quality(File::create(&source).unwrap(), 90)
            .write_image(picture.as_raw(), 12, 8, ExtendedColorType::Rgb8)
            .unwrap();
        let output = dir.path().join("out.png");
        convert(&source, &output, png()).unwrap();
        assert_eq!(open(&output).into_rgb8().dimensions(), (12, 8));
    }

    /// A JPEG whose EXIF says "rotate 90° clockwise to display", as phones write.
    fn write_rotated_jpeg(path: &Path) {
        let picture = RgbImage::from_fn(30, 10, |x, _| if x < 3 { Rgb([255, 0, 0]) } else { Rgb([255, 255, 255]) });
        let mut plain = Vec::new();
        JpegEncoder::new_with_quality(&mut plain, 95).write_image(picture.as_raw(), 30, 10, ExtendedColorType::Rgb8).unwrap();
        // TIFF header (little-endian), one IFD entry: Orientation (0x0112), SHORT, 1 value, 6.
        let mut exif = b"Exif\0\0II*\0\x08\0\0\0\x01\0".to_vec();
        exif.extend_from_slice(&[0x12, 0x01, 3, 0, 1, 0, 0, 0, 6, 0, 0, 0, 0, 0, 0, 0]);
        let mut segment = vec![0xFF, 0xE1];
        segment.extend_from_slice(&((exif.len() + 2) as u16).to_be_bytes());
        segment.extend_from_slice(&exif);
        let mut data = plain[..2].to_vec();
        data.extend_from_slice(&segment);
        data.extend_from_slice(&plain[2..]);
        std::fs::write(path, data).unwrap();
    }

    #[test]
    fn photos_are_turned_the_right_way_up() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("phone.jpg");
        write_rotated_jpeg(&source);
        let output = dir.path().join("upright.png");
        convert(&source, &output, png()).unwrap();
        let picture = open(&output).into_rgb8();
        assert_eq!(picture.dimensions(), (10, 30));
        // The red stripe on the left now runs along the top.
        assert!(picture.get_pixel(5, 1)[1] < 80 && picture.get_pixel(5, 28)[1] > 200);
    }

    #[test]
    fn damaged_unsupported_and_missing_files_get_clear_errors() {
        let dir = tempfile::tempdir().unwrap();
        let output = dir.path().join("out.jpg");
        let truncated = dir.path().join("cut.webp");
        let whole = std::fs::read(fixture("photo-lossy.webp")).unwrap();
        std::fs::write(&truncated, &whole[..whole.len() / 2]).unwrap();
        assert!(matches!(convert(&truncated, &output, jpg()), Err(MediaForgeError::UnreadableFile(_))));

        let avif = dir.path().join("photo.webp");
        std::fs::write(&avif, b"\0\0\0\x1cftypavif\0\0\0\0mif1avifmiaf").unwrap();
        assert_eq!(convert(&avif, &output, jpg()), Err(MediaForgeError::UnsupportedImage("AVIF".into())));

        let text = dir.path().join("notes.png");
        std::fs::write(&text, "not a picture").unwrap();
        assert!(matches!(convert(&text, &output, jpg()), Err(MediaForgeError::UnreadableFile(_))));

        assert_eq!(convert(&dir.path().join("gone.webp"), &output, jpg()), Err(MediaForgeError::SourceFileMissing("gone.webp".into())));
        assert!(!output.exists() || std::fs::metadata(&output).unwrap().len() == 0);
    }

    #[test]
    fn makes_thumbnails_and_reads_sizes_from_partial_data() {
        let thumbnail = thumbnail(&fixture("photo-lossy.webp"), 32).unwrap();
        assert_eq!(crate::artwork::image_mime_type(&thumbnail), Some("image/jpeg"));
        let with_alpha = thumbnail_of_transparent();
        assert_eq!(crate::artwork::image_mime_type(&with_alpha), Some("image/png"));

        let whole = std::fs::read(fixture("photo-lossy.webp")).unwrap();
        assert_eq!(dimensions(&whole), Some((64, 48)));
        assert_eq!(dimensions(&whole[..40]), Some((64, 48)));
        assert_eq!(dimensions(b"<html>"), None);
    }

    fn thumbnail_of_transparent() -> Vec<u8> {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("t.webp");
        write_transparent_webp(&source);
        thumbnail(&source, 16).unwrap()
    }

    #[test]
    fn icc_profiles_are_kept_only_when_they_match_the_pixels() {
        let mut profile = vec![0u8; 128];
        profile[16..20].copy_from_slice(b"RGB ");
        assert!(profile_fits(&profile, false) && !profile_fits(&profile, true));
        profile[16..20].copy_from_slice(b"CMYK");
        assert!(!profile_fits(&profile, false));
        assert!(!profile_fits(&[0; 4], false));
        assert_eq!((over_white(0, 0), over_white(0, 255), over_white(200, 128)), (255, 0, 227));
    }
}
