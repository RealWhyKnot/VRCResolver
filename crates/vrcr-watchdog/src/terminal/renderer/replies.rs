use super::*;

impl Renderer {
    pub fn log_ready(&self) {
        let message = "Type help for commands.";
        self.inner
            .services
            .ux
            .log_success(Component::Terminal, message);
    }

    pub fn info(&self, message: &str) {
        self.inner
            .services
            .ux
            .log_write(Component::Terminal, message);
        self.inner.services.ux.notice(Color::Gray, message);
    }

    pub fn success(&self, message: &str) {
        self.inner
            .services
            .ux
            .log_success(Component::Terminal, message);
        self.inner.services.ux.notice(Color::White, message);
    }

    pub fn warn(&self, message: &str) {
        self.inner
            .services
            .ux
            .log_warn(Component::Terminal, message);
        self.inner.services.ux.notice(Color::Yellow, message);
    }

    pub fn error(&self, message: &str) {
        self.inner
            .services
            .ux
            .log_error(Component::Terminal, message);
        self.inner.services.ux.notice(Color::Red, message);
    }
}
