use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::PROTOCOL_VERSION;

pub const QR_URI_SCHEME: &str = "androidconnect";
pub const QR_URI_HOST: &str = "pair";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QrPairingPayload {
    pub protocol_version: u16,
    pub addresses: Vec<String>,
    pub pairing_token: String,
    pub desktop_id: String,
    pub desktop_name: String,
    pub issued_unix_ms: u64,
    pub expires_unix_ms: u64,
}

impl QrPairingPayload {
    pub fn new(
        addresses: Vec<String>,
        pairing_token: impl Into<String>,
        desktop_id: impl Into<String>,
        desktop_name: impl Into<String>,
        issued_unix_ms: u64,
        expires_unix_ms: u64,
    ) -> Self {
        Self {
            protocol_version: PROTOCOL_VERSION,
            addresses,
            pairing_token: pairing_token.into(),
            desktop_id: desktop_id.into(),
            desktop_name: desktop_name.into(),
            issued_unix_ms,
            expires_unix_ms,
        }
    }
}

#[derive(Debug, Error)]
pub enum QrPayloadError {
    #[error("json encode failed: {0}")]
    JsonEncode(#[source] serde_json::Error),
    #[error("json decode failed: {0}")]
    JsonDecode(#[source] serde_json::Error),
    #[error("uri scheme must be \"{expected}://{host}\", got \"{actual}\"")]
    InvalidScheme {
        expected: &'static str,
        host: &'static str,
        actual: String,
    },
    #[error("uri is missing the `data` query parameter")]
    MissingData,
    #[error("base64url decode failed: {0}")]
    Base64Decode(String),
    #[error(
        "protocol version mismatch: payload encodes v{payload}, this build expects v{expected}"
    )]
    VersionMismatch { payload: u16, expected: u16 },
}

pub fn encode_qr_payload(payload: &QrPairingPayload) -> Result<String, QrPayloadError> {
    let json = serde_json::to_vec(payload).map_err(QrPayloadError::JsonEncode)?;
    let data = base64url_encode(&json);
    Ok(format!(
        "{QR_URI_SCHEME}://{QR_URI_HOST}?v={}&data={data}",
        payload.protocol_version
    ))
}

pub fn decode_qr_payload(uri: &str) -> Result<QrPairingPayload, QrPayloadError> {
    let prefix = format!("{QR_URI_SCHEME}://{QR_URI_HOST}?");
    let query = uri
        .strip_prefix(&prefix)
        .ok_or_else(|| QrPayloadError::InvalidScheme {
            expected: QR_URI_SCHEME,
            host: QR_URI_HOST,
            actual: uri.to_owned(),
        })?;

    let mut data: Option<&str> = None;
    for pair in query.split('&') {
        let mut split = pair.splitn(2, '=');
        let key = split.next().unwrap_or("");
        let value = split.next().unwrap_or("");
        if key == "data" {
            data = Some(value);
        }
    }

    let data = data.ok_or(QrPayloadError::MissingData)?;
    let bytes = base64url_decode(data).map_err(QrPayloadError::Base64Decode)?;
    let payload: QrPairingPayload =
        serde_json::from_slice(&bytes).map_err(QrPayloadError::JsonDecode)?;
    Ok(payload)
}

pub fn decode_qr_payload_strict(uri: &str) -> Result<QrPairingPayload, QrPayloadError> {
    let payload = decode_qr_payload(uri)?;
    if payload.protocol_version != PROTOCOL_VERSION {
        return Err(QrPayloadError::VersionMismatch {
            payload: payload.protocol_version,
            expected: PROTOCOL_VERSION,
        });
    }
    Ok(payload)
}

const BASE64URL_ALPHABET: &[u8; 64] =
    b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

pub fn base64url_encode(input: &[u8]) -> String {
    let mut out = String::with_capacity((input.len() * 4).div_ceil(3));
    for chunk in input.chunks(3) {
        let b0 = chunk[0];
        let b1 = chunk.get(1).copied().unwrap_or(0);
        let b2 = chunk.get(2).copied().unwrap_or(0);

        out.push(BASE64URL_ALPHABET[(b0 >> 2) as usize] as char);
        out.push(BASE64URL_ALPHABET[(((b0 & 0x03) << 4) | (b1 >> 4)) as usize] as char);
        if chunk.len() > 1 {
            out.push(BASE64URL_ALPHABET[(((b1 & 0x0f) << 2) | (b2 >> 6)) as usize] as char);
        }
        if chunk.len() > 2 {
            out.push(BASE64URL_ALPHABET[(b2 & 0x3f) as usize] as char);
        }
    }
    out
}

pub fn base64url_decode(input: &str) -> Result<Vec<u8>, String> {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len() * 3 / 4);
    let mut buffer = 0_u32;
    let mut bits = 0_u8;

    for &byte in bytes {
        if byte == b'=' {
            break;
        }
        let value = decode_base64url_byte(byte)
            .ok_or_else(|| format!("invalid base64url byte: 0x{byte:02x}"))?;
        buffer = (buffer << 6) | u32::from(value);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buffer >> bits) as u8);
            buffer &= (1 << bits) - 1;
        }
    }

    Ok(out)
}

fn decode_base64url_byte(byte: u8) -> Option<u8> {
    match byte {
        b'A'..=b'Z' => Some(byte - b'A'),
        b'a'..=b'z' => Some(byte - b'a' + 26),
        b'0'..=b'9' => Some(byte - b'0' + 52),
        b'-' => Some(62),
        b'_' => Some(63),
        _ => None,
    }
}
