//! Checks the committed C header: it must match what cbindgen generates now,
//! and it must compile as C11 and C++11 with warnings as errors, both with
//! and without GRIN_FFI_TEST_UTILS.

use std::path::{Path, PathBuf};
use std::process::Command;

fn committed_header() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("include")
        .join("grin_ffi.h")
}

fn find_compiler(candidates: &[&str]) -> Option<String> {
    candidates
        .iter()
        .find(|c| {
            Command::new(c)
                .arg("--version")
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false)
        })
        .map(|c| (*c).to_owned())
}

const WRAPPER: &str = r#"
#include "grin_ffi.h"

int grin_header_probe(void);
int grin_header_probe(void) {
    GrinFfiResult (*init_logs)(const char *) = grin_init_logs;
    void (*finalizer)(void *) = grin_wallet_close_finalizer;
    GrinErrorCode code = GRIN_ERROR_CODE_WALLET_BUSY;
#ifdef GRIN_FFI_TEST_UTILS
    GrinFfiResult (*panic_fn)(void) = grin_test_panic;
    (void)panic_fn;
#endif
    (void)init_logs;
    (void)finalizer;
    return (int)code;
}
"#;

fn compile(compiler: &str, std: &str, ext: &str, define_test_utils: bool) {
    let dir = tempfile::tempdir().expect("tempdir");
    let src = dir.path().join(format!("probe.{ext}"));
    std::fs::write(&src, WRAPPER).expect("write probe");
    let include = committed_header()
        .parent()
        .expect("include dir")
        .to_path_buf();
    let mut cmd = Command::new(compiler);
    cmd.arg(format!("-std={std}"))
        .args(["-Wall", "-Wextra", "-pedantic", "-Werror", "-fsyntax-only"])
        .arg("-I")
        .arg(&include);
    if define_test_utils {
        cmd.arg("-DGRIN_FFI_TEST_UTILS");
    }
    cmd.arg(&src);
    let out = cmd.output().expect("run compiler");
    assert!(
        out.status.success(),
        "{compiler} -std={std} (test utils: {define_test_utils}) rejected grin_ffi.h:\n{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn committed_header_matches_generated() {
    let generated = std::fs::read_to_string(env!("GRIN_FFI_HEADER")).expect("generated header");
    let committed = std::fs::read_to_string(committed_header()).expect("committed header");
    assert!(
        generated == committed,
        "include/grin_ffi.h is stale; rebuild the crate in a writable tree and commit the result"
    );
}

#[test]
fn header_exports_expected_functions() {
    let header = std::fs::read_to_string(committed_header()).expect("committed header");
    let count = header
        .lines()
        .filter(|l| {
            let l = l.trim_start();
            (l.starts_with("struct GrinFfiResult grin_") || l.starts_with("void grin_"))
                && l.contains('(')
        })
        .count();
    assert_eq!(
        count, 38,
        "35 standard functions plus 3 test-utils functions"
    );
    assert_eq!(
        header.matches("#if defined(GRIN_FFI_TEST_UTILS)").count(),
        3
    );
}

#[test]
fn header_compiles_as_c11() {
    let Some(cc) = find_compiler(&["cc", "gcc", "clang"]) else {
        eprintln!("SKIPPED: no C compiler (cc, gcc or clang) found on PATH");
        return;
    };
    compile(&cc, "c11", "c", false);
    compile(&cc, "c11", "c", true);
}

#[test]
fn header_compiles_as_cpp11() {
    let Some(cxx) = find_compiler(&["c++", "g++", "clang++"]) else {
        eprintln!("SKIPPED: no C++ compiler (c++, g++ or clang++) found on PATH");
        return;
    };
    compile(&cxx, "c++11", "cpp", false);
    compile(&cxx, "c++11", "cpp", true);
}
