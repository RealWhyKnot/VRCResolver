use std::net::Ipv4Addr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use url::Url;
use vrcr_watchdog::local::store::{
    FAILURES_BEFORE_REST, FAULTS_BEFORE_REST, MANIFEST_TTL, MAX_MANIFESTS, Manifests, REST,
};
use vrcr_watchdog::local::{self, Ask, Local, Origin, Step, master, youtube};
use vrcr_watchdog::mesh::connect::platform_tls;

const VIDEO: &str = "aqz-KE-bpKQ";
const WATCH: &str = "https://www.youtube.com/watch?v=aqz-KE-bpKQ";
const VISITOR: &str = "CgtBYmNkZWZnaGlqayiA";
const RELAY: &str = "https://localhost.youtube.com:4443";
const MASTER: &str = "#EXTM3U
#EXT-X-INDEPENDENT-SEGMENTS
#EXT-X-MEDIA:URI=\"audio/fr.m3u8\",TYPE=AUDIO,GROUP-ID=\"234\",LANGUAGE=\"fr\",NAME=\"French\",DEFAULT=YES,AUTOSELECT=YES
#EXT-X-MEDIA:URI=\"audio/en.m3u8\",TYPE=AUDIO,GROUP-ID=\"234\",LANGUAGE=\"en-US\",NAME=\"English, original\",DEFAULT=NO,AUTOSELECT=YES
#EXT-X-MEDIA:URI=\"audio/low.m3u8\",TYPE=AUDIO,GROUP-ID=\"233\",LANGUAGE=\"en\",NAME=\"English\",DEFAULT=YES,AUTOSELECT=YES
#EXT-X-STREAM-INF:BANDWIDTH=500000,CODECS=\"avc1.4D401E,mp4a.40.5\",RESOLUTION=640x360,FRAME-RATE=30,AUDIO=\"233\",CLOSED-CAPTIONS=NONE
v360.m3u8
#EXT-X-STREAM-INF:BANDWIDTH=2500000,CODECS=\"avc1.64001F,mp4a.40.2\",RESOLUTION=1280x720,FRAME-RATE=60,AUDIO=\"234\",CLOSED-CAPTIONS=NONE
v720.m3u8
#EXT-X-STREAM-INF:BANDWIDTH=4500000,CODECS=\"avc1.640028,mp4a.40.2\",RESOLUTION=1920x1080,FRAME-RATE=30,AUDIO=\"234\",CLOSED-CAPTIONS=NONE
v1080.m3u8
#EXT-X-STREAM-INF:BANDWIDTH=5500000,CODECS=\"avc1.64002A,mp4a.40.2\",RESOLUTION=1920x1080,FRAME-RATE=60,AUDIO=\"234\",CLOSED-CAPTIONS=NONE
v1080p60.m3u8
#EXT-X-STREAM-INF:BANDWIDTH=18000000,CODECS=\"vp09.00.51.08,mp4a.40.2\",RESOLUTION=3840x2160,FRAME-RATE=60,AUDIO=\"234\",CLOSED-CAPTIONS=NONE
v2160.m3u8
";
const VP9_ONLY: &str = "#EXTM3U
#EXT-X-STREAM-INF:BANDWIDTH=18000000,CODECS=\"vp09.00.51.08,mp4a.40.2\",RESOLUTION=3840x2160
v2160.m3u8
";
const PLAYLIST: &str = "#EXTM3U
#EXT-X-TARGETDURATION:6
#EXTINF:5.0,
seg0.ts
#EXTINF:5.0,
seg1.ts
#EXT-X-ENDLIST
";

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Plays,
    SignInWall,
    Live,
    NoManifest,
    ForeignManifest,
    Vp9Only,
    ForeignAudio,
    SegmentMissing,
    VisitorDown,
}

struct FakeOrigin {
    port: u16,
    hits: Arc<Mutex<Vec<String>>>,
    is_walled: Arc<AtomicBool>,
}

impl FakeOrigin {
    async fn start(mode: Mode) -> FakeOrigin {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap_or_else(|e| panic!("{e}"));
        let port = listener
            .local_addr()
            .unwrap_or_else(|e| panic!("{e}"))
            .port();
        let hits = Arc::new(Mutex::new(Vec::new()));
        let is_walled = Arc::new(AtomicBool::new(mode == Mode::SignInWall));
        let seen = Arc::clone(&hits);
        let walled = Arc::clone(&is_walled);
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let seen = Arc::clone(&seen);
                let walled = Arc::clone(&walled);
                tokio::spawn(async move { serve(stream, port, mode, &seen, &walled).await });
            }
        });
        FakeOrigin {
            port,
            hits,
            is_walled,
        }
    }

    fn hits(&self) -> Vec<String> {
        self.hits.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    fn local(&self, manifests: &Arc<Manifests>) -> Local {
        self.local_behind(manifests, Some(RELAY))
    }

    fn local_behind(&self, manifests: &Arc<Manifests>, relay: Option<&'static str>) -> Local {
        let tls = (*platform_tls().unwrap_or_else(|e| panic!("{e}"))).clone();
        let site = Url::parse(&format!("http://127.0.0.1:{}/", self.port))
            .unwrap_or_else(|e| panic!("{e}"));
        Local::new(
            local::client(tls).unwrap_or_else(|e| panic!("{e}")),
            Origin {
                site,
                media_hosts: vec!["127.0.0.1".to_owned()],
            },
            Arc::clone(manifests),
            Arc::new(move || relay.map(str::to_owned)),
        )
    }
}

