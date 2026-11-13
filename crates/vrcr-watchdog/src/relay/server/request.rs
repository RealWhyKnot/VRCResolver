use super::*;

pub(super) async fn handle(
    inner: Arc<Inner>,
    req: Request<Incoming>,
    abort: CancellationToken,
) -> Response<Body> {
    let started = Instant::now();
    let req_id = inner.next_id("relay");
    let path = req.uri().path().to_owned();
    inner.deps.activity.record(&path);
    if inner.in_flight.fetch_add(1, Ordering::SeqCst) >= MAX_IN_FLIGHT {
        inner.in_flight.fetch_sub(1, Ordering::SeqCst);
        return inner.reject(
            &req,
            &req_id,
            StatusCode::SERVICE_UNAVAILABLE,
            "connection_limit",
            started,
        );
    }
    let guard = InFlight(std::sync::Arc::clone(&inner));
    let method = req.method().clone();
    if method != Method::GET && method != Method::HEAD {
        let mut resp = inner.reject(
            &req,
            &req_id,
            StatusCode::METHOD_NOT_ALLOWED,
            "method_refused",
            started,
        );
        resp.headers_mut()
            .insert(ALLOW, HeaderValue::from_static("GET, HEAD"));
        return resp;
    }
    if req.headers().contains_key(ORIGIN) {
        return inner.reject(
            &req,
            &req_id,
            StatusCode::FORBIDDEN,
            "origin_refused",
            started,
        );
    }
    let host = req.headers().get(HOST).and_then(|h| h.to_str().ok());
    if !security::is_allowed_host_header(host, inner.port) {
        return inner.reject(
            &req,
            &req_id,
            StatusCode::FORBIDDEN,
            "host_refused",
            started,
        );
    }
    if !targets::is_play_path(&path) || crate::relay::activity::play_id(&path).is_none() {
        return inner.reject(
            &req,
            &req_id,
            StatusCode::NOT_FOUND,
            "route_missing",
            started,
        );
    }
    if let Some(token) = crate::local::token(&path) {
        let manifest = inner.deps.manifests.get(token);

        return match manifest {
            Some(body) => {
                let size = u64::try_from(body.len()).unwrap_or(u64::MAX);
                let shape = manifest::LocalizeResult {
                    bytes_in: size,
                    bytes_out: size,
                    kind: manifest::PlaylistKind::Master,
                    ..Default::default()
                }
                .shape();
                inner.record_hitch(&TimingSample {
                    suppressed: false,
                    recovery: false,
                    retry_reason: None,
                    request_id: req_id,
                    stream_id: String::new(),
                    player_status: 200,
                    playlist_shape: Some(shape),
                    method: method.to_string(),
                    local_path: path.clone(),
                    target_url: String::new(),
                    status: 200,
                    header_ms: ms(started.elapsed()),
                    total_ms: ms(started.elapsed()),
                    bytes_out: if method == Method::HEAD { 0 } else { size },
                    media: None,
                    lazy_state: None,
                    lazy_wait_ms: -1,
                    lazy_generator: None,
                    failure: None,
                });
                local_manifest(body, method == Method::HEAD)
            }
            None => inner.reject(
                &req,
                &req_id,
                StatusCode::NOT_FOUND,
                "local_manifest_gone",
                started,
            ),
        };
    }
    let policy = inner
        .deps
        .policy
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    let raw_query = req.uri().query().unwrap_or_default();
    let query = if raw_query.is_empty() {
        String::new()
    } else {
        format!("?{raw_query}")
    };
    let target_param = query_value(raw_query, "target");
    let Some(target) = inner
        .targets
        .resolve(&policy, &path, &query, target_param.as_deref())
    else {
        return inner.reject(
            &req,
            &req_id,
            StatusCode::BAD_REQUEST,
            "target_missing",
            started,
        );
    };
    let target_url = match security::check_target_url(&policy, &target.url) {
        Ok(u) => u,
        Err(reason) => {
            return inner.reject(&req, &req_id, StatusCode::FORBIDDEN, reason, started);
        }
    };

    let upstream_class = target_url
        .host_str()
        .is_some_and(|h| policy.is_first_party_host(h));
    inner.deps.stats.record_relay_request(upstream_class);

    let mut ex = Exchange {
        method: method.clone(),
        path,
        target: target.url.clone(),
        upstream_class,
        started,
        header_ms: -1,
        status: 0,
        lazy_state: None,
        lazy_wait_ms: -1,
        lazy_generator: None,
        suppressed: false,
        recovery: false,
        retry_reason: None,
        req_id,
        stream_id: crate::diagnostics::stream_id(&target.url),
        player_status: 0,
    };
    inner.deps.stats.playback.begin_request(
        &ex.path,
        &ex.target,
        &ex.req_id,
        (inner.deps.now)(),
        &inner.deps.ux,
    );
    let mut outbound = UpstreamRequest {
        method: method.clone(),
        url: target_url,
        headers: headers::forward_request_headers(req.headers()),
    };
    let eligible = matches!(
        hitch::relay_kind(&ex.sample(0, None)),
        crate::stats::RelayKind::Manifest | crate::stats::RelayKind::Segment
    );
    let playback = crate::relay::activity::play_id(&ex.path)
        .unwrap_or_default()
        .to_owned();
    let key = crate::relay::retry::key(&playback, &ex.target, &method, &outbound.headers);
    outbound.headers.insert(
        "x-request-id",
        HeaderValue::from_str(&ex.req_id).expect("generated request id"),
    );
    let mut attempt = HeaderAttempt {
        inner: Arc::clone(&inner),
        sample: ex.sample(0, Some("client_disconnect")),
        started: ex.started,
        complete: false,
    };
    let now = (inner.deps.now)();
    let retired = eligible && inner.retries.is_retired(&playback, now);
    let cached = eligible.then(|| inner.retries.get(&key, now)).flatten();
    let mut resp = if retired {
        ex.suppressed = true;
        ex.retry_reason = Some(crate::relay::retry::RECREATE.to_owned());
        retry::terminal()
    } else if let Some(answer) = cached {
        ex.suppressed = true;
        ex.retry_reason = Some(answer.reason.to_owned());
        retry::response(answer)
    } else {
        let sent = tokio::select! {
            () = inner.shutdown.cancelled() => { attempt.sample.failure = Some("shutdown".to_owned()); attempt.sample.player_status = 503; return status_only(StatusCode::SERVICE_UNAVAILABLE) },
            r = tokio::time::timeout(inner.header_deadline, inner.deps.upstream.send(outbound)) => r,
        };
        match sent {
            Err(_) => {
                attempt.complete = true;
                ex.player_status = 504;
                if upstream_class {
                    inner.deps.stats.playback.playback_error(
                        &ex.path,
                        &vrcr_shared::console::PlaybackError::Timeout,
                        &inner.deps.ux,
                    );
                }
                inner.record_hitch(&ex.sample(0, Some("upstream_timeout")));
                return status_only(StatusCode::GATEWAY_TIMEOUT);
            }
            Ok(Err(e)) => {
                attempt.complete = true;
                ex.player_status = 502;
                if upstream_class {
                    let error = match e.kind {
                        "timeout" => vrcr_shared::console::PlaybackError::Timeout,
                        "connect_error" => vrcr_shared::console::PlaybackError::Unreachable,
                        _ => vrcr_shared::console::PlaybackError::proxy("request_failed"),
                    };
                    inner
                        .deps
                        .stats
                        .playback
                        .playback_error(&ex.path, &error, &inner.deps.ux);
                }
                inner.record_hitch(&ex.sample(0, Some(e.kind)));
                return status_only(StatusCode::BAD_GATEWAY);
            }
            Ok(Ok(r)) => r,
        }
    };
    let playback_error = if !ex.suppressed
        && upstream_class
        && resp.direct
        && (resp.status.is_client_error() || resp.status.is_server_error())
        && let Some(reason) = resp.headers.get("x-vrcr-reason")
    {
        let reason = String::from_utf8_lossy(reason.as_bytes());
        Some(vrcr_shared::console::PlaybackError::proxy(&reason))
    } else {
        None
    };
    if eligible && !ex.suppressed {
        if resp.status.is_success() {
            inner.retries.clear(&key);
        } else if matches!(resp.status.as_u16(), 404 | 410 | 429 | 502 | 503 | 504) {
            ex.status = resp.status.as_u16();
            if let Some(id) = headers::first_header(&resp.headers, "x-stream-id") {
                ex.stream_id = crate::diagnostics::token(&id);
            }
            let trusted = resp.direct
                && inner
                    .deps
                    .stats
                    .playback
                    .trusts_origin(&ex.path, &ex.target);
            let header_reason = if trusted {
                crate::relay::retry::reason(&resp.headers, &[])
            } else {
                "upstream_status"
            };
            let (received, body) = if method == Method::HEAD
                || header_reason == crate::relay::retry::RECREATE
            {
                (resp, (method == Method::HEAD).then(Bytes::new))
            } else {
                tokio::select! {
                    () = inner.shutdown.cancelled() => return status_only(StatusCode::SERVICE_UNAVAILABLE),
                    inspected = retry::inspect(resp) => inspected,
                }
            };
            resp = received;
            let reason = if trusted {
                crate::relay::retry::reason(&resp.headers, body.as_deref().unwrap_or_default())
            } else {
                "upstream_status"
            };
            if reason != "upstream_status" || matches!(resp.status.as_u16(), 429 | 502 | 503 | 504)
            {
                ex.retry_reason = Some(reason.to_owned());
            }
            if reason == crate::relay::retry::RECREATE {
                inner.retire(&mut ex, &playback);
                resp = retry::terminal();
            } else if let Some(body) = body {
                let now = (inner.deps.now)();
                if let Some(delay) =
                    crate::relay::retry::delay(resp.status.as_u16(), reason, &resp.headers, now)
                {
                    let answer = crate::relay::retry::Answer {
                        status: resp.status,
                        headers: resp.headers.clone(),
                        body,
                        reason,
                        until: now + delay,
                    };
                    if inner.retries.confirmed(&key, &answer, now) {
                        inner.retries.put(key, answer, now);
                    }
                }
            }
        }
    }
    if (resp.status.is_client_error() || resp.status.is_server_error())
        && let Some(error) = playback_error
    {
        inner
            .deps
            .stats
            .playback
            .playback_error(&ex.path, &error, &inner.deps.ux);
    }
    attempt.complete = true;
    ex.header_ms = ms(ex.started.elapsed());
    if ex.status == 0 {
        ex.status = resp.status.as_u16();
    }
    ex.player_status = resp.status.as_u16();
    if let Some(id) = headers::first_header(&resp.headers, "x-stream-id") {
        ex.stream_id = crate::diagnostics::token(&id);
    }
    ex.lazy_state = headers::first_header(&resp.headers, "x-lazy-hls");
    ex.lazy_wait_ms = headers::long_header(&resp.headers, "x-lazy-hls-wait-ms");
    ex.lazy_generator = headers::first_header(&resp.headers, "x-lazy-hls-generator");

    let is_head = method == Method::HEAD;
    let content_type = headers::first_header(&resp.headers, "content-type");
    if !is_head
        && resp.status.is_success()
        && manifest::is_likely_manifest(&ex.path, &ex.target, content_type.as_deref())
    {
        return respond_manifest(&inner, ex, resp, policy, content_type, guard, abort);
    }
    respond_stream(&inner, ex, resp, content_type, is_head, guard, abort)
}
