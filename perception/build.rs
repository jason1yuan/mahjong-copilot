//! macOS link settings:
//!
//! - ONNX Runtime's prebuilt static library calls `__isPlatformVersionAtLeast` (behind
//!   Objective-C `@available`), which lives in clang's runtime library. rustc links with
//!   `-nodefaultlibs`, so that library is linked here explicitly.
//! - screencapturekit's Swift bridge loads the Swift runtime from `@rpath`. Its own build
//!   script adds `/usr/lib/swift` (the system copy) only to its own binaries, so this
//!   crate's tests and examples add it too; the app does the same in its build script.

use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos") {
        return;
    }
    println!("cargo:rustc-link-arg=-Wl,-rpath,/usr/lib/swift");
    let dir = Command::new("xcrun")
        .args(["clang", "-print-resource-dir"])
        .output()
        .ok()
        .filter(|out| out.status.success())
        .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_owned());
    match dir {
        Some(dir) => {
            println!("cargo:rustc-link-search=native={dir}/lib/darwin");
            println!("cargo:rustc-link-lib=static=clang_rt.osx");
        }
        None => println!(
            "cargo:warning=`xcrun clang -print-resource-dir` failed; linking ONNX Runtime may fail"
        ),
    }
}
