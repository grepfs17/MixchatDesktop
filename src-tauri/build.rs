fn main() {
    // The target triple names the bundled Node sidecar in dev builds.
    let target = std::env::var("TARGET").unwrap_or_default();
    println!("cargo:rustc-env=TARGET_TRIPLE={target}");
    tauri_build::build()
}
