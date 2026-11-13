use super::*;

impl PlaybackLog {
    pub fn playback_error(&self, path: &str, error: &PlaybackError, ux: &Ux) {
        let Some(playback) = crate::relay::activity::play_id(path) else {
            return;
        };
        let alias = format!("play-{}", token(playback));
        let mut plays = self.lock();
        let Some(play) = plays
            .values_mut()
            .filter(|play| play.aliases.contains(&alias))
            .max_by_key(|play| play.started)
        else {
            return;
        };
        let host = (error.message().is_some()
            && play
                .console_errors
                .insert((playback.to_owned(), error.clone())))
        .then(|| play.host.clone());
        drop(plays);
        if let Some(host) = host {
            ux.playback_error(&host, error);
        }
    }

    pub fn begin_request(
        &self,
        path: &str,
        target: &str,
        request_id: &str,
        now: SystemTime,
        ux: &Ux,
    ) {
        let target_key = url_key(target);
        let playback = crate::relay::activity::play_id(path)
            .map(token)
            .unwrap_or_else(|| "unknown".to_owned());
        let alias = format!("play-{playback}");
        let mut plays = self.lock();
        let key = plays
            .iter()
            .filter(|(_, play)| play.aliases.contains(&alias) || play.aliases.contains(&target_key))
            .max_by_key(|(_, play)| (play.aliases.contains(&alias), play.started))
            .map_or_else(|| playback.clone(), |(key, _)| key.clone());
        let lines = if plays.contains_key(&key) {
            Vec::new()
        } else {
            Self::make_room(&mut plays, now)
        };
        let play = plays
            .entry(key.clone())
            .or_insert_with(|| Play::new(key, target, now));
        play.alias(alias);
        play.seen = now;
        play.active.insert(request_id.to_owned());
        drop(plays);
        emit(ux, lines);
    }

