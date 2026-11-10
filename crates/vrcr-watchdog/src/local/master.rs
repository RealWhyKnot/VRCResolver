use url::Url;

const DEFAULT_MAX_HEIGHT: u32 = 1080;
const PREFERRED_LANGUAGE: &str = "en";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Picked {
    pub manifest: String,
    pub variant_url: String,
    pub audio_url: Option<String>,
    pub height: u32,
    pub video_codec: String,
    pub audio_codec: Option<String>,
}

struct Variant {
    resolution: String,
    frame_rate_text: Option<String>,
    codecs: String,
    url: String,
    height: u32,
    frame_rate: u32,
    bandwidth: u64,
    video_codec: String,
    audio_codec: Option<String>,
    audio_group: Option<String>,
}

struct Audio {
    line: String,
    url: String,
    group: String,
    language: Option<String>,
    is_default: bool,
}

pub fn pick(master: &str, base: &Url, max_height: Option<i32>) -> Option<Picked> {
    let limit = max_height
        .and_then(|height| u32::try_from(height).ok())
        .filter(|height| *height > 0)
        .unwrap_or(DEFAULT_MAX_HEIGHT);
    let (variants, audio) = parse(master, base);
    let variant = variants
        .iter()
        .filter(|variant| variant.height <= limit)
        .max_by_key(|variant| (variant.height, variant.frame_rate, variant.bandwidth))?;
    let mut manifest = String::from("#EXTM3U\n#EXT-X-INDEPENDENT-SEGMENTS\n");
    let mut audio_url = None;
    if let Some(group) = variant.audio_group.as_deref() {
        let rendition = pick_audio(&audio, group)?;
        manifest.push_str(&rendition.line);
        manifest.push('\n');
        audio_url = Some(rendition.url.clone());
    }
    manifest.push_str(&variant.line());
    manifest.push('\n');
    manifest.push_str(&variant.url);
    manifest.push('\n');
    Some(Picked {
        manifest,
        variant_url: variant.url.clone(),
        audio_url,
        height: variant.height,
        video_codec: variant.video_codec.clone(),
        audio_codec: variant.audio_codec.clone(),
    })
}

impl Variant {
    fn line(&self) -> String {
        let mut line = format!(
            "#EXT-X-STREAM-INF:BANDWIDTH={},CODECS=\"{}\",RESOLUTION={}",
            self.bandwidth, self.codecs, self.resolution
        );
        if let Some(rate) = &self.frame_rate_text {
            line.push_str(",FRAME-RATE=");
            line.push_str(rate);
        }
        if let Some(group) = &self.audio_group {
            line.push_str(",AUDIO=\"");
            line.push_str(group);
            line.push('"');
        }
        line
    }
}

pub fn first_segment(playlist: &str, base: &Url) -> Option<Url> {
    let mut lines = playlist.lines().map(str::trim);
    lines.by_ref().find(|line| line.starts_with("#EXTINF"))?;
    let segment = lines.find(|line| !line.is_empty() && !line.starts_with('#'))?;
    web_url(segment, base)
}

fn pick_audio<'a>(audio: &'a [Audio], group: &str) -> Option<&'a Audio> {
    let renditions: Vec<&Audio> = audio.iter().filter(|a| a.group == group).collect();
    let preferred: Vec<&Audio> = renditions
        .iter()
        .copied()
        .filter(|a| {
            a.language.as_deref().is_some_and(|language| {
                language
                    .to_ascii_lowercase()
                    .starts_with(PREFERRED_LANGUAGE)
            })
        })
        .collect();
    let pool = if preferred.is_empty() {
        &renditions
    } else {
        &preferred
    };
    pool.iter()
        .copied()
        .find(|a| a.is_default)
        .or_else(|| pool.first().copied())
}

