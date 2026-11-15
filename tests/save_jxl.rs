//! JPEG XL encoding tests, compiled only when the `jxl` feature is enabled
//! *and* the build script found a usable system libjxl.
#![cfg(all(feature = "jxl", jxl_available))]

extern crate image;

use image::codecs::jxl::{JxlEffort, JxlEncodeErrorKind, JxlEncoder, JxlThreads};
use image::error::{ImageError, UnsupportedErrorKind};
use image::{ExtendedColorType, ImageEncoder, ImageFormat};

/// libjxl wraps 16-bit codestreams in the BMFF container, so accept both
/// JPEG XL signatures.
fn has_jxl_signature(out: &[u8]) -> bool {
    out.starts_with(b"\xff\x0a") || out.starts_with(b"\x00\x00\x00\x0cJXL \r\n\x87\n")
}

fn rgb8(w: u32, h: u32) -> Vec<u8> {
    (0..w * h)
        .flat_map(|i| {
            let i = i as u8;
            [i, i.wrapping_mul(2), i.wrapping_add(7)]
        })
        .collect()
}

fn rgba8(w: u32, h: u32) -> Vec<u8> {
    (0..w * h)
        .flat_map(|i| {
            let i = i as u8;
            [i, i.wrapping_mul(2), i.wrapping_add(7), i.wrapping_add(128)]
        })
        .collect()
}

fn rgb16(w: u32, h: u32) -> Vec<u8> {
    (0..w * h)
        .flat_map(|i| {
            let v = (i as u16).wrapping_mul(7919);
            [v, v ^ 0x55aa, v.wrapping_add(1009)]
        })
        .flat_map(|s| s.to_ne_bytes())
        .collect()
}

fn rgba16(w: u32, h: u32) -> Vec<u8> {
    (0..w * h)
        .flat_map(|i| {
            let v = (i as u16).wrapping_mul(4099);
            [v, v ^ 0x1234, v.wrapping_add(333), v ^ 0xa5a5]
        })
        .flat_map(|s| s.to_ne_bytes())
        .collect()
}

fn jxl_kind(err: &ImageError) -> JxlEncodeErrorKind {
    match err {
        ImageError::Encoding(e) => {
            let src = std::error::Error::source(e)
                .expect("JXL encoding errors carry a JxlEncodeError source");
            src.downcast_ref::<image::codecs::jxl::JxlEncodeError>()
                .expect("source is not JxlEncodeError")
                .kind()
        }
        other => panic!("expected Encoding error, got {other:?}"),
    }
}

// A deterministic, lcms2-generated sRGB v4 profile known to be accepted by
// libjxl (588 bytes). Keeping it in the test avoids a runtime ICC dependency.
const SRGB_ICC: &[u8] = include_bytes!("jxl/srgb.icc");

#[test]
fn lossless_rgb8_and_rgba8_roundtrip_sizes() {
    for (color, channels) in [
        (ExtendedColorType::Rgb8, 3usize),
        (ExtendedColorType::Rgba8, 4),
    ] {
        let (w, h) = (12u32, 9u32);
        let data = if channels == 3 {
            rgb8(w, h)
        } else {
            rgba8(w, h)
        };

        let mut out = Vec::new();
        let mut enc = JxlEncoder::new(&mut out);
        enc.set_lossless(true);
        enc.set_effort(JxlEffort::Falcon);
        enc.write_image(&data, w, h, color).unwrap();

        // Bare codestream signature.
        assert_eq!(&out[..2], b"\xff\x0a", "{color:?} missing JXL signature");
        // Lossless output of a small synthetic image should be compact but
        // non-trivial, and distinct from the raw input size.
        assert!(out.len() > 32, "{color:?} output implausibly small");
        assert!(out.len() < data.len(), "{color:?} did not compress");
    }
}

