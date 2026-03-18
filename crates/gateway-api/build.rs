fn main() {
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();

    if target_os == "macos" {
        // Prebuilt ONNX Runtime includes CoreML EP which requires CoreML.framework.
        // This is needed when building with the omniparser feature.
        println!("cargo:rustc-link-lib=framework=CoreML");
        // MLComputePlan was introduced in macOS 14.4 / Xcode 15.3.
        // Use -U to allow the specific unresolved symbol when building
        // with older Xcode SDKs. At runtime, CoreML EP auto-detects availability.
        println!("cargo:rustc-link-arg=-Wl,-U,_OBJC_CLASS_$_MLComputePlan");
    }
}
