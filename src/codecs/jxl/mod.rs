//! JPEG XL (JXL) encoding.
//!
//! # Availability
//!
//! JPEG XL encoding is gated behind the `jxl` feature, which links against a
//! system libjxl. There are three relevant build states:
//!
//! - **`jxl` feature off** (the default): the crate builds exactly as before.
//!   Every JXL encoding entry point returns an explicit
//!   [`ImageError::Unsupported`](crate::error::ImageError::Unsupported) through
//!   the `JxlEncoderUnsupported` placeholder.
//! - **`jxl` feature on, libjxl found by the build script**: the `JxlEncoder`
//!   type provides quality/lossless/effort/thread controls, RGB/RGBA in 8- and
//!   16-bit samples and ICC embedding.
//! - **`jxl` feature on, but no usable libjxl on the system**: the build still
//!   succeeds (with a Cargo warning) and falls back to the same
//!   `JxlEncoderUnsupported` placeholder, so `--all-features` builds do not
//!   require the native dependency.
//!
//! In every state, encoding is never silently re-routed to PNG or another
//! format.
//!
//! # Streaming and memory
//!
//! Compressed output is pulled from libjxl in fixed-size chunks and written
//! directly to the caller's [`Write`](std::io::Write), so the codestream is
//! never buffered in full. Input pixels are borrowed, not copied. Peak decoder
//! scratch memory is one 64 KiB output chunk regardless of image size.

#[cfg(all(feature = "jxl", jxl_available))]
mod encoder;
#[cfg(all(feature = "jxl", jxl_available))]
mod error;
#[cfg(all(feature = "jxl", jxl_available))]
mod ffi;

#[cfg(all(feature = "jxl", jxl_available))]
pub use self::encoder::{JxlEffort, JxlEncoder, JxlThreads};
#[cfg(all(feature = "jxl", jxl_available))]
pub use self::error::{JxlEncodeError, JxlEncodeErrorKind};

#[cfg(not(all(feature = "jxl", jxl_available)))]
use crate::error::{
    ImageError, ImageFormatHint, ImageResult, UnsupportedError, UnsupportedErrorKind,
};
#[cfg(not(all(feature = "jxl", jxl_available)))]
use crate::io::encoder::ImageEncoder;
#[cfg(not(all(feature = "jxl", jxl_available)))]
use crate::{ExtendedColorType, ImageFormat};

/// Placeholder encoder used when JPEG XL encoding is unavailable, either
/// because the `jxl` feature is disabled or because no usable system libjxl
/// was found at build time.
///
/// Constructing the type is infallible, but every encoding attempt fails with
/// a distinguishable [`ImageError::Unsupported`] explaining why. This keeps
/// the rest of the crate fully functional on systems without libjxl while
/// making the missing capability explicit.
///
/// When JPEG XL encoding is available this type is not exported; use
/// `JxlEncoder` instead.
#[cfg(not(all(feature = "jxl", jxl_available)))]
pub struct JxlEncoderUnsupported<W> {
    writer: W,
}

#[cfg(not(all(feature = "jxl", jxl_available)))]
impl<W> JxlEncoderUnsupported<W> {
    /// Create the placeholder encoder. No JXL library is loaded.
    pub fn new(writer: W) -> Self {
        JxlEncoderUnsupported { writer }
    }

    /// Consume the encoder and return the wrapped writer unchanged.
    pub fn into_inner(self) -> W {
        self.writer
    }

    #[cfg(feature = "jxl")]
    const REASON: &'static str = "no usable system libjxl was found at build time";
    #[cfg(not(feature = "jxl"))]
    const REASON: &'static str = "the `image` crate was built without the `jxl` feature";

    fn unsupported() -> ImageError {
        ImageError::Unsupported(UnsupportedError::from_format_and_kind(
            ImageFormatHint::Exact(ImageFormat::Jxl),
            UnsupportedErrorKind::GenericFeature(format!(
                "JPEG XL encoding is not available: {}",
                Self::REASON
            )),
        ))
    }
}

#[cfg(not(all(feature = "jxl", jxl_available)))]
impl<W> ImageEncoder for JxlEncoderUnsupported<W> {
    fn write_image(
        self,
        _buf: &[u8],
        _width: u32,
        _height: u32,
        _color_type: ExtendedColorType,
    ) -> ImageResult<()> {
        Err(Self::unsupported())
    }

    fn set_icc_profile(&mut self, _icc_profile: Vec<u8>) -> Result<(), UnsupportedError> {
        Err(UnsupportedError::from_format_and_kind(
            ImageFormatHint::Exact(ImageFormat::Jxl),
            UnsupportedErrorKind::GenericFeature(format!(
                "JPEG XL encoding is not available: {}",
                Self::REASON
            )),
        ))
    }
}