#[test]
fn lossy_quality_scales_output_size() {
    let (w, h) = (32u32, 32u32);
    let data = rgb8(w, h);

    let encode = |quality: f32| {
        let mut out = Vec::new();
        let mut enc = JxlEncoder::new(&mut out);
        enc.set_quality(quality).unwrap();
        enc.set_effort(JxlEffort::Falcon);
        enc.write_image(&data, w, h, ExtendedColorType::Rgb8)
            .unwrap();
        out.len()
    };

    let high = encode(95.0);
    let low = encode(30.0);
    assert!(
        low < high,
        "lower quality should produce fewer bytes: {low} >= {high}"
    );

    // quality 100 selects lossless.
    let lossless = encode(100.0);
    assert!(
        lossless >= high,
        "quality 100 (lossless) should be >= quality 95"
    );
}

#[test]
fn sixteen_bit_formats_encode() {
    let (w, h) = (10u32, 7u32);
    for (color, data) in [
        (ExtendedColorType::Rgb16, rgb16(w, h)),
        (ExtendedColorType::Rgba16, rgba16(w, h)),
    ] {
        let mut out = Vec::new();
        let mut enc = JxlEncoder::new(&mut out);
        enc.set_lossless(true);
        enc.write_image(&data, w, h, color).unwrap();
        assert!(has_jxl_signature(&out), "{color:?} missing JXL signature");
        // The output must encode the full 16-bit sample precision rather than
        // silently truncating to 8-bit; a truncation would be far smaller.
        assert!(out.len() >= 32, "{color:?} output implausibly small");
    }
}

#[test]
fn icc_profile_is_embedded_and_recoverable() {
    let (w, h) = (6u32, 6u32);
    let data = rgb8(w, h);

    let mut out = Vec::new();
    let mut enc = JxlEncoder::new(&mut out);
    enc.set_lossless(true);
    enc.set_icc_profile_bytes(SRGB_ICC.to_vec());
    enc.write_image(&data, w, h, ExtendedColorType::Rgb8)
        .unwrap();

    let recovered = recover_icc(&out);
    assert_eq!(
        recovered.as_deref(),
        Some(SRGB_ICC),
        "embedded ICC profile must be preserved byte-for-byte"
    );
}

#[test]
fn bad_icc_profile_is_rejected() {
    let (w, h) = (4u32, 4u32);
    let data = rgb8(w, h);

    let mut out = Vec::new();
    let mut enc = JxlEncoder::new(&mut out);
    enc.set_icc_profile_bytes(vec![0u8; 64]);
    let err = enc
        .write_image(&data, w, h, ExtendedColorType::Rgb8)
        .unwrap_err();
    assert_eq!(jxl_kind(&err), JxlEncodeErrorKind::BadIccProfile);
    // No partial output may be treated as a valid codestream.
    assert!(out.is_empty() || out.get(..2) != Some(b"\xff\x0a"));
}

#[test]
fn unsupported_color_types_are_rejected() {
    let (w, h) = (4u32, 4u32);
    for color in [
        ExtendedColorType::L8,
        ExtendedColorType::La8,
        ExtendedColorType::L16,
        ExtendedColorType::La16,
        ExtendedColorType::Rgb32F,
        ExtendedColorType::Rgba32F,
        ExtendedColorType::A8,
    ] {
        // Buffer matches the color type's own geometry so the rejection comes
        // from the color-type check, not the public trait's length assertion.
        let data = vec![0u8; color.buffer_size(w, h) as usize];
        let enc = JxlEncoder::new(Vec::new());
        let err = enc.write_image(&data, w, h, color).unwrap_err();
        match err {
            ImageError::Unsupported(u) => {
                assert!(matches!(u.kind(), UnsupportedErrorKind::Color(c) if c == color));
                assert_eq!(
                    u.format_hint(),
                    image::error::ImageFormatHint::Exact(ImageFormat::Jxl)
                );
            }
            other => panic!("{color:?} expected Unsupported, got {other:?}"),
        }
    }
}

