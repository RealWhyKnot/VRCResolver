use std::process::Command;

pub const BUILD_VARS: [&str; 4] = ["VRCR_VERSION", "VRCR_GIT_SHA", "VRCR_BUILD_TIME", "VRCR_DEV"];

pub struct Exe<'a> {
    pub description: &'a str,
    pub internal_name: &'a str,
    pub original_filename: &'a str,
}

pub fn numeric_version(version: &str) -> [u16; 4] {
    let head = version.trim().trim_start_matches(['v', 'V']);
    let head = head.split_once('-').map_or(head, |(h, _)| h);
    let parts: Vec<&str> = head.split('.').collect();
    let mut out = [0u16; 4];
    if parts.is_empty() || parts.len() > 4 {
        return out;
    }
    for (slot, part) in out.iter_mut().zip(&parts) {
        match part.parse::<u16>() {
            Ok(n) => *slot = n,
            Err(_) => return [0; 4],
        }
    }
    out
}

pub fn file_version(version: &str) -> String {
    let [a, b, c, d] = numeric_version(version);
    format!("{a}.{b}.{c}.{d}")
}

pub fn packed_version(version: &str) -> u64 {
    numeric_version(version)
        .iter()
        .fold(0u64, |acc, part| (acc << 16) | u64::from(*part))
}

pub fn rustc_version() -> String {
    let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".to_owned());
    Command::new(rustc)
        .arg("-V")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|v| v.split_whitespace().take(2).collect::<Vec<_>>().join(" "))
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| "rustc".to_owned())
}

pub fn export_rustc_version() {
    println!("cargo::rustc-env=VRCR_RUSTC={}", rustc_version());
}

pub fn stamp(exe: &Exe<'_>) {
    println!("cargo::rerun-if-changed=app.manifest");
    for var in BUILD_VARS {
        println!("cargo::rerun-if-env-changed={var}");
    }
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let version = std::env::var("VRCR_VERSION").unwrap_or_else(|_| "0.0.0.0".to_owned());
    let numeric = file_version(&version);
    let packed = packed_version(&version);
    let mut res = winresource::WindowsResource::new();
    res.set("CompanyName", "RealWhyKnot")
        .set("ProductName", "VRCResolver")
        .set("FileDescription", exe.description)
        .set("InternalName", exe.internal_name)
        .set("OriginalFilename", exe.original_filename)
        .set("FileVersion", &numeric)
        .set("ProductVersion", &version)
        .set_version_info(winresource::VersionInfo::FILEVERSION, packed)
        .set_version_info(winresource::VersionInfo::PRODUCTVERSION, packed)
        .set_manifest_file("app.manifest");
    if let Err(e) = res.compile() {
        eprintln!("version resource failed: {e}");
        std::process::exit(1);
    }
}
