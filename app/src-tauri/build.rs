use std::process::Command;

fn main() {
    // perception's ScreenCaptureKit bridge loads the Swift runtime from `@rpath`; the
    // system copy lives in /usr/lib/swift. Link arguments do not pass on from
    // dependencies, so the app adds the path itself.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        println!("cargo:rustc-link-arg=-Wl,-rpath,/usr/lib/swift");
    }
    // The commit built, for the dev area's build info: from git when the repository is
    // there, else from CI, else "unknown". Local git only; nothing goes online.
    println!("cargo:rustc-env=BUILD_COMMIT={}", commit());
    for path in [
        "../../.git/HEAD",
        "../../.git/refs/heads",
        "../../.git/index",
    ] {
        println!("cargo:rerun-if-changed={path}");
    }
    tauri_build::build()
}

fn commit() -> String {
    let git = |args: &[&str]| {
        Command::new("git")
            .args(args)
            .output()
            .ok()
            .filter(|out| out.status.success())
            .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_owned())
    };
    if let Some(head) = git(&["rev-parse", "--short=7", "HEAD"]).filter(|h| !h.is_empty()) {
        let dirty =
            git(&["status", "--porcelain", "--untracked-files=no"]).is_some_and(|s| !s.is_empty());
        return if dirty { format!("{head}-dirty") } else { head };
    }
    std::env::var("GITHUB_SHA")
        .ok()
        .and_then(|sha| sha.get(..7).map(str::to_owned))
        .unwrap_or_else(|| "unknown".to_owned())
}
