use super::*;

#[test]
fn relative_bases_stay_bounded() {
    let resolver = TargetResolver::new();
    let target = encode_target_param("https://us1.vrcresolver.com/api/proxy/x/index.m3u8");
    for i in 0..600 {
        resolver.resolve(
            &policy(),
            &format!("/play/p{i}/manifest.m3u8"),
            "",
            Some(&target),
        );
    }
    assert!(resolver.remembered() <= 512, "{}", resolver.remembered());
    assert!(resolver.remembered() > 256);
}
