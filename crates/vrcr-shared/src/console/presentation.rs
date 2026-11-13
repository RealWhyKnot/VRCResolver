use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VideoStatus {
    Playing,
    Skipped,
    Stalled,
    Problem,
    Failed,
}

impl VideoStatus {
    pub fn word(self) -> &'static str {
        match self {
            Self::Playing => "Playing",
            Self::Skipped => "Skipped",
            Self::Stalled => "Stalled",
            Self::Problem => "Problem",
            Self::Failed => "Failed",
        }
    }

    pub fn color(self) -> Color {
        match self {
            Self::Playing => Color::Green,
            Self::Skipped | Self::Stalled | Self::Problem => Color::Yellow,
            Self::Failed => Color::Red,
        }
    }
}

pub fn fallback_sentence(reason: &str) -> &'static str {
    match reason {
        "server_unreachable" => "Couldn't reach the server.",
        "resolver_paused" => "VRCResolver is paused after repeated video failures.",
        "prior_load_failure" => "VRChat couldn't play this video through VRCResolver earlier.",
        "client_deadline_exceeded" => "VRCResolver couldn't get this video in time.",
        "internal_error" => "VRCResolver ran into a problem getting this video.",
        "rate_limited" => "The server asked VRCResolver to slow down.",
        "protocol_error" => "The server couldn't understand a request from VRCResolver.",
        "unity_unsupported_format" => {
            "This world uses VRChat's Unity player, which can't play this video. Worlds with the AVPro player can."
        }
        "content_not_found" => "The video was removed or doesn't exist.",
        "validation_recently_failed" => "The server couldn't get this video a moment ago.",
        "resolve_cost_exceeds_deadline" => "The server couldn't get this video in time.",
        _ => "The server couldn't get this video.",
    }
}

pub fn connection_duration(duration: Duration) -> String {
    let seconds = duration.as_secs();
    if seconds >= 3600 {
        format!("{}h {}m", seconds / 3600, seconds % 3600 / 60)
    } else if seconds >= 60 {
        format!("{}m {}s", seconds / 60, seconds % 60)
    } else {
        format!("{seconds}s")
    }
}

impl Ux {
    pub fn log_write(&self, component: Component, body: &str) {
        self.log_stamped(&format!(
            "{} {} {body}",
            self.style.bullet(),
            component.tag()
        ));
    }

    pub fn log_success(&self, component: Component, body: &str) {
        self.log_write(component, body);
    }

    pub fn log_warn(&self, component: Component, body: &str) {
        self.log_stamped(&format!(
            "{} {}[warn] {body}",
            self.style.bullet(),
            component.tag()
        ));
    }

    pub fn log_error(&self, component: Component, body: &str) {
        self.log_stamped(&format!(
            "{} {}[err] {body}",
            self.style.bullet(),
            component.tag()
        ));
    }

    pub fn log_fatal(&self, body: &str) {
        self.log_stamped(&format!("{} [FATAL] {body}", self.style.bullet()));
    }

    pub(super) fn log_stamped(&self, body: &str) {
        if let Some(log) = &self.state().tee {
            log.line(&format!("[{}] {body}", (self.clock)()));
        }
    }

    pub fn notice(&self, color: Color, sentence: &str) {
        self.present(
            &[(Color::DarkGray, format!("{}  ", (self.clock)()))],
            color,
            sentence,
            10,
        );
    }

