//! Decoding and Encoding of PNG Images
//!
//! PNG (Portable Network Graphics) is an image format that supports lossless compression.
//!
//! # Related Links
//! * <http://www.w3.org/TR/PNG/> - The PNG Specification
use core::num::NonZeroU32;
use std::borrow::Cow;
use std::io::{BufRead, Seek, Write};

use png::DeflateCompression;

pub use png::{BlendOp, DisposeOp};

use crate::animation::{Delay, Frame};
use crate::color::{ColorType, ExtendedColorType};
use crate::error::{
    DecodingError, EncodingError, ImageError, ImageResult, LimitError, LimitErrorKind,
    ParameterError, ParameterErrorKind, UnsupportedError, UnsupportedErrorKind,
};
use crate::io::decoder::DecodedMetadataHint;
use crate::io::{
    DecodedAnimationAttributes, DecodedImageAttributes, DecoderPreparedImage, FormatAttributes,
    SequenceControl,
};
use crate::math::Rect;
use crate::metadata::LoopCount;
use crate::utils::vec_try_with_capacity;
use crate::{
    DynamicImage, GenericImage, ImageDecoder, ImageEncoder, ImageFormat, ImageLayout, Limits, Luma,
    LumaA, Rgb, Rgba,
};

// http://www.w3.org/TR/PNG-Structure.html
// The first eight bytes of a PNG file always contain the following (decimal) values:
pub(crate) const PNG_SIGNATURE: [u8; 8] = [137, 80, 78, 71, 13, 10, 26, 10];
const XMP_KEY: &str = "XML:com.adobe.xmp";
const IPTC_KEYS: &[&str] = &["Raw profile type iptc", "Raw profile type 8bim"];

/// PNG decoder
pub struct PngDecoder<R: BufRead + Seek> {
    decoder: Option<png::Decoder<R>>,
    reader: Option<png::Reader<R>>,
    color_type: ColorType,
    limits: Limits,
}

impl<R: BufRead + Seek> PngDecoder<R> {
    /// Creates a new decoder that decodes from the stream ```r```
    pub fn new(r: R) -> PngDecoder<R> {
        Self::with_limits(r, Limits::no_limits())
    }

    /// Creates a new decoder that decodes from the stream ```r``` with the given limits.
    pub fn with_limits(r: R, limits: Limits) -> PngDecoder<R> {
        let max_bytes = usize::try_from(limits.max_alloc.unwrap_or(u64::MAX)).unwrap_or(usize::MAX);
        let mut decoder = png::Decoder::new_with_limits(r, png::Limits { bytes: max_bytes });
        decoder.set_ignore_text_chunk(false);

        PngDecoder {
            decoder: Some(decoder),
            // We'll replace this once we have a reader.
            color_type: ColorType::L8,
            reader: None,
            limits,
        }
    }

    fn ensure_reader_and_header(&mut self) -> ImageResult<&mut png::Reader<R>> {
        if self.reader.is_some() {
            // We do this for borrow-checking issues, do not borrow self outside the conditional
            // branch. So the None/Err case here is not reachable.
            return self.reader.as_mut().ok_or_else(|| unreachable!());
        }

        let Some(mut decoder) = self.decoder.take() else {
            return Err(reader_finished_already());
        };

        self.limits.check_support(&crate::LimitSupport::default())?;

        let info = decoder.read_header_info().map_err(ImageError::from_png)?;
        self.limits.check_dimensions(info.width, info.height)?;

        // By default the PNG decoder will scale 16 bpc to 8 bpc, so custom
        // transformations must be set. EXPAND preserves the default behavior
        // expanding bpc < 8 to 8 bpc.
        decoder.set_transformations(png::Transformations::EXPAND);
        let reader = decoder.read_info().map_err(ImageError::from_png)?;
        let (color_type, bits) = reader.output_color_type();

        let color_type = match (color_type, bits) {
            (png::ColorType::Grayscale, png::BitDepth::Eight) => ColorType::L8,
            (png::ColorType::Grayscale, png::BitDepth::Sixteen) => ColorType::L16,
            (png::ColorType::GrayscaleAlpha, png::BitDepth::Eight) => ColorType::La8,
            (png::ColorType::GrayscaleAlpha, png::BitDepth::Sixteen) => ColorType::La16,
            (png::ColorType::Rgb, png::BitDepth::Eight) => ColorType::Rgb8,
            (png::ColorType::Rgb, png::BitDepth::Sixteen) => ColorType::Rgb16,
            (png::ColorType::Rgba, png::BitDepth::Eight) => ColorType::Rgba8,
            (png::ColorType::Rgba, png::BitDepth::Sixteen) => ColorType::Rgba16,

            (png::ColorType::Grayscale, png::BitDepth::One) => {
                return Err(unsupported_color(ExtendedColorType::L1))
            }
            (png::ColorType::GrayscaleAlpha, png::BitDepth::One) => {
                return Err(unsupported_color(ExtendedColorType::La1))
            }
            (png::ColorType::Rgb, png::BitDepth::One) => {
                return Err(unsupported_color(ExtendedColorType::Rgb1))
            }
            (png::ColorType::Rgba, png::BitDepth::One) => {
                return Err(unsupported_color(ExtendedColorType::Rgba1))
            }

            (png::ColorType::Grayscale, png::BitDepth::Two) => {
                return Err(unsupported_color(ExtendedColorType::L2))
            }
            (png::ColorType::GrayscaleAlpha, png::BitDepth::Two) => {
                return Err(unsupported_color(ExtendedColorType::La2))
            }
            (png::ColorType::Rgb, png::BitDepth::Two) => {
                return Err(unsupported_color(ExtendedColorType::Rgb2))
            }
            (png::ColorType::Rgba, png::BitDepth::Two) => {
                return Err(unsupported_color(ExtendedColorType::Rgba2))
            }

            (png::ColorType::Grayscale, png::BitDepth::Four) => {
                return Err(unsupported_color(ExtendedColorType::L4))
            }
            (png::ColorType::GrayscaleAlpha, png::BitDepth::Four) => {
                return Err(unsupported_color(ExtendedColorType::La4))
            }
            (png::ColorType::Rgb, png::BitDepth::Four) => {
                return Err(unsupported_color(ExtendedColorType::Rgb4))
            }
            (png::ColorType::Rgba, png::BitDepth::Four) => {
                return Err(unsupported_color(ExtendedColorType::Rgba4))
            }

            (png::ColorType::Indexed, bits) => {
                return Err(unsupported_color(ExtendedColorType::Unknown(bits as u8)))
            }
        };

        self.color_type = color_type;
        Ok(self.reader.insert(reader))
    }

    /// Returns the gamma value of the image or None if no gamma value is indicated.
    ///
    /// If an sRGB chunk is present this method returns a gamma value of 0.45455 and ignores the
    /// value in the gAMA chunk. This is the recommended behavior according to the PNG standard:
    ///
    /// > When the sRGB chunk is present, [...] decoders that recognize the sRGB chunk but are not
    /// > capable of colour management are recommended to ignore the gAMA and cHRM chunks, and use
    /// > the values given above as if they had appeared in gAMA and cHRM chunks.
    pub fn gamma_value(&mut self) -> ImageResult<Option<f64>> {
        let reader = self.ensure_reader_and_header()?;
        Ok(reader
            .info()
            .source_gamma
            .map(|x| f64::from(x.into_scaled()) / 100_000.0))
    }

    /// Turn this into an iterator over the animation frames.
    ///
    /// Reading the complete animation requires more memory than reading the data from the IDAT
    /// frame–multiple frame buffers need to be reserved at the same time. We further do not
    /// support compositing 16-bit colors. In any case this would be lossy as the interface of
    /// animation decoders does not support 16-bit colors.
    ///
    /// If something is not supported or a limit is violated then the decoding step that requires
    /// them will fail and an error will be returned instead of the frame. No further frames will
    /// be returned.
    pub fn apng(self) -> ImageResult<ApngDecoder<R>> {
        ApngDecoder::read_sequence_data(self)
    }

