//! JPEG XL encoding backed by the system libjxl.
//!
//! The encoder is only compiled when the `jxl` feature is enabled *and* the
//! build script found a usable system libjxl. It exposes
//! quality/lossless/effort/thread controls, accepts RGB and RGBA images in 8-
//! and 16-bit samples, and can embed an ICC profile.
//!
//! Output is produced incrementally through libjxl's pull-based
//! `JxlEncoderProcessOutput` API into the caller's [`Write`]: compressed bytes
//! are streamed out of a single fixed-size scratch buffer as they are
//! generated, so the encoder never materialises the whole codestream.
//!
//! Memory is bounded by the caller's input plus one encoding buffer: the
//! caller's pixel slice is handed to libjxl without an additional
//! image-sized copy, libjxl keeps its own internal copy of the input (an
//! unavoidable part of its encoder state with this API), and our own scratch
//! is a single 64 KiB output chunk. Nothing in this module allocates in
//! proportion to the *compressed* size, so a highly compressible large image
//! does not create a second image-sized or larger buffer.
use std::io::Write;

use crate::error::{
    EncodingError, ImageError, ImageFormatHint, ImageResult, ParameterError, ParameterErrorKind,
    UnsupportedError, UnsupportedErrorKind,
};
use crate::io::encoder::ImageEncoder;
use crate::{ExtendedColorType, ImageFormat};

use super::error::{JxlEncodeError, JxlEncodeErrorKind};
use super::ffi;

/// JPEG XL codestream level-5 geometry limits (ISO/IEC 18181).
const MAX_DIMENSION: u32 = 262_144;
const MAX_AREA: u64 = 268_435_456;
/// One fixed-size scratch chunk that compressed output is streamed through.
const OUTPUT_CHUNK: usize = 64 * 1024;

/// Encoding effort, mirroring libjxl's named speed levels.
///
/// Higher effort spends more CPU time for better compression; it never changes
/// image correctness.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum JxlEffort {
    /// `lightning`
    Lightning = 1,
    /// `thunder`
    Thunder = 2,
    /// `falcon`
    Falcon = 3,
    /// `cheetah`
    Cheetah = 4,
    /// `hare`
    Hare = 5,
    /// `wombat`
    Wombat = 6,
    /// `squirrel` (libjxl default)
    Squirrel = 7,
    /// `kitten`
    Kitten = 8,
    /// `tortoise`
    Tortoise = 9,
}

impl JxlEffort {
    /// Build an effort from its integer level (valid range `1..=9`).
    ///
    /// # Errors
    ///
    /// Returns [`JxlEncodeErrorKind::InvalidParameter`] for levels outside
    /// `1..=9`.
    pub fn from_level(level: u8) -> Result<Self, JxlEncodeError> {
        match level {
            1 => Ok(JxlEffort::Lightning),
            2 => Ok(JxlEffort::Thunder),
            3 => Ok(JxlEffort::Falcon),
            4 => Ok(JxlEffort::Cheetah),
            5 => Ok(JxlEffort::Hare),
            6 => Ok(JxlEffort::Wombat),
            7 => Ok(JxlEffort::Squirrel),
            8 => Ok(JxlEffort::Kitten),
            9 => Ok(JxlEffort::Tortoise),
            _ => Err(JxlEncodeError::new(JxlEncodeErrorKind::InvalidParameter)),
        }
    }
}

/// Number of worker threads used for encoding.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum JxlThreads {
    /// Single-threaded encoding (no `libjxl_threads` runner).
    One,
    /// Explicit worker-thread count (must be non-zero).
    Exact(usize),
    /// Let libjxl choose based on the available hardware concurrency.
    Auto,
}

/// Pixel layout resolved from an [`ExtendedColorType`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct PixelLayout {
    /// libjxl channel count (3 = RGB, 4 = RGBA).
    channels: u32,
    /// Bytes per sample (1 or 2).
    sample_bytes: usize,
    /// libjxl data type.
    data_type: ffi::JxlDataType,
}

