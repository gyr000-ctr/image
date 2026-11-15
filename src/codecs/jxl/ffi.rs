//! Hand-written FFI declarations for the subset of the libjxl C API used by
//! the encoder.
//!
//! We deliberately do not depend on a `-sys` crate: the only permissively
//! licensed wrapper crates available require newer compiler versions than this
//! crate's MSRV, while the libjxl ABI is stable (all symbols carry the `JXL_0`
//! version tag) and the surface we need is small. Every declaration mirrors
//! `jxl/encode.h`, `jxl/types.h`, `jxl/codestream_header.h`,
//! `jxl/color_encoding.h` and `jxl/parallel_runner.h` of libjxl 0.7+.
#![allow(non_camel_case_types, non_snake_case, dead_code, unreachable_pub)]

use std::os::raw::{c_float, c_int, c_void};

// Compile-time ABI checks. These sizes/offsets were measured against libjxl's
// C headers (and verified by a C program built with the system `cc`). If a
// field is added to or reordered in one of these mirrors without updating the
// others, the build fails here instead of corrupting memory at run time.
const _: () = {
    use std::mem::{offset_of, size_of};
    assert!(size_of::<JxlBasicInfo>() == 204);
    assert!(offset_of!(JxlBasicInfo, preview) == 72);
    assert!(offset_of!(JxlBasicInfo, animation) == 80);
    assert!(offset_of!(JxlBasicInfo, intrinsic_xsize) == 96);
    assert!(size_of::<JxlExtraChannelInfo>() == 44);
    assert!(size_of::<JxlColorEncoding>() == 104);
    assert!(size_of::<JxlPixelFormat>() == 24);
    assert!(size_of::<JxlBitDepth>() == 12);
};

// --- jxl/types.h -----------------------------------------------------------

pub type JXL_BOOL = c_int;
pub const JXL_TRUE: JXL_BOOL = 1;
pub const JXL_FALSE: JXL_BOOL = 0;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct JxlPixelFormat {
    pub num_channels: u32,
    pub data_type: JxlDataType,
    pub endianness: JxlEndianness,
    pub align: usize,
}

#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum JxlDataType {
    Float = 0,
    Uint8 = 2,
    Uint16 = 3,
    Float16 = 5,
}

