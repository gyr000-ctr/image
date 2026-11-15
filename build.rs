//! Build script for the optional `jxl` feature.
//!
//! The JPEG XL encoder is a thin, hand-written FFI layer over the system
//! libjxl reference implementation (no Rust wrapper crate is pulled in). The
//! actual library directives live on `#[link(...)]` attributes in
//! `src/codecs/jxl/ffi.rs`, since those propagate to downstream crates
//! correctly (build-script `rustc-link-arg` does not). This script probes the
//! system for a usable shared object and then:
//!
//! - if a usable libjxl + libjxl_threads pair is found, emits `jxl_available`
//!   plus exactly one soname cfg so `#[link]` selects the right library;
//! - if none is found, emits a `cargo:warning` and *nothing else*. The crate
//!   then compiles the explicit `JxlEncoderUnsupported` placeholder, so a
//!   build without the system dependency still succeeds and JXL encoding is
//!   refused at runtime with a distinguishable `Unsupported` error.
//!
//! This second point matters: failing the build here would make
//! `--all-features` builds (docs.rs, `cargo clippy --all-features`) unusable on
//! machines without libjxl, and it would violate the crate's promise that all
//! optional formats degrade rather than break the build.
//!
//! Most distributions ship the runtime shared objects (`libjxl.so.0.x`)
//! without a development package providing the unversioned `libjxl.so` linker
//! symlink, so both are probed.
//!
//! The probe compiles and links a tiny program that *calls* the symbols the
//! encoder relies on (`JxlEncoderCreate`, `JxlEncoderProcessOutput`,
//! `JxlThreadParallelRunnerCreate`). Checking only that the soname is
//! linkable would accept an ABI that lacks the required entry points and defer
//! the failure to an unresolved-symbol error when the user's binary is linked.

use std::path::PathBuf;

/// Sonames accepted for the 0.x ABI series (all symbols carry the stable
/// `JXL_0` version tag). The encoder uses only the long-standing
/// `JxlEncoderProcessOutput` pull API, which every series from 0.7 onwards
/// provides, so the whole range is safe.
const SONAMES: [(&str, &str); 6] = [
    ("0.12", "jxl_link_soname_0_12"),
    ("0.11", "jxl_link_soname_0_11"),
    ("0.10", "jxl_link_soname_0_10"),
    ("0.9", "jxl_link_soname_0_9"),
    ("0.8", "jxl_link_soname_0_8"),
    ("0.7", "jxl_link_soname_0_7"),
];

fn main() {
    // Declare the custom cfgs unconditionally so `unexpected_cfgs` stays
    // silent even when the feature is off.
    println!("cargo:rustc-check-cfg=cfg(jxl_available)");
    for (_, cfg) in &SONAMES {
        println!("cargo:rustc-check-cfg=cfg({cfg})");
    }
    println!("cargo:rustc-check-cfg=cfg(jxl_link_unversioned)");
    println!("cargo:rerun-if-changed=build.rs");

    if !std::env::var("CARGO_FEATURE_JXL")
        .map(|v| v == "1")
        .unwrap_or(false)
    {
        return;
    }

    let target = std::env::var("TARGET").unwrap_or_default();
    if target.contains("msvc") {
        // vcpkg provides plain import libraries; the `#[link]` fallback in
        // ffi.rs uses the unversioned name. We cannot run the `cc` probe
        // reliably there, so trust the package manager and let the linker
        // report a missing library.
        println!("cargo:rustc-cfg=jxl_available");
        return;
    }

    // Prefer an unversioned development install (macOS dylibs, an explicit
    // `-dev`/`-devel` package), then fall back to the versioned runtime
    // sonames. Both libraries must come from the same series.
    if probe("libjxl.so", "libjxl_threads.so").unwrap_or(false) {
        println!("cargo:rustc-cfg=jxl_link_unversioned");
        println!("cargo:rustc-cfg=jxl_available");
        return;
    }

    for (version, cfg) in SONAMES {
        let jxl = format!("libjxl.so.{version}");
        let threads = format!("libjxl_threads.so.{version}");
        if probe(&jxl, &threads).unwrap_or(false) {
            println!("cargo:rustc-cfg={cfg}");
            println!("cargo:rustc-cfg=jxl_available");
            return;
        }
    }

    // Not found: keep building, but say so loudly. `JxlEncoderUnsupported`
    // will reject every encoding request with an `Unsupported` error, and
    // `ImageFormat::Jxl::writing_enabled()` reports false.
    println!(
        "cargo:warning=the `jxl` feature is enabled but no usable system libjxl was found \
         (looked for libjxl.so plus versioned sonames libjxl.so.0.7..0.12 and their \
         libjxl_threads counterparts). Building without JPEG XL encoding; requests to \
         encode JXL will be explicitly rejected. Install libjxl (e.g. the `libjxl` \
         package) and rebuild to enable it."
    );
}

/// Returns whether a program calling the required libjxl entry points can be
/// compiled and linked against the given pair of library names.
fn probe(jxl: &str, threads: &str) -> std::io::Result<bool> {
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR is set by cargo"));
    let src = out_dir.join("jxl_link_probe.c");
    let out = out_dir.join(format!(
        "jxl_link_probe_{}.out",
        jxl.replace(['.', '-'], "_")
    ));

    // Reference (and call) exactly the symbols the encoder needs, so an
    // older/partial ABI is rejected here instead of at final link time.
    std::fs::write(
        &src,
        br#"typedef int JxlBool;
typedef void JxlEncoder;
typedef void JxlThreadParallelRunner;
extern JxlEncoder* JxlEncoderCreate(const void* memory_manager);
extern int JxlEncoderProcessOutput(JxlEncoder* enc, unsigned char** next_out,
                                   unsigned long* avail_out);
extern void JxlEncoderDestroy(JxlEncoder* enc);
extern JxlThreadParallelRunner* JxlThreadParallelRunnerCreate(const void* mm,
                                                              unsigned long n);
extern void JxlThreadParallelRunnerDestroy(JxlThreadParallelRunner* r);
int main(void) {
    JxlEncoder* enc = JxlEncoderCreate(0);
    JxlThreadParallelRunner* runner = JxlThreadParallelRunnerCreate(0, 1);
    unsigned char buf[64];
    unsigned char* next = buf;
    unsigned long avail = sizeof(buf);
    int ok = enc != 0 && runner != 0 &&
             JxlEncoderProcessOutput(enc, &next, &avail) != 1;
    JxlThreadParallelRunnerDestroy(runner);
    JxlEncoderDestroy(enc);
    return ok ? 0 : 1;
}
"#,
    )?;

    let cc = std::env::var("CC").unwrap_or_else(|_| "cc".to_owned());
    let status = std::process::Command::new(cc)
        .arg(&src)
        .arg("-o")
        .arg(&out)
        .arg(format!("-l:{jxl}"))
        .arg(format!("-l:{threads}"))
        // Keep the probe quiet; only our own diagnostics should reach the user.
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()?;
    Ok(status.success())
}