impl PixelLayout {
    fn resolve(color: ExtendedColorType) -> ImageResult<Self> {
        use ffi::JxlDataType;
        let (channels, sample_bytes, data_type) = match color {
            ExtendedColorType::Rgb8 => (3, 1, JxlDataType::Uint8),
            ExtendedColorType::Rgba8 => (4, 1, JxlDataType::Uint8),
            ExtendedColorType::Rgb16 => (3, 2, JxlDataType::Uint16),
            ExtendedColorType::Rgba16 => (4, 2, JxlDataType::Uint16),
            // Grayscale and the sub-byte / floating-point layouts are not part
            // of the encoder's supported input set.
            _ => {
                return Err(ImageError::Unsupported(
                    UnsupportedError::from_format_and_kind(
                        ImageFormat::Jxl.into(),
                        UnsupportedErrorKind::Color(color),
                    ),
                ));
            }
        };
        Ok(PixelLayout {
            channels,
            sample_bytes,
            data_type,
        })
    }
}

/// JPEG XL encoder.
///
/// Construct with [`JxlEncoder::new`] and optionally tune it through the
/// builder methods before passing it to
/// [`ImageEncoder::write_image`](crate::ImageEncoder::write_image) or
/// [`DynamicImage::write_with_encoder`](crate::DynamicImage::write_with_encoder).
///
/// # Feature gate
///
/// This type exists only with the `jxl` feature enabled. Without it, the
/// `JxlEncoderUnsupported` placeholder (or the generic encoding entry points)
/// returns an explicit `Unsupported` error instead of silently re-routing to
/// another format.
pub struct JxlEncoder<W> {
    writer: W,
    quality: f32,
    lossless: bool,
    effort: JxlEffort,
    threads: JxlThreads,
    icc_profile: Option<Vec<u8>>,
}

impl<W: Write> JxlEncoder<W> {
    /// Create an encoder writing to `w` with default settings: quality 90
    /// (lossy), effort [`JxlEffort::Squirrel`] and automatic threading.
    #[must_use]
    pub fn new(writer: W) -> Self {
        JxlEncoder {
            writer,
            quality: 90.0,
            lossless: false,
            effort: JxlEffort::Squirrel,
            threads: JxlThreads::Auto,
            icc_profile: None,
        }
    }

    /// Set lossy quality on the JPEG-style `0..=100` scale.
    ///
    /// A value of `100` selects lossless encoding; lower values are mapped to
    /// libjxl's butteraugli distance through `JxlEncoderDistanceFromQuality`.
    ///
    /// # Errors
    ///
    /// Returns [`ImageError::Parameter`] with
    /// [`ParameterErrorKind::Generic`] if `quality` is not finite or outside
    /// `0..=100`.
    pub fn set_quality(&mut self, quality: f32) -> ImageResult<()> {
        if !(0.0..=100.0).contains(&quality) || !quality.is_finite() {
            return Err(parameter_error(
                "JPEG XL quality must be a finite number in the range 0..=100",
            ));
        }
        self.quality = quality;
        self.lossless = (quality - 100.0).abs() < f32::EPSILON;
        Ok(())
    }

    /// Explicitly select lossless (`true`) or lossy (`false`) encoding.
    ///
    /// Switching back to lossy keeps the previously configured quality value.
    pub fn set_lossless(&mut self, lossless: bool) {
        self.lossless = lossless;
    }

    /// Set the encoding effort/speed level.
    pub fn set_effort(&mut self, effort: JxlEffort) {
        self.effort = effort;
    }

    /// Configure the worker thread count.
    ///
    /// `Exact(n)` requests `n` worker threads; the count is capped at a
    /// sane upper bound so an accidental huge value cannot make libjxl try to
    /// spawn millions of threads.
    ///
    /// # Errors
    ///
    /// Returns [`ImageError::Parameter`] if a zero [`JxlThreads::Exact`] thread
    /// count is supplied.
    pub fn set_threads(&mut self, threads: JxlThreads) -> ImageResult<()> {
        if let JxlThreads::Exact(0) = threads {
            return Err(parameter_error(
                "JPEG XL thread count must be greater than zero",
            ));
        }
        self.threads = threads;
        Ok(())
    }

