use super::*;
use crate::relay::retry::{self, Answer, MAX_BODY, RECREATE};

pub(super) async fn inspect(mut response: UpstreamResponse) -> (UpstreamResponse, Option<Bytes>) {
    let mut buffered = Vec::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(1);
    let complete = loop {
        match tokio::time::timeout_at(deadline, response.body.next()).await {
            Ok(Some(Ok(chunk))) => {
                if buffered.len().saturating_add(chunk.len()) > MAX_BODY {
                    response.body = Box::pin(
                        futures_util::stream::once(async { Ok(chunk) }).chain(response.body),
                    );
                    break false;
                }
                buffered.extend_from_slice(&chunk);
            }
            Ok(None) => break true,
            Ok(Some(Err(error))) => {
                response.body =
                    Box::pin(futures_util::stream::once(async { Err(error) }).chain(response.body));
                break false;
            }
            Err(_) => break false,
        }
    };
    let body = Bytes::from(buffered);
    let saved = (complete
        && content_length(&response.headers)
            .is_none_or(|length| length == u64::try_from(body.len()).unwrap_or(u64::MAX)))
    .then(|| body.clone());
    if !body.is_empty() {
        let prefix = body;
        response.body =
            Box::pin(futures_util::stream::once(async { Ok(prefix) }).chain(response.body));
    }
    (response, saved)
}

pub(super) fn response(answer: Answer) -> UpstreamResponse {
    UpstreamResponse {
        direct: false,
        status: answer.status,
        headers: answer.headers,
        body: Box::pin(futures_util::stream::once(async { Ok(answer.body) })),
    }
}

pub(super) fn terminal() -> UpstreamResponse {
    let body = Bytes::from_static(RECREATE.as_bytes());
    let mut headers = http::HeaderMap::new();
    headers.insert(CONTENT_LENGTH, HeaderValue::from(body.len()));
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("text/plain"));
    headers.insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert("x-vrcr-reason", HeaderValue::from_static(RECREATE));
    headers.insert("retry-after", HeaderValue::from_static("5"));
    response(Answer {
        status: StatusCode::GONE,
        headers,
        body,
        reason: RECREATE,
        until: std::time::SystemTime::UNIX_EPOCH,
    })
}

impl Inner {
    pub(super) fn report_retries(&self, force: bool) {
        if let Some(report) = &self.deps.recovery.report {
            for feedback in self
                .deps
                .stats
                .playback
                .retry_reports((self.deps.now)(), force)
            {
                report(feedback);
            }
        }
    }

    pub(super) fn retire(&self, ex: &mut Exchange, playback: &str) {
        if self.retries.retire(playback, (self.deps.now)()) {
            ex.recovery = true;
            if let Some(invalidate) = &self.deps.recovery.invalidate {
                if let Some(source) = self.deps.stats.playback.source_for_path(&ex.path) {
                    invalidate(&source);
                }
                invalidate(&ex.target);
            }
        }
        ex.retry_reason = Some(retry::RECREATE.to_owned());
    }
}
