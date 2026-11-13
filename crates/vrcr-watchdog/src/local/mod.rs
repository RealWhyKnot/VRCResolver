use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use reqwest::header::{ACCEPT, ACCEPT_LANGUAGE, CONTENT_TYPE, ORIGIN, REFERER};
use reqwest::{Client, RequestBuilder};
use url::Url;
use vrcr_shared::trust_gateway::is_local_gateway_host;

pub mod master;
pub mod store;
mod xtags;
pub mod youtube;

use store::Manifests;

pub const MIN_BUDGET: Duration = Duration::from_millis(1_500);
pub const MAX_BUDGET: Duration = Duration::from_millis(2_500);
pub const BUDGET_RESERVE: Duration = Duration::from_millis(500);
pub const PATH_PREFIX: &str = "/play/local-";
pub const MANIFEST_NAME: &str = "master.m3u8";
pub const ENGINE: &str = "local";
pub const MANIFEST_TYPE: &str = "application/vnd.apple.mpegurl";
const VISITOR_TTL: Duration = Duration::from_secs(6 * 60 * 60);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(2);
const MEDIA_HOSTS: [&str; 2] = ["googlevideo.com", "youtube.com"];
const MAX_REDIRECTS: usize = 3;
const VISITOR_CAP: usize = 256 * 1024;
const PLAYER_CAP: usize = 4 * 1024 * 1024;
const MASTER_CAP: usize = 1024 * 1024;
const PLAYLIST_HEAD: usize = 64 * 1024;

pub struct Ask<'a> {
    pub url: &'a str,
    pub max_height: Option<i32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Play {
    pub url: String,
    pub height: u32,
    pub video_codec: String,
    pub audio_codec: Option<String>,
}

pub type Attempt<'a> = Pin<Box<dyn Future<Output = Result<Play, &'static str>> + Send + 'a>>;

pub trait Step: Send + Sync {
    fn covers(&self, url: &str) -> bool;
    fn play<'a>(&'a self, ask: Ask<'a>) -> Attempt<'a>;
}

pub fn token(path: &str) -> Option<&str> {
    let (token, name) = path.strip_prefix(PATH_PREFIX)?.split_once('/')?;
    (!token.is_empty() && name == MANIFEST_NAME).then_some(token)
}

pub fn is_play_url(url: &str) -> bool {
    Url::parse(url.trim()).is_ok_and(|url| {
        url.host_str().is_some_and(is_local_gateway_host) && token(url.path()).is_some()
    })
}

pub type RelayBase = Arc<dyn Fn() -> Option<String> + Send + Sync>;

pub struct Origin {
    pub site: Url,
    pub media_hosts: Vec<String>,
}

impl Origin {
    pub fn youtube() -> Origin {
        Origin {
            site: Url::parse("https://www.youtube.com/").expect("the site address is a valid URL"),
            media_hosts: MEDIA_HOSTS.map(str::to_owned).to_vec(),
        }
    }

    fn media_url(&self, raw: &str) -> Result<Url, &'static str> {
        let url = Url::parse(raw).ok().ok_or("foreign_host")?;
        let host = url.host_str().unwrap_or_default().to_ascii_lowercase();
        let is_known = self
            .media_hosts
            .iter()
            .any(|known| is_host_within(&host, known));
        if url.scheme() == self.site.scheme() && is_known {
            Ok(url)
        } else {
            Err("foreign_host")
        }
    }
}

fn is_host_within(host: &str, known: &str) -> bool {
    host == known
        || host
            .strip_suffix(known)
            .is_some_and(|rest| rest.ends_with('.'))
}