    /// Embed an ICC color profile in the JPEG XL codestream metadata.
    ///
    /// The profile is validated by libjxl when encoding starts; an invalid
    /// profile then produces a distinguishable
    /// [`JxlEncodeErrorKind::BadIccProfile`] error.
    pub fn set_icc_profile_bytes(&mut self, icc_profile: Vec<u8>) {
        self.icc_profile = Some(icc_profile);
    }

    /// Encode `data` with the given geometry and color type, streaming the
    /// compressed bytes into the wrapped writer.
    fn encode(
        &mut self,
        data: &[u8],
        width: u32,
        height: u32,
        color: ExtendedColorType,
    ) -> ImageResult<()> {
        // --- Geometry validation -------------------------------------------
        if width == 0 || height == 0 {
            return Err(jxl_error(JxlEncodeErrorKind::InvalidDimensions));
        }
        if width > MAX_DIMENSION || height > MAX_DIMENSION {
            return Err(jxl_error(JxlEncodeErrorKind::ImageTooLarge));
        }
        let area = u64::from(width).saturating_mul(u64::from(height));
        if area > MAX_AREA {
            return Err(jxl_error(JxlEncodeErrorKind::ImageTooLarge));
        }

        let layout = PixelLayout::resolve(color)?;
        let bytes_per_pixel = (layout.channels as usize) * layout.sample_bytes;
        let expected_len = (area as usize)
            .checked_mul(bytes_per_pixel)
            .ok_or_else(|| jxl_error(JxlEncodeErrorKind::ImageTooLarge))?;
        if data.len() != expected_len {
            return Err(ImageError::Parameter(ParameterError::from_kind(
                ParameterErrorKind::BufferSizeMismatch,
            )));
        }

        // libjxl reads 16-bit samples in the byte order named by the pixel
        // format; we declare `JxlEndianness::Native` below and pass the
        // caller's buffer through unchanged, which is native order by
        // definition (see `ImageEncoder::write_image`). No copy is made.
        let pixel_data: &[u8] = data;

        // --- Encoder lifecycle (all fallible paths destroy the encoder) ----
        let enc = unsafe { ffi::JxlEncoderCreate(std::ptr::null()) };
        if enc.is_null() {
            return Err(jxl_error(JxlEncodeErrorKind::Backend(
                ffi::JxlEncoderError::Oom,
            )));
        }
        let result = self.run_encoder(enc, pixel_data, width, height, layout);
        unsafe { ffi::JxlEncoderDestroy(enc) };
        result?;

        self.writer.flush()?;
        Ok(())
    }

