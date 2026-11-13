use super::*;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;

const AUTODUB: &str = include_str!("../../fixtures/youtube_autodub_ja_original_master.m3u8");
const ORIGINAL: &str = "ChEKBWFjb250EghvcmlnaW5hbAoKCgRsYW5nEgJqYQ";
const DUB: &str = "ChQKBWFjb250EgtkdWJiZWQtYXV0bwoNCgRsYW5nEgVlbi1VUw";

fn assert_original_in_both_groups(playlist: &str) {
    for (group, other, expected) in [
        ("233", "234", "audio_2.m3u8"),
        ("234", "233", "audio_4.m3u8"),
    ] {
        let playlist =
            playlist.replace(&format!("AUDIO=\"{other}\""), &format!("AUDIO=\"{group}\""));
        let picked = master::pick(&playlist, &base(), None)
            .unwrap_or_else(|| panic!("nothing picked for group {group}"));
        assert_eq!(
            picked.audio_url,
            Some(base().join(expected).expect("audio URL").to_string())
        );
        assert_eq!(picked.manifest.matches("#EXT-X-MEDIA:").count(), 1);
        assert!(picked.manifest.contains("LANGUAGE=\"ja\""));
        assert!(!picked.manifest.contains("dubbed-auto"));
    }
}

fn dub_default() -> String {
    AUTODUB
        .replace("DEFAULT=YES", "DEFAULT=TEMP")
        .replace("DEFAULT=NO", "DEFAULT=YES")
        .replace("DEFAULT=TEMP", "DEFAULT=NO")
}

fn audio_playlist(renditions: &str) -> String {
    format!(
        "#EXTM3U\n{renditions}\n#EXT-X-STREAM-INF:BANDWIDTH=1,CODECS=\"avc1.64001F,mp4a.40.2\",RESOLUTION=1280x720,AUDIO=\"audio\"\nvideo.m3u8\n"
    )
}

fn selected_audio(renditions: &str) -> String {
    master::pick(&audio_playlist(renditions), &base(), None)
        .expect("picked variant")
        .audio_url
        .expect("picked audio")
}

#[test]
fn the_real_youtube_master_selects_japanese_in_both_groups() {
    assert_original_in_both_groups(AUTODUB);
}

#[test]
fn japanese_original_wins_when_the_english_dub_is_default() {
    assert_original_in_both_groups(&dub_default());
}

#[test]
fn older_masters_select_the_untagged_original() {
    let playlist = dub_default()
        .replace(&format!(",YT-EXT-XTAGS=\"{ORIGINAL}\""), "")
        .replace(&format!(",YT-EXT-XTAGS=\"{DUB}\""), "")
        .replace(",YT-EXT-AUDIO-CONTENT-ID=\"ja.4\"", "");
    assert_original_in_both_groups(&playlist);
}

#[test]
fn xtags_alone_identify_dubs_with_an_untagged_original() {
    let playlist = dub_default()
        .replace(&format!(",YT-EXT-XTAGS=\"{ORIGINAL}\""), "")
        .replace(",YT-EXT-AUDIO-CONTENT-ID=\"ja.4\"", "")
        .replace(",YT-EXT-AUDIO-CONTENT-ID=\"en-US.10\"", "");
    assert_original_in_both_groups(&playlist);
}

#[test]
fn plain_masters_use_default_then_first_regardless_of_language() {
    let renditions =
        "#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID=\"audio\",URI=\"en.m3u8\",LANGUAGE=\"en\",DEFAULT=NO
#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID=\"audio\",URI=\"es.m3u8\",LANGUAGE=\"es\",DEFAULT=YES";
    assert!(selected_audio(renditions).ends_with("/es.m3u8"));
    let no_default = renditions
        .replace("DEFAULT=YES", "DEFAULT=NO")
        .lines()
        .rev()
        .collect::<Vec<_>>()
        .join("\n");
    assert!(selected_audio(&no_default).ends_with("/es.m3u8"));
}

#[test]
fn invalid_xtags_fall_back_to_default_then_first() {
    let truncated = URL_SAFE_NO_PAD.encode(b"\x0a\x11\x0a\x05acont\x12\x08origina");
    for invalid in ["not base64!", "日本語", "A", truncated.as_str()] {
        let renditions = format!(
            "#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID=\"audio\",URI=\"es.m3u8\",LANGUAGE=\"es\",YT-EXT-XTAGS=\"{invalid}\",DEFAULT=NO\n#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID=\"audio\",URI=\"de.m3u8\",LANGUAGE=\"de\",DEFAULT=YES"
        );
        assert!(selected_audio(&renditions).ends_with("/de.m3u8"));
        assert!(
            selected_audio(&renditions.replace("DEFAULT=YES", "DEFAULT=NO")).ends_with("/es.m3u8")
        );
    }
}

#[test]
fn invalid_xtags_still_allow_the_legacy_untagged_original() {
    let playlist = dub_default()
        .replace(ORIGINAL, "not base64!")
        .replace(",YT-EXT-AUDIO-CONTENT-ID=\"ja.4\"", "");
    assert_original_in_both_groups(&playlist);
}

#[test]
fn tagged_non_original_tracks_use_default_when_no_original_exists() {
    for content in ["dubbed-auto", "dubbed", "descriptive", "secondary"] {
        let mut entry = b"\x0a\x05acont\x12".to_vec();
        entry.push(u8::try_from(content.len()).expect("content length"));
        entry.extend(content.as_bytes());
        let mut bytes = vec![0x0a, u8::try_from(entry.len()).expect("entry length")];
        bytes.extend(entry);
        let playlist = dub_default().replace(ORIGINAL, &URL_SAFE_NO_PAD.encode(bytes));
        let picked = master::pick(&playlist, &base(), None).expect("picked variant");
        assert!(
            picked
                .audio_url
                .expect("picked audio")
                .ends_with("/audio_3.m3u8")
        );
    }
}

#[test]
fn youtube_tags_in_another_group_do_not_override_a_plain_default() {
    let renditions = format!(
        "#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID=\"other\",URI=\"original.m3u8\",YT-EXT-XTAGS=\"{ORIGINAL}\",DEFAULT=YES\n#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID=\"audio\",URI=\"en.m3u8\",LANGUAGE=\"en\",DEFAULT=NO\n#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID=\"audio\",URI=\"es.m3u8\",LANGUAGE=\"es\",DEFAULT=YES"
    );
    assert!(selected_audio(&renditions).ends_with("/es.m3u8"));
}

#[test]
fn the_selected_renditions_attributes_are_preserved_for_lf_and_crlf() {
    for ending in ["\n", "\r\n"] {
        let playlist = dub_default().replace('\n', ending);
        let picked = master::pick(&playlist, &base(), None).expect("picked variant");
        let source = playlist
            .lines()
            .find(|line| line.contains("URI=\"audio_4.m3u8\""))
            .expect("original rendition");
        let expected = source.replace(
            "audio_4.m3u8",
            base().join("audio_4.m3u8").expect("audio URL").as_str(),
        );
        assert_eq!(
            picked
                .manifest
                .lines()
                .find(|line| line.starts_with("#EXT-X-MEDIA:")),
            Some(expected.as_str())
        );
    }
}