    /// Returns if the image contains an animation.
    ///
    /// Note that the file itself decides if the default image is considered to be part of the
    /// animation. When it is not the common interpretation is to use it as a thumbnail.
    ///
    /// If a non-animated image is converted into an `ApngDecoder` then its iterator is empty.
    pub fn is_apng(&mut self) -> ImageResult<bool> {
        let reader = self.ensure_reader_and_header()?;
        Ok(reader.info().animation_control.is_some())
    }

    fn color_type_info(info: &png::Info<'_>) -> ExtendedColorType {
        match (info.color_type, info.bit_depth) {
            (png::ColorType::Grayscale, png::BitDepth::One) => ExtendedColorType::L1,
            (png::ColorType::Grayscale, png::BitDepth::Two) => ExtendedColorType::L2,
            (png::ColorType::Grayscale, png::BitDepth::Four) => ExtendedColorType::L4,
            (png::ColorType::Grayscale, png::BitDepth::Eight) => ExtendedColorType::L8,
            (png::ColorType::Grayscale, png::BitDepth::Sixteen) => ExtendedColorType::L16,
            (png::ColorType::GrayscaleAlpha, png::BitDepth::One) => ExtendedColorType::La1,
            (png::ColorType::GrayscaleAlpha, png::BitDepth::Two) => ExtendedColorType::La2,
            (png::ColorType::GrayscaleAlpha, png::BitDepth::Four) => ExtendedColorType::La4,
            (png::ColorType::GrayscaleAlpha, png::BitDepth::Eight) => ExtendedColorType::La8,
            (png::ColorType::GrayscaleAlpha, png::BitDepth::Sixteen) => ExtendedColorType::La16,
            (png::ColorType::Rgb, png::BitDepth::One) => ExtendedColorType::Rgb1,
            (png::ColorType::Rgb, png::BitDepth::Two) => ExtendedColorType::Rgb2,
            (png::ColorType::Rgb, png::BitDepth::Four) => ExtendedColorType::Rgb4,
            (png::ColorType::Rgb, png::BitDepth::Eight) => ExtendedColorType::Rgb8,
            (png::ColorType::Rgb, png::BitDepth::Sixteen) => ExtendedColorType::Rgb16,
            (png::ColorType::Rgba, png::BitDepth::One) => ExtendedColorType::Rgba1,
            (png::ColorType::Rgba, png::BitDepth::Two) => ExtendedColorType::Rgba2,
            (png::ColorType::Rgba, png::BitDepth::Four) => ExtendedColorType::Rgba4,
            (png::ColorType::Rgba, png::BitDepth::Eight) => ExtendedColorType::Rgba8,
            (png::ColorType::Rgba, png::BitDepth::Sixteen) => ExtendedColorType::Rgba16,
            (png::ColorType::Indexed, png::BitDepth::One) => ExtendedColorType::Unknown(1),
            (png::ColorType::Indexed, png::BitDepth::Two) => ExtendedColorType::Unknown(2),
            (png::ColorType::Indexed, png::BitDepth::Four) => ExtendedColorType::Unknown(4),
            (png::ColorType::Indexed, png::BitDepth::Eight) => ExtendedColorType::Unknown(8),
            (png::ColorType::Indexed, png::BitDepth::Sixteen) => ExtendedColorType::Unknown(16),
        }
    }

    /// The maximum number of bytes iTXt and zTXt are allowed to decompress to.
    /// This guards against decompression bombs.
    fn text_decompress_limit(&mut self) -> usize {
        let max = png::text_metadata::DECOMPRESSION_LIMIT as u64;
        self.limits.max_alloc.unwrap_or(max).min(max) as usize
    }
}

fn attributes_from_info(info: &png::Info<'_>) -> DecodedImageAttributes {
    let delay = info.frame_control().map(|fc| {
        // PNG delays are rations in seconds.
        let num = u32::from(fc.delay_num) * 1_000u32;
        let denom = match fc.delay_den {
            // The standard dictates to replace by 100 when the denominator is 0.
            0 => 100,
            d => u32::from(d),
        };

        Delay::from_numer_denom_ms(num, denom)
    });

    DecodedImageAttributes {
        // We do not set x_offset and y_offset since the decoder performs composition according
        // to Dispose and blend. For reading raw frames we'd pass the `fc.x_offset` here.
        delay,
        ..DecodedImageAttributes::default()
    }
}

fn unsupported_color(ect: ExtendedColorType) -> ImageError {
    ImageError::Unsupported(UnsupportedError::from_format_and_kind(
        ImageFormat::Png.into(),
        UnsupportedErrorKind::Color(ect),
    ))
}

fn decoding_started_already() -> ImageError {
    ImageError::Parameter(ParameterError::from_kind(ParameterErrorKind::NoMoreData))
}

fn reader_finished_already() -> ImageError {
    ImageError::Parameter(ParameterError::from_kind(ParameterErrorKind::NoMoreData))
}

/// PNG images are big endian. For 16 bit per channel and larger types, the buffer may need
/// to be reordered to native endianness per the contract of `read_image`. Assumes equal
/// depth which is the only supported output from `png` with our options.
fn big_endian_to_native_endian(buf: &mut [u8], color: ColorType) {
    let bytes_per_channel = color.bytes_per_pixel() / color.channel_count();

    match bytes_per_channel {
        1 => (), // No reodering necessary for u8
        2 => buf.as_chunks_mut::<2>().0.iter_mut().for_each(|c| {
            *c = u16::from_be_bytes(*c).to_ne_bytes();
        }),
        // not emitted by png crate
        _ => unreachable!(),
    }
}

impl<R: BufRead + Seek> ImageDecoder for PngDecoder<R> {
    fn prepare_image(&mut self) -> ImageResult<DecoderPreparedImage> {
        let reader = self.ensure_reader_and_header()?;
        let (width, height) = reader.info().size();
        Ok(DecoderPreparedImage::new(width, height, self.color_type))
    }

    fn format_attributes(&self) -> FormatAttributes {
        FormatAttributes {
            // is any sort of iTXT chunk.
            // FIXME: we do not collect these in advance.
            xmp: DecodedMetadataHint::InHeader,
            // is any sort of iTXT chunk.
            // FIXME: we do not collect these in advance.
            iptc: DecodedMetadataHint::InHeader,
            // see iCCP chunk order.
            icc: DecodedMetadataHint::InHeader,
            // see eXIf chunk order.
            exif: DecodedMetadataHint::InHeader,
            ..FormatAttributes::default()
        }
    }

    /// Only for [`ApngDecoder`].
    fn animation_attributes(&mut self) -> Option<DecodedAnimationAttributes> {
        None
    }

    fn icc_profile(&mut self) -> ImageResult<Option<Vec<u8>>> {
        let reader = self.ensure_reader_and_header()?;
        Ok(reader.info().icc_profile.as_ref().map(|x| x.to_vec()))
    }

    fn exif_metadata(&mut self) -> ImageResult<Option<Vec<u8>>> {
        let reader = self.ensure_reader_and_header()?;
        Ok(reader.info().exif_metadata.as_ref().map(|x| x.to_vec()))
    }

    fn xmp_metadata(&mut self) -> ImageResult<Option<Vec<u8>>> {
        let decompression_limit = self.text_decompress_limit();
        let reader = self.ensure_reader_and_header()?;

        if let Some(mut itx_chunk) = reader
            .info()
            .utf8_text
            .iter()
            .find(|chunk| chunk.keyword == XMP_KEY)
            .cloned()
        {
            itx_chunk
                .decompress_text_with_limit(decompression_limit)
                .map_err(ImageError::from_png)?;
            return itx_chunk
                .get_text()
                .map(|text| Some(text.into_bytes()))
                .map_err(ImageError::from_png);
        }

        Ok(None)
    }

