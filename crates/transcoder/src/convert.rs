//! Decoding, resizing and encoding an image with the `image` crate.
//!
//! In process, where video goes through `ffmpeg`: an image is small enough that starting two
//! processes for it costs more than the work. The bytes come from another machine, so only
//! the formats a poster or an avatar comes in are accepted, and a declared canvas is checked
//! before anything is allocated for it.
//!
//! All of this is CPU work; the caller runs it off the async workers.

use std::io::Cursor;

use image::codecs::jpeg::JpegEncoder;
use image::codecs::png::PngEncoder;
use image::codecs::pnm::{PnmEncoder, PnmSubtype, SampleEncoding};
use image::imageops::FilterType;
use image::{DynamicImage, GenericImageView, ImageDecoder, ImageFormat, ImageReader};

use crate::{ConvertedImage, Fit, ImageRequest, OutputFormat, TranscodeError};

/// A small file can declare a huge canvas.
const MAX_SOURCE_PIXELS: u64 = 64_000_000;

/// The reference fails beyond this too (13344×5000), at a threshold not worth locating:
/// nothing displays such an image.
const MAX_OUTPUT_SIDE: u32 = 8192;

/// Where the reference's default falls on its own scale: between its 50 and its 90.
const DEFAULT_JPEG_QUALITY: u8 = 75;

pub(crate) struct SourceImage {
    pub(crate) pixels: DynamicImage,
    is_png: bool,
}

pub(crate) fn convert(
    bytes: &[u8],
    request: &ImageRequest,
) -> Result<ConvertedImage, TranscodeError> {
    let source = decode(bytes)?;
    let size =
        target_size(request, source.pixels.dimensions()).ok_or(TranscodeError::UnusableSize)?;
    let format = output_format(
        request.format,
        source.is_png,
        source.pixels.color().has_alpha(),
    );
    Ok(ConvertedImage {
        bytes: encode(&source.pixels, size, format, request.quality)?,
        format,
    })
}

/// Decodes `bytes`, upright: EXIF orientation is applied.
pub(crate) fn decode(bytes: &[u8]) -> Result<SourceImage, TranscodeError> {
    let reader = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|_| TranscodeError::Undecodable)?;
    let format = reader.format();
    if !matches!(
        format,
        Some(ImageFormat::Jpeg | ImageFormat::Png | ImageFormat::WebP)
    ) {
        return Err(TranscodeError::Undecodable);
    }

    let mut decoder = reader
        .into_decoder()
        .map_err(|_| TranscodeError::Undecodable)?;
    let (width, height) = decoder.dimensions();
    if u64::from(width) * u64::from(height) > MAX_SOURCE_PIXELS {
        return Err(TranscodeError::SourceTooLarge);
    }
    let orientation = decoder
        .orientation()
        .map_err(|_| TranscodeError::Undecodable)?;
    let mut pixels =
        DynamicImage::from_decoder(decoder).map_err(|_| TranscodeError::Undecodable)?;
    pixels.apply_orientation(orientation);

    Ok(SourceImage {
        pixels,
        is_png: format == Some(ImageFormat::Png),
    })
}

/// The output size, `None` when it rounds to nothing, is negative or is absurdly large.
fn target_size(request: &ImageRequest, source: (u32, u32)) -> Option<(u32, u32)> {
    let (source_width, source_height) = (f64::from(source.0), f64::from(source.1));
    let by_width = f64::from(request.width) / source_width;
    let by_height = f64::from(request.height) / source_height;
    let scale = match request.fit {
        Fit::Cover => by_width.max(by_height),
        Fit::Contain => by_width.min(by_height),
    };
    let scale = if request.upscale {
        scale
    } else {
        scale.min(1.0)
    };
    Some((
        scaled_side(source_width, scale)?,
        scaled_side(source_height, scale)?,
    ))
}

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the range is checked before the cast"
)]
fn scaled_side(side: f64, scale: f64) -> Option<u32> {
    let scaled = (side * scale).round();
    (scaled >= 1.0 && scaled <= f64::from(MAX_OUTPUT_SIDE)).then_some(scaled as u32)
}

