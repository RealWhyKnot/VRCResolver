fn main() {
    let build = vrcr_watchdog::app::BuildInfo::new(
        option_env!("VRCR_VERSION").unwrap_or(env!("CARGO_PKG_VERSION")),
        option_env!("VRCR_GIT_SHA"),
        option_env!("VRCR_BUILD_TIME"),
        option_env!("VRCR_DEV"),
        env!("VRCR_RUSTC"),
    );
    let args: Vec<String> = std::env::args().skip(1).collect();
    #[cfg(windows)]
    std::process::exit(vrcr_watchdog::app::run(build, args));
    #[cfg(not(windows))]
    {
        let _ = (build, args);
        eprintln!("vrcresolver runs on Windows only.");
        std::process::exit(1);
    }
}