    pub fn request(&self, sample: &TimingSample, reasons: &str, now: SystemTime, ux: &Ux) {
        let target = url_key(&sample.target_url);
        let playback = crate::relay::activity::play_id(&sample.local_path)
            .map(token)
            .unwrap_or_else(|| "unknown".to_owned());
        let alias = format!("play-{playback}");
        let mut plays = self.lock();
        let key = plays
            .iter()
            .filter(|(_, play)| play.aliases.contains(&alias) || play.aliases.contains(&target))
            .max_by_key(|(_, play)| (play.aliases.contains(&alias), play.started))
            .map_or_else(|| playback.clone(), |(key, _)| key.clone());
        let mut lines = if plays.contains_key(&key) {
            Vec::new()
        } else {
            Self::make_room(&mut plays, now)
        };
        let play = plays
            .entry(key.clone())
            .or_insert_with(|| Play::new(key.clone(), &sample.target_url, now));
        play.alias(alias);
        play.seen = now;
        play.active.remove(&sample.request_id);
        if matches!(sample.status, 200 | 206) && sample.failure.is_none() && !sample.suppressed {
            play.console_errors
                .retain(|(stream, _)| *stream != playback);
        }
        if sample.suppressed {
            play.context.suppressed_requests += 1;
        }
        if sample.recovery {
            play.context.recovery_attempts += 1;
            play.context.recovery_result = "clean_failure".to_owned();
        }
        if let Some(reason) = &sample.retry_reason {
            play.context.retry_reason = token(reason);
        }
        let segment_retry = reasons.split(',').any(|reason| reason == "segment-retry");
        if segment_retry {
            play.retries += 1;
        }
        play.bytes = play.bytes.saturating_add(sample.bytes_out);
        let kind = match relay_kind(sample) {
            RelayKind::Manifest => {
                play.manifests += 1;
                play.manifest_max = play.manifest_max.max(sample.total_ms);
                "manifest"
            }
            RelayKind::Segment => {
                play.segments += 1;
                play.segment_max = play.segment_max.max(sample.total_ms);
                "segment"
            }
            RelayKind::Other => {
                play.other += 1;
                if sample.method == "CONNECT" {
                    "connection"
                } else {
                    "other"
                }
            }
        };
        for reason in reasons.split(',').filter(|reason| !reason.is_empty()) {
            count(&mut play.reasons, token(reason));
        }
        if !matches!(sample.status, 200 | 206) {
            count(&mut play.statuses, sample.status.to_string());
        }
        play.context.request_id.clone_from(&sample.request_id);
        if !sample.stream_id.is_empty() {
            play.context.stream_id = token(&sample.stream_id);
        }
        play.context.upstream_status = sample.status;
        play.context.source =
            if sample.status >= 400 {
                "server"
            } else if sample.failure.as_deref().is_some_and(|reason| {
                reason.starts_with("client") || reason.starts_with("body_write")
            }) {
                "client"
            } else {
                "unknown"
            }
            .to_owned();
        if let Some(shape) = &sample.playlist_shape {
            let identity = crate::relay::live::history::identity(&sample.target_url);
            if play.playlists.len() < 16 && play.playlists.insert(identity) {
                lines.push(format!(
                    "playlist req={} playback={} stream={} host={} {shape}",
                    sample.request_id,
                    play.id,
                    play.context.stream_id,
                    host(&sample.target_url)
                ));
            }
        }
        if sample.status >= 300 || sample.failure.is_some() {
            let reason = token(sample.failure.as_deref().unwrap_or("upstream_status"));
            let key = format!("{kind}-{}-{reason}", sample.status);
            if play.failures.len() >= 64
                && !play.failures.contains_key(&key)
                && let Some(oldest) = play
                    .failures
                    .iter()
                    .min_by_key(|(_, repeat)| repeat.at)
                    .map(|(key, _)| key.clone())
                && let Some(repeat) = play.failures.remove(&oldest)
                && repeat.reported != repeat.count
            {
                lines.push(format!(
                    "{} count={} suppressed={}",
                    repeat.line, repeat.count, play.context.suppressed_requests
                ));
            }
            {
                let retry = segment_retry
                    || play
                        .failures
                        .get(&key)
                        .is_some_and(|repeat| repeat.target == target);
                if retry && !segment_retry {
                    play.retries += 1;
                }
                let line = format!(
                    "request failed req={} playback={} stream={} kind={kind} host={} status={} elapsed_ms={} bytes={} retry={} sent={} reason={reason} hitches={} body={}",
                    sample.request_id,
                    play.id,
                    play.context.stream_id,
                    host(&sample.target_url),
                    sample.status,
                    sample.total_ms,
                    sample.bytes_out,
                    retry,
                    sample.player_status,
                    if reasons.is_empty() { "none" } else { reasons },
                    if sample.player_status == 0 {
                        "unknown"
                    } else if sample.status == 0 {
                        "empty"
                    } else if sample.failure.as_deref() == Some("manifest_size_limit") {
                        "truncated"
                    } else if sample.failure.is_some() {
                        "aborted"
                    } else {
                        "complete"
                    },
                );
                let repeat = play.failures.entry(key).or_insert_with(|| Repeat {
                    count: 0,
                    reported: 0,
                    at: now,
                    line: String::new(),
                    target: target.clone(),
                });
                repeat.target.clone_from(&target);
                repeat.count += 1;
                repeat.line = if let Some(reason) = &sample.retry_reason {
                    format!(
                        "{line} retry_reason={} recovery={}",
                        token(reason),
                        if sample.recovery {
                            "clean_failure"
                        } else {
                            "none"
                        }
                    )
                } else {
                    line
                };
                if repeat.reported == 0
                    || now
                        .duration_since(repeat.at)
                        .is_ok_and(|duration| duration >= REPEAT)
                {
                    repeat.at = now;
                    repeat.reported = repeat.count;
                    lines.push(format!(
                        "{} count={} suppressed={}",
                        repeat.line, repeat.count, play.context.suppressed_requests
                    ));
                }
            }
        }
        let ending = (play.active.is_empty())
            .then(|| play.end_reason.clone())
            .flatten();
        if let Some(reason) = ending
            && let Some(play) = plays.remove(&key)
        {
            lines.extend(play.close(&reason, now));
        }
        drop(plays);
        emit(ux, lines);
    }
}