fn output_format(requested: Option<OutputFormat>, is_png: bool, has_alpha: bool) -> OutputFormat {
    match requested {
        Some(format @ (OutputFormat::Png | OutputFormat::Ppm)) => format,
        // JPEG cannot hold transparency, which wins over the request.
        _ if has_alpha => OutputFormat::Png,
        None if is_png => OutputFormat::Png,
        _ => OutputFormat::Jpeg,
    }
}

fn encode(
    source: &DynamicImage,
    (width, height): (u32, u32),
    format: OutputFormat,
    quality: Option<u8>,
) -> Result<Vec<u8>, TranscodeError> {
    let resized;
    let image = if source.dimensions() == (width, height) {
        source
    } else {
        resized = source.resize_exact(width, height, FilterType::Lanczos3);
        &resized
    };

    let mut output = Vec::new();
    match format {
        // Three channels whatever the source: a greyscale JPEG is not what the reference
        // sends, and the encoder would write one from a grey image.
        OutputFormat::Jpeg => DynamicImage::ImageRgb8(image.to_rgb8()).write_with_encoder(
            JpegEncoder::new_with_quality(
                &mut output,
                quality.unwrap_or(DEFAULT_JPEG_QUALITY).clamp(1, 100),
            ),
        ),
        OutputFormat::Png => image.write_with_encoder(PngEncoder::new(&mut output)),
        OutputFormat::Ppm => DynamicImage::ImageRgb8(image.to_rgb8()).write_with_encoder(
            PnmEncoder::new(&mut output).with_subtype(PnmSubtype::Pixmap(SampleEncoding::Binary)),
        ),
    }
    .map_err(TranscodeError::Encode)?;
    Ok(output)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use image::{GrayImage, Luma, RgbImage, Rgba, RgbaImage};

    use super::*;

    /// A 1708×640 JPEG the reference ships in Plex Web; every expected size below is what
    /// the reference answered for it.
    const SOURCE: (u32, u32) = (1708, 640);

    fn request(width: i32, height: i32, fit: Fit, upscale: bool) -> ImageRequest {
        ImageRequest {
            source: String::new(),
            width,
            height,
            fit,
            upscale,
            format: None,
            quality: None,
        }
    }

    fn size(width: i32, height: i32, fit: Fit, upscale: bool) -> Option<(u32, u32)> {
        target_size(&request(width, height, fit, upscale), SOURCE)
    }

    fn png_of(image: &DynamicImage) -> Vec<u8> {
        let mut bytes = Vec::new();
        image
            .write_with_encoder(PngEncoder::new(&mut bytes))
            .unwrap();
        bytes
    }

    fn gray(width: u32, height: u32) -> DynamicImage {
        DynamicImage::ImageLuma8(GrayImage::from_pixel(width, height, Luma([120])))
    }

    fn converted(
        source: &DynamicImage,
        width: i32,
        format: Option<OutputFormat>,
    ) -> ConvertedImage {
        let mut asked = request(width, width, Fit::Contain, false);
        asked.format = format;
        convert(&png_of(source), &asked).unwrap()
    }

    #[test]
    fn fits_inside_the_box_by_default() {
        assert_eq!(size(300, 300, Fit::Contain, false), Some((300, 112)));
        assert_eq!(size(100, 300, Fit::Contain, false), Some((100, 37)));
        assert_eq!(size(300, 100, Fit::Contain, false), Some((267, 100)));
        assert_eq!(size(333, 777, Fit::Contain, false), Some((333, 125)));
        assert_eq!(size(7, 100, Fit::Contain, false), Some((7, 3)));
        assert_eq!(size(256, 256, Fit::Contain, false), Some((256, 96)));
    }

    #[test]
    fn cover_fills_the_box() {
        assert_eq!(size(300, 300, Fit::Cover, false), Some((801, 300)));
        assert_eq!(size(100, 300, Fit::Cover, false), Some((801, 300)));
        assert_eq!(size(300, 100, Fit::Cover, false), Some((300, 112)));
        assert_eq!(size(256, 256, Fit::Cover, true), Some((683, 256)));
    }

    #[test]
    fn stays_at_the_source_size_unless_told_to_upscale() {
        assert_eq!(size(5000, 5000, Fit::Contain, false), Some(SOURCE));
        assert_eq!(size(5000, 5000, Fit::Cover, false), Some(SOURCE));
        assert_eq!(size(333, 777, Fit::Cover, false), Some(SOURCE));
        assert_eq!(size(5000, 5000, Fit::Contain, true), Some((5000, 1874)));
    }

    #[test]
    fn an_impossible_size_yields_nothing() {
        assert_eq!(size(1, 1, Fit::Contain, false), None);
        assert_eq!(size(-1, 150, Fit::Contain, false), None);
        assert_eq!(size(5000, 5000, Fit::Cover, true), None);
    }

    #[test]
    fn a_negative_dimension_is_harmless_when_the_other_decides() {
        assert_eq!(
            target_size(&request(-5, 60, Fit::Cover, true), (256, 256)),
            Some((60, 60))
        );
    }

    #[test]
    fn keeps_the_source_format_when_none_is_asked() {
        assert_eq!(output_format(None, true, false), OutputFormat::Png);
        assert_eq!(output_format(None, false, false), OutputFormat::Jpeg);
    }

    #[test]
    fn honors_a_requested_format() {
        assert_eq!(
            output_format(Some(OutputFormat::Jpeg), true, false),
            OutputFormat::Jpeg
        );
        assert_eq!(
            output_format(Some(OutputFormat::Png), false, false),
            OutputFormat::Png
        );
        assert_eq!(
            output_format(Some(OutputFormat::Ppm), false, false),
            OutputFormat::Ppm
        );
    }

    #[test]
    fn transparency_stays_png_even_when_jpeg_is_asked() {
        assert_eq!(
            output_format(Some(OutputFormat::Jpeg), true, true),
            OutputFormat::Png
        );
    }

    #[test]
    fn reports_what_the_source_is() {
        let opaque = decode(&png_of(&gray(40, 20))).unwrap();
        assert_eq!(opaque.pixels.dimensions(), (40, 20));
        assert!(opaque.is_png && !opaque.pixels.color().has_alpha());

        let transparent = DynamicImage::ImageRgba8(RgbaImage::from_pixel(4, 4, Rgba([1, 2, 3, 4])));
        assert!(decode(&png_of(&transparent))
            .unwrap()
            .pixels
            .color()
            .has_alpha());
    }

    #[test]
    fn refuses_what_is_not_an_accepted_image() {
        assert!(matches!(
            decode(b"User-agent: *\nDisallow: /"),
            Err(TranscodeError::Undecodable)
        ));
        // A valid image, in a format no poster or avatar comes in.
        assert!(matches!(
            decode(b"GIF89a\x01\x00\x01\x00\x00\x00\x00;"),
            Err(TranscodeError::Undecodable)
        ));
    }

    #[test]
    fn resizes_a_png_and_keeps_it_grey() {
        let output = converted(&gray(40, 20), 10, None);
        assert_eq!(output.format, OutputFormat::Png);
        let again = decode(&output.bytes).unwrap();
        assert_eq!(again.pixels.dimensions(), (10, 5));
        assert_eq!(again.pixels.color(), image::ColorType::L8);
    }

    #[test]
    fn a_jpeg_of_a_grey_image_has_three_channels() {
        let output = converted(&gray(16, 16), 8, Some(OutputFormat::Jpeg));
        let again = decode(&output.bytes).unwrap();
        assert!(!again.is_png);
        assert_eq!(again.pixels.color(), image::ColorType::Rgb8);
    }

    #[test]
    fn writes_a_binary_pixmap() {
        let black = DynamicImage::ImageRgb8(RgbImage::new(3, 2));
        let output = converted(&black, 3, Some(OutputFormat::Ppm));
        assert!(output.bytes.starts_with(b"P6"));
        assert!(output.bytes.ends_with(&[0; 18]));
    }
}
