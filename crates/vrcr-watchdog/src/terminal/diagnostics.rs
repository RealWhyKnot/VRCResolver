use super::renderer::Services;
use super::session::SessionStore;

pub(super) fn rows(s: &Services, session: &SessionStore) -> Vec<(String, String)> {
    let shown = |p: &std::path::Path| p.display().to_string();
    let build = crate::app::BuildInfo::new(
        option_env!("VRCR_VERSION").unwrap_or(env!("CARGO_PKG_VERSION")),
        option_env!("VRCR_GIT_SHA"),
        option_env!("VRCR_BUILD_TIME"),
        option_env!("VRCR_CHANNEL"),
        env!("VRCR_RUSTC"),
    );
    vec![
        ("version".to_owned(), build.version),
        ("sha".to_owned(), build.sha),
        ("build time".to_owned(), build.build_time),
        ("os".to_owned(), vrcr_shared::win::os_description()),
        ("rustc".to_owned(), build.runtime),
        (
            "vrc tools".to_owned(),
            s.paths
                .vrc_tools_dir(None)
                .map_or_else(|| "<not found>".to_owned(), |p| shown(&p)),
        ),
        ("install".to_owned(), shown(&s.install_dir)),
        ("state".to_owned(), shown(&s.paths.state_root())),
        ("settings".to_owned(), shown(s.settings.path())),
        ("logs".to_owned(), shown(&s.paths.logs_dir())),
        ("crashes".to_owned(), shown(&s.paths.crashes_dir())),
        ("terminal history".to_owned(), shown(session.history_path())),
        ("this session".to_owned(), shown(session.session_path())),
    ]
}
