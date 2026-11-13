use super::*;

impl Renderer {
    pub fn log_ready(&self) {
        let message = "interactive terminal ready; type /help for commands.";
        self.inner.session.record_output("success", message);
        self.inner
            .services
            .ux
            .log_success(Component::Terminal, message);
    }

    pub fn log_permissions(&self) {
        let message = "permissions: local watchdog commands only; shell execution isn't exposed.";
        self.inner.session.record_output("info", message);
        self.inner
            .services
            .ux
            .log_write(Component::Terminal, message);
    }

    pub fn info(&self, message: &str) {
        self.inner.session.record_output("info", message);
        self.inner
            .services
            .ux
            .log_write(Component::Terminal, message);
        self.inner.services.ux.notice(Color::Gray, message);
    }

    pub fn success(&self, message: &str) {
        self.inner.session.record_output("success", message);
        self.inner
            .services
            .ux
            .log_success(Component::Terminal, message);
        self.inner.services.ux.notice(Color::White, message);
    }

    pub fn warn(&self, message: &str) {
        self.inner.session.record_output("warn", message);
        self.inner
            .services
            .ux
            .log_warn(Component::Terminal, message);
        self.inner.services.ux.notice(Color::Yellow, message);
    }

    pub fn error(&self, message: &str) {
        self.inner.session.record_output("error", message);
        self.inner
            .services
            .ux
            .log_error(Component::Terminal, message);
        self.inner.services.ux.notice(Color::Red, message);
    }
}
