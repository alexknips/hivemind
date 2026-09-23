// Parent module gates this file with #[cfg(test)]; repeat the marker so UBS can filter test-only assertions.
#[cfg(test)]
use super::*;

/// Slack's own documented example vector
/// (<https://api.slack.com/authentication/verifying-requests-from-slack>):
/// proves the hand-rolled HMAC-SHA256 construction is byte-correct against a
/// third party's worked example, not just self-consistent with itself.
const DOC_SIGNING_SECRET: &str = "8f742231b10e8888abcd99yyyzzz85a5"; // ggignore
const DOC_TIMESTAMP: &str = "1531420618";
const DOC_BODY: &str = "token=xyzz0WbapA4vBCDEFasx0q6G&team_id=T1DC2JH3J&team_domain=testteamnow&channel_id=G8PSS9T3V&channel_name=foobar&user_id=U2CERLKJA&user_name=roadrunner&command=%2Fwebhook-collect&text=&response_url=https%3A%2F%2Fhooks.slack.com%2Fcommands%2FT1DC2JH3J%2F397700885554%2F96rGlfmibIGlgcZRskXaIFfN&trigger_id=398738663015.47445629121.803a0bc887a14d10d2c447fce8b6703c";
const DOC_EXPECTED_SIGNATURE: &str =
    "v0=a2114d57b48eac39b9ad189dd8316235a7b4a8d21a10bd27519666489c69b503";

#[test]
fn hmac_matches_slacks_documented_example_vector() {
    let mut base = format!("v0:{DOC_TIMESTAMP}:").into_bytes();
    base.extend_from_slice(DOC_BODY.as_bytes());
    let signature = format!(
        "v0={}",
        hmac_sha256_hex(DOC_SIGNING_SECRET.as_bytes(), &base)
    );
    assert_eq!(signature, DOC_EXPECTED_SIGNATURE);
}

fn current_timestamp() -> String {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock after unix epoch")
        .as_secs()
        .to_string()
}

fn sign(secret: &str, timestamp: &str, body: &[u8]) -> String {
    let mut base = format!("v0:{timestamp}:").into_bytes();
    base.extend_from_slice(body);
    format!("v0={}", hmac_sha256_hex(secret.as_bytes(), &base))
}

#[test]
fn verify_slack_signature_accepts_a_correctly_signed_request() {
    let ts = current_timestamp();
    let body = b"payload".to_vec();
    let signature = sign("shh", &ts, &body);
    verify_slack_signature("shh", &ts, &body, &signature).expect("valid signature accepted");
}

#[test]
fn verify_slack_signature_rejects_bad_signature() {
    let ts = current_timestamp();
    let body = b"payload".to_vec();
    let error = verify_slack_signature("shh", &ts, &body, "v0=deadbeef")
        .expect_err("wrong signature rejected");
    assert_eq!(error, "invalid slack request signature");
}

#[test]
fn verify_slack_signature_rejects_a_replayed_old_timestamp() {
    let stale_ts = (std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
        - SLACK_TIMESTAMP_TOLERANCE_SECS
        - 60)
        .to_string();
    let body = b"payload".to_vec();
    let signature = sign("shh", &stale_ts, &body);
    let error = verify_slack_signature("shh", &stale_ts, &body, &signature)
        .expect_err("stale timestamp rejected even with a matching signature");
    assert_eq!(error, "stale slack request timestamp");
}

#[test]
fn verify_slack_signature_rejects_a_body_tampered_after_signing() {
    let ts = current_timestamp();
    let signed_body = b"original payload".to_vec();
    let signature = sign("shh", &ts, &signed_body);
    let tampered_body = b"original payload, but attacker-modified".to_vec();
    let error = verify_slack_signature("shh", &ts, &tampered_body, &signature)
        .expect_err("signature computed over the original body must not match the tampered one");
    assert_eq!(error, "invalid slack request signature");
}

#[test]
fn constant_time_eq_rejects_mismatched_lengths_and_bytes() {
    assert!(constant_time_eq(b"abc", b"abc"));
    assert!(!constant_time_eq(b"abc", b"abd"));
    assert!(!constant_time_eq(b"abc", b"ab"));
}