    fn iptc_metadata(&mut self) -> ImageResult<Option<Vec<u8>>> {
        let decompression_limit = self.text_decompress_limit();
        let reader = self.ensure_reader_and_header()?;

        if let Some(mut text_chunk) = reader
            .info()
            .compressed_latin1_text
            .iter()
            .find(|chunk| IPTC_KEYS.iter().any(|key| chunk.keyword.contains(key)))
            .cloned()
        {
            text_chunk
                .decompress_text_with_limit(decompression_limit)
                .map_err(ImageError::from_png)?;
            return text_chunk
                .get_text()
                .map(|text| Some(text.into_bytes()))
                .map_err(ImageError::from_png);
        }

        if let Some(text_chunk) = reader
            .info()
            .uncompressed_latin1_text
            .iter()
            .find(|chunk| IPTC_KEYS.iter().any(|key| chunk.keyword.contains(key)))
            .cloned()
        {
            return Ok(Some(text_chunk.text.into_bytes()));
        }
        Ok(None)
    }

    fn read_image(&mut self, buf: &mut [u8]) -> ImageResult<DecodedImageAttributes> {
        let layout = self.prepare_image()?;
        assert_eq!(u64::try_from(buf.len()), Ok(layout.total_bytes()));

        let reader = self.ensure_reader_and_header()?;
        let original_color_type = Self::color_type_info(reader.info());
        reader.next_frame(buf).map_err(ImageError::from_png)?;

        big_endian_to_native_endian(buf, layout.layout.color);

        Ok(DecodedImageAttributes {
            original_color_type: Some(original_color_type),
            ..DecodedImageAttributes::default()
        })
    }

    fn set_limits(&mut self, limits: Limits) -> ImageResult<()> {
        limits.check_support(&crate::LimitSupport::default())?;

        if let Some(decoder) = &mut self.decoder {
            decoder.set_limits(png::Limits {
                bytes: match limits.max_alloc {
                    None => usize::MAX,
                    Some(limit) => limit.try_into().unwrap_or(usize::MAX),
                },
            });

            self.limits = limits;
            Ok(())
        } else {
            Err(decoding_started_already())
        }
    }
}

/// An animated adapter of [`PngDecoder`].
///
/// See [`PngDecoder::apng`] for more information.
pub struct ApngDecoder<R: BufRead + Seek> {
    inner: PngDecoder<R>,
    /// The current output buffer.
    current: Option<DynamicImage>,
    /// The previous output buffer, used for dispose op previous.
    previous: Option<DynamicImage>,
    /// The dispose op of the current frame.
    dispose: DisposeOp,
    /// Buffer to put the frame data which is to be composed onto the current frame.
    raw_frame_buffer: Vec<u8>,

    /// The region to dispose of the previous frame.
    dispose_region: Option<Rect>,
    /// The number of image still expected to be able to load.
    remaining: u32,
    /// The next (first) image is the thumbnail.
    has_thumbnail: bool,
}

impl<R: BufRead + Seek> ApngDecoder<R> {
    fn read_sequence_data(mut inner: PngDecoder<R>) -> ImageResult<Self> {
        let reader = inner.ensure_reader_and_header()?;
        let remaining = match reader.info().animation_control() {
            // The expected number of fcTL in the remaining image.
            Some(actl) => actl.num_frames,
            None => 0,
        };

        // If the IDAT has no fcTL then it is not part of the animation counted by
        // num_frames. All following fdAT chunks must be preceded by an fcTL
        let has_thumbnail = reader.info().frame_control.is_none();

        Ok(ApngDecoder {
            inner,
            current: None,
            previous: None,
            raw_frame_buffer: vec![],
            dispose: DisposeOp::Background,
            dispose_region: None,
            remaining,
            has_thumbnail,
        })
    }

    /// Decode one subframe and overlay it on the canvas.
    fn mix_next_frame(
        &mut self,
        buf: &mut [u8],
    ) -> Result<Option<DecodedImageAttributes>, ImageError> {
        // Remove this image from remaining.
        self.remaining = match self.remaining.checked_sub(1) {
            None => return Ok(None),
            Some(next) => next,
        };

        // Allocate the buffers, honoring the memory limits
        let layout = self.inner.prepare_image()?;
        let ImageLayout {
            width,
            height,
            color,
        } = layout.layout;

        assert_eq!(u64::try_from(buf.len()), Ok(layout.total_bytes()));

        // Shorten ourselves to 0 in case of error.
        let remaining = self.remaining;
        self.remaining = 0;

        // Skip the thumbnail that is not part of the animation.
        if self.has_thumbnail {
            let reader = self.inner.ensure_reader_and_header()?;
            reader.next_frame(buf).map_err(ImageError::from_png)?;
            self.has_thumbnail = false;
        }

        {
            let limits = &mut self.inner.limits;

            if self.previous.is_none() {
                limits.reserve_buffer(width, height, color)?;
                self.previous = Some(DynamicImage::new(width, height, color));
            }

            if self.current.is_none() {
                limits.reserve_buffer(width, height, color)?;
                self.current = Some(DynamicImage::new(width, height, color));
            }
        }

        self.animatable_color_type()?;

        // We've initialized them earlier in this function
        let previous = self.previous.as_mut().unwrap();
        let current = self.current.as_mut().unwrap();

        // Dispose of the previous frame.
        match self.dispose {
            DisposeOp::None => {
                previous.clone_from(current);
            }
            DisposeOp::Background => {
                previous.clone_from(current);
                if let Some(rect) = self.dispose_region {
                    clear_pixel_region(current.as_mut_bytes(), &layout.layout, &rect);
                } else {
                    // The first frame is always a background frame.
                    current.as_mut_bytes().fill(0);
                }
            }
            DisposeOp::Previous => {
                let rect = self
                    .dispose_region
                    .expect("The first frame must not set dispose=Previous");
                let region_previous = previous.sub_image(rect);
                current
                    .copy_from(&*region_previous, rect.x, rect.y)
                    .unwrap();
            }
        }

        // The allocations from now on are not going to persist,
        // and will be destroyed at the end of the scope.
        // Clone the limits so that any changes to them die with the allocations.
        let mut limits = self.inner.limits.clone();
        let reader = self.inner.ensure_reader_and_header()?;

        // Read next frame data.
        let raw_frame_size = reader.output_buffer_size().ok_or_else(|| {
            ImageError::Limits(LimitError::from_kind(LimitErrorKind::InsufficientMemory))
        })?;

        // The frame size depends on frame control. If possible, we want to read it into the
        // (temporary) output buffer that's been allocated for us anyways.
        let buffer = if raw_frame_size <= buf.len() {
            &mut buf[..raw_frame_size]
        } else if raw_frame_size <= self.raw_frame_buffer.len() {
            &mut self.raw_frame_buffer[..raw_frame_size]
        } else {
            limits.free_usize(self.raw_frame_buffer.len());
            limits.reserve_usize(raw_frame_size)?;
            self.raw_frame_buffer.resize(raw_frame_size, 0);
            &mut self.raw_frame_buffer[..]
        };

        // TODO: add `png::Reader::change_limits()` and call it here
        // to also constrain the internal buffer allocations in the PNG crate
        reader.next_frame(buffer).map_err(ImageError::from_png)?;

        big_endian_to_native_endian(buffer, color);

        // Find out how to interpret the decoded frame.
        let info = reader.info();
        let attributes = attributes_from_info(info);

        let (dispose_region, blend);
        match info.frame_control() {
            None => {
                dispose_region = Rect {
                    width: info.width,
                    height: info.height,
                    x: 0,
                    y: 0,
                };

                blend = BlendOp::Source;
            }
            Some(fc) => {
                dispose_region = Rect {
                    width: fc.width,
                    height: fc.height,
                    x: fc.x_offset,
                    y: fc.y_offset,
                };

                blend = fc.blend_op;
                self.dispose = fc.dispose_op;
            }
        }

        self.dispose_region = Some(dispose_region);

        match blend {
            BlendOp::Source => {
                copy_pixel_bytes(
                    current.as_mut_bytes(),
                    &layout.layout,
                    &buffer[..],
                    &dispose_region,
                );
            }
            BlendOp::Over => {
                // TODO: investigate speed, speed-ups, and bounds-checks.
                blend_pixel_bytes(
                    current.as_mut_bytes(),
                    &layout.layout,
                    &buffer[..],
                    &dispose_region,
                )
            }
        }

        // Ok, we can proceed with actually remaining images.
        self.remaining = remaining;

        // Return composited output buffer.
        buf.copy_from_slice(current.as_bytes());

        Ok(Some(attributes))
    }

