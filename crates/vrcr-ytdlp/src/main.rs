use std::process::ExitCode;

use vrcr_ytdlp::WRAPPER_MARKER;
use vrcr_ytdlp::shim::{Shim, ShimConfig, exec_og_without_runtime};

#[used]
static MARKER: [u8; WRAPPER_MARKER.len()] = WRAPPER_MARKER;

const VERSION: &str = match option_env!("VRCR_VERSION") {
    Some(v) => v,
    None => env!("CARGO_PKG_VERSION"),
};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args_os()
        .skip(1)
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    let cfg = ShimConfig::from_env(VERSION);
    let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    else {
        return exit_code(exec_og_without_runtime(&cfg, &args));
    };
    let mut shim = Shim::new(
        cfg,
        Box::new(std::io::stdout()),
        Box::new(std::io::stderr()),
    );
    let code = runtime.block_on(shim.run(&args));
    std::hint::black_box(&MARKER);
    exit_code(code)
}

fn exit_code(code: i32) -> ExitCode {
    ExitCode::from(u8::try_from(code & 0xFF).unwrap_or(1))
}