    fn run_encoder(
        &mut self,
        enc: *mut ffi::JxlEncoder,
        data: &[u8],
        width: u32,
        height: u32,
        layout: PixelLayout,
    ) -> ImageResult<()> {
        unsafe {
            // Threading: NULL runner means single-threaded.
            let mut runner_holder = ThreadRunner::default();
            match self.threads {
                JxlThreads::One => {}
                JxlThreads::Auto => {
                    runner_holder.create(ffi::JxlThreadParallelRunnerDefaultNumWorkerThreads())?;
                }
                JxlThreads::Exact(n) => runner_holder.create(n)?,
            }
            if !runner_holder.ptr.is_null() {
                check_status(
                    ffi::JxlEncoderSetParallelRunner(
                        enc,
                        Some(ffi::JxlThreadParallelRunner),
                        runner_holder.ptr,
                    ),
                    enc,
                    false,
                )?;
            }

            // Basic image info. MaybeUninit is required because the structs
            // contain C enums whose declared variants do not include zero, so
            // `mem::zeroed` would be UB before the libjxl initializer runs.
            let mut info = std::mem::MaybeUninit::<ffi::JxlBasicInfo>::uninit();
            ffi::JxlEncoderInitBasicInfo(info.as_mut_ptr());
            let mut info = info.assume_init();
            info.xsize = width;
            info.ysize = height;
            let has_alpha = layout.channels == 4;
            info.num_color_channels = 3;
            info.bits_per_sample = (layout.sample_bytes as u32) * 8;
            info.exponent_bits_per_sample = 0;
            info.alpha_bits = if has_alpha {
                (layout.sample_bytes as u32) * 8
            } else {
                0
            };
            info.alpha_exponent_bits = 0;
            info.num_extra_channels = u32::from(has_alpha);
            info.intrinsic_xsize = width;
            info.intrinsic_ysize = height;
            // ICC profiles are stored in the codestream metadata header, so no
            // BMFF container is required; emit the bare JXL codestream.
            info.have_container = ffi::JXL_FALSE;
            // Lossless encoding requires the original color profile to be
            // retained; libjxl rejects the configuration otherwise.
            info.uses_original_profile = if self.lossless {
                ffi::JXL_TRUE
            } else {
                ffi::JXL_FALSE
            };
            check_status(ffi::JxlEncoderSetBasicInfo(enc, &info), enc, false)?;

            // Alpha extra-channel metadata.
            if has_alpha {
                let mut alpha = std::mem::MaybeUninit::<ffi::JxlExtraChannelInfo>::uninit();
                ffi::JxlEncoderInitExtraChannelInfo(
                    ffi::JxlExtraChannelType::Alpha,
                    alpha.as_mut_ptr(),
                );
                let mut alpha = alpha.assume_init();
                alpha.bits_per_sample = (layout.sample_bytes as u32) * 8;
                alpha.exponent_bits_per_sample = 0;
                check_status(
                    ffi::JxlEncoderSetExtraChannelInfo(enc, 0, &alpha),
                    enc,
                    false,
                )?;
            }

            // Color profile: explicit ICC or nonlinear sRGB.
            match &self.icc_profile {
                Some(icc) => {
                    check_status(
                        ffi::JxlEncoderSetICCProfile(enc, icc.as_ptr(), icc.len()),
                        enc,
                        true,
                    )?;
                }
                None => {
                    let mut color = std::mem::MaybeUninit::<ffi::JxlColorEncoding>::uninit();
                    ffi::JxlColorEncodingSetToSRGB(color.as_mut_ptr(), ffi::JXL_FALSE);
                    let color = color.assume_init();
                    check_status(ffi::JxlEncoderSetColorEncoding(enc, &color), enc, false)?;
                }
            }

            // Frame settings.
            let options = ffi::JxlEncoderFrameSettingsCreate(enc, std::ptr::null());
            if options.is_null() {
                return Err(jxl_error(JxlEncodeErrorKind::Backend(
                    ffi::JxlEncoderError::Oom,
                )));
            }
            check_status(
                ffi::JxlEncoderFrameSettingsSetOption(
                    options,
                    ffi::JxlEncoderFrameSettingId::Effort,
                    i64::from(self.effort as u8),
                ),
                enc,
                false,
            )?;
            // Drop the streaming-input switch: `JXL_ENC_FRAME_SETTING_BUFFERING`
            // only affects the chunked-input API, not the whole-buffer
            // `JxlEncoderAddImageFrame` path used here, so it did nothing and
            // its comment was misleading.
            if self.lossless {
                check_status(
                    ffi::JxlEncoderSetFrameLossless(options, ffi::JXL_TRUE),
                    enc,
                    false,
                )?;
            } else {
                let distance = ffi::JxlEncoderDistanceFromQuality(self.quality);
                check_status(
                    ffi::JxlEncoderSetFrameDistance(options, distance),
                    enc,
                    false,
                )?;
            }

            let pixel_format = ffi::JxlPixelFormat {
                num_channels: layout.channels,
                data_type: layout.data_type,
                endianness: ffi::JxlEndianness::Native,
                align: 0,
            };

            check_status(
                ffi::JxlEncoderAddImageFrame(
                    options,
                    &pixel_format,
                    data.as_ptr().cast(),
                    data.len(),
                ),
                enc,
                false,
            )?;
            ffi::JxlEncoderCloseInput(enc);

            // Pull compressed bytes out in fixed-size chunks straight into the
            // caller's writer. libjxl guarantees forward progress when given
            // >= 32 bytes, so this loop always terminates; the only allocation
            // is the one fixed scratch buffer, independent of image size.
            let mut scratch = crate::utils::vec_try_with_capacity::<u8>(OUTPUT_CHUNK)
                .map_err(ImageError::from)?;
            scratch.resize(OUTPUT_CHUNK, 0);
            loop {
                let mut next_out = scratch.as_mut_ptr();
                let mut avail_out = scratch.len();
                let status = ffi::JxlEncoderProcessOutput(enc, &mut next_out, &mut avail_out);
                let written = scratch.len() - avail_out;
                if written > 0 {
                    self.writer.write_all(&scratch[..written])?;
                }
                match status {
                    ffi::JxlEncoderStatus::Success => break,
                    ffi::JxlEncoderStatus::NeedMoreOutput => continue,
                    ffi::JxlEncoderStatus::Error => {
                        let code = ffi::JxlEncoderGetError(enc);
                        return Err(jxl_backend_error(code, false));
                    }
                }
            }
            Ok(())
        }
    }
}