#[test]
fn invalid_parameters_are_rejected() {
    let mut enc = JxlEncoder::new(Vec::new());
    assert!(matches!(
        enc.set_quality(-1.0).unwrap_err(),
        ImageError::Parameter(_)
    ));
    assert!(matches!(
        enc.set_quality(100.1).unwrap_err(),
        ImageError::Parameter(_)
    ));
    assert!(matches!(
        enc.set_quality(f32::NAN).unwrap_err(),
        ImageError::Parameter(_)
    ));
    assert!(matches!(
        enc.set_threads(JxlThreads::Exact(0)).unwrap_err(),
        ImageError::Parameter(_)
    ));
    assert!(JxlEffort::from_level(0).is_err());
    assert!(JxlEffort::from_level(10).is_err());
    assert_eq!(JxlEffort::from_level(7).unwrap(), JxlEffort::Squirrel);
    // Valid configuration is accepted.
    enc.set_quality(0.0).unwrap();
    enc.set_quality(100.0).unwrap();
    enc.set_threads(JxlThreads::One).unwrap();
    enc.set_threads(JxlThreads::Auto).unwrap();
    enc.set_threads(JxlThreads::Exact(4)).unwrap();
}

#[test]
fn thread_options_produce_valid_output() {
    let (w, h) = (8u32, 8u32);
    let data = rgba8(w, h);
    for threads in [
        JxlThreads::One,
        JxlThreads::Exact(1),
        JxlThreads::Exact(2),
        JxlThreads::Auto,
    ] {
        let mut out = Vec::new();
        let mut enc = JxlEncoder::new(&mut out);
        enc.set_lossless(true);
        enc.set_effort(JxlEffort::Lightning);
        enc.set_threads(threads).unwrap();
        enc.write_image(&data, w, h, ExtendedColorType::Rgba8)
            .unwrap();
        assert_eq!(&out[..2], b"\xff\x0a", "{threads:?} produced bad output");
    }
}

#[test]
fn effort_levels_all_encode() {
    let (w, h) = (8u32, 8u32);
    let data = rgb8(w, h);
    for level in 1..=9 {
        let mut out = Vec::new();
        let mut enc = JxlEncoder::new(&mut out);
        enc.set_lossless(true);
        enc.set_effort(JxlEffort::from_level(level).unwrap());
        enc.write_image(&data, w, h, ExtendedColorType::Rgb8)
            .unwrap();
        assert_eq!(&out[..2], b"\xff\x0a", "effort {level} failed");
    }
}

#[test]
fn never_silently_degrades_to_png() {
    // A failed or successful JXL encode must never be rerouted to PNG.
    let (w, h) = (4u32, 4u32);
    let data = rgb8(w, h);
    let mut buf = Vec::new();
    let mut enc = JxlEncoder::new(&mut buf);
    enc.set_lossless(true);
    enc.write_image(&data, w, h, ExtendedColorType::Rgb8)
        .unwrap();
    assert_eq!(&buf[..2], b"\xff\x0a");
    assert_ne!(&buf[..8], b"\x89PNG\r\n\x1a\n");
}

#[test]
fn generic_save_entry_point_routes_to_jxl() {
    let (w, h) = (4u32, 4u32);
    let data = rgb8(w, h);
    let mut writer = std::io::Cursor::new(Vec::new());
    image::write_buffer_with_format(
        &mut writer,
        &data,
        w,
        h,
        ExtendedColorType::Rgb8,
        ImageFormat::Jxl,
    )
    .unwrap();
    let bytes = writer.into_inner();
    assert_eq!(&bytes[..2], b"\xff\x0a");
    assert_ne!(&bytes[..8], b"\x89PNG\r\n\x1a\n");
}

#[test]
fn dynamic_image_grayscale_is_converted_not_rejected() {
    use image::{DynamicImage, GrayImage};
    let gray = GrayImage::from_fn(5, 3, |x, y| image::Luma([((x + y) * 30) as u8]));
    let img = DynamicImage::from(gray);
    let mut buf = Vec::new();
    img.write_with_encoder(JxlEncoder::new(&mut buf)).unwrap();
    assert!(has_jxl_signature(&buf));
}

#[test]
fn dynamic_image_16bit_preserves_depth() {
    use image::{DynamicImage, ImageBuffer, Rgb};
    let rgb16 = ImageBuffer::from_fn(4, 4, |x, y| {
        Rgb([(x * 1000) as u16, (y * 2000) as u16, 40000])
    });
    let img = DynamicImage::from(rgb16);
    let mut buf = Vec::new();
    img.write_with_encoder(JxlEncoder::new(&mut buf)).unwrap();
    // 16-bit codestreams use the BMFF container.
    assert!(has_jxl_signature(&buf));
}