#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum JxlEndianness {
    Native = 0,
    Little = 1,
    Big = 2,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct JxlBitDepth {
    pub r#type: JxlBitDepthType,
    pub bits_per_sample: u32,
    pub exponent_bits_per_sample: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub enum JxlBitDepthType {
    FromPixelFormat = 0,
    FromCodestream = 1,
    Custom = 2,
}

// --- jxl/codestream_header.h -----------------------------------------------

#[repr(C)]
#[derive(Clone, Copy)]
pub enum JxlExtraChannelType {
    Alpha = 0,
    Depth,
    SpotColor,
    SelectionMask,
    Black,
    Cfa,
    Thermal,
    Reserved0,
    Reserved1,
    Reserved2,
    Reserved3,
    Reserved4,
    Reserved5,
    Reserved6,
    Reserved7,
    Unknown,
    Optional,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct JxlPreviewHeader {
    pub xsize: u32,
    pub ysize: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct JxlAnimationHeader {
    pub tps_numerator: u32,
    pub tps_denominator: u32,
    pub num_loops: u32,
    pub have_timecodes: JXL_BOOL,
}

/// Mirrors `JxlBasicInfo`. The trailing 100-byte padding is part of the ABI.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct JxlBasicInfo {
    pub have_container: JXL_BOOL,
    pub xsize: u32,
    pub ysize: u32,
    pub bits_per_sample: u32,
    pub exponent_bits_per_sample: u32,
    pub intensity_target: c_float,
    pub min_nits: c_float,
    pub relative_to_max_display: JXL_BOOL,
    pub linear_below: c_float,
    pub uses_original_profile: JXL_BOOL,
    pub have_preview: JXL_BOOL,
    pub have_animation: JXL_BOOL,
    pub orientation: c_int,
    pub num_color_channels: u32,
    pub num_extra_channels: u32,
    pub alpha_bits: u32,
    pub alpha_exponent_bits: u32,
    pub alpha_premultiplied: JXL_BOOL,
    pub preview: JxlPreviewHeader,
    pub animation: JxlAnimationHeader,
    pub intrinsic_xsize: u32,
    pub intrinsic_ysize: u32,
    pub _padding: [u8; 100],
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct JxlExtraChannelInfo {
    pub r#type: JxlExtraChannelType,
    pub bits_per_sample: u32,
    pub exponent_bits_per_sample: u32,
    pub dim_shift: u32,
    pub name_length: u32,
    pub alpha_premultiplied: JXL_BOOL,
    pub spot_color: [c_float; 4],
    pub cfa_channel: u32,
}

// --- jxl/color_encoding.h --------------------------------------------------

#[repr(C)]
#[derive(Clone, Copy)]
pub enum JxlColorSpace {
    Rgb = 0,
    Gray,
    Xyb,
    Unknown,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub enum JxlWhitePoint {
    D65 = 1,
    Custom = 2,
    E = 10,
    Dci = 11,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub enum JxlPrimaries {
    Custom = 0,
    #[allow(clippy::upper_case_acronyms)]
    Srgb = 1,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub enum JxlTransferFunction {
    // Only the values needed for sRGB fallback.
    #[allow(clippy::upper_case_acronyms)]
    Srgb = 1,
    Linear = 8,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub enum JxlRenderingIntent {
    Relative = 0,
    Perceptual,
    Saturation,
    Absolute,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct JxlColorEncoding {
    pub color_space: JxlColorSpace,
    pub white_point: JxlWhitePoint,
    pub white_point_xy: [f64; 2],
    pub primaries: JxlPrimaries,
    pub primaries_red_xy: [f64; 2],
    pub primaries_green_xy: [f64; 2],
    pub primaries_blue_xy: [f64; 2],
    pub transfer_function: JxlTransferFunction,
    pub gamma: f64,
    pub rendering_intent: JxlRenderingIntent,
}

// --- jxl/encode.h ----------------------------------------------------------

pub type JxlEncoder = c_void;
pub type JxlEncoderFrameSettings = c_void;

#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum JxlEncoderStatus {
    Success = 0,
    Error = 1,
    NeedMoreOutput = 2,
}

#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum JxlEncoderError {
    Ok = 0,
    Generic = 1,
    Oom = 2,
    Jbrd = 3,
    BadInput = 4,
    NotSupported = 0x80,
    ApiUsage = 0x81,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub enum JxlEncoderFrameSettingId {
    /// Effort/speed level, `1..=9` (libjxl also accepts 10 on some releases,
    /// but the public API only exposes the documented `1..=9`).
    Effort = 0,
}

// --- jxl/parallel_runner.h -------------------------------------------------

pub type JxlParallelRetCode = c_int;
pub type JxlParallelRunInit =
    unsafe extern "C" fn(jpegxl_opaque: *mut c_void, num_threads: usize) -> JxlParallelRetCode;
pub type JxlParallelRunFunction =
    unsafe extern "C" fn(jpegxl_opaque: *mut c_void, value: u32, thread_id: usize);
pub type JxlParallelRunner = unsafe extern "C" fn(
    runner_opaque: *mut c_void,
    jpegxl_opaque: *mut c_void,
    init: JxlParallelRunInit,
    func: JxlParallelRunFunction,
    start_range: u32,
    end_range: u32,
) -> JxlParallelRetCode;

// Versioned soname directives. The leading `:` tells the linker (both GNU ld
// and rust-lld) to use the name verbatim, so no unversioned `-dev` symlink is
// required. Unlike build-script `rustc-link-arg`, `#[link]` directives are
// embedded in the rlib and re-applied when downstream crates link their final
// binary. The active soname is chosen by `build.rs`, which probes the system
// and emits exactly one `jxl_link_soname_*` cfg.
//
// Stable Rust does not allow `cfg(...)` inside `#[link]`, so each soname gets
// its own fully cfg-gated `extern` block; the macro keeps the declarations in
// one place.
macro_rules! jxl_encoder_extern {
    ($($link_attr:tt)*) => {
        $($link_attr)*
        extern "C" {
            pub fn JxlEncoderVersion() -> u32;
            pub fn JxlEncoderCreate(memory_manager: *const c_void) -> *mut JxlEncoder;
            pub fn JxlEncoderDestroy(enc: *mut JxlEncoder);
            pub fn JxlEncoderReset(enc: *mut JxlEncoder);
            pub fn JxlEncoderGetError(enc: *mut JxlEncoder) -> JxlEncoderError;
            pub fn JxlEncoderSetParallelRunner(
                enc: *mut JxlEncoder,
                parallel_runner: Option<JxlParallelRunner>,
                parallel_runner_opaque: *mut c_void,
            ) -> JxlEncoderStatus;
            pub fn JxlEncoderUseContainer(
                enc: *mut JxlEncoder,
                use_container: JXL_BOOL,
            ) -> JxlEncoderStatus;

            // Metadata
            pub fn JxlEncoderInitBasicInfo(info: *mut JxlBasicInfo);
            pub fn JxlEncoderSetBasicInfo(
                enc: *mut JxlEncoder,
                info: *const JxlBasicInfo,
            ) -> JxlEncoderStatus;
            pub fn JxlEncoderInitExtraChannelInfo(
                r#type: JxlExtraChannelType,
                info: *mut JxlExtraChannelInfo,
            );
            pub fn JxlEncoderSetExtraChannelInfo(
                enc: *mut JxlEncoder,
                index: usize,
                info: *const JxlExtraChannelInfo,
            ) -> JxlEncoderStatus;
            pub fn JxlEncoderSetColorEncoding(
                enc: *mut JxlEncoder,
                color: *const JxlColorEncoding,
            ) -> JxlEncoderStatus;
            pub fn JxlEncoderSetICCProfile(
                enc: *mut JxlEncoder,
                icc_profile: *const u8,
                size: usize,
            ) -> JxlEncoderStatus;
            pub fn JxlColorEncodingSetToSRGB(
                color_encoding: *mut JxlColorEncoding,
                is_gray: JXL_BOOL,
            );

            // Frame settings
            pub fn JxlEncoderFrameSettingsCreate(
                enc: *mut JxlEncoder,
                source: *const JxlEncoderFrameSettings,
            ) -> *mut JxlEncoderFrameSettings;
            pub fn JxlEncoderFrameSettingsSetOption(
                frame_settings: *mut JxlEncoderFrameSettings,
                option: JxlEncoderFrameSettingId,
                value: i64,
            ) -> JxlEncoderStatus;
            pub fn JxlEncoderFrameSettingsSetFloatOption(
                frame_settings: *mut JxlEncoderFrameSettings,
                option: JxlEncoderFrameSettingId,
                value: c_float,
            ) -> JxlEncoderStatus;
            pub fn JxlEncoderSetFrameLossless(
                frame_settings: *mut JxlEncoderFrameSettings,
                lossless: JXL_BOOL,
            ) -> JxlEncoderStatus;
            pub fn JxlEncoderSetFrameDistance(
                frame_settings: *mut JxlEncoderFrameSettings,
                distance: c_float,
            ) -> JxlEncoderStatus;
            pub fn JxlEncoderSetFrameBitDepth(
                frame_settings: *mut JxlEncoderFrameSettings,
                bit_depth: *const JxlBitDepth,
            ) -> JxlEncoderStatus;
            pub fn JxlEncoderDistanceFromQuality(quality: c_float) -> c_float;

            // Input / output
            pub fn JxlEncoderAddImageFrame(
                frame_settings: *const JxlEncoderFrameSettings,
                pixel_format: *const JxlPixelFormat,
                buffer: *const c_void,
                size: usize,
            ) -> JxlEncoderStatus;
            pub fn JxlEncoderCloseInput(enc: *mut JxlEncoder);
            /// Pull-based output. Present in every supported libjxl series
            /// (0.7+), unlike the callback-based `SetOutputProcessor` API
            /// which only exists from 0.11 on.
            ///
            /// `next_out`/`avail_out` are in/out: on return they point past
            /// the bytes written and hold the remaining room. The caller must
            /// guarantee `*avail_out >= 32`.
            pub fn JxlEncoderProcessOutput(
                enc: *mut JxlEncoder,
                next_out: *mut *mut u8,
                avail_out: *mut usize,
            ) -> JxlEncoderStatus;
        }
    };
}

#[cfg(all(unix, not(target_os = "macos"), jxl_link_soname_0_7))]
jxl_encoder_extern!(#[link(name = ":libjxl.so.0.7", kind = "dylib")]);
#[cfg(all(unix, not(target_os = "macos"), jxl_link_soname_0_8))]
jxl_encoder_extern!(#[link(name = ":libjxl.so.0.8", kind = "dylib")]);
#[cfg(all(unix, not(target_os = "macos"), jxl_link_soname_0_9))]
jxl_encoder_extern!(#[link(name = ":libjxl.so.0.9", kind = "dylib")]);
#[cfg(all(unix, not(target_os = "macos"), jxl_link_soname_0_10))]
jxl_encoder_extern!(#[link(name = ":libjxl.so.0.10", kind = "dylib")]);
#[cfg(all(unix, not(target_os = "macos"), jxl_link_soname_0_11))]
jxl_encoder_extern!(#[link(name = ":libjxl.so.0.11", kind = "dylib")]);
#[cfg(all(unix, not(target_os = "macos"), jxl_link_soname_0_12))]
jxl_encoder_extern!(#[link(name = ":libjxl.so.0.12", kind = "dylib")]);
// Unversioned library (macOS dylibs, MSVC import libraries, or an explicit
// `-dev` package detected by the build script).
#[cfg(any(target_os = "macos", target_env = "msvc", jxl_link_unversioned))]
jxl_encoder_extern!(#[link(name = "jxl", kind = "dylib")]);

// libjxl_threads: same soname-selection mechanism as libjxl above.
macro_rules! jxl_threads_extern {
    ($($link_attr:tt)*) => {
        $($link_attr)*
        extern "C" {
            pub fn JxlThreadParallelRunner(
                runner_opaque: *mut c_void,
                jpegxl_opaque: *mut c_void,
                init: JxlParallelRunInit,
                func: JxlParallelRunFunction,
                start_range: u32,
                end_range: u32,
            ) -> JxlParallelRetCode;
            pub fn JxlThreadParallelRunnerCreate(memory_manager: *const c_void, num_worker_threads: usize)
                -> *mut c_void;
            pub fn JxlThreadParallelRunnerDestroy(runner_opaque: *mut c_void);
            pub fn JxlThreadParallelRunnerDefaultNumWorkerThreads() -> usize;
        }
    };
}

#[cfg(all(unix, not(target_os = "macos"), jxl_link_soname_0_7))]
jxl_threads_extern!(#[link(name = ":libjxl_threads.so.0.7", kind = "dylib")]);
#[cfg(all(unix, not(target_os = "macos"), jxl_link_soname_0_8))]
jxl_threads_extern!(#[link(name = ":libjxl_threads.so.0.8", kind = "dylib")]);
#[cfg(all(unix, not(target_os = "macos"), jxl_link_soname_0_9))]
jxl_threads_extern!(#[link(name = ":libjxl_threads.so.0.9", kind = "dylib")]);
#[cfg(all(unix, not(target_os = "macos"), jxl_link_soname_0_10))]
jxl_threads_extern!(#[link(name = ":libjxl_threads.so.0.10", kind = "dylib")]);
#[cfg(all(unix, not(target_os = "macos"), jxl_link_soname_0_11))]
jxl_threads_extern!(#[link(name = ":libjxl_threads.so.0.11", kind = "dylib")]);
#[cfg(all(unix, not(target_os = "macos"), jxl_link_soname_0_12))]
jxl_threads_extern!(#[link(name = ":libjxl_threads.so.0.12", kind = "dylib")]);
#[cfg(any(target_os = "macos", target_env = "msvc", jxl_link_unversioned))]
jxl_threads_extern!(#[link(name = "jxl_threads", kind = "dylib")]);