fn redirects() -> reqwest::redirect::Policy {
    reqwest::redirect::Policy::custom(|attempt| {
        let is_allowed = attempt.previous().len() <= MAX_REDIRECTS
            && attempt.url().host_str().is_some_and(|host| {
                let host = host.to_ascii_lowercase();
                let is_same = attempt
                    .previous()
                    .last()
                    .and_then(Url::host_str)
                    .is_some_and(|previous| previous.eq_ignore_ascii_case(&host));
                is_same || MEDIA_HOSTS.iter().any(|known| is_host_within(&host, known))
            });
        if is_allowed {
            attempt.follow()
        } else {
            attempt.stop()
        }
    })
}

struct Fault<'a> {
    manifests: &'a Manifests,
    is_armed: bool,
}

impl Drop for Fault<'_> {
    fn drop(&mut self) {
        if self.is_armed {
            self.manifests.fault();
        }
    }
}

fn is_about_the_video(reason: &str) -> bool {
    matches!(
        reason,
        "not_youtube" | "relay_down" | "resting" | "not_playable" | "live" | "no_hls" | "no_h264"
    )
}

pub struct Local {
    http: Client,
    origin: Origin,
    manifests: Arc<Manifests>,
    relay: RelayBase,
    visitor: Mutex<Option<(String, Instant)>>,
}

pub fn client(tls: rustls::ClientConfig) -> Result<Client, String> {
    Client::builder()
        .tls_backend_preconfigured(tls)
        .no_proxy()
        .referer(false)
        .redirect(redirects())
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(REQUEST_TIMEOUT)
        .user_agent(youtube::USER_AGENT)
        .build()
        .map_err(|e| format!("configure local resolve HTTP client: {e}"))
}

impl Local {
    pub fn new(http: Client, origin: Origin, manifests: Arc<Manifests>, relay: RelayBase) -> Local {
        Local {
            http,
            origin,
            manifests,
            relay,
            visitor: Mutex::new(None),
        }
    }

    async fn watched(&self, ask: Ask<'_>) -> Result<Play, &'static str> {
        let mut fault = Fault {
            manifests: &self.manifests,
            is_armed: true,
        };
        let played = self.attempt(ask).await;
        match &played {
            Ok(_) => {
                fault.is_armed = false;
                self.manifests.answered();
            }
            Err(reason) => fault.is_armed = !is_about_the_video(reason),
        }
        played
    }

    async fn attempt(&self, ask: Ask<'_>) -> Result<Play, &'static str> {
        let id = youtube::video_id(ask.url).ok_or("not_youtube")?;
        if self.manifests.is_resting() {
            return Err("resting");
        }
        let relay = self.relay_base().await.ok_or("relay_down")?;
        let visitor = self.visitor().await?;
        let player = self.player(&id, &visitor).await?;
        let hls = match youtube::manifest_url(&player, &id) {
            Ok(hls) => hls,
            Err(reason) => {
                if reason == youtube::SIGN_IN_WALL {
                    self.forget_visitor();
                }
                return Err(reason);
            }
        };
        let master_url = self.origin.media_url(&hls)?;
        let master = read(
            self.http.get(master_url.clone()),
            MASTER_CAP,
            "master_failed",
        )
        .await?;
        let picked = master::pick(&master, &master_url, ask.max_height).ok_or("no_h264")?;
        let variant_url = self.origin.media_url(&picked.variant_url)?;
        if let Some(audio) = &picked.audio_url {
            self.origin.media_url(audio)?;
        }
        let playlist = read_head(self.http.get(variant_url.clone()), PLAYLIST_HEAD).await?;
        let segment = master::first_segment(&playlist, &variant_url).ok_or("no_segment")?;
        let segment = self.origin.media_url(segment.as_str())?;
        self.probe(segment).await?;
        let token = self.manifests.insert(picked.manifest, ask.url);
        Ok(Play {
            url: format!("{relay}{PATH_PREFIX}{token}/{MANIFEST_NAME}"),
            height: picked.height,
            video_codec: picked.video_codec,
            audio_codec: picked.audio_codec,
        })
    }

    async fn relay_base(&self) -> Option<String> {
        let relay = Arc::clone(&self.relay);
        tokio::task::spawn_blocking(move || relay())
            .await
            .ok()
            .flatten()
    }

    async fn visitor(&self) -> Result<String, &'static str> {
        let known = self
            .visitor
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        if let Some((visitor, fetched)) = known
            && fetched.elapsed() < VISITOR_TTL
        {
            return Ok(visitor);
        }
        let url = self
            .origin
            .site
            .join("sw.js_data")
            .ok()
            .ok_or("visitor_failed")?;
        let referer = self
            .origin
            .site
            .join("sw.js")
            .ok()
            .ok_or("visitor_failed")?;
        let request = self
            .http
            .get(url)
            .header(ACCEPT, "*/*")
            .header(ACCEPT_LANGUAGE, "en-US,en;q=0.9")
            .header(REFERER, referer.as_str());
        let body = read(request, VISITOR_CAP, "visitor_failed").await?;
        let visitor = youtube::visitor_data(&body).ok_or("visitor_unreadable")?;
        *self.visitor.lock().unwrap_or_else(|e| e.into_inner()) =
            Some((visitor.clone(), Instant::now()));
        Ok(visitor)
    }

    fn forget_visitor(&self) {
        *self.visitor.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }

    async fn player(&self, id: &str, visitor: &str) -> Result<String, &'static str> {
        let url = self
            .origin
            .site
            .join("youtubei/v1/player?prettyPrint=false")
            .ok()
            .ok_or("player_failed")?;
        let request = self
            .http
            .post(url)
            .header(CONTENT_TYPE, "application/json")
            .header("X-Youtube-Client-Name", youtube::CLIENT_NAME_ID)
            .header("X-Youtube-Client-Version", youtube::CLIENT_VERSION)
            .header("X-Goog-Visitor-Id", visitor)
            .header(ORIGIN, self.origin.site.origin().ascii_serialization())
            .body(youtube::player_body(id, visitor));
        read(request, PLAYER_CAP, "player_failed").await
    }

