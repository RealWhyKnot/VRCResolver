use super::*;

const ORIGINAL: &str =
    ",YT-EXT-XTAGS=\"ChEKBWFjb250EghvcmlnaW5hbAoKCgRsYW5nEgJqYQ\",YT-EXT-AUDIO-CONTENT-ID=\"ja.4\"";
const DUB: &str = ",YT-EXT-XTAGS=\"ChQKBWFjb250EgtkdWJiZWQtYXV0bwoNCgRsYW5nEgVlbi1VUw\",YT-EXT-AUDIO-CONTENT-ID=\"en-US.10\"";

fn variant(name: &str, height: u32, rate: u32, bandwidth: u64, tags: &str) -> String {
    format!(
        "#EXT-X-STREAM-INF:BANDWIDTH={bandwidth},CODECS=\"avc1.64001F,mp4a.40.2\",RESOLUTION=1920x{height},FRAME-RATE={rate}{tags}\n{name}.m3u8\n"
    )
}

fn assert_variant(variants: &[String], limit: Option<i32>, name: &str) {
    let playlist = format!("#EXTM3U\n{}", variants.concat());
    let picked = master::pick(&playlist, &base(), limit).expect("picked variant");
    assert_eq!(
        picked.variant_url,
        base()
            .join(&format!("{name}.m3u8"))
            .expect("variant URL")
            .to_string()
    );
    assert_eq!(picked.audio_url, None);
    assert_eq!(picked.manifest.matches("#EXT-X-STREAM-INF:").count(), 1);
}

#[test]
fn older_muxed_masters_prefer_untagged_original_over_higher_bandwidth_dub() {
    for tags in [DUB, ",YT-EXT-AUDIO-CONTENT-ID=\"en-US.10\""] {
        let mut variants = [
            variant("original", 1080, 30, 4_000_000, ""),
            variant("dub", 1080, 30, 4_100_000, tags),
        ];
        assert_variant(&variants, None, "original");
        variants.reverse();
        assert_variant(&variants, None, "original");
    }
}

#[test]
fn tagged_original_beats_untagged_and_dubbed_variants_at_the_same_height() {
    assert_variant(
        &[
            variant("original", 1080, 30, 4_000_000, ORIGINAL),
            variant("untagged", 1080, 60, 4_100_000, ""),
            variant("dub", 1080, 60, 4_200_000, DUB),
        ],
        None,
        "original",
    );
}

#[test]
fn original_preference_is_applied_at_the_chosen_height() {
    let variants = [
        variant("low-original", 720, 60, 2_000_000, ORIGINAL),
        variant("high-original", 1080, 30, 4_000_000, ""),
        variant("high-dub", 1080, 60, 4_100_000, DUB),
        variant("too-high", 2160, 60, 8_000_000, ORIGINAL),
    ];
    assert_variant(&variants, None, "high-original");
    assert_variant(&variants, Some(720), "low-original");
    assert_variant(&variants, Some(2160), "too-high");
}

#[test]
fn plain_variants_keep_height_frame_rate_and_bandwidth_order() {
    let variants = [
        variant("low", 720, 60, 9_000_000, ""),
        variant("slow", 1080, 30, 8_000_000, ""),
        variant("fast", 1080, 60, 4_000_000, ""),
        variant("fast-best", 1080, 60, 4_100_000, ""),
        variant("too-high", 2160, 60, 5_000_000, ""),
    ];
    assert_variant(&variants, None, "fast-best");
    assert_variant(&variants, Some(720), "low");
}

#[test]
fn equally_tagged_variants_keep_the_quality_tiebreaks() {
    for tags in [ORIGINAL, DUB, ",YT-EXT-XTAGS=\"not base64!\""] {
        assert_variant(
            &[
                variant("slow", 1080, 30, 5_000_000, tags),
                variant("fast", 1080, 60, 4_000_000, tags),
                variant("fast-best", 1080, 60, 4_100_000, tags),
            ],
            None,
            "fast-best",
        );
    }
}