impl<W: Write> ImageEncoder for JxlEncoder<W> {
    #[track_caller]
    fn write_image(
        mut self,
        buf: &[u8],
        width: u32,
        height: u32,
        color_type: ExtendedColorType,
    ) -> ImageResult<()> {
        let expected_buffer_len = color_type.buffer_size(width, height);
        assert_eq!(
            expected_buffer_len,
            buf.len() as u64,
            "Invalid buffer length: expected {expected_buffer_len} got {} for {width}x{height} image",
            buf.len(),
        );
        self.encode(buf, width, height, color_type)
    }

    fn set_icc_profile(&mut self, icc_profile: Vec<u8>) -> Result<(), UnsupportedError> {
        self.icc_profile = Some(icc_profile);
        Ok(())
    }

    fn make_compatible_img(
        &self,
        _: crate::io::encoder::MethodSealedToImage,
        img: &crate::DynamicImage,
    ) -> Option<crate::DynamicImage> {
        use crate::ColorType::*;
        // Keep 8/16-bit RGB(A) untouched (the encoder's native inputs), and
        // convert everything else into one of them rather than failing.
        // 16-bit precision is preserved for grayscale/float sources where
        // possible instead of being truncated to 8-bit.
        match img.color() {
            Rgb8 | Rgba8 | Rgb16 | Rgba16 => None,
            L8 | La8 => Some(img.to_rgb8().into()),
            L16 | La16 | L32F | La32F | Rgb32F | Rgba32F => Some(img.to_rgb16().into()),
        }
    }
}

/// RAII holder for a `libjxl_threads` parallel runner.
#[derive(Default)]
struct ThreadRunner {
    ptr: *mut std::os::raw::c_void,
}

/// Upper bound on worker threads requested from `libjxl_threads`. A worker
/// thread costs real memory in libjxl, so an accidental `Exact(usize::MAX)`
/// must not be turned into a spawn attempt that can only fail or exhaust the
/// process.
const MAX_WORKERS: usize = 1024;

impl ThreadRunner {
    fn create(&mut self, num_workers: usize) -> ImageResult<()> {
        // Zero workers is rejected by libjxl; clamp to one. Large values are
        // capped so a nonsense request degrades to a bounded thread pool.
        let workers = num_workers.clamp(1, MAX_WORKERS);
        let ptr = unsafe { ffi::JxlThreadParallelRunnerCreate(std::ptr::null(), workers) };
        if ptr.is_null() {
            return Err(jxl_error(JxlEncodeErrorKind::Backend(
                ffi::JxlEncoderError::Oom,
            )));
        }
        self.ptr = ptr;
        Ok(())
    }
}

impl Drop for ThreadRunner {
    fn drop(&mut self) {
        if !self.ptr.is_null() {
            unsafe { ffi::JxlThreadParallelRunnerDestroy(self.ptr) };
        }
    }
}

// ThreadRunner's pointer is handed to libjxl, which only uses it during the
// synchronous encode call on this thread.
unsafe impl Send for ThreadRunner {}

