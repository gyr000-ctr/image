//! JPEG XL tests for builds **without** usable JPEG XL encoding: either the
//! `jxl` feature is disabled, or it is enabled but the build script could not
//! find a usable system libjxl.
//!
//! The crate must remain fully usable in both cases: the placeholder encoder
//! exists, constructs, and rejects every operation with a distinguishable
//! `Unsupported` error rather than silently producing PNG.
#![cfg(not(all(feature = "jxl", jxl_available)))]

extern crate image;

use image::codecs::jxl::JxlEncoderUnsupported;
use image::error::{ImageError, ImageFormatHint, UnsupportedErrorKind};
use image::{ExtendedColorType, ImageEncoder, ImageFormat};

#[test]
fn encoding_is_explicitly_rejected_without_dependency() {
    let data = vec![10u8; 4 * 4 * 3];
    let encoder = JxlEncoderUnsupported::<Vec<u8>>::new(Vec::new());
    let err = encoder
        .write_image(&data, 4, 4, ExtendedColorType::Rgb8)
        .unwrap_err();
    match err {
        ImageError::Unsupported(u) => {
            assert_eq!(
                u.format_hint(),
                ImageFormatHint::Exact(ImageFormat::Jxl),
                "error must be attributed to the JXL format"
            );
            match u.kind() {
                UnsupportedErrorKind::GenericFeature(message) => {
                    assert!(message.contains("JPEG XL"), "got {message}");
                    // The message must say *why*: feature disabled versus
                    // feature enabled but library absent.
                    assert!(
                        message.contains("feature") || message.contains("libjxl"),
                        "message should explain the cause: {message}"
                    );
                }
                other => panic!("expected GenericFeature, got {other:?}"),
            }
        }
        other => panic!("expected Unsupported error, got {other:?}"),
    }
}

#[test]
fn icc_profile_rejected_without_dependency() {
    let mut encoder = JxlEncoderUnsupported::<Vec<u8>>::new(Vec::new());
    let err = encoder.set_icc_profile(vec![1, 2, 3]).unwrap_err();
    assert_eq!(err.format_hint(), ImageFormatHint::Exact(ImageFormat::Jxl));
}

#[test]
fn writer_is_returned_unchanged() {
    let encoder = JxlEncoderUnsupported::new(42u8);
    assert_eq!(encoder.into_inner(), 42);
}

#[test]
fn generic_entry_point_reports_unsupported_not_png() {
    let data = vec![0u8; 4 * 4 * 3];
    let mut writer = std::io::Cursor::new(Vec::new());
    let err = image::write_buffer_with_format(
        &mut writer,
        &data,
        4,
        4,
        ExtendedColorType::Rgb8,
        ImageFormat::Jxl,
    )
    .unwrap_err();
    assert!(matches!(err, ImageError::Unsupported(_)));
    // Nothing was written, and certainly no PNG.
    assert!(writer.get_ref().is_empty());
}

#[test]
fn reading_jxl_is_unsupported_too() {
    // There is no JXL decoder in either build configuration; an explicit read
    // request must fail with Unsupported rather than guessing another format.
    let data = b"\xff\x0abogus jxl-ish bytes";
    let reader =
        image::ImageReaderOptions::with_format(std::io::Cursor::new(data), ImageFormat::Jxl);
    let err = reader.decode().unwrap_err();
    assert!(matches!(err, ImageError::Unsupported(_)));
}

#[test]
fn format_metadata_still_recognizes_jxl() {
    assert_eq!(ImageFormat::from_extension("jxl"), Some(ImageFormat::Jxl));
    assert_eq!(
        ImageFormat::from_mime_type("image/jxl"),
        Some(ImageFormat::Jxl)
    );
    assert!(!ImageFormat::Jxl.writing_enabled());
    assert!(!ImageFormat::Jxl.reading_enabled());
}
