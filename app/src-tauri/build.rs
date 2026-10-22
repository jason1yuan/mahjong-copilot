fn main() {
    // perception's ScreenCaptureKit bridge loads the Swift runtime from `@rpath`; the
    // system copy lives in /usr/lib/swift. Link arguments do not pass on from
    // dependencies, so the app adds the path itself.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        println!("cargo:rustc-link-arg=-Wl,-rpath,/usr/lib/swift");
    }
    tauri_build::build()
}
