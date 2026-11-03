use super::*;

pub(super) fn respond_manifest(
    inner: &Arc<Inner>,
    ex: Exchange,
    resp: UpstreamResponse,
    policy: FirstPartyPolicy,
    content_type: Option<String>,
    guard: InFlight,
    abort: CancellationToken,
) -> Response<Body> {
    let copied = headers::copy_response_headers(&resp.headers, true);
    let mut out = Response::new(Empty::new().map_err(never).boxed());
    *out.status_mut() = resp.status;
    *out.headers_mut() = copied.headers;
    let ct = content_type.unwrap_or_else(|| manifest::DEFAULT_CONTENT_TYPE.to_owned());
    if let Ok(v) = HeaderValue::from_str(&ct) {
        out.headers_mut().insert(CONTENT_TYPE, v);
    }
    inner.diag_verbose(
        &ex.req_id,
        &format!(
            "response-headers passed={} dropped={} ct='{ct}' chunked=true",
            copied.passed, copied.dropped
        ),
    );
    let (tx, body) = channel_body(&abort);
    *out.body_mut() = body;
    let task_inner = std::sync::Arc::clone(inner);
    inner.tasks.spawn(async move {
        let _guard = guard;
        pump_manifest(task_inner, ex, resp.body, policy, tx, abort).await;
    });
    out
}

pub(super) async fn pump_manifest(
    inner: Arc<Inner>,
    ex: Exchange,
    upstream: BodyStream,
    policy: FirstPartyPolicy,
    tx: mpsc::Sender<Bytes>,
    abort: CancellationToken,
) {
    let mut sink = ChannelSink {
        tx,
        idle: inner.idle_timeout,
    };
    let work =
        manifest::localize_stream(&policy, upstream, &mut sink, manifest::MAX_MANIFEST_BYTES);
    let outcome = tokio::select! {
        () = inner.shutdown.cancelled() => None,
        r = tokio::time::timeout(inner.manifest_deadline, work) => Some(r),
    };
    let (bytes_out, failure): (u64, Option<String>) = match outcome {
        None => (0, None),
        Some(Err(_)) => {
            inner.deps.ux.warn(
                Component::Relay,
                &format!("upstream timed out for {}", short_url(&ex.target)),
            );
            (0, Some("upstream_timeout".to_owned()))
        }
        Some(Ok(Err(e))) => (0, Some(sink_failure(&inner, &ex, &e))),
        Some(Ok(Ok(result))) => {
            if result.exceeded {
                inner.deps.ux.warn(
                    Component::Relay,
                    &format!(
                        "manifest body exceeded sanity cap; truncated upstream={}",
                        short_url(&ex.target)
                    ),
                );
            }
            inner.diag_verbose(
                &ex.req_id,
                &format!(
                    "localized-manifest bytes-in={} bytes-out={} changed={}{}",
                    result.bytes_in,
                    result.bytes_out,
                    result.changed,
                    if result.exceeded { " EXCEEDED" } else { "" }
                ),
            );
            inner.deps.stats.record_relay_bytes(
                ex.upstream_class,
                result.bytes_out,
                (inner.deps.now)(),
            );
            (result.bytes_out, None)
        }
    };
    inner.record_hitch(&ex.sample(bytes_out, failure.as_deref()));
    if failure.is_some() {
        abort.cancel();
    }
}

pub(super) fn sink_failure(inner: &Inner, ex: &Exchange, e: &io::Error) -> String {
    match e.kind() {
        io::ErrorKind::TimedOut => {
            inner.deps.ux.warn(
                Component::Relay,
                &format!("client write stalled for {}", short_url(&ex.target)),
            );
            "body_write_timeout".to_owned()
        }
        io::ErrorKind::BrokenPipe => "client_disconnect".to_owned(),
        _ => {
            inner.deps.ux.warn(
                Component::Relay,
                &format!(
                    "request failed for {}: upstream_body_error: {e}",
                    short_url(&ex.target)
                ),
            );
            "upstream_body_error".to_owned()
        }
    }
}

