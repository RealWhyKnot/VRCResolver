use std::process::Command;

fn main() {
    println!("cargo::rerun-if-changed=app.manifest");
    for var in ["VRCR_VERSION", "VRCR_GIT_SHA", "VRCR_BUILD_TIME", "VRCR_DEV"] {
        println!("cargo::rerun-if-env-changed={var}");
    }
    let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".to_owned());
    let runtime = Command::new(rustc)
        .arg("-V")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|v| v.split_whitespace().take(2).collect::<Vec<_>>().join(" "))
        .filter(|v: &String| !v.is_empty())
        .unwrap_or_else(|| "rustc".to_owned());
    println!("cargo::rustc-env=VRCR_RUSTC={runtime}");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let version = std::env::var("VRCR_VERSION").unwrap_or_else(|_| "0.0.0.0".to_owned());
    let mut res = winresource::WindowsResource::new();
    res.set("CompanyName", "RealWhyKnot")
        .set("ProductName", "VRCResolver")
        .set("FileDescription", "VRCResolver")
        .set("InternalName", "vrcresolver")
        .set("OriginalFilename", "vrcresolver.exe")
        .set("FileVersion", &version)
        .set("ProductVersion", &version)
        .set_manifest_file("app.manifest");
    if let Err(e) = res.compile() {
        eprintln!("version resource failed: {e}");
        std::process::exit(1);
    }
}
