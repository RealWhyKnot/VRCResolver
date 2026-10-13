fn main() {
    vrcr_build::export_rustc_version();
    vrcr_build::stamp(&vrcr_build::Exe {
        description: "VRCResolver",
        internal_name: "vrcresolver",
        original_filename: "vrcresolver.exe",
    });
}