pub(super) fn respond_stream(
    inner: &Arc<Inner>,
    ex: Exchange,
    resp: UpstreamResponse,
    content_type: Option<String>,
    is_head: bool,
    guard: InFlight,
    abort: CancellationToken,
) -> Response<Body> {
    let copied = headers::copy_response_headers(&resp.headers, false);
    let declared = content_length(&resp.headers);
    let mut out = Response::new(Empty::new().map_err(never).boxed());
    *out.status_mut() = resp.status;
    *out.headers_mut() = copied.headers;
    if let Some(v) = content_type
        .filter(|c| !c.trim().is_empty())
        .and_then(|c| HeaderValue::from_str(&c).ok())
    {
        out.headers_mut().insert(CONTENT_TYPE, v);
    }
    if let Some(len) = declared {
        out.headers_mut()
            .insert(CONTENT_LENGTH, HeaderValue::from(len));
    }
    inner.diag_verbose(
        &ex.req_id,
        &format!(
            "response-headers passed={} dropped={} len={}",
            copied.passed,
            copied.dropped,
            declared.map_or_else(|| "null".to_owned(), |l| l.to_string())
        ),
    );
    if is_head {
        inner.record_hitch(&ex.sample(0, None));
        drop(guard);
        return out;
    }
    let (tx, body) = channel_body(&abort);
    *out.body_mut() = body;
    let task_inner = std::sync::Arc::clone(inner);
    inner.tasks.spawn(async move {
        let _guard = guard;
        pump_body(task_inner, ex, resp.body, declared, tx, abort).await;
    });
    out
}

pub(super) async fn pump_body(
    inner: Arc<Inner>,
    ex: Exchange,
    mut upstream: BodyStream,
    declared: Option<u64>,
    tx: mpsc::Sender<Bytes>,
    abort: CancellationToken,
) {
    let mut bytes_out: u64 = 0;
    let mut failure: Option<String> = None;
    let mut stopped = false;
    loop {
        let read = tokio::select! {
            () = inner.shutdown.cancelled() => { stopped = true; break; }
            () = tx.closed() => { failure = Some("client_disconnect".to_owned()); break; }
            r = tokio::time::timeout(inner.idle_timeout, upstream.next()) => r,
        };
        let chunk = match read {
            Err(_) => {
                inner.deps.ux.warn(
                    Component::Relay,
                    &format!("stream idle timeout for {}", short_url(&ex.target)),
                );
                failure = Some("body_idle_timeout".to_owned());
                break;
            }
            Ok(None) => break,
            Ok(Some(Err(e))) => {
                inner.deps.ux.warn(
                    Component::Relay,
                    &format!(
                        "request failed for {}: upstream_body_error: {e}",
                        short_url(&ex.target)
                    ),
                );
                failure = Some("upstream_body_error".to_owned());
                break;
            }
            Ok(Some(Ok(c))) => c,
        };
        let n = u64::try_from(chunk.len()).expect("length fits the byte counter");
        let sent = tokio::select! {
            () = inner.shutdown.cancelled() => { stopped = true; break; }
            r = tokio::time::timeout(inner.idle_timeout, tx.send(chunk)) => r,
        };
        match sent {
            Err(_) => {
                inner.deps.ux.warn(
                    Component::Relay,
                    &format!("client write stalled for {}", short_url(&ex.target)),
                );
                failure = Some("body_write_timeout".to_owned());
                break;
            }
            Ok(Err(_)) => {
                failure = Some("client_disconnect".to_owned());
                break;
            }
            Ok(Ok(())) => {
                bytes_out += n;
                inner
                    .deps
                    .stats
                    .record_relay_bytes(ex.upstream_class, n, (inner.deps.now)());
                if declared.is_some_and(|len| bytes_out >= len) {
                    break;
                }
            }
        }
    }
    if failure.is_none()
        && !stopped
        && let Some(len) = declared
        && bytes_out < len
    {
        inner.deps.ux.warn(
            Component::Relay,
            &format!(
                "upstream ended early ({bytes_out}/{len} bytes) for {}",
                short_url(&ex.target)
            ),
        );
        failure = Some("upstream_short_body".to_owned());
    }
    inner.diag_verbose(
        &ex.req_id,
        &format!(
            "-> {} stream bytes-out={bytes_out} elapsed={}ms",
            ex.status,
            ms(ex.started.elapsed())
        ),
    );
    inner.record_hitch(&ex.sample(bytes_out, failure.as_deref()));
    drop(upstream);
    if failure.is_some() || stopped {
        abort.cancel();
    }
}

pub(super) fn channel_body(abort: &CancellationToken) -> (mpsc::Sender<Bytes>, Body) {
    let (tx, mut rx) = mpsc::channel::<Bytes>(1);
    let abort = abort.clone();
    let stream = futures_util::stream::poll_fn(move |cx| match rx.poll_recv(cx) {
        Poll::Ready(Some(b)) => Poll::Ready(Some(Ok::<_, io::Error>(Frame::data(b)))),
        Poll::Ready(None) if abort.is_cancelled() => Poll::Ready(Some(Err(io::Error::new(
            io::ErrorKind::ConnectionAborted,
            "relay aborted the response",
        )))),
        Poll::Ready(None) => Poll::Ready(None),
        Poll::Pending => Poll::Pending,
    });
    (tx, BodyExt::boxed(StreamBody::new(stream)))
}