    async fn probe(&self, segment: Url) -> Result<(), &'static str> {
        let mut response = self
            .http
            .get(segment)
            .send()
            .await
            .ok()
            .ok_or("segment_failed")?;
        if !response.status().is_success() {
            return Err("segment_failed");
        }
        match response.chunk().await {
            Ok(Some(chunk)) if !chunk.is_empty() => Ok(()),
            _ => Err("segment_failed"),
        }
    }
}

impl Step for Local {
    fn covers(&self, url: &str) -> bool {
        youtube::video_id(url).is_some()
    }

    fn play<'a>(&'a self, ask: Ask<'a>) -> Attempt<'a> {
        Box::pin(self.watched(ask))
    }
}

async fn read(
    request: RequestBuilder,
    cap: usize,
    failure: &'static str,
) -> Result<String, &'static str> {
    let (body, is_complete) = collect(request, cap, failure).await?;
    if is_complete { Ok(body) } else { Err(failure) }
}

async fn read_head(request: RequestBuilder, cap: usize) -> Result<String, &'static str> {
    let (mut body, is_complete) = collect(request, cap, "playlist_failed").await?;
    if !is_complete {
        body.truncate(body.rfind('\n').map_or(0, |end| end + 1));
    }
    Ok(body)
}

async fn collect(
    request: RequestBuilder,
    cap: usize,
    failure: &'static str,
) -> Result<(String, bool), &'static str> {
    let mut response = request.send().await.ok().ok_or(failure)?;
    if !response.status().is_success() {
        return Err(failure);
    }
    let mut bytes = Vec::new();
    let mut is_complete = true;
    while let Some(chunk) = response.chunk().await.ok().ok_or(failure)? {
        bytes.extend_from_slice(&chunk);
        if bytes.len() > cap {
            is_complete = false;
            break;
        }
    }
    Ok((String::from_utf8_lossy(&bytes).into_owned(), is_complete))
}
