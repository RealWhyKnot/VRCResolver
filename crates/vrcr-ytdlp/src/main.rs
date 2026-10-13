use std::process::ExitCode;

use vrcr_ytdlp::WRAPPER_MARKER;
use vrcr_ytdlp::shim::{Shim, ShimConfig};

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
    let runtime = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
        Ok(rt) => rt,
        Err(_) => return ExitCode::SUCCESS,
    };
    let mut shim = Shim::new(
        ShimConfig::from_env(VERSION),
        Box::new(std::io::stdout()),
        Box::new(std::io::stderr()),
    );
    let code = runtime.block_on(shim.run(&args));
    std::hint::black_box(&MARKER);
    ExitCode::from(u8::try_from(code & 0xFF).unwrap_or(1))
}