/// Minimal libjxl decoder FFI, used to verify round-trips and ICC embedding.
mod decode {
    #[repr(C)]
    pub struct JxlDecoder {
        _p: [u8; 0],
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    pub struct PixelFormat {
        pub num_channels: u32,
        pub data_type: u32,
        pub endianness: u32,
        pub align: usize,
    }

    // Each soname needs its own cfg-gated extern block (cfg attributes on one
    // block are ANDed), mirroring the mechanism in src/codecs/jxl/ffi.rs.
    macro_rules! jxl_decoder_extern {
        ($($link_attr:tt)*) => {
            $($link_attr)*
            extern "C" {
                pub fn JxlDecoderCreate(mm: *const std::ffi::c_void) -> *mut JxlDecoder;
                pub fn JxlDecoderDestroy(d: *mut JxlDecoder);
                pub fn JxlDecoderSetInput(d: *mut JxlDecoder, data: *const u8, size: usize)
                    -> usize;
                pub fn JxlDecoderCloseInput(d: *mut JxlDecoder);
                pub fn JxlDecoderProcessInput(d: *mut JxlDecoder) -> u32;
                pub fn JxlDecoderSubscribeEvents(d: *mut JxlDecoder, events: i32) -> u32;
                pub fn JxlDecoderGetICCProfileSize(
                    d: *const JxlDecoder,
                    target: i32,
                    size: *mut usize,
                ) -> u32;
                pub fn JxlDecoderGetColorAsICCProfile(
                    d: *const JxlDecoder,
                    target: i32,
                    buf: *mut u8,
                    size: usize,
                ) -> u32;
                pub fn JxlDecoderImageOutBufferSize(
                    d: *const JxlDecoder,
                    fmt: *const PixelFormat,
                    size: *mut usize,
                ) -> u32;
                pub fn JxlDecoderSetImageOutBuffer(
                    d: *mut JxlDecoder,
                    fmt: *const PixelFormat,
                    buf: *mut std::ffi::c_void,
                    size: usize,
                ) -> u32;
            }
        };
    }

