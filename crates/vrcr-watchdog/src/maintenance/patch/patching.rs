use super::*;

impl PatchManager {
    pub(super) fn emit(&self, st: &mut TickState, outcome: TickOutcome, message: &str) {
        if st.last_outcome == outcome {
            return;
        }
        st.last_outcome = outcome;
        self.ux.log_write(Component::Patch, strip_prefix(message));
    }

    pub(super) fn emit_file_only(&self, st: &mut TickState, outcome: TickOutcome, message: &str) {
        if st.last_outcome == outcome {
            return;
        }
        st.last_outcome = outcome;
        self.log.line(message);
    }

    pub(super) fn observe(
        &self,
        st: &mut TickState,
        target: &Path,
        exists: bool,
    ) -> (bool, Option<Stamp>) {
        let stamp = if exists { stamp_of(target) } else { None };
        let settled = stamp.is_some() && stamp == st.previous;
        st.previous = stamp;
        (settled, stamp)
    }

    pub(super) fn ready_to_capture(
        &self,
        st: &mut TickState,
        target: &Path,
        settled: bool,
        stamp: Option<Stamp>,
    ) -> bool {
        if !settled {
            return false;
        }
        if wrapper_identity::looks_complete(target) {
            return true;
        }
        if st.incomplete_reported != stamp {
            st.incomplete_reported = stamp;
            self.ux.log_write(
                Component::Patch,
                &format!(
                    "{TARGET_NAME} does not look like a complete download yet; leaving it alone until it does"
                ),
            );
        }
        false
    }

    pub(super) fn report_lock_failure(&self, st: &mut TickState, stage: &str, e: &io::Error) {
        st.lock_failures += 1;
        let detail = format!(
            "{}: {}",
            kind_text(e),
            sanitize_for_console(&e.to_string(), 120)
        );
        if st.lock_failures == 3 {
            self.ux.log_warn(
                Component::Patch,
                &format!(
                    "{stage} has failed 3 times in a row -- possible antivirus interference or permissions issue. Last error: {detail}"
                ),
            );
        } else if st.lock_failures > 3 && st.lock_failures.is_multiple_of(20) {
            self.ux.log_warn(
                Component::Patch,
                &format!(
                    "{stage} still failing after {} ticks ({detail})",
                    st.lock_failures
                ),
            );
        }
    }