    fn animatable_color_type(&self) -> Result<(), ImageError> {
        match self.inner.color_type {
            ColorType::L8
            | ColorType::Rgb8
            | ColorType::La8
            | ColorType::Rgba8
            | ColorType::L16
            | ColorType::Rgb16
            | ColorType::La16
            | ColorType::Rgba16 => Ok(()),
            _ => {
                debug_assert!(false, "{:?} not a valid png color", self.inner.color_type);
                Err(unsupported_color(self.inner.color_type.into()))
            }
        }
    }
}

impl<R: BufRead + Seek> ImageDecoder for ApngDecoder<R> {
    fn format_attributes(&self) -> FormatAttributes {
        FormatAttributes {
            supports_animation: true,
            ..self.inner.format_attributes()
        }
    }

    fn animation_attributes(&mut self) -> Option<DecodedAnimationAttributes> {
        let count = if let Ok(reader) = self.inner.ensure_reader_and_header() {
            reader.info().animation_control()
        } else {
            return None;
        };

        let loop_count = match count {
            None => LoopCount::Infinite,
            Some(actl) if actl.num_plays == 0 => LoopCount::Infinite,
            Some(actl) => LoopCount::Finite(
                NonZeroU32::new(actl.num_plays).expect("num_plays should be non-zero"),
            ),
        };

        Some(DecodedAnimationAttributes { loop_count })
    }

    fn prepare_image(&mut self) -> ImageResult<DecoderPreparedImage> {
        self.inner.prepare_image()
    }

    fn read_image(&mut self, buf: &mut [u8]) -> ImageResult<DecodedImageAttributes> {
        self.mix_next_frame(buf)?
            .ok_or_else(reader_finished_already)
    }

    fn more_images(&self) -> SequenceControl {
        if self.remaining > 0 {
            SequenceControl::MaybeMore
        } else {
            SequenceControl::None
        }
    }
}

/// PNG encoder
pub struct PngEncoder<W: Write> {
    w: W,
    compression: CompressionType,
    filter: FilterType,
    icc_profile: Vec<u8>,
    exif_metadata: Vec<u8>,
    xmp_metadata: Option<String>,
}

/// DEFLATE compression level of a PNG encoder. The default setting is `Fast`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
#[derive(Default)]
pub enum CompressionType {
    /// No compression whatsoever
    Uncompressed,
    /// Fast, minimal compression
    #[default]
    Fast,
    /// Balance between speed and compression level
    Balanced,
    /// High compression level
    Best,
    /// Detailed compression level between 1 and 9
    Level(u8),
}

/// Filter algorithms used to process image data to improve compression.
///
/// The default filter is `Adaptive`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
#[derive(Default)]
pub enum FilterType {
    /// No processing done, best used for low bit depth grayscale or data with a
    /// low color count
    NoFilter,
    /// Filters based on previous pixel in the same scanline
    Sub,
    /// Filters based on the scanline above
    Up,
    /// Filters based on the average of left and right neighbor pixels
    Avg,
    /// Algorithm that takes into account the left, upper left, and above pixels
    Paeth,
    /// Uses a heuristic to select one of the preceding filters for each
    /// scanline rather than one filter for the entire image
    #[default]
    Adaptive,
}

impl<W: Write> PngEncoder<W> {
    /// Create a new encoder that writes its output to ```w```
    pub fn new(w: W) -> PngEncoder<W> {
        PngEncoder {
            w,
            compression: CompressionType::default(),
            filter: FilterType::default(),
            icc_profile: Vec::new(),
            exif_metadata: Vec::new(),
            xmp_metadata: None,
        }
    }

    /// Create a new encoder that writes its output to `w` with `CompressionType` `compression` and
    /// `FilterType` `filter`.
    ///
    /// It is best to view the options as a _hint_ to the implementation on the smallest or fastest
    /// option for encoding a particular image. That is, using options that map directly to a PNG
    /// image parameter will use this parameter where possible. But variants that have no direct
    /// mapping may be interpreted differently in minor versions. The exact output is expressly
    /// __not__ part of the SemVer stability guarantee.
    ///
    /// Note that it is not optimal to use a single filter type, so an adaptive
    /// filter type is selected as the default. The filter which best minimizes
    /// file size may change with the type of compression used.
    pub fn new_with_quality(
        w: W,
        compression: CompressionType,
        filter: FilterType,
    ) -> PngEncoder<W> {
        PngEncoder {
            w,
            compression,
            filter,
            icc_profile: Vec::new(),
            exif_metadata: Vec::new(),
            xmp_metadata: None,
        }
    }

    fn encode_inner(
        self,
        data: &[u8],
        width: u32,
        height: u32,
        color: ExtendedColorType,
    ) -> ImageResult<()> {
        let encoder = Self::create_encoder(
            self.w,
            width,
            height,
            color,
            self.compression,
            self.filter,
            &self.icc_profile,
            &self.exif_metadata,
            self.xmp_metadata,
        )?;

        let mut writer = encoder.write_header().map_err(ImageError::from_png_enc)?;
        writer
            .write_image_data(data)
            .map_err(ImageError::from_png_enc)?;
        writer.finish().map_err(ImageError::from_png_enc)
    }

    /// Convert this encoder into an [`ApngEncoder`] writing an animation of `num_frames` frames.
    ///
    /// The compression, filter and metadata settings of this encoder are carried over. The
    /// `loop_count` determines how often the animation is played, see [`LoopCount`].
    ///
    /// Returns an error if `num_frames` is `0`, an APNG animation must contain at least one
    /// frame.
    pub fn into_apng_encoder(
        self,
        num_frames: u32,
        loop_count: LoopCount,
    ) -> ImageResult<ApngEncoder<W>> {
        if num_frames == 0 {
            return Err(ImageError::Encoding(EncodingError::new(
                ImageFormat::Png.into(),
                "APNG animations must contain at least one frame",
            )));
        }

        let num_plays = match loop_count {
            LoopCount::Infinite => 0,
            LoopCount::Finite(n) => n.get(),
        };

        Ok(ApngEncoder {
            w: Some(self.w),
            compression: self.compression,
            filter: self.filter,
            icc_profile: self.icc_profile,
            exif_metadata: self.exif_metadata,
            xmp_metadata: self.xmp_metadata,
            num_frames,
            num_plays,
            writer: None,
            width: 0,
            height: 0,
            frames_written: 0,
        })
    }