    #[cfg(all(unix, not(target_os = "macos"), jxl_link_soname_0_7))]
    jxl_decoder_extern!(#[link(name = ":libjxl.so.0.7", kind = "dylib")]);
    #[cfg(all(unix, not(target_os = "macos"), jxl_link_soname_0_8))]
    jxl_decoder_extern!(#[link(name = ":libjxl.so.0.8", kind = "dylib")]);
    #[cfg(all(unix, not(target_os = "macos"), jxl_link_soname_0_9))]
    jxl_decoder_extern!(#[link(name = ":libjxl.so.0.9", kind = "dylib")]);
    #[cfg(all(unix, not(target_os = "macos"), jxl_link_soname_0_10))]
    jxl_decoder_extern!(#[link(name = ":libjxl.so.0.10", kind = "dylib")]);
    #[cfg(all(unix, not(target_os = "macos"), jxl_link_soname_0_11))]
    jxl_decoder_extern!(#[link(name = ":libjxl.so.0.11", kind = "dylib")]);
    #[cfg(all(unix, not(target_os = "macos"), jxl_link_soname_0_12))]
    jxl_decoder_extern!(#[link(name = ":libjxl.so.0.12", kind = "dylib")]);
    #[cfg(any(target_os = "macos", target_env = "msvc", jxl_link_unversioned))]
    jxl_decoder_extern!(#[link(name = "jxl", kind = "dylib")]);
}

/// Decoder data types as libjxl's `JxlDataType`.
const DT_U8: u32 = 2;
const DT_U16: u32 = 3;

/// Decode `codestream`, returning (pixels, embedded ICC profile if any).
///
/// `channels` is 3 for RGB or 4 for RGBA; `data_type` selects 8- or 16-bit
/// output (native byte order). The decoder is driven to completion so any
/// partially written output is rejected.
fn decode(
    codestream: &[u8],
    channels: u32,
    data_type: u32,
    pixels: usize,
) -> (Vec<u8>, Option<Vec<u8>>) {
    use decode::{
        JxlDecoderCloseInput, JxlDecoderCreate, JxlDecoderDestroy, JxlDecoderGetColorAsICCProfile,
        JxlDecoderGetICCProfileSize, JxlDecoderImageOutBufferSize, JxlDecoderProcessInput,
        JxlDecoderSetImageOutBuffer, JxlDecoderSetInput, JxlDecoderSubscribeEvents, PixelFormat,
    };
    unsafe {
        let d = JxlDecoderCreate(std::ptr::null());
        assert!(!d.is_null(), "decoder allocation failed");
        assert_eq!(
            JxlDecoderSetInput(d, codestream.as_ptr(), codestream.len()),
            0
        );
        JxlDecoderCloseInput(d);
        assert_eq!(JxlDecoderSubscribeEvents(d, 0x40 | 0x1000), 0);

        let fmt = PixelFormat {
            num_channels: channels,
            data_type,
            endianness: 0, // Native: matches what the encoder received.
            align: 0,
        };
        let mut buf = vec![0u8; pixels];
        let mut got_pixels = false;
        loop {
            match JxlDecoderProcessInput(d) {
                0 => break,
                1 => {
                    JxlDecoderDestroy(d);
                    panic!("libjxl failed to decode our own encoder output");
                }
                5 => {
                    let mut sz = 0;
                    assert_eq!(JxlDecoderImageOutBufferSize(d, &fmt, &mut sz), 0);
                    assert_eq!(sz, pixels, "decoded size must match the source");
                    assert_eq!(
                        JxlDecoderSetImageOutBuffer(d, &fmt, buf.as_mut_ptr().cast(), sz),
                        0
                    );
                    got_pixels = true;
                }
                _ => {}
            }
        }
        assert!(got_pixels, "decoder never requested a pixel buffer");

        let mut icc = None;
        let mut size = 0;
        if JxlDecoderGetICCProfileSize(d, 0, &mut size) == 0 {
            let mut profile = vec![0u8; size];
            if JxlDecoderGetColorAsICCProfile(d, 0, profile.as_mut_ptr(), size) == 0 {
                icc = Some(profile);
            }
        }
        JxlDecoderDestroy(d);
        (buf, icc)
    }
}

fn recover_icc(codestream: &[u8]) -> Option<Vec<u8>> {
    decode(codestream, 3, DT_U8, 6 * 6 * 3).1
}

/// Encode with a freshly configured encoder and return the codestream.
fn encode(
    data: &[u8],
    w: u32,
    h: u32,
    color: ExtendedColorType,
    configure: impl FnOnce(&mut JxlEncoder<&mut Vec<u8>>),
) -> Vec<u8> {
    let mut out = Vec::new();
    let mut enc = JxlEncoder::new(&mut out);
    configure(&mut enc);
    enc.write_image(data, w, h, color).unwrap();
    out
}

#[test]
fn lossless_roundtrips_pixels_exactly() {
    // The strongest correctness check: lossless output must decode back to
    // byte-identical pixels for every supported layout, including an image
    // larger than one 256x256 group so multi-group tiling is exercised.
    struct Case {
        color: ExtendedColorType,
        channels: u32,
        data_type: u32,
        w: u32,
        h: u32,
        data: Vec<u8>,
    }
    let cases = [
        Case {
            color: ExtendedColorType::Rgb8,
            channels: 3,
            data_type: DT_U8,
            w: 64,
            h: 48,
            data: rgb8(64, 48),
        },
        Case {
            color: ExtendedColorType::Rgba8,
            channels: 4,
            data_type: DT_U8,
            w: 64,
            h: 48,
            data: rgba8(64, 48),
        },
        Case {
            color: ExtendedColorType::Rgb16,
            channels: 3,
            data_type: DT_U16,
            w: 64,
            h: 48,
            data: rgb16(64, 48),
        },
        Case {
            color: ExtendedColorType::Rgba16,
            channels: 4,
            data_type: DT_U16,
            w: 64,
            h: 48,
            data: rgba16(64, 48),
        },
        Case {
            color: ExtendedColorType::Rgb8,
            channels: 3,
            data_type: DT_U8,
            w: 300,
            h: 260,
            data: rgb8(300, 260),
        },
    ];

    for case in cases {
        let out = encode(&case.data, case.w, case.h, case.color, |e| {
            e.set_lossless(true);
            e.set_effort(JxlEffort::Tortoise);
        });
        let (got, _) = decode(&out, case.channels, case.data_type, case.data.len());
        assert_eq!(
            got, case.data,
            "lossless {:?} {}x{} did not round-trip",
            case.color, case.w, case.h
        );
    }
}

#[test]
fn lossy_rgba_preserves_alpha_and_structure() {
    // Lossy coding perturbs colour, but a smooth alpha ramp must survive
    // well enough that the image stays recognisably the same; this catches an
    // encoder that silently dropped or misread the alpha channel.
    let (w, h) = (48u32, 32u32);
    let data: Vec<u8> = (0..w * h)
        .flat_map(|i| {
            let x = i % w;
            let y = i / w;
            [
                (x * 5) as u8,
                (y * 8) as u8,
                128,
                ((x * 255) / (w - 1)) as u8,
            ]
        })
        .collect();
    let out = encode(&data, w, h, ExtendedColorType::Rgba8, |e| {
        e.set_quality(90.0).unwrap();
        e.set_effort(JxlEffort::Falcon);
    });
    let (got, _) = decode(&out, 4, DT_U8, data.len());

    let mut max_alpha_err = 0i32;
    for (src, dec) in data.as_chunks::<4>().0.iter().zip(got.as_chunks::<4>().0) {
        max_alpha_err = max_alpha_err.max((i32::from(src[3]) - i32::from(dec[3])).abs());
    }
    assert!(
        max_alpha_err <= 2,
        "lossy alpha ramp drifted by up to {max_alpha_err} levels"
    );
}

#[test]
fn icc_profile_survives_lossy_encoding() {
    // Lossless canonicalises the profile, so check the lossy path separately:
    // the profile must still be embedded (values may be re-encoded, but its
    // presence and non-triviality must be preserved).
    let (w, h) = (8u32, 8u32);
    let data = rgb8(w, h);
    let out = encode(&data, w, h, ExtendedColorType::Rgb8, |e| {
        e.set_quality(75.0).unwrap();
        e.set_icc_profile_bytes(SRGB_ICC.to_vec());
    });
    let (_, icc) = decode(&out, 3, DT_U8, data.len());
    let icc = icc.expect("lossy encode dropped the ICC profile entirely");
    assert!(
        icc.len() >= 100,
        "embedded ICC profile is implausibly short"
    );
}

#[test]
fn sixteen_bit_lossless_roundtrip_is_exact() {
    // Guards against silent 8-bit truncation of 16-bit input.
    let (w, h) = (32u32, 21u32);
    let data = rgba16(w, h);
    let out = encode(&data, w, h, ExtendedColorType::Rgba16, |e| {
        e.set_lossless(true);
        e.set_effort(JxlEffort::Falcon);
    });
    let (got, _) = decode(&out, 4, DT_U16, data.len());
    assert_eq!(got, data, "16-bit RGBA data was not preserved exactly");
}

#[test]
fn bad_geometry_errors_are_distinguishable() {
    // Zero dimensions and oversize geometry must produce Encoding errors that
    // carry the matching JxlEncodeErrorKind, not a panic and not a generic
    // IO/format error. Buffers are sized per the color type so the trait's
    // pre-flight length assertion is satisfied and the geometry check is what
    // actually rejects the call.
    let err = JxlEncoder::new(Vec::new())
        .write_image(&[], 0, 4, ExtendedColorType::Rgb8)
        .unwrap_err();
    assert_eq!(jxl_kind(&err), JxlEncodeErrorKind::InvalidDimensions);

    let width = 262_145u32; // one past the level-5 per-side limit
    let data = vec![0u8; width as usize * 3];
    let err = JxlEncoder::new(Vec::new())
        .write_image(&data, width, 1, ExtendedColorType::Rgb8)
        .unwrap_err();
    assert_eq!(jxl_kind(&err), JxlEncodeErrorKind::ImageTooLarge);
}
