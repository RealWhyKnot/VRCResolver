use super::*;

#[tokio::test]
async fn rate_limited_cooldown_is_clamped_to_the_ceiling() {
    let h = Harness::new();

    h.json(r#"{"action":"rate_limited","meshAction":"resolve","limit":10,"windowSeconds":60,"retryAfterSeconds":86400}"#)
        .await;

    let until = h.cooldown().unwrap_or_else(|| panic!("no cooldown"));
    assert!(until <= Instant::now() + Duration::from_secs(61));
    assert!(until > Instant::now());
}