    /// Build the underlying `png` encoder with the configuration shared by the static and the
    /// animated (APNG) encoding paths.
    #[allow(clippy::too_many_arguments)]
    fn create_encoder<'a>(
        w: W,
        width: u32,
        height: u32,
        color: ExtendedColorType,
        compression: CompressionType,
        filter: FilterType,
        icc_profile: &'a [u8],
        exif_metadata: &'a [u8],
        xmp_metadata: Option<String>,
    ) -> ImageResult<png::Encoder<'a, W>> {
        let (ct, bits) = match color {
            ExtendedColorType::L8 => (png::ColorType::Grayscale, png::BitDepth::Eight),
            ExtendedColorType::L16 => (png::ColorType::Grayscale, png::BitDepth::Sixteen),
            ExtendedColorType::La8 => (png::ColorType::GrayscaleAlpha, png::BitDepth::Eight),
            ExtendedColorType::La16 => (png::ColorType::GrayscaleAlpha, png::BitDepth::Sixteen),
            ExtendedColorType::Rgb8 => (png::ColorType::Rgb, png::BitDepth::Eight),
            ExtendedColorType::Rgb16 => (png::ColorType::Rgb, png::BitDepth::Sixteen),
            ExtendedColorType::Rgba8 => (png::ColorType::Rgba, png::BitDepth::Eight),
            ExtendedColorType::Rgba16 => (png::ColorType::Rgba, png::BitDepth::Sixteen),
            _ => {
                return Err(ImageError::Unsupported(
                    UnsupportedError::from_format_and_kind(
                        ImageFormat::Png.into(),
                        UnsupportedErrorKind::Color(color),
                    ),
                ))
            }
        };

        let comp = match compression {
            CompressionType::Balanced => png::Compression::Balanced,
            CompressionType::Best => png::Compression::High,
            CompressionType::Fast => png::Compression::Fast,
            CompressionType::Uncompressed => png::Compression::NoCompression,
            CompressionType::Level(0) => png::Compression::NoCompression,
            CompressionType::Level(_) => png::Compression::Fast, // whatever, will be overridden
        };

        let advanced_comp = match compression {
            // Do not set level 0 as a Zlib level to avoid Zlib backend variance.
            // For example, in miniz_oxide level 0 is very slow.
            CompressionType::Level(n @ 1..) => Some(DeflateCompression::Level(n)),
            _ => None,
        };

        let filter = match filter {
            FilterType::NoFilter => png::Filter::NoFilter,
            FilterType::Sub => png::Filter::Sub,
            FilterType::Up => png::Filter::Up,
            FilterType::Avg => png::Filter::Avg,
            FilterType::Paeth => png::Filter::Paeth,
            FilterType::Adaptive => png::Filter::Adaptive,
        };

        let mut info = png::Info::with_size(width, height);

        if !icc_profile.is_empty() {
            info.icc_profile = Some(Cow::Borrowed(icc_profile));
        }
        if !exif_metadata.is_empty() {
            info.exif_metadata = Some(Cow::Borrowed(exif_metadata));
        }

        let mut encoder = png::Encoder::with_info(w, info).map_err(ImageError::from_png_enc)?;

        if let Some(xmp_text) = xmp_metadata {
            encoder
                .add_itxt_chunk(XMP_KEY.to_string(), xmp_text)
                .map_err(ImageError::from_png_enc)?;
        }

        encoder.set_color(ct);
        encoder.set_depth(bits);
        encoder.set_compression(comp);
        if let Some(compression) = advanced_comp {
            encoder.set_deflate_compression(compression);
        }
        encoder.set_filter(filter);

        Ok(encoder)
    }
}

/// A single frame of an APNG animation.
///
/// This wraps an [`animation::Frame`](Frame) with the blend and dispose operations that APNG
/// supports in addition to the frame delay. See [`ApngEncoder`].
///
/// All frames of one animation must have the same dimensions, the dimensions of the first frame
/// determine the size of the animation canvas.
#[derive(Clone)]
pub struct ApngFrame {
    frame: Frame,
    blend_op: BlendOp,
    dispose_op: DisposeOp,
}

impl ApngFrame {
    /// Create a frame with the default operations, [`BlendOp::Source`] and [`DisposeOp::None`].
    #[must_use]
    pub fn new(frame: Frame) -> Self {
        ApngFrame {
            frame,
            blend_op: BlendOp::Source,
            dispose_op: DisposeOp::None,
        }
    }

    /// Create a frame with explicit blend and dispose operations.
    #[must_use]
    pub fn from_parts(frame: Frame, blend_op: BlendOp, dispose_op: DisposeOp) -> Self {
        ApngFrame {
            frame,
            blend_op,
            dispose_op,
        }
    }

    /// The underlying frame, containing the pixel data, delay and offsets.
    #[must_use]
    pub fn frame(&self) -> &Frame {
        &self.frame
    }

    /// Convert back into the underlying [`Frame`], discarding the blend and dispose operations.
    #[must_use]
    pub fn into_frame(self) -> Frame {
        self.frame
    }

    /// The blend operation of this frame.
    #[must_use]
    pub fn blend_op(&self) -> BlendOp {
        self.blend_op
    }

    /// Set the blend operation of this frame.
    pub fn set_blend_op(&mut self, blend_op: BlendOp) {
        self.blend_op = blend_op;
    }

    /// The dispose operation of this frame.
    #[must_use]
    pub fn dispose_op(&self) -> DisposeOp {
        self.dispose_op
    }

    /// Set the dispose operation of this frame.
    pub fn set_dispose_op(&mut self, dispose_op: DisposeOp) {
        self.dispose_op = dispose_op;
    }
}

impl From<Frame> for ApngFrame {
    fn from(frame: Frame) -> Self {
        ApngFrame::new(frame)
    }
}

/// Encoder for animated PNG (APNG) images.
///
/// Created through [`PngEncoder::into_apng_encoder`]. Frames are written one at a time with
/// [`ApngEncoder::encode_frame`], the animation is finalized with [`ApngEncoder::finish`].
///
/// The first frame is encoded as the default image (`IDAT`) so that decoders without APNG
/// support display it as a static image, all further frames are encoded as `fdAT` chunks. All
/// frames must have the same dimensions.
///
/// Dropping the encoder without calling [`ApngEncoder::finish`] still writes a valid file
/// trailer but does not report errors and does not verify that all announced frames were
/// written.
pub struct ApngEncoder<W: Write> {
    /// The writer, taken once the first frame is written and the header is emitted.
    w: Option<W>,
    compression: CompressionType,
    filter: FilterType,
    icc_profile: Vec<u8>,
    exif_metadata: Vec<u8>,
    xmp_metadata: Option<String>,
    num_frames: u32,
    num_plays: u32,
    /// The underlying writer, created when the first frame determines the canvas dimensions.
    writer: Option<png::Writer<W>>,
    width: u32,
    height: u32,
    frames_written: u32,
}

impl<W: Write> ApngEncoder<W> {
    /// Encode one frame of the animation.
    ///
    /// The first frame determines the dimensions of the animation canvas. Returns an error if
    /// the dimensions of a later frame differ from it, or if more frames are written than the
    /// `num_frames` announced in [`PngEncoder::into_apng_encoder`].
    pub fn encode_frame(&mut self, frame: ApngFrame) -> ImageResult<()> {
        if self.frames_written >= self.num_frames {
            return Err(ImageError::Parameter(ParameterError::from_kind(
                ParameterErrorKind::NoMoreData,
            )));
        }

        let ApngFrame {
            frame,
            blend_op,
            dispose_op,
        } = frame;
        let buffer = frame.buffer();

        if self.writer.is_none() {
            let mut encoder = PngEncoder::create_encoder(
                self.w.take().ok_or_else(|| {
                    ImageError::Parameter(ParameterError::from_kind(
                        ParameterErrorKind::FailedAlready,
                    ))
                })?,
                buffer.width(),
                buffer.height(),
                ExtendedColorType::Rgba8,
                self.compression,
                self.filter,
                &self.icc_profile,
                &self.exif_metadata,
                self.xmp_metadata.take(),
            )?;

            encoder
                .set_animated(self.num_frames, self.num_plays)
                .map_err(ImageError::from_png_enc)?;
            encoder.validate_sequence(true);

            self.width = buffer.width();
            self.height = buffer.height();
            self.writer = Some(encoder.write_header().map_err(ImageError::from_png_enc)?);
        }

        if buffer.width() != self.width || buffer.height() != self.height {
            return Err(ImageError::Parameter(ParameterError::from_kind(
                ParameterErrorKind::DimensionMismatch,
            )));
        }

        let (delay_num, delay_den) = delay_to_fraction(frame.delay());

        // Unwrap: the writer was initialized above if it did not exist yet.
        let writer = self.writer.as_mut().unwrap();
        writer
            .set_frame_delay(delay_num, delay_den)
            .map_err(ImageError::from_png_enc)?;
        writer
            .set_blend_op(blend_op)
            .map_err(ImageError::from_png_enc)?;
        writer
            .set_dispose_op(dispose_op)
            .map_err(ImageError::from_png_enc)?;
        writer
            .write_image_data(buffer.as_raw())
            .map_err(ImageError::from_png_enc)?;

        self.frames_written += 1;
        Ok(())
    }