async fn serve(
    mut stream: TcpStream,
    port: u16,
    mode: Mode,
    hits: &Mutex<Vec<String>>,
    is_walled: &AtomicBool,
) {
    let Some((head, body)) = read_request(&mut stream).await else {
        return;
    };
    let line = head.lines().next().unwrap_or_default().to_owned();
    let path = line.split(' ').nth(1).unwrap_or_default().to_owned();
    hits.lock()
        .unwrap_or_else(|e| e.into_inner())
        .push(path.clone());
    let has_visitor = head.to_ascii_lowercase().contains(&format!(
        "x-goog-visitor-id: {}",
        VISITOR.to_ascii_lowercase()
    ));
    let (status, reply) = match path.as_str() {
        "/sw.js_data" if mode == Mode::VisitorDown => (503, String::new()),
        "/sw.js_data" => (200, visitor_reply()),
        "/youtubei/v1/player?prettyPrint=false" => {
            let is_known = has_visitor && body.contains(VIDEO) && body.contains("VISIONOS");
            let is_refused = !is_known || is_walled.load(Ordering::SeqCst);
            (200, player_reply(port, mode, is_refused))
        }
        "/hls/master.m3u8" if mode == Mode::Vp9Only => (200, VP9_ONLY.to_owned()),
        "/hls/master.m3u8" if mode == Mode::ForeignAudio => (
            200,
            MASTER.replace("audio/en.m3u8", "http://example.invalid/en.m3u8"),
        ),
        "/hls/master.m3u8" => (200, MASTER.to_owned()),
        "/hls/v1080p60.m3u8" | "/hls/v720.m3u8" => (200, PLAYLIST.to_owned()),
        "/hls/seg0.ts" if mode == Mode::SegmentMissing => (404, String::new()),
        "/hls/seg0.ts" => (200, "G".repeat(188)),
        _ => (404, String::new()),
    };
    let text = format!(
        "HTTP/1.1 {status} X\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}",
        reply.len()
    );
    let _ = stream.write_all(text.as_bytes()).await;
    let _ = stream.shutdown().await;
}

async fn read_request(stream: &mut TcpStream) -> Option<(String, String)> {
    let mut bytes = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        let text = String::from_utf8_lossy(&bytes).into_owned();
        if let Some((head, body)) = text.split_once("\r\n\r\n") {
            let length = head
                .lines()
                .filter_map(|line| line.split_once(':'))
                .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                .and_then(|(_, value)| value.trim().parse::<usize>().ok())
                .unwrap_or(0);
            if body.len() >= length {
                return Some((head.to_owned(), body.to_owned()));
            }
        }
        match stream.read(&mut chunk).await {
            Ok(0) | Err(_) => return None,
            Ok(n) => bytes.extend_from_slice(&chunk[..n]),
        }
    }
}

fn visitor_reply() -> String {
    let mut fields = vec!["null".to_owned(); 13];
    fields.push(format!("\"{VISITOR}\""));
    format!(")]}}'\n[[null,null,[[[{}]]]]]", fields.join(","))
}

fn player_reply(port: u16, mode: Mode, is_refused: bool) -> String {
    if is_refused {
        return "{\"playabilityStatus\":{\"status\":\"LOGIN_REQUIRED\"}}".to_owned();
    }
    let manifest = match mode {
        Mode::NoManifest => String::new(),
        Mode::ForeignManifest => "http://example.invalid/hls/master.m3u8".to_owned(),
        _ => format!("http://127.0.0.1:{port}/hls/master.m3u8"),
    };
    serde_json::json!({
        "playabilityStatus": {"status": "OK"},
        "videoDetails": {"videoId": VIDEO, "isLive": mode == Mode::Live},
        "streamingData": {"hlsManifestUrl": manifest},
    })
    .to_string()
}

fn ask() -> Ask<'static> {
    Ask {
        url: WATCH,
        max_height: None,
    }
}

fn base() -> Url {
    Url::parse("https://manifest.googlevideo.com/api/manifest/hls_variant/x/master.m3u8")
        .unwrap_or_else(|e| panic!("{e}"))
}

mod origin_cases;
mod parsing_cases;
