//! Error type produced by the libjxl-backed encoder.
//!
//! The high-level [`ImageError`] mapping in [`super::encoder`] keeps the
//! existing error categories for the other formats untouched, while JXL
//! failures remain distinguishable through this type as the error source and
//! through the [`JxlEncodeErrorKind`] carried within it.

use std::fmt;

use super::ffi::JxlEncoderError;

/// Subclass of a [`JxlEncodeError`].
///
/// Allows callers to distinguish an illegal geometry, an oversized image, an
/// out-of-range parameter, a rejected ICC profile and a generic libjxl
/// failure without matching on human-readable messages.
///
/// Unsupported *color types* are reported through the crate's usual
/// [`ImageError::Unsupported`](crate::ImageError::Unsupported) with an
/// [`UnsupportedErrorKind::Color`](crate::error::UnsupportedErrorKind::Color)
/// payload, and a mismatched pixel buffer through
/// [`ImageError::Parameter`](crate::ImageError::Parameter) with
/// `ParameterErrorKind::BufferSizeMismatch`, matching the other encoders.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[non_exhaustive]
pub enum JxlEncodeErrorKind {
    /// A zero dimension or an otherwise illegal image geometry was supplied.
    InvalidDimensions,
    /// The image exceeds the maximum geometry representable by a JPEG XL
    /// codestream (at most 262144 pixels per side and 268435456 pixels total).
    ImageTooLarge,
    /// An encoding parameter (quality, effort or thread count) is outside its
    /// valid range.
    InvalidParameter,
    /// libjxl rejected the supplied ICC profile.
    BadIccProfile,
    /// The underlying libjxl call failed for any other reason.
    Backend(JxlEncoderError),
}

/// Error returned by the JPEG XL encoder.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct JxlEncodeError {
    kind: JxlEncodeErrorKind,
}

impl JxlEncodeError {
    pub(crate) fn new(kind: JxlEncodeErrorKind) -> Self {
        JxlEncodeError { kind }
    }

    /// The precise kind of the encoding failure.
    #[must_use]
    pub fn kind(&self) -> JxlEncodeErrorKind {
        self.kind
    }

    /// Build an error from a libjxl status error code, mapping the
    /// `BAD_INPUT`/`NOT_SUPPORTED` answers that follow an ICC-profile call to
    /// [`JxlEncodeErrorKind::BadIccProfile`] when `icc_context` is set.
    pub(crate) fn from_backend(code: JxlEncoderError, icc_context: bool) -> Self {
        let kind = match (code, icc_context) {
            (JxlEncoderError::BadInput, true) | (JxlEncoderError::NotSupported, true) => {
                JxlEncodeErrorKind::BadIccProfile
            }
            _ => JxlEncodeErrorKind::Backend(code),
        };
        JxlEncodeError { kind }
    }
}

impl fmt::Display for JxlEncodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.kind {
            JxlEncodeErrorKind::InvalidDimensions => {
                f.write_str("JPEG XL encoding requires non-zero width and height")
            }
            JxlEncodeErrorKind::ImageTooLarge => f.write_str(
                "image dimensions exceed the JPEG XL level-5 limits \
                 (262144 px per side, 268435456 px total)",
            ),
            JxlEncodeErrorKind::InvalidParameter => {
                f.write_str("an invalid JPEG XL encoding parameter was supplied")
            }
            JxlEncodeErrorKind::BadIccProfile => f.write_str("the ICC profile is invalid"),
            JxlEncodeErrorKind::Backend(code) => {
                write!(f, "libjxl encoder failed with error code {code:?}")
            }
        }
    }
}

impl std::error::Error for JxlEncodeError {}