    pub fn video(&self, status: VideoStatus, host: &str, sentence: &str) {
        let host = crate::logutil::bare_host(&if host.contains("://") {
            host.to_owned()
        } else {
            format!("https://{host}")
        });
        let host = crate::logutil::public_message(&host);
        let host = if host.len() >= 27 {
            format!("{}...", &host[..24])
        } else {
            host
        };
        let width = (self.width)().map(|width| width.saturating_sub(1));
        let narrow = width.is_some_and(|width| width.saturating_sub(49) < 30);
        let mut prefix = vec![
            (Color::DarkGray, format!("{}  ", (self.clock)())),
            (
                status.color(),
                if narrow {
                    status.word().to_owned()
                } else {
                    format!("{:<11}", status.word())
                },
            ),
        ];
        if narrow {
            let room = width.unwrap_or_default().saturating_sub(10);
            let host = if host.len() > room {
                if room >= 3 {
                    format!("{}...", &host[..room - 3])
                } else {
                    host[..room].to_owned()
                }
            } else {
                host
            };
            if width.is_some_and(|width| 12 + status.word().len() + host.len() <= width) {
                prefix.push((Color::Gray, format!("  {host}\n          ")));
            } else {
                let separator = if host.len() == room {
                    "\n          "
                } else {
                    " "
                };
                prefix.push((Color::Gray, format!("\n          {host}{separator}")));
            }
        } else {
            prefix.push((Color::Gray, format!("{host:<28}")));
        }
        let color = if status == VideoStatus::Playing {
            Color::Gray
        } else {
            Color::White
        };
        self.present(&prefix, color, sentence, if narrow { 10 } else { 49 });
    }

    pub fn skipped(&self, host: &str, reason: &str, message: Option<&str>) {
        let clean = crate::logutil::public_message(message.unwrap_or_default());
        let why = if clean.is_empty() {
            fallback_sentence(reason)
        } else {
            &clean
        };
        self.video(
            VideoStatus::Skipped,
            host,
            &format!("{why} VRChat will try it on its own."),
        );
    }

    pub fn resolver_paused(&self) {
        self.notice(Color::Yellow, "Too many videos failed in a row. VRChat plays videos on its own until the server is back.");
    }

    pub fn resolver_restored(&self) {
        self.notice(Color::White, "VRCResolver is playing videos again.");
    }

    fn present(&self, prefix: &[(Color, String)], color: Color, sentence: &str, column: usize) {
        let sentence = crate::logutil::console_sentence(sentence);
        let width = (self.width)().map(|w| w.saturating_sub(1).max(1));
        let indent = width.map_or(column, |w| column.min(w.saturating_sub(1)));
        let mut lines: Vec<Vec<(Color, char)>> = vec![Vec::new()];
        for (color, text) in prefix {
            for ch in text.chars() {
                if ch == '\n' {
                    lines.push(Vec::new());
                    continue;
                }
                if width.is_some_and(|w| lines.last().expect("at least one output line").len() >= w)
                {
                    while lines
                        .last()
                        .expect("at least one output line")
                        .last()
                        .is_some_and(|(_, c)| *c == ' ')
                    {
                        lines.last_mut().expect("at least one output line").pop();
                    }
                    lines.push(vec![(Color::Gray, ' '); indent]);
                }
                lines
                    .last_mut()
                    .expect("at least one output line")
                    .push((*color, ch));
            }
        }
        for (index, word) in sentence.split_whitespace().enumerate() {
            let line = lines.last_mut().expect("at least one output line");
            let gap = usize::from(index > 0);
            if width.is_some_and(|w| {
                line.len() + gap + word.len() > w && word.len() <= w.saturating_sub(indent)
            }) {
                while lines
                    .last()
                    .expect("at least one output line")
                    .last()
                    .is_some_and(|(_, c)| *c == ' ')
                {
                    lines.last_mut().expect("at least one output line").pop();
                }
                lines.push(vec![(color, ' '); indent]);
            } else if gap > 0 && width.is_none_or(|w| line.len() < w) {
                line.push((color, ' '));
            }
            for ch in word.chars() {
                if width.is_some_and(|w| lines.last().expect("at least one output line").len() >= w)
                {
                    lines.push(vec![(color, ' '); indent]);
                }
                lines
                    .last_mut()
                    .expect("at least one output line")
                    .push((color, ch));
            }
        }
        let mut st = self.state();
        clear_overlay(&mut st);
        for line in lines {
            let mut previous = None;
            for (color, ch) in line {
                if self.style.color && previous != Some(color) {
                    let _ = write!(st.out, "{}", color.sgr());
                    previous = Some(color);
                }
                let _ = write!(st.out, "{ch}");
            }
            if self.style.color {
                let _ = write!(st.out, "{RESET}");
            }
            let _ = writeln!(st.out);
        }
        let _ = st.out.flush();
        render_overlay(&mut st);
        drop(st);
    }
}
