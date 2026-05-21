use std::io::{self, Read, Write};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const PROTOCOL_VERSION: u16 = 1;
pub const DEFAULT_CONTROL_PORT: u16 = 48172;
pub const DEFAULT_VIDEO_PORT: u16 = 48173;
pub const MAX_CONTROL_FRAME_BYTES: usize = 256 * 1024;
pub const MAX_VIDEO_FRAME_BYTES: usize = 32 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Envelope {
    pub version: u16,
    pub sequence: u64,
    pub timestamp_micros: u64,
    pub payload: Payload,
}

impl Envelope {
    pub fn new(sequence: u64, payload: Payload) -> Self {
        Self {
            version: PROTOCOL_VERSION,
            sequence,
            timestamp_micros: now_micros(),
            payload,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Payload {
    Hello(DeviceHello),
    VideoFormat(VideoFormat),
    VideoFrame(VideoFrame),
    Input(InputEvent),
    Ping { nonce: u64 },
    Pong { nonce: u64 },
    Error { message: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceHello {
    pub device_id: String,
    pub device_name: String,
    pub protocol_version: u16,
    pub capabilities: Capabilities,
}

impl DeviceHello {
    pub fn android(device_id: impl Into<String>, device_name: impl Into<String>) -> Self {
        Self {
            device_id: device_id.into(),
            device_name: device_name.into(),
            protocol_version: PROTOCOL_VERSION,
            capabilities: Capabilities::android_mvp(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Capabilities {
    pub video_codecs: Vec<VideoCodec>,
    pub pointer_input: bool,
    pub keyboard_input: bool,
    pub system_actions: bool,
    pub audio_capture: bool,
}

impl Capabilities {
    pub fn android_mvp() -> Self {
        Self {
            video_codecs: vec![VideoCodec::H264],
            pointer_input: true,
            keyboard_input: true,
            system_actions: true,
            audio_capture: false,
        }
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum VideoCodec {
    H264,
    Hevc,
    Av1,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VideoFormat {
    pub stream_id: u32,
    pub codec: VideoCodec,
    pub width: u32,
    pub height: u32,
    pub rotation_degrees: u16,
    pub dpi: u32,
    pub frame_rate: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VideoFrame {
    pub stream_id: u32,
    pub presentation_time_us: i64,
    pub flags: VideoFrameFlags,
    pub data: Vec<u8>,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct VideoFrameFlags {
    pub key_frame: bool,
    pub codec_config: bool,
    pub end_of_stream: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum InputEvent {
    Pointer(PointerEvent),
    Key(KeyEvent),
    Text(TextInput),
    System(SystemAction),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PointerEvent {
    pub pointer_id: u32,
    pub x: i32,
    pub y: i32,
    pub phase: PointerPhase,
    pub button: PointerButton,
    pub delta_x: i32,
    pub delta_y: i32,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PointerPhase {
    Down,
    Move,
    Up,
    Wheel,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PointerButton {
    None,
    Left,
    Middle,
    Right,
    Back,
    Forward,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyEvent {
    pub key_code: u32,
    pub pressed: bool,
    pub modifiers: Modifiers,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Modifiers {
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
    pub meta: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextInput {
    pub text: String,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SystemAction {
    Back,
    Home,
    Recents,
    LockScreen,
}

#[derive(Debug, Error)]
pub enum WireError {
    #[error("codec error: {0}")]
    Codec(#[from] Box<bincode::ErrorKind>),
    #[error("io error: {0}")]
    Io(#[from] io::Error),
    #[error("frame is {size} bytes, max is {max} bytes")]
    FrameTooLarge { size: usize, max: usize },
}

pub fn encode_envelope(envelope: &Envelope) -> Result<Vec<u8>, WireError> {
    Ok(bincode::serialize(envelope)?)
}

pub fn decode_envelope(bytes: &[u8]) -> Result<Envelope, WireError> {
    Ok(bincode::deserialize(bytes)?)
}

pub fn write_length_prefixed<W: Write>(
    writer: &mut W,
    envelope: &Envelope,
) -> Result<(), WireError> {
    let bytes = encode_envelope(envelope)?;
    writer.write_all(&(bytes.len() as u32).to_be_bytes())?;
    writer.write_all(&bytes)?;
    Ok(())
}

pub fn read_length_prefixed<R: Read>(
    reader: &mut R,
    max_frame_bytes: usize,
) -> Result<Envelope, WireError> {
    let mut len = [0_u8; 4];
    reader.read_exact(&mut len)?;
    let len = u32::from_be_bytes(len) as usize;
    if len > max_frame_bytes {
        return Err(WireError::FrameTooLarge {
            size: len,
            max: max_frame_bytes,
        });
    }

    let mut bytes = vec![0_u8; len];
    reader.read_exact(&mut bytes)?;
    decode_envelope(&bytes)
}

pub fn video_flags_from_android_media_codec(flags: i32) -> VideoFrameFlags {
    const BUFFER_FLAG_CODEC_CONFIG: i32 = 2;
    const BUFFER_FLAG_END_OF_STREAM: i32 = 4;
    const BUFFER_FLAG_KEY_FRAME: i32 = 1;

    VideoFrameFlags {
        key_frame: flags & BUFFER_FLAG_KEY_FRAME != 0,
        codec_config: flags & BUFFER_FLAG_CODEC_CONFIG != 0,
        end_of_stream: flags & BUFFER_FLAG_END_OF_STREAM != 0,
    }
}

fn now_micros() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_micros() as u64)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn envelope_round_trips() {
        let envelope = Envelope::new(
            7,
            Payload::Hello(DeviceHello::android("phone-1", "Pixel test device")),
        );

        let bytes = encode_envelope(&envelope).expect("encode");
        let decoded = decode_envelope(&bytes).expect("decode");

        assert_eq!(decoded.version, PROTOCOL_VERSION);
        assert_eq!(decoded.sequence, 7);
        assert_eq!(decoded.payload, envelope.payload);
    }

    #[test]
    fn rejects_oversized_length_prefix() {
        let mut encoded = Vec::new();
        encoded.extend_from_slice(&1024_u32.to_be_bytes());

        let error = read_length_prefixed(&mut &encoded[..], 4).expect_err("must reject");
        assert!(matches!(
            error,
            WireError::FrameTooLarge { size: 1024, max: 4 }
        ));
    }
}