    /// Encode all frames of the animation.
    ///
    /// Consider using [`ApngEncoder::try_encode_frames`] to encode an iterator of fallible
    /// frames instead.
    pub fn encode_frames<F>(&mut self, frames: F) -> ImageResult<()>
    where
        F: IntoIterator<Item = ApngFrame>,
    {
        for frame in frames {
            self.encode_frame(frame)?;
        }
        Ok(())
    }

    /// Try to encode a collection of `ImageResult<ApngFrame>` objects.
    ///
    /// Whenever an `Err` item is encountered, that value is returned without further actions.
    pub fn try_encode_frames<F>(&mut self, frames: F) -> ImageResult<()>
    where
        F: IntoIterator<Item = ImageResult<ApngFrame>>,
    {
        for frame in frames {
            self.encode_frame(frame?)?;
        }
        Ok(())
    }

    /// Finalize the animation, writing the image trailer.
    ///
    /// Returns an error if no frame was written at all, or if fewer frames were written than
    /// the `num_frames` announced in [`PngEncoder::into_apng_encoder`].
    pub fn finish(mut self) -> ImageResult<()> {
        let Some(writer) = self.writer.take() else {
            return Err(ImageError::Encoding(EncodingError::new(
                ImageFormat::Png.into(),
                "APNG animation contains no frames",
            )));
        };

        // With sequence validation enabled this also verifies that exactly the announced number
        // of frames was written.
        writer.finish().map_err(ImageError::from_png_enc)
    }
}

/// Convert a delay into the `delay_num`/`delay_den` fraction of seconds stored in an `fcTL`
/// chunk. Both numerator and denominator are clamped to the `u16` range of the chunk, very
/// large or very precise delays are approximated.
fn delay_to_fraction(delay: Delay) -> (u16, u16) {
    fn gcd(mut a: u64, mut b: u64) -> u64 {
        while b != 0 {
            (a, b) = (b, a % b);
        }
        a
    }

    let (numer, denom) = delay.numer_denom_ms();
    // The delay is numer/denom milliseconds, that is numer/(denom*1000) seconds.
    let mut num = u64::from(numer);
    let mut den = u64::from(denom) * 1000;

    let divisor = gcd(num, den);
    num /= divisor;
    den /= divisor;

    // Reduce the fraction until both parts fit into a u16. Each step at least halves the larger
    // part so this terminates quickly.
    while num > u64::from(u16::MAX) || den > u64::from(u16::MAX) {
        num = (num + 1) / 2;
        den = (den + 1) / 2;
    }

    (num as u16, den as u16)
}

impl<W: Write> ImageEncoder for PngEncoder<W> {
    /// Write a PNG image with the specified width, height, and color type.
    ///
    /// For color types with 16-bit per channel or larger, the contents of `buf` should be in
    /// native endian. `PngEncoder` will automatically convert to big endian as required by the
    /// underlying PNG format.
    #[track_caller]
    fn write_image(
        self,
        buf: &[u8],
        width: u32,
        height: u32,
        color_type: ExtendedColorType,
    ) -> ImageResult<()> {
        use ExtendedColorType::*;

        let expected_buffer_len = color_type.buffer_size(width, height);
        assert_eq!(
            expected_buffer_len,
            buf.len() as u64,
            "Invalid buffer length: expected {expected_buffer_len} got {} for {width}x{height} image",
            buf.len(),
        );

        // PNG images are big endian. For 16 bit per channel and larger types,
        // the buffer may need to be reordered to big endian per the
        // contract of `write_image`.
        // TODO: assumes equal channel bit depth.
        match color_type {
            L8 | La8 | Rgb8 | Rgba8 => {
                // No reodering necessary for u8
                self.encode_inner(buf, width, height, color_type)
            }
            L16 | La16 | Rgb16 | Rgba16 => {
                // Because the buffer is immutable and the PNG encoder does not
                // yet take Write/Read traits, create a temporary buffer for
                // big endian reordering.
                let mut reordered;
                let buf = if cfg!(target_endian = "little") {
                    reordered = vec_try_with_capacity(buf.len())?;
                    reordered.extend(buf.as_chunks::<2>().0.iter().flat_map(|le| [le[1], le[0]]));
                    &reordered
                } else {
                    buf
                };
                self.encode_inner(buf, width, height, color_type)
            }
            _ => Err(ImageError::Unsupported(
                UnsupportedError::from_format_and_kind(
                    ImageFormat::Png.into(),
                    UnsupportedErrorKind::Color(color_type),
                ),
            )),
        }
    }

    fn set_icc_profile(&mut self, icc_profile: Vec<u8>) -> Result<(), UnsupportedError> {
        self.icc_profile = icc_profile;
        Ok(())
    }

    fn set_exif_metadata(&mut self, exif: Vec<u8>) -> Result<(), UnsupportedError> {
        self.exif_metadata = exif;
        Ok(())
    }

    fn set_xmp_metadata(&mut self, xmp: Vec<u8>) -> Result<(), UnsupportedError> {
        self.xmp_metadata = Some(String::from_utf8(xmp).map_err(|_| {
            UnsupportedError::from_format_and_kind(
                ImageFormat::Png.into(),
                UnsupportedErrorKind::GenericFeature("XMP metadata is not valid UTF-8".to_string()),
            )
        })?);
        Ok(())
    }

    fn make_compatible_img(
        &self,
        _: crate::io::encoder::MethodSealedToImage,
        img: &DynamicImage,
    ) -> Option<DynamicImage> {
        use ColorType::*;
        match img.color() {
            L32F => Some(img.to_luma16().into()),
            La32F => Some(img.to_luma_alpha16().into()),
            Rgb32F => Some(img.to_rgb16().into()),
            Rgba32F => Some(img.to_rgba16().into()),
            L8 | La8 | Rgb8 | Rgba8 | L16 | La16 | Rgb16 | Rgba16 => None,
        }
    }
}

impl ImageError {
    fn from_png(err: png::DecodingError) -> ImageError {
        use png::DecodingError::*;
        match err {
            IoError(err) => ImageError::IoError(err),
            // The input image was not a valid PNG.
            err @ Format(_) => {
                ImageError::Decoding(DecodingError::new(ImageFormat::Png.into(), err))
            }
            // Other is used when:
            // - The decoder is polled for more animation frames despite being done (or not being animated
            //   in the first place).
            // - The output buffer does not have the required size.
            err @ Parameter(_) => ImageError::Parameter(ParameterError::from_kind(
                ParameterErrorKind::Generic(err.to_string()),
            )),
            LimitsExceeded => {
                ImageError::Limits(LimitError::from_kind(LimitErrorKind::InsufficientMemory))
            }
        }
    }
    fn from_png_enc(err: png::EncodingError) -> ImageError {
        use png::EncodingError::*;
        match err {
            IoError(error) => ImageError::IoError(error),
            LimitsExceeded => {
                ImageError::Limits(LimitError::from_kind(LimitErrorKind::InsufficientMemory))
            }
            _ => ImageError::Encoding(EncodingError::new(ImageFormat::Png.into(), Box::new(err))),
        }
    }
}

fn copy_pixel_bytes(bytes: &mut [u8], layout: &ImageLayout, from: &[u8], region: &Rect) {
    let bpp = usize::from(layout.color.bytes_per_pixel());

    let bytes_per_row = layout.width as usize * bpp;
    let bytes_per_copy = region.width as usize * bpp;

    let start = region.x as usize * bpp + region.y as usize * bytes_per_row;
    let from = &from[..region.height as usize * bytes_per_copy];

    for (target, src) in bytes[start..]
        .chunks_exact_mut(bytes_per_row)
        .zip(from.chunks_exact(bytes_per_copy))
    {
        target[..bytes_per_copy].copy_from_slice(src);
    }
}

fn clear_pixel_region(bytes: &mut [u8], layout: &ImageLayout, region: &Rect) {
    let bpp = usize::from(layout.color.bytes_per_pixel());
    let bytes_per_row = layout.width as usize * bpp;
    let bytes_per_clear = region.width as usize * bpp;
    let row_start = region.y as usize * bytes_per_row;
    let column = region.x as usize * bpp;

    for row in bytes[row_start..]
        .chunks_exact_mut(bytes_per_row)
        .take(region.height as usize)
    {
        row[column..][..bytes_per_clear].fill(0);
    }
}