    pub(super) fn tick_core(&self, st: &mut TickState) {
        let Some(dir) = self.tools_dir.as_deref() else {
            return;
        };
        let target = dir.join(TARGET_NAME);
        let backup = dir.join(OG_NAME);
        let mut target_exists = target.is_file();
        let backup_exists = backup.is_file();
        let (settled, stamp) = self.observe(st, &target, target_exists);

        if !backup_exists {
            if !target_exists {
                self.emit(
                    st,
                    TickOutcome::Waiting,
                    "[patch] tick: VRChat has not installed yt-dlp yet -- waiting",
                );
                return;
            }
            match self.classify(st, &target) {
                WrapperKind::VrcBundledYtDlp => {
                    if !self.wrapper.is_file() {
                        self.emit(
                            st,
                            TickOutcome::WrapperMissing,
                            "[patch] tick: our yt-dlp wrapper is missing from the install -- leaving VRChat's yt-dlp.exe in place",
                        );
                        return;
                    }
                    if !self.ready_to_capture(st, &target, settled, stamp) {
                        return;
                    }
                    if is_target_in_use(&target) {
                        self.emit(
                            st,
                            TickOutcome::Locked,
                            "[patch] tick: yt-dlp.exe locked (VRChat may be mid-CreateProcess) -- deferring backup creation",
                        );
                        return;
                    }
                    if let Err(e) = tools::move_file(&target, &backup, false) {
                        self.report_lock_failure(st, "backup", &e);
                        return;
                    }
                    self.emit(
                        st,
                        TickOutcome::BackupCreated,
                        "[patch] tick: preserved VRChat's yt-dlp.exe as yt-dlp-og.exe",
                    );
                    target_exists = false;
                }
                WrapperKind::Ours => {
                    if is_target_in_use(&target) {
                        self.emit(
                            st,
                            TickOutcome::Locked,
                            "[patch] tick: yt-dlp.exe locked -- deferring recovery delete",
                        );
                        return;
                    }
                    match fs::remove_file(&target) {
                        Ok(()) => self.emit(
                            st,
                            TickOutcome::BackupLost,
                            "[patch] tick: yt-dlp-og.exe is missing and target is our wrapper -- removed the wrapper so VRChat redownloads its yt-dlp",
                        ),
                        Err(e) => self.report_lock_failure(st, "recovery-delete", &e),
                    }
                    return;
                }
                WrapperKind::Unknown => {
                    self.emit(
                        st,
                        TickOutcome::UnknownTarget,
                        "[patch] tick: Tools/yt-dlp.exe is not classified as ours or VRChat-bundled -- not mutating",
                    );
                    return;
                }
            }
        }

        if !self.wrapper.is_file() {
            if st.last_outcome != TickOutcome::WrapperMissing {
                let restored = restore_ytdlp_in_tools(dir, None, &self.ux, &self.log);
                st.last_outcome = TickOutcome::WrapperMissing;
                self.ux.log_warn(
                    Component::Patch,
                    &format!(
                        "our yt-dlp wrapper is missing from the install; VRChat uses its own resolver until the file is back (restored={restored})"
                    ),
                );
            }
            return;
        }

        if !target_exists {
            if is_target_in_use(&target) {
                self.emit(
                    st,
                    TickOutcome::Locked,
                    "[patch] tick: yt-dlp.exe locked at initial-stage -- deferring",
                );
                return;
            }
            match atomic_copy(&self.wrapper, &target) {
                Ok(()) => {
                    st.last_patch = Some(Instant::now());
                    st.lock_failures = 0;
                    self.emit(
                        st,
                        TickOutcome::InitialStaged,
                        "[patch] tick: wrapper installed in VRChat's Tools folder",
                    );
                }
                Err(e) => self.report_lock_failure(st, "initial-stage", &e),
            }
            return;
        }

        let kind = self.classify(st, &target);
        if kind == WrapperKind::Ours {
            st.lock_failures = 0;
            self.emit_file_only(
                st,
                TickOutcome::Match,
                "[patch] tick: target is our wrapper, no action",
            );
            return;
        }

        if st.last_patch.is_some_and(|t| t.elapsed() < MIN_REAPPLY_GAP) {
            return;
        }

        if kind == WrapperKind::VrcBundledYtDlp {
            if !self.ready_to_capture(st, &target, settled, stamp) {
                return;
            }
            if is_target_in_use(&target) {
                self.emit(
                    st,
                    TickOutcome::Locked,
                    "[patch] tick: yt-dlp.exe locked (VRChat or yt-dlp running) -- deferring re-apply",
                );
                return;
            }
            match atomic_copy(&target, &backup).and_then(|()| atomic_copy(&self.wrapper, &target)) {
                Ok(()) => {
                    st.last_patch = Some(Instant::now());
                    st.lock_failures = 0;
                    self.emit(
                        st,
                        TickOutcome::Reapplied,
                        "[patch] yt-dlp.exe was updated by VRChat -- refreshed yt-dlp-og.exe, wrapper re-applied.",
                    );
                }
                Err(e) => {
                    self.report_lock_failure(st, "re-apply", &e);
                    self.emit(
                        st,
                        TickOutcome::ReapplyFailed,
                        "[patch] tick: re-apply failed (sharing violation) -- retry next tick",
                    );
                }
            }
            return;
        }

        self.emit(
            st,
            TickOutcome::UnknownTarget,
            "[patch] tick: target is neither our wrapper nor VRChat-bundled -- not mutating",
        );
    }
}
