//! Generates the C header with cbindgen.
//!
//! The header is always written to `$OUT_DIR/grin_ffi.h`, and
//! tests/header_test.rs compares it with the committed `include/grin_ffi.h`,
//! so a change to an `extern "C"` signature fails the tests until the
//! committed header is regenerated. The build only writes into the source
//! tree when `GRIN_FFI_WRITE_HEADER=1` is set (tool/regen_bindings.dart sets
//! it); then it copies the header to `include/grin_ffi.h` if the content
//! differs. That copy is best effort, so a read-only source tree (Nix) still
//! builds.

use std::env;
use std::fs;
use std::path::PathBuf;

fn main() {
    let crate_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR"));

    println!("cargo:rerun-if-changed=src");
    println!("cargo:rerun-if-changed=cbindgen.toml");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=GRIN_FFI_WRITE_HEADER");

    let config = cbindgen::Config::from_file(crate_dir.join("cbindgen.toml"))
        .expect("cbindgen.toml should parse");
    // with_src follows `mod` declarations from lib.rs without running
    // `cargo metadata`, so the build script never invokes cargo again.
    let bindings = cbindgen::Builder::new()
        .with_config(config)
        .with_src(crate_dir.join("src/lib.rs"))
        .generate()
        .expect("cbindgen should generate the header");

    let generated = out_dir.join("grin_ffi.h");
    bindings.write_to_file(&generated);
    println!("cargo:rustc-env=GRIN_FFI_HEADER={}", generated.display());

    if env::var("GRIN_FFI_WRITE_HEADER").as_deref() != Ok("1") {
        return;
    }
    let committed = crate_dir.join("include").join("grin_ffi.h");
    let new = fs::read(&generated).expect("generated header should be readable");
    if fs::read(&committed).ok().as_deref() != Some(new.as_slice()) {
        let _ = fs::create_dir_all(committed.parent().expect("include dir"));
        if let Err(e) = fs::write(&committed, &new) {
            println!(
                "cargo:warning=could not update {}: {e}",
                committed.display()
            );
        }
    }
}