fn blend_pixel_bytes(bytes: &mut [u8], layout: &ImageLayout, from: &[u8], region: &Rect) {
    fn inner<P: crate::Pixel>(bytes: &mut [u8], region: &[u8])
    where
        P::Subpixel: bytemuck::Pod,
    {
        let target = bytemuck::cast_slice_mut::<_, P::Subpixel>(bytes);
        let source = bytemuck::cast_slice::<_, P::Subpixel>(region);

        for (target, source) in target
            .chunks_exact_mut(usize::from(P::CHANNEL_COUNT))
            .zip(source.chunks_exact(usize::from(P::CHANNEL_COUNT)))
        {
            P::from_slice_mut(target).blend(P::from_slice(source));
        }
    }

    let row_transformer = match layout.color {
        ColorType::L8 => inner::<Luma<u8>>,
        ColorType::La8 => inner::<LumaA<u8>>,
        ColorType::Rgb8 => inner::<Rgb<u8>>,
        ColorType::Rgba8 => inner::<Rgba<u8>>,
        ColorType::L16 => inner::<Luma<u16>>,
        ColorType::La16 => inner::<LumaA<u16>>,
        ColorType::Rgb16 => inner::<Rgb<u16>>,
        ColorType::Rgba16 => inner::<Rgba<u16>>,
        ColorType::L32F | ColorType::La32F | ColorType::Rgb32F | ColorType::Rgba32F => {
            unreachable!("No floating point formats in PNG")
        }
    };

    let bpp = usize::from(layout.color.bytes_per_pixel());

    let bytes_per_row = layout.width as usize * bpp;
    let bytes_per_copy = region.width as usize * bpp;

    let start = region.x as usize * bpp + region.y as usize * bytes_per_row;
    let from = &from[..region.height as usize * bytes_per_copy];

    for (target, src) in bytes[start..]
        .chunks_exact_mut(bytes_per_row)
        .zip(from.chunks_exact(bytes_per_copy))
    {
        row_transformer(&mut target[..bytes_per_copy], src);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::io::free_functions::decoder_to_vec;
    use std::io::{BufReader, Cursor, Read};

    #[test]
    fn ensure_no_decoder_off_by_one() {
        let mut dec = PngDecoder::new(BufReader::new(
            std::fs::File::open("tests/images/png/bugfixes/debug_triangle_corners_widescreen.png")
                .unwrap(),
        ));

        let layout = dec
            .prepare_image()
            .expect("Unable to read PNG file (does it exist?)");

        assert_eq![(2000, 1000), layout.layout.dimensions()];

        assert_eq![
            ColorType::Rgb8,
            layout.layout.color,
            "Image MUST have the Rgb8 format"
        ];

        let (data, _) = decoder_to_vec(&mut dec).expect("Unable to read file");

        let correct_bytes = data
            .bytes()
            .map(|x| x.expect("Unable to read byte"))
            .collect::<Vec<u8>>();

        assert_eq![6_000_000, correct_bytes.len()];
    }

    #[test]
    fn underlying_error() {
        use std::error::Error;

        let mut not_png =
            std::fs::read("tests/images/png/bugfixes/debug_triangle_corners_widescreen.png")
                .unwrap();
        not_png[0] = 0;

        let mut decoder = PngDecoder::new(Cursor::new(&not_png));
        let error = decoder.prepare_image().err().unwrap();

        let _ = error
            .source()
            .unwrap()
            .downcast_ref::<png::DecodingError>()
            .expect("Caused by a png error");
    }

    #[test]
    fn encode_bad_color_type() {
        // regression test for issues #1663 and #2787
        let image = DynamicImage::new_rgb32f(1, 1);
        let mut target = Cursor::new(vec![]);
        assert!(image.write_to(&mut target, ImageFormat::Png).is_ok());
    }

    #[test]
    fn roundtrip_xmp() {
        let img = [255u8, 0, 0, 0, 255, 0, 0, 0, 255];
        let xmp = b"<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"><rdf:RDF></rdf:RDF></x:xmpmeta>".to_vec();

        let mut encoded = Vec::new();
        {
            let mut encoder = PngEncoder::new(&mut encoded);
            encoder.set_xmp_metadata(xmp.clone()).unwrap();
            encoder
                .write_image(&img, 3, 1, ExtendedColorType::Rgb8)
                .expect("Could not encode image");
        }

        let mut decoder = PngDecoder::new(Cursor::new(&encoded));
        let _ = decoder.prepare_image().unwrap();
        let decoded_xmp = decoder
            .xmp_metadata()
            .expect("Error decoding XMP")
            .expect("XMP is empty");
        assert_eq!(xmp, decoded_xmp);
    }

    #[test]
    fn is_apng() {
        let file = BufReader::new(std::fs::File::open("tests/images/png/apng/ball.png").unwrap());
        let is_apng = PngDecoder::new(file).is_apng().unwrap();
        assert!(is_apng);
    }

    #[test]
    fn gamma() {
        let file = BufReader::new(std::fs::File::open("tests/images/png/apng/ball.png").unwrap());
        let gamma = PngDecoder::new(file).gamma_value().unwrap();
        assert_eq!(gamma, None);
    }

    #[test]
    fn apng_16_bits_per_channel() {
        let file = BufReader::new(std::fs::File::open("tests/images/png/apng/rgba16.png").unwrap());
        let decoder = PngDecoder::new(file).apng().unwrap();
        let reader = crate::ImageReader::from_decoder(Box::new(decoder));
        let frames = reader.into_frames().collect_frames().unwrap();
        assert_eq!(frames.len(), 3);

        let colors = [
            *frames[0].buffer().get_pixel(0, 0),
            *frames[1].buffer().get_pixel(0, 0),
            *frames[2].buffer().get_pixel(0, 0),
        ];
        assert_eq!(
            colors,
            [
                Rgba([255, 0, 0, 255]),
                Rgba([0, 255, 0, 255]),
                Rgba([0, 0, 255, 255]),
            ]
        );
    }

    fn apng_test_frame(pixel: [u8; 4], width: u32, height: u32, delay_ms: u32) -> ApngFrame {
        let buffer = crate::RgbaImage::from_pixel(width, height, Rgba(pixel));
        ApngFrame::new(Frame::from_parts(
            buffer,
            0,
            0,
            Delay::from_millis(delay_ms),
        ))
    }

    fn collect_apng_frames(encoded: &[u8]) -> Vec<Frame> {
        let decoder = PngDecoder::new(Cursor::new(encoded)).apng().unwrap();
        let reader = crate::ImageReader::from_decoder(Box::new(decoder));
        reader.into_frames().collect_frames().unwrap()
    }

    #[test]
    fn apng_encoder_roundtrip() {
        let frames = vec![
            apng_test_frame([255, 0, 0, 255], 4, 4, 100),
            apng_test_frame([0, 255, 0, 255], 4, 4, 200),
            apng_test_frame([0, 0, 255, 255], 4, 4, 300),
        ];

        let mut encoded = Vec::new();
        {
            let mut encoder = PngEncoder::new(&mut encoded)
                .into_apng_encoder(3, LoopCount::Infinite)
                .unwrap();
            encoder.encode_frames(frames).unwrap();
            encoder.finish().unwrap();
        }

        // The animation chunks must be present: acTL, one fcTL per frame and fdAT for all
        // frames after the first (which is stored as IDAT).
        let has_chunk = |name: &[u8; 4]| encoded.windows(4).any(|w| w == name);
        assert!(has_chunk(b"acTL"));
        assert!(has_chunk(b"fcTL"));
        assert!(has_chunk(b"fdAT"));

        let mut decoder = PngDecoder::new(Cursor::new(&encoded));
        assert!(decoder.is_apng().unwrap());

        let mut decoder = PngDecoder::new(Cursor::new(&encoded)).apng().unwrap();
        match decoder.animation_attributes() {
            Some(DecodedAnimationAttributes {
                loop_count: LoopCount::Infinite,
            }) => {}
            other => panic!("unexpected animation attributes: {other:?}"),
        }

        let frames = collect_apng_frames(&encoded);
        assert_eq!(frames.len(), 3);
        assert_eq!(frames[0].delay(), Delay::from_millis(100));
        assert_eq!(frames[1].delay(), Delay::from_millis(200));
        assert_eq!(frames[2].delay(), Delay::from_millis(300));

        let colors = [
            *frames[0].buffer().get_pixel(0, 0),
            *frames[1].buffer().get_pixel(0, 0),
            *frames[2].buffer().get_pixel(0, 0),
        ];
        assert_eq!(
            colors,
            [
                Rgba([255, 0, 0, 255]),
                Rgba([0, 255, 0, 255]),
                Rgba([0, 0, 255, 255]),
            ]
        );
    }

    #[test]
    fn apng_encoder_finite_loop_count() {
        let mut encoded = Vec::new();
        let mut encoder = PngEncoder::new(&mut encoded)
            .into_apng_encoder(2, LoopCount::Finite(NonZeroU32::new(7).unwrap()))
            .unwrap();
        encoder
            .encode_frame(apng_test_frame([0, 0, 0, 255], 2, 2, 10))
            .unwrap();
        encoder
            .encode_frame(apng_test_frame([255, 255, 255, 255], 2, 2, 10))
            .unwrap();
        encoder.finish().unwrap();

        let mut decoder = PngDecoder::new(Cursor::new(&encoded)).apng().unwrap();
        match decoder.animation_attributes() {
            Some(DecodedAnimationAttributes {
                loop_count: LoopCount::Finite(n),
            }) => assert_eq!(n.get(), 7),
            other => panic!("unexpected animation attributes: {other:?}"),
        }
    }

    #[test]
    fn apng_encoder_blend_and_dispose() {
        // Frame 0: opaque red, disposed to the background before the next frame.
        // Frame 1: fully transparent, blended over the (cleared) canvas.
        let frame0 = ApngFrame::from_parts(
            apng_test_frame([255, 0, 0, 255], 2, 2, 50).into_frame(),
            BlendOp::Source,
            DisposeOp::Background,
        );
        let frame1 = ApngFrame::from_parts(
            apng_test_frame([0, 0, 0, 0], 2, 2, 50).into_frame(),
            BlendOp::Over,
            DisposeOp::None,
        );

        let mut encoded = Vec::new();
        let mut encoder = PngEncoder::new(&mut encoded)
            .into_apng_encoder(2, LoopCount::Infinite)
            .unwrap();
        encoder.encode_frames(vec![frame0, frame1]).unwrap();
        encoder.finish().unwrap();

        let frames = collect_apng_frames(&encoded);
        assert_eq!(frames.len(), 2);
        assert_eq!(*frames[0].buffer().get_pixel(0, 0), Rgba([255, 0, 0, 255]));
        // The red frame was disposed to the background, blending a transparent frame over it
        // leaves a transparent pixel. With DisposeOp::None it would stay red.
        assert_eq!(*frames[1].buffer().get_pixel(0, 0), Rgba([0, 0, 0, 0]));
    }

    #[test]
    fn apng_encoder_blend_over_alpha() {
        // Frame 0: opaque red, kept on the canvas.
        // Frame 1: half transparent blue, blended over it.
        let frame0 = ApngFrame::from_parts(
            apng_test_frame([255, 0, 0, 255], 2, 2, 50).into_frame(),
            BlendOp::Source,
            DisposeOp::None,
        );
        let frame1 = ApngFrame::from_parts(
            apng_test_frame([0, 0, 255, 128], 2, 2, 50).into_frame(),
            BlendOp::Over,
            DisposeOp::None,
        );

        let mut encoded = Vec::new();
        let mut encoder = PngEncoder::new(&mut encoded)
            .into_apng_encoder(2, LoopCount::Infinite)
            .unwrap();
        encoder.encode_frames(vec![frame0, frame1]).unwrap();
        encoder.finish().unwrap();

        let frames = collect_apng_frames(&encoded);
        assert_eq!(frames.len(), 2);
        assert_eq!(
            *frames[1].buffer().get_pixel(0, 0),
            Rgba([127, 0, 128, 255])
        );
    }

    #[test]
    fn apng_encoder_rejects_zero_frames() {
        let mut encoded = Vec::new();
        let err = PngEncoder::new(&mut encoded)
            .into_apng_encoder(0, LoopCount::Infinite)
            .err()
            .unwrap();
        assert!(matches!(err, ImageError::Encoding(_)));
    }

    #[test]
    fn apng_encoder_frame_dimension_mismatch() {
        let mut encoded = Vec::new();
        let mut encoder = PngEncoder::new(&mut encoded)
            .into_apng_encoder(2, LoopCount::Infinite)
            .unwrap();
        encoder
            .encode_frame(apng_test_frame([255, 0, 0, 255], 4, 4, 100))
            .unwrap();

        let err = encoder
            .encode_frame(apng_test_frame([0, 0, 0, 255], 2, 4, 100))
            .err()
            .unwrap();
        assert!(
            matches!(
                err,
                ImageError::Parameter(ref p) if p.kind() == ParameterErrorKind::DimensionMismatch
            ),
            "unexpected error: {err:?}"
        );
    }

    #[test]
    fn apng_encoder_too_many_frames() {
        let mut encoded = Vec::new();
        let mut encoder = PngEncoder::new(&mut encoded)
            .into_apng_encoder(1, LoopCount::Infinite)
            .unwrap();
        encoder
            .encode_frame(apng_test_frame([255, 0, 0, 255], 4, 4, 100))
            .unwrap();

        let err = encoder
            .encode_frame(apng_test_frame([0, 0, 0, 255], 4, 4, 100))
            .err()
            .unwrap();
        assert!(
            matches!(
                err,
                ImageError::Parameter(ref p) if p.kind() == ParameterErrorKind::NoMoreData
            ),
            "unexpected error: {err:?}"
        );
    }

    #[test]
    fn apng_encoder_finish_without_frames() {
        let mut encoded = Vec::new();
        let encoder = PngEncoder::new(&mut encoded)
            .into_apng_encoder(1, LoopCount::Infinite)
            .unwrap();
        let err = encoder.finish().err().unwrap();
        assert!(matches!(err, ImageError::Encoding(_)));
    }

    #[test]
    fn apng_encoder_finish_with_missing_frames() {
        let mut encoded = Vec::new();
        let mut encoder = PngEncoder::new(&mut encoded)
            .into_apng_encoder(2, LoopCount::Infinite)
            .unwrap();
        encoder
            .encode_frame(apng_test_frame([255, 0, 0, 255], 4, 4, 100))
            .unwrap();
        // Only one of the two announced frames was written.
        assert!(encoder.finish().is_err());
    }

    #[test]
    fn apng_encoder_static_png_output_unchanged() {
        // A gradient so that filtering and compression actually have something to do.
        let mut img = Vec::new();
        for y in 0..16u8 {
            for x in 0..16u8 {
                img.extend_from_slice(&[x * 16, y * 16, x ^ y, 255]);
            }
        }

        let encode = || {
            let mut out = Vec::new();
            PngEncoder::new(&mut out)
                .write_image(&img, 16, 16, ExtendedColorType::Rgba8)
                .unwrap();
            out
        };

        let a = encode();
        let b = encode();

        // Encoding is deterministic and contains no animation chunks.
        assert_eq!(a, b);
        assert!(!a.windows(4).any(|w| w == b"acTL"));
        assert!(!a.windows(4).any(|w| w == b"fcTL"));
        assert!(!a.windows(4).any(|w| w == b"fdAT"));

        // The static image still decodes to the original pixels.
        let mut decoder = PngDecoder::new(Cursor::new(&a));
        assert!(!decoder.is_apng().unwrap());
        let (decoded, _) = decoder_to_vec::<u8>(&mut decoder).unwrap();
        assert_eq!(decoded, img);
    }
}
