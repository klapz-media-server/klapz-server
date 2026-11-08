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

use super::PhotoError;

/// A small file can declare a huge canvas.
const MAX_SOURCE_PIXELS: u64 = 64_000_000;

/// Where the reference's default falls on its own scale: between its 50 and its 90.
const DEFAULT_JPEG_QUALITY: u8 = 75;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Encoding {
    Jpeg(Option<u8>),
    Png,
    Ppm,
}

#[derive(Debug)]
pub(super) struct SourceImage {
    pub pixels: DynamicImage,
    pub is_png: bool,
}

impl SourceImage {
    pub fn dimensions(&self) -> (u32, u32) {
        self.pixels.dimensions()
    }

    pub fn has_alpha(&self) -> bool {
        self.pixels.color().has_alpha()
    }
}

/// Decodes `bytes`, upright: EXIF orientation is applied, as the reference does by default.
pub(super) fn decode(bytes: &[u8]) -> Result<SourceImage, PhotoError> {
    let reader = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|_| PhotoError::Undecodable)?;
    let format = reader.format();
    if !matches!(
        format,
        Some(ImageFormat::Jpeg | ImageFormat::Png | ImageFormat::WebP)
    ) {
        return Err(PhotoError::Undecodable);
    }

    let mut decoder = reader.into_decoder().map_err(|_| PhotoError::Undecodable)?;
    let (width, height) = decoder.dimensions();
    if u64::from(width) * u64::from(height) > MAX_SOURCE_PIXELS {
        return Err(PhotoError::SourceTooLarge);
    }
    let orientation = decoder.orientation().map_err(|_| PhotoError::Undecodable)?;
    let mut pixels = DynamicImage::from_decoder(decoder).map_err(|_| PhotoError::Undecodable)?;
    pixels.apply_orientation(orientation);

    Ok(SourceImage {
        pixels,
        is_png: format == Some(ImageFormat::Png),
    })
}

pub(super) fn encode(
    source: &SourceImage,
    (width, height): (u32, u32),
    encoding: Encoding,
) -> Result<Vec<u8>, PhotoError> {
    let resized;
    let image = if source.dimensions() == (width, height) {
        &source.pixels
    } else {
        resized = source
            .pixels
            .resize_exact(width, height, FilterType::Lanczos3);
        &resized
    };

    let mut output = Vec::new();
    match encoding {
        // Three channels whatever the source: a greyscale JPEG is not what the reference
        // sends, and the encoder would write one from a grey image.
        Encoding::Jpeg(quality) => DynamicImage::ImageRgb8(image.to_rgb8()).write_with_encoder(
            JpegEncoder::new_with_quality(&mut output, jpeg_quality(quality)),
        ),
        Encoding::Png => image.write_with_encoder(PngEncoder::new(&mut output)),
        Encoding::Ppm => DynamicImage::ImageRgb8(image.to_rgb8()).write_with_encoder(
            PnmEncoder::new(&mut output).with_subtype(PnmSubtype::Pixmap(SampleEncoding::Binary)),
        ),
    }
    .map_err(PhotoError::Encode)?;
    Ok(output)
}

/// The reference treats anything from 100 to 127 as 100.
fn jpeg_quality(requested: Option<u8>) -> u8 {
    requested.map_or(DEFAULT_JPEG_QUALITY, |quality| quality.clamp(1, 100))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use image::{GrayImage, Luma, RgbImage, Rgba, RgbaImage};

    use super::*;

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

    #[test]
    fn reports_what_the_source_is() {
        let opaque = decode(&png_of(&gray(40, 20))).unwrap();
        assert_eq!(opaque.dimensions(), (40, 20));
        assert!(opaque.is_png && !opaque.has_alpha());

        let transparent = DynamicImage::ImageRgba8(RgbaImage::from_pixel(4, 4, Rgba([1, 2, 3, 4])));
        assert!(decode(&png_of(&transparent)).unwrap().has_alpha());
    }

    #[test]
    fn refuses_what_is_not_an_accepted_image() {
        assert!(matches!(
            decode(b"User-agent: *\nDisallow: /"),
            Err(PhotoError::Undecodable)
        ));
        // A valid image, in a format no poster or avatar comes in.
        assert!(matches!(
            decode(b"GIF89a\x01\x00\x01\x00\x00\x00\x00;"),
            Err(PhotoError::Undecodable)
        ));
    }

    #[test]
    fn resizes_a_png_and_keeps_it_grey() {
        let source = decode(&png_of(&gray(40, 20))).unwrap();
        let output = encode(&source, (10, 5), Encoding::Png).unwrap();
        let again = decode(&output).unwrap();
        assert_eq!(again.dimensions(), (10, 5));
        assert_eq!(again.pixels.color(), image::ColorType::L8);
    }

    #[test]
    fn a_jpeg_of_a_grey_image_has_three_channels() {
        let source = decode(&png_of(&gray(16, 16))).unwrap();
        let output = encode(&source, (8, 8), Encoding::Jpeg(None)).unwrap();
        let again = decode(&output).unwrap();
        assert!(!again.is_png);
        assert_eq!(again.pixels.color(), image::ColorType::Rgb8);
    }

    #[test]
    fn writes_a_binary_pixmap() {
        let rgb = DynamicImage::ImageRgb8(RgbImage::new(3, 2));
        let source = decode(&png_of(&rgb)).unwrap();
        let output = encode(&source, (3, 2), Encoding::Ppm).unwrap();
        assert!(output.starts_with(b"P6"));
        assert!(output.ends_with(&[0; 18]));
    }

    #[test]
    fn quality_is_clamped_to_what_the_encoder_takes() {
        assert_eq!(jpeg_quality(None), DEFAULT_JPEG_QUALITY);
        assert_eq!(jpeg_quality(Some(127)), 100);
        assert_eq!(jpeg_quality(Some(10)), 10);
    }
}