fn parse(master: &str, base: &Url) -> (Vec<Variant>, Vec<Audio>) {
    let mut variants = Vec::new();
    let mut audio = Vec::new();
    let lines: Vec<&str> = master.lines().map(str::trim).collect();
    for (index, line) in lines.iter().enumerate() {
        if let Some(attributes) = line.strip_prefix("#EXT-X-MEDIA:") {
            audio.extend(parse_audio(line, attributes, base));
        } else if let Some(attributes) = line.strip_prefix("#EXT-X-STREAM-INF:") {
            let target = lines[index + 1..]
                .iter()
                .find(|next| !next.is_empty() && !next.starts_with('#'));
            if let Some(target) = target {
                variants.extend(parse_variant(attributes, target, base));
            }
        }
    }
    (variants, audio)
}

fn parse_audio(line: &str, attributes: &str, base: &Url) -> Option<Audio> {
    let attributes = parse_attributes(attributes);
    if value(&attributes, "TYPE")? != "AUDIO" {
        return None;
    }
    let uri = value(&attributes, "URI")?;
    let absolute = web_url(uri, base)?;
    let quoted = format!("URI=\"{uri}\"");
    if !line.contains(&quoted) {
        return None;
    }
    Some(Audio {
        line: line.replacen(&quoted, &format!("URI=\"{absolute}\""), 1),
        url: absolute.to_string(),
        group: value(&attributes, "GROUP-ID")?.to_owned(),
        language: value(&attributes, "LANGUAGE").map(str::to_owned),
        is_default: value(&attributes, "DEFAULT") == Some("YES"),
    })
}

fn parse_variant(attributes: &str, target: &str, base: &Url) -> Option<Variant> {
    let attributes = parse_attributes(attributes);
    let resolution = value(&attributes, "RESOLUTION")?;
    let (width, height) = resolution.split_once('x')?;
    width.parse::<u32>().ok()?;
    let codec_list = value(&attributes, "CODECS")?;
    let codecs: Vec<&str> = codec_list.split(',').map(str::trim).collect();
    let video_codec = codecs.iter().find(|codec| codec.starts_with("avc1"))?;
    let audio_codec = codecs.iter().find(|codec| codec.starts_with("mp4a"));
    let frame_rate_text = value(&attributes, "FRAME-RATE")
        .filter(|rate| !rate.is_empty() && rate.chars().all(|c| c.is_ascii_digit() || c == '.'));
    let frame_rate = frame_rate_text
        .and_then(|rate| rate.split('.').next())
        .and_then(|rate| rate.parse().ok())
        .unwrap_or(0);
    Some(Variant {
        resolution: resolution.to_owned(),
        frame_rate_text: frame_rate_text.map(str::to_owned),
        codecs: codec_list.to_owned(),
        url: web_url(target, base)?.to_string(),
        height: height.parse().ok()?,
        frame_rate,
        bandwidth: value(&attributes, "BANDWIDTH")
            .and_then(|bandwidth| bandwidth.parse().ok())
            .unwrap_or(0),
        video_codec: (*video_codec).to_owned(),
        audio_codec: audio_codec.map(|codec| (*codec).to_owned()),
        audio_group: value(&attributes, "AUDIO").map(str::to_owned),
    })
}

fn web_url(target: &str, base: &Url) -> Option<Url> {
    let url = base.join(target).ok()?;
    matches!(url.scheme(), "http" | "https").then_some(url)
}

fn value<'a>(attributes: &'a [(String, String)], name: &str) -> Option<&'a str> {
    attributes
        .iter()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.as_str())
}

fn parse_attributes(text: &str) -> Vec<(String, String)> {
    let mut attributes = Vec::new();
    let mut rest = text;
    while let Some((key, after)) = rest.split_once('=') {
        let (value, remaining) = match after.strip_prefix('"') {
            Some(quoted) => match quoted.split_once('"') {
                Some((value, remaining)) => (value, remaining),
                None => (quoted, ""),
            },
            None => after.split_once(',').unwrap_or((after, "")),
        };
        attributes.push((key.trim().to_owned(), value.to_owned()));
        rest = remaining.trim_start_matches(',');
    }
    attributes
}
