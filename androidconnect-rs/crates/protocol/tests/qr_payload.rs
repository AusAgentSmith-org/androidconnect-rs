use androidconnect_protocol::PROTOCOL_VERSION;
use androidconnect_protocol::qr::{
    QrPairingPayload, QrPayloadError, base64url_decode, base64url_encode, decode_qr_payload,
    decode_qr_payload_strict, encode_qr_payload,
};

fn sample_payload() -> QrPairingPayload {
    QrPairingPayload::new(
        vec![
            "192.168.1.50:48172".to_owned(),
            "[fd00::dead:beef]:48172".to_owned(),
        ],
        "12 34 56",
        "desktop-abc",
        "Sprooty's Desktop & Stuff?",
        1_700_000_000_000,
        1_700_000_900_000,
    )
}

#[test]
fn round_trip_encode_decode() {
    let original = sample_payload();
    let encoded = encode_qr_payload(&original).expect("encode");
    assert!(encoded.starts_with("androidconnect://pair?"));
    assert!(encoded.contains(&format!("v={PROTOCOL_VERSION}")));

    let decoded = decode_qr_payload(&encoded).expect("decode");
    assert_eq!(decoded, original);
}

#[test]
fn decode_strict_accepts_current_version() {
    let encoded = encode_qr_payload(&sample_payload()).expect("encode");
    let decoded = decode_qr_payload_strict(&encoded).expect("strict decode");
    assert_eq!(decoded.protocol_version, PROTOCOL_VERSION);
}

#[test]
fn decode_strict_rejects_mismatched_version() {
    let mut payload = sample_payload();
    payload.protocol_version = PROTOCOL_VERSION.saturating_sub(1).max(1);
    if payload.protocol_version == PROTOCOL_VERSION {
        // Protocol version is already at the floor; nothing to test.
        return;
    }
    let encoded = encode_qr_payload(&payload).expect("encode");
    let error = decode_qr_payload_strict(&encoded).expect_err("must reject");
    assert!(matches!(error, QrPayloadError::VersionMismatch { .. }));
}

#[test]
fn decode_rejects_wrong_scheme() {
    let bogus = "https://example.com/pair?data=abc";
    let error = decode_qr_payload(bogus).expect_err("must reject");
    assert!(matches!(error, QrPayloadError::InvalidScheme { .. }));
}

#[test]
fn decode_rejects_missing_data_param() {
    let bogus = "androidconnect://pair?v=5";
    let error = decode_qr_payload(bogus).expect_err("must reject");
    assert!(matches!(error, QrPayloadError::MissingData));
}

#[test]
fn decode_rejects_invalid_base64() {
    let bogus = "androidconnect://pair?data=!!!not-base64!!!";
    let error = decode_qr_payload(bogus).expect_err("must reject");
    assert!(matches!(error, QrPayloadError::Base64Decode(_)));
}

#[test]
fn base64url_round_trip_byte_lengths() {
    for len in 0..=64_usize {
        let bytes: Vec<u8> = (0..len).map(|i| (i * 7) as u8).collect();
        let encoded = base64url_encode(&bytes);
        let decoded = base64url_decode(&encoded).expect("decode");
        assert_eq!(decoded, bytes, "round-trip failed for length {len}");
        assert!(
            !encoded.contains('+') && !encoded.contains('/') && !encoded.contains('='),
            "encoded value must use the url-safe alphabet without padding"
        );
    }
}

#[test]
fn uri_query_value_contains_no_raw_ampersand() {
    let payload = sample_payload();
    let encoded = encode_qr_payload(&payload).expect("encode");
    let (_, query) = encoded
        .split_once('?')
        .expect("encoded uri has a query string");
    let data = query
        .split('&')
        .find_map(|pair| pair.strip_prefix("data="))
        .expect("data param present");
    assert!(
        !data.contains('&') && !data.contains('='),
        "base64url data must be safe inside a query parameter"
    );
}
