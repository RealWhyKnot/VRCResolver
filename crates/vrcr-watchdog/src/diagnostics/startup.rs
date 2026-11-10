use std::fs;
use std::path::Path;

use vrcr_shared::console::Style;

pub const MARKER: &str = "watchdog-last-run";

pub fn begin(state: &Path) -> String {
    let path = state.join(MARKER);
    let previous = fs::metadata(&path)
        .ok()
        .filter(|metadata| metadata.len() <= 128)
        .and_then(|_| fs::read_to_string(&path).ok())
        .map(|text| text.trim().to_owned());
    let reason = match previous.as_deref() {
        None => "unknown".to_owned(),
        Some("running") => "unclean_unknown".to_owned(),
        Some("panic_observed") => "panic_observed".to_owned(),
        Some(value) if value.starts_with("shutdown-") => super::token(value),
        _ => "unknown".to_owned(),
    };
    record(state, "running");
    reason
}

pub fn record(state: &Path, reason: &str) {
    let _ = fs::write(state.join(MARKER), super::token(reason));
}

pub fn console_style(redirected: bool, no_color: Option<&str>) -> Style {
    Style::detect(redirected, no_color, Some("1"))
}

pub struct Environment<'a> {
    pub version: &'a str,
    pub os: &'a str,
    pub server: &'a str,
    pub port: u16,
    pub tls: &'a str,
    pub hosts: &'a str,
    pub vrchat: bool,
    pub previous: &'a str,
}

pub fn line(environment: &Environment<'_>) -> String {
    let os = environment
        .os
        .chars()
        .filter(|character| {
            character.is_ascii_alphanumeric() || *character == '.' || *character == ' '
        })
        .collect::<String>()
        .replace(' ', "_");
    format!(
        "environment version={} os={} server={} relay_port={} tls={} hosts={} vrchat={} player=not_seen previous={}",
        super::token(environment.version),
        super::token(&os),
        super::host(&format!("https://{}", environment.server)),
        environment.port,
        super::token(environment.tls),
        super::token(environment.hosts),
        if environment.vrchat {
            "running"
        } else {
            "not_seen"
        },
        super::token(environment.previous),
    )
}
