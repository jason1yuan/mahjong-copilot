//! ONNX Runtime's prebuilt static library calls `__isPlatformVersionAtLeast` (behind
//! Objective-C `@available`), which lives in clang's runtime library. rustc links with
//! `-nodefaultlibs`, so on macOS that library is linked here explicitly.

use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos") {
        return;
    }
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
