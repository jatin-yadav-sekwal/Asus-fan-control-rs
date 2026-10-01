fn main() {
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if target_os != "windows" {
        return;
    }

    // Win32 resources: icon and version info.
    //
    // This lives in `asus-driver` — a dependency of every binary in the
    // workspace — so exactly one `resource.lib` of ours exists on the link
    // line. Adding it here *and* in `asus-fan-app` produced two VERSION
    // resources and CVT1100 "duplicate resource" failures. Cargo propagates
    // `rustc-link-lib` from a dependency's build script to its dependents, so
    // the GUI inherits these resources too.
    //
    // No manifest is set: `gpui`'s build script already embeds one through
    // `embed-resource`, and a second RT_MANIFEST collides with it the same
    // way. Elevation happens at runtime instead (`ensure_elevated()`), which
    // is what the application has always done.
    let mut res = winres::WindowsResource::new();

    let icon = std::path::Path::new(&std::env::var("CARGO_MANIFEST_DIR").unwrap_or_default())
        .join("..")
        .join("..")
        .join("assets")
        .join("app.ico");
    if icon.exists() {
        res.set_icon(&icon.to_string_lossy());
    } else {
        println!(
            "cargo:warning={} not found; run tools/svg2ico to regenerate it (default icon used)",
            icon.display()
        );
    }

    res.set("ProductName", "Asus Fan Control");
    // One build script covers every binary in the workspace, so only
    // package-level identity is set — no per-file OriginalFilename.
    res.set("FileDescription", "Asus Fan Control");
    res.set(
        "LegalCopyright",
        "Copyright (c) 2023 Karmel0x and contributors; MIT",
    );

    if let Err(e) = res.compile() {
        println!("cargo:warning=failed to embed Windows resources: {}", e);
    }
}
