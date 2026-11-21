use tokio::time::Instant;
use vrcr_shared::console::{Component, Notice, Ux};

#[derive(Debug, Default)]
pub(super) struct PipeFailures {
    count: u32,
    last_warning: Option<Instant>,
}

impl PipeFailures {
    pub(super) fn failed(&mut self, now: Instant) -> Option<u32> {
        self.count = self.count.saturating_add(1);
        super::should_warn_at_capacity(&mut self.last_warning, now).then_some(self.count)
    }

    pub(super) fn recovered(&mut self) -> u32 {
        self.last_warning = None;
        std::mem::take(&mut self.count)
    }
}

pub(super) fn create_failure_line(count: u32, error: &std::io::Error) -> String {
    if count <= 1 {
        format!("could not create pipe instance: {error}")
    } else {
        format!("could not create pipe instance ({count} tries so far): {error}")
    }
}

pub(super) const CONNECT_BACKOFF: std::time::Duration = std::time::Duration::from_millis(250);

pub(super) fn connect_failure_line(count: u32, error: &std::io::Error) -> String {
    if count <= 1 {
        format!("accept failed: {error}")
    } else {
        format!("accept failed ({count} tries so far): {error}")
    }
}

pub(super) fn pipe_security(sid: std::io::Result<String>, ux: &Ux) -> Option<String> {
    match sid {
        Ok(sid) => Some(crate::ipc::pipe::pipe_sddl(&sid)),
        Err(error) => {
            ux.event(Notice::StartupFailed);
            ux.log_error(
                Component::Ipc,
                &format!("could not resolve current user SID: {error}"),
            );
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[test]
    fn pipe_create_failures_warn_once_a_minute_and_reset_after_recovery() {
        let start = Instant::now();
        let mut failures = PipeFailures::default();
        assert_eq!(failures.failed(start), Some(1));
        let quiet: Vec<Option<u32>> = (1..60)
            .map(|second| failures.failed(start + Duration::from_secs(second)))
            .collect();
        assert_eq!(quiet, vec![None; 59]);
        assert_eq!(failures.failed(start + Duration::from_secs(60)), Some(61));
        assert_eq!(failures.recovered(), 61);
        assert_eq!(failures.recovered(), 0);
        assert_eq!(failures.failed(start + Duration::from_secs(61)), Some(1));
        let error = std::io::Error::other("busy");
        assert_eq!(
            create_failure_line(1, &error),
            "could not create pipe instance: busy"
        );
        assert_eq!(
            create_failure_line(61, &error),
            "could not create pipe instance (61 tries so far): busy"
        );
    }

    #[test]
    fn pipe_connect_failures_back_off_and_warn_once_a_minute() {
        let start = Instant::now();
        let mut failures = PipeFailures::default();
        let warned: Vec<Option<u32>> = (0..240)
            .map(|step| failures.failed(start + CONNECT_BACKOFF * step))
            .collect();
        assert_eq!(warned.into_iter().flatten().collect::<Vec<u32>>(), vec![1]);
        assert_eq!(failures.failed(start + Duration::from_secs(60)), Some(241));
        assert_eq!(CONNECT_BACKOFF, Duration::from_millis(250));
        let error = std::io::Error::other("pipe broken");
        assert_eq!(
            connect_failure_line(1, &error),
            "accept failed: pipe broken"
        );
        assert_eq!(
            connect_failure_line(241, &error),
            "accept failed (241 tries so far): pipe broken"
        );
    }

    #[test]
    fn sid_failure_reports_startup_problem_and_keeps_technical_log() {
        let dir = tempfile::TempDir::new().expect("test directory");
        let console = dir.path().join("console.txt");
        let detail = dir.path().join("detail.log");
        let ux = Ux::new(
            Box::new(std::fs::File::create(&console).expect("console file")),
            vrcr_shared::console::Style::plain(),
            || "12:34:56".to_owned(),
        );
        ux.tee_to(vrcr_shared::log::Log::to_file(&detail));
        assert_eq!(
            pipe_security(Ok("S-1-5-21-test".to_owned()), &ux),
            Some(crate::ipc::pipe::pipe_sddl("S-1-5-21-test"))
        );
        assert_eq!(
            std::fs::read_to_string(&console).expect("console output"),
            ""
        );
        assert!(pipe_security(Err(std::io::Error::other("test SID failure")), &ux).is_none());
        assert_eq!(
            std::fs::read_to_string(console).expect("console output"),
            "12:34:56  VRCResolver couldn't start. Try opening it again.\n"
        );
        assert!(
            std::fs::read_to_string(detail)
                .expect("log output")
                .contains("[ipc][err] could not resolve current user SID: test SID failure")
        );
    }
}
