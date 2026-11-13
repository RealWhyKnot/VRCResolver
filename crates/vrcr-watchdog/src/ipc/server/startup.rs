use vrcr_shared::console::{Component, Notice, Ux};

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
    use super::*;

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
