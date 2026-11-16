use super::*;

pub(super) struct Stub {
    pub(super) addr: SocketAddr,
    pub(super) heads: Arc<Mutex<Vec<String>>>,
}

pub(super) async fn stub_origin() -> Stub {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    let addr = listener.local_addr().unwrap_or_else(|e| panic!("{e}"));
    let heads = Arc::new(Mutex::new(Vec::new()));
    let log = std::sync::Arc::clone(&heads);
    tokio::spawn(async move {
        while let Ok((mut s, _)) = listener.accept().await {
            let log = std::sync::Arc::clone(&log);
            tokio::spawn(async move {
                let mut buf = Vec::new();
                let mut chunk = [0u8; 4096];
                while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
                    match s.read(&mut chunk).await {
                        Ok(0) | Err(_) => return,
                        Ok(n) => buf.extend_from_slice(&chunk[..n]),
                    }
                }
                let head = String::from_utf8_lossy(&buf).into_owned();
                let path = head.split(' ').nth(1).unwrap_or_default().to_owned();
                log.lock().unwrap_or_else(|e| e.into_inner()).push(head);
                let port = addr.port();
                let reply = match path.as_str() {
                    "/api/proxy/full/video.mp4" => {
                        let mut r = b"HTTP/1.1 200 OK\r\nContent-Type: video/mp4\r\nContent-Length: 50000\r\nConnection: close\r\n\r\n".to_vec();
                        r.extend(std::iter::repeat_n(b'o', 50000));
                        r
                    }
                    "/api/proxy/redirect/x.ts" => format!(
                        "HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:{port}/api/proxy/full/video.mp4\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                    )
                    .into_bytes(),
                    "/api/proxy/hop/x.ts" => format!(
                        "HTTP/1.1 302 Found\r\nLocation: http://us1.vrcresolver.com:{port}/api/proxy/full/video.mp4\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                    )
                    .into_bytes(),
                    _ => b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec(),
                };
                let _ = s.write_all(&reply).await;
                let _ = s.shutdown().await;
            });
        }
    });
    Stub { addr, heads }
}
