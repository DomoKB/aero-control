fn main() {
    println!("cargo:rerun-if-changed=aero-gui.exe.manifest");
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if target_os != "windows" {
        return;
    }
    let manifest = std::path::Path::new("aero-gui.exe.manifest");
    let target_env = std::env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
    if target_env == "msvc" {
        if let Ok(full) = manifest.canonicalize() {
            // The linker adds its own asInvoker UAC block unless told not to,
            // which clashes with requireAdministrator in our manifest.
            println!("cargo:rustc-link-arg=/MANIFEST:EMBED");
            println!("cargo:rustc-link-arg=/MANIFESTUAC:NO");
            println!("cargo:rustc-link-arg=/MANIFESTINPUT:{}", full.display());
            // The linker embeds the manifest; a second copy from a resource would collide.
            return;
        }
    }
    let mut resource = winresource::WindowsResource::new();
    resource.set_manifest_file("aero-gui.exe.manifest");
    if let Err(err) = resource.compile() {
        println!(
            "cargo:warning=aero-gui manifest resource was not compiled ({err}). MSVC builds still pass /MANIFESTINPUT, and a non-admin token stays read-only."
        );
    }
}
