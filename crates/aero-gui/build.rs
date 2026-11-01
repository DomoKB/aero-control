fn main() {
    println!("cargo:rerun-if-changed=aero-gui.exe.manifest");
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if target_os != "windows" {
        return;
    }
    let manifest = std::path::Path::new("aero-gui.exe.manifest");
    let target_env = std::env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
    let mut manifest_linked = false;
    if target_env == "msvc" {
        if let Ok(full) = manifest.canonicalize() {
            // The linker adds its own asInvoker UAC block unless told not to,
            // which clashes with requireAdministrator in our manifest.
            println!("cargo:rustc-link-arg=/MANIFEST:EMBED");
            println!("cargo:rustc-link-arg=/MANIFESTUAC:NO");
            println!("cargo:rustc-link-arg=/MANIFESTINPUT:{}", full.display());
            manifest_linked = true;
        }
    }
    let mut resource = winresource::WindowsResource::new();
    // WindowsResource takes both numeric and displayed versions from Cargo.
    resource.set("ProductName", "aero-control");
    resource.set("OriginalFilename", "aero-gui.exe");
    if !manifest_linked {
        // MSVC embeds its manifest above; duplicating it here would collide.
        resource.set_manifest_file("aero-gui.exe.manifest");
    }
    resource
        .compile()
        .expect("Windows version/manifest resource compilation failed");
}