/// Translate a libjxl status into the crate's error taxonomy.
fn check_status(
    status: ffi::JxlEncoderStatus,
    enc: *mut ffi::JxlEncoder,
    icc_context: bool,
) -> ImageResult<()> {
    match status {
        ffi::JxlEncoderStatus::Success => Ok(()),
        ffi::JxlEncoderStatus::NeedMoreOutput => {
            // Only `JxlEncoderProcessOutput` returns this, and it is handled in
            // the streaming loop, not passed here.
            Err(jxl_error(JxlEncodeErrorKind::Backend(
                ffi::JxlEncoderError::Generic,
            )))
        }
        ffi::JxlEncoderStatus::Error => {
            let code = unsafe { ffi::JxlEncoderGetError(enc) };
            Err(jxl_backend_error(code, icc_context))
        }
    }
}

fn jxl_backend_error(code: ffi::JxlEncoderError, icc_context: bool) -> ImageError {
    ImageError::Encoding(EncodingError::new(
        ImageFormatHint::Exact(ImageFormat::Jxl),
        JxlEncodeError::from_backend(code, icc_context),
    ))
}

fn jxl_error(kind: JxlEncodeErrorKind) -> ImageError {
    ImageError::Encoding(EncodingError::new(
        ImageFormatHint::Exact(ImageFormat::Jxl),
        JxlEncodeError::new(kind),
    ))
}

fn parameter_error(message: &str) -> ImageError {
    ImageError::Parameter(ParameterError::from_kind(ParameterErrorKind::Generic(
        message.to_owned(),
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn jxl_kind(err: &ImageError) -> JxlEncodeErrorKind {
        match err {
            ImageError::Encoding(e) => {
                let src = std::error::Error::source(e).expect("JxlEncodeError source present");
                src.downcast_ref::<JxlEncodeError>()
                    .expect("source is a JxlEncodeError")
                    .kind()
            }
            other => panic!("expected Encoding error, got {other:?}"),
        }
    }

    #[test]
    fn oversized_side_is_rejected_before_backend() {
        let mut enc = JxlEncoder::new(Vec::new());
        let err = enc
            .encode(&[], MAX_DIMENSION + 1, 2, ExtendedColorType::Rgb8)
            .unwrap_err();
        assert_eq!(jxl_kind(&err), JxlEncodeErrorKind::ImageTooLarge);
    }

    #[test]
    fn oversized_area_is_rejected_before_backend() {
        let mut enc = JxlEncoder::new(Vec::new());
        let err = enc
            .encode(&[], MAX_DIMENSION, MAX_DIMENSION, ExtendedColorType::Rgb8)
            .unwrap_err();
        assert_eq!(jxl_kind(&err), JxlEncodeErrorKind::ImageTooLarge);
    }

    #[test]
    fn zero_dimensions_are_rejected() {
        let mut enc = JxlEncoder::new(Vec::new());
        let err = enc.encode(&[], 0, 4, ExtendedColorType::Rgb8).unwrap_err();
        assert_eq!(jxl_kind(&err), JxlEncodeErrorKind::InvalidDimensions);
    }

    #[test]
    fn buffer_length_mismatch_is_a_parameter_error() {
        let mut enc = JxlEncoder::new(Vec::new());
        let err = enc
            .encode(&[0u8, 1, 2], 4, 4, ExtendedColorType::Rgb8)
            .unwrap_err();
        assert!(matches!(err, ImageError::Parameter(_)), "got {err:?}");
    }

    #[test]
    fn luma_is_an_unsupported_color_type() {
        let mut enc = JxlEncoder::new(Vec::new());
        let err = enc
            .encode(&[0u8; 16], 4, 4, ExtendedColorType::L8)
            .unwrap_err();
        match err {
            ImageError::Unsupported(u) => assert!(matches!(
                u.kind(),
                UnsupportedErrorKind::Color(ExtendedColorType::L8)
            )),
            other => panic!("expected Unsupported, got {other:?}"),
        }
    }
}
