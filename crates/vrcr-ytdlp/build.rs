fn main() {
    println!("cargo::rerun-if-changed=app.manifest");
    println!("cargo::rerun-if-env-changed=VRCR_VERSION");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let version = std::env::var("VRCR_VERSION").unwrap_or_else(|_| "0.0.0.0".to_owned());
    let mut res = winresource::WindowsResource::new();
    res.set("CompanyName", "RealWhyKnot")
        .set("ProductName", "VRCResolver")
        .set("FileDescription", "VRCResolver yt-dlp shim")
        .set("InternalName", "yt-dlp")
        .set("OriginalFilename", "yt-dlp.exe")
        .set("FileVersion", &version)
        .set("ProductVersion", &version)
        .set_manifest_file("app.manifest");
    if let Err(e) = res.compile() {
        eprintln!("version resource failed: {e}");
        std::process::exit(1);
    }
}
