use std::io::{self, Read, Write};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use thiserror::Error;

pub mod qr;

pub const PROTOCOL_VERSION: u16 = 7;
pub const DEFAULT_CONTROL_PORT: u16 = 48172;
pub const DEFAULT_VIDEO_PORT: u16 = 48173;
pub const MAX_CONTROL_FRAME_BYTES: usize = 256 * 1024;
pub const MAX_VIDEO_FRAME_BYTES: usize = 32 * 1024 * 1024;
pub const AUTH_CHALLENGE_BYTES: usize = 32;
pub const AUTH_RESPONSE_BYTES: usize = 32;
pub const PAIRED_SECRET_BYTES: usize = 32;
pub const SESSION_KEY_BYTES: usize = 32;
pub const SESSION_KEY_FINGERPRINT_BYTES: usize = 8;
const PAIRING_AUTH_CONTEXT: &[u8] = b"AndroidConnect pairing v2 auth";
const PAIRED_SECRET_CONTEXT: &[u8] = b"AndroidConnect pairing v2 secret";
const TRUSTED_AUTH_CONTEXT: &[u8] = b"AndroidConnect trusted session v1";
const SESSION_KEY_CONTEXT: &[u8] = b"AndroidConnect session key v1";
const SESSION_KEY_FINGERPRINT_CONTEXT: &[u8] = b"AndroidConnect session key fingerprint v1";

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
    DeviceStatus(DeviceStatus),
    StorageStatus(StorageStatus),
    MediaStatus(MediaStatus),
    MediaControl(MediaControl),
    ClipboardText(ClipboardText),
    ClipboardImage(ClipboardImage),
    FileTransferStart(FileTransferStart),
    FileTransferChunk(FileTransferChunk),
    FileTransferComplete(FileTransferComplete),
    FileBrowseRequest(FileBrowseRequest),
    FileBrowseResponse(FileBrowseResponse),
    FileMutation(FileMutation),
    NotificationPosted(NotificationPosted),
    NotificationRemoved(NotificationRemoved),
    NotificationAction(NotificationAction),
    NotificationFilterUpdate(NotificationFilterUpdate),
    AudioFormat(AudioFormat),
    AudioFrame(AudioFrame),
    AudioControl(AudioControl),
    AppWindowOpen(AppWindowOpen),
    AppWindowClose(AppWindowClose),
    AppWindowInput(AppWindowInput),
    MessageThreadList(MessageThreadList),
    MessageEvent(MessageEvent),
    MessageSendRequest(MessageSendRequest),
    MessageThreadOpen(MessageThreadOpen),
    MessageThreadDetail(MessageThreadDetail),
    MessageSendResponse(MessageSendResponse),
    FileTransferRequest(FileTransferRequest),
    CallState(CallState),
    CallAction(CallAction),
    PhotoAssetList(PhotoAssetList),
    PhotoAssetTransfer(PhotoAssetTransfer),
    RelayOffer(RelayOffer),
    RelayStatus(RelayStatus),
    ClientList(ClientList),
    ClientRoleUpdate(ClientRoleUpdate),
    Ping { nonce: u64 },
    Pong { nonce: u64 },
    Error { message: String },
    AuthChallenge(AuthChallenge),
    AuthResponse(AuthResponse),
    AuthResult(AuthResult),
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
    pub utility_features: Vec<UtilityFeature>,
}

impl Capabilities {
    pub fn android_mvp() -> Self {
        Self {
            video_codecs: vec![VideoCodec::H264],
            pointer_input: true,
            keyboard_input: true,
            system_actions: true,
            audio_capture: false,
            utility_features: UtilityFeature::mvp2_all(),
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

#[derive(Debug, Copy, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum UtilityFeature {
    DeviceStatus,
    MediaControls,
    ClipboardText,
    ClipboardImage,
    FileTransfer,
    FileBrowser,
    Notifications,
    AudioForwarding,
    AppWindows,
    Messages,
    Calls,
    Storage,
    Photos,
    Relay,
    MultiClient,
}

impl UtilityFeature {
    pub fn mvp2_all() -> Vec<Self> {
        vec![
            Self::DeviceStatus,
            Self::MediaControls,
            Self::ClipboardText,
            Self::ClipboardImage,
            Self::FileTransfer,
            Self::FileBrowser,
            Self::Notifications,
            Self::AudioForwarding,
            Self::AppWindows,
            Self::Messages,
            Self::Calls,
            Self::Storage,
            Self::Photos,
            Self::Relay,
            Self::MultiClient,
        ]
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum FeatureState {
    Available,
    Disabled,
    PermissionRequired,
    Unsupported,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FeatureStatus {
    pub feature: UtilityFeature,
    pub state: FeatureState,
    pub message: String,
}

impl FeatureStatus {
    pub fn available(feature: UtilityFeature) -> Self {
        Self {
            feature,
            state: FeatureState::Available,
            message: String::new(),
        }
    }

    pub fn disabled(feature: UtilityFeature, message: impl Into<String>) -> Self {
        Self {
            feature,
            state: FeatureState::Disabled,
            message: message.into(),
        }
    }

    pub fn permission_required(feature: UtilityFeature, message: impl Into<String>) -> Self {
        Self {
            feature,
            state: FeatureState::PermissionRequired,
            message: message.into(),
        }
    }

    pub fn unsupported(feature: UtilityFeature, message: impl Into<String>) -> Self {
        Self {
            feature,
            state: FeatureState::Unsupported,
            message: message.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceStatus {
    pub device_id: String,
    pub device_name: String,
    pub manufacturer: String,
    pub model: String,
    pub android_sdk: u32,
    pub battery_percent: Option<u8>,
    pub charging: Option<bool>,
    pub interactive: Option<bool>,
    pub features: Vec<FeatureStatus>,
    #[serde(default)]
    pub wifi_state: Option<WifiState>,
    #[serde(default)]
    pub bluetooth_state: Option<BluetoothState>,
    #[serde(default)]
    pub dnd_state: Option<DndState>,
    #[serde(default)]
    pub volume: Option<VolumeState>,
}

/// Storage breakdown for the device's primary external storage volume.
/// All values are bytes. Photos + videos + apps + music + system + other
/// should sum to `used`; `free` plus `used` should equal `total`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StorageBreakdown {
    pub total: u64,
    pub used: u64,
    pub free: u64,
    pub photos: u64,
    pub videos: u64,
    pub apps: u64,
    pub music: u64,
    pub system: u64,
    pub other: u64,
}

/// Push-style update for primary device storage.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StorageStatus {
    pub primary: StorageBreakdown,
    pub status: FeatureStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WifiState {
    pub connected: bool,
    pub ssid: Option<String>,
    pub signal_strength: Option<i32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BluetoothState {
    pub enabled: bool,
    pub connected_devices: u8,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DndState {
    pub enabled: bool,
    pub mode: DndMode,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DndMode {
    Off,
    Priority,
    Alarms,
    TotalSilence,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VolumeState {
    pub media_percent: u8,
    pub ring_percent: Option<u8>,
    pub max_percent: u8,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MediaStatus {
    pub active: bool,
    pub app_package: Option<String>,
    pub app_name: Option<String>,
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub playback_state: MediaPlaybackState,
    pub position_ms: Option<u64>,
    pub duration_ms: Option<u64>,
    pub supported_actions: Vec<MediaControlAction>,
    pub status: FeatureStatus,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum MediaPlaybackState {
    Unknown,
    None,
    Stopped,
    Paused,
    Playing,
    Buffering,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MediaControl {
    pub action: MediaControlAction,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum MediaControlAction {
    Play,
    Pause,
    PlayPause,
    Previous,
    Next,
    Stop,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClipboardText {
    pub sequence: u64,
    pub text: String,
    pub source: ClipboardSource,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClipboardImage {
    pub sequence: u64,
    pub mime_type: String,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub data: Vec<u8>,
    pub source: ClipboardSource,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClipboardSource {
    Android,
    Desktop,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileTransferStart {
    pub transfer_id: String,
    pub direction: TransferDirection,
    pub file_name: String,
    pub mime_type: Option<String>,
    pub size_bytes: Option<u64>,
    pub target_path: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileTransferChunk {
    pub transfer_id: String,
    pub offset: u64,
    pub data: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileTransferComplete {
    pub transfer_id: String,
    pub status: TransferStatus,
    pub message: Option<String>,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TransferDirection {
    AndroidToDesktop,
    DesktopToAndroid,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TransferStatus {
    Completed,
    Cancelled,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileBrowseRequest {
    pub request_id: String,
    pub path: String,
    pub include_thumbnails: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileBrowseResponse {
    pub request_id: String,
    pub path: String,
    pub entries: Vec<FileEntry>,
    pub status: FeatureStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileEntry {
    pub name: String,
    pub path: String,
    pub entry_type: FileEntryType,
    pub size_bytes: Option<u64>,
    pub modified_unix_ms: Option<u64>,
    pub mime_type: Option<String>,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum FileEntryType {
    File,
    Directory,
    Media,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileMutation {
    pub request_id: String,
    pub mutation: FileMutationKind,
    pub path: String,
    pub new_path: Option<String>,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum FileMutationKind {
    CreateFolder,
    Delete,
    Rename,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NotificationPosted {
    pub notification_id: String,
    pub app_package: String,
    pub app_name: String,
    pub title: Option<String>,
    pub text: Option<String>,
    pub timestamp_unix_ms: u64,
    pub sensitive: bool,
    pub actions: Vec<NotificationActionDescriptor>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NotificationActionDescriptor {
    pub action_id: String,
    pub title: String,
    pub allows_reply: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NotificationRemoved {
    pub notification_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NotificationAction {
    pub notification_id: String,
    pub action_id: String,
    pub reply_text: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NotificationFilterUpdate {
    pub package_name: String,
    pub enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AudioFormat {
    pub stream_id: u32,
    pub codec: AudioCodec,
    pub sample_rate_hz: u32,
    pub channels: u16,
    pub status: FeatureStatus,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AudioCodec {
    PcmS16Le,
    Opus,
    Aac,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AudioFrame {
    pub stream_id: u32,
    pub presentation_time_us: i64,
    pub data: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AudioControl {
    pub command: AudioControlCommand,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AudioControlCommand {
    Start,
    Stop,
    Mute,
    Unmute,
    SetVolume { percent: u8 },
    SetDnd { mode: DndMode },
    SetBluetooth { enabled: bool },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppWindowOpen {
    pub request_id: String,
    pub package_name: String,
    pub activity_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppWindowClose {
    pub window_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppWindowInput {
    pub window_id: String,
    pub event: InputEvent,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageThreadList {
    pub threads: Vec<MessageThreadSummary>,
    pub status: FeatureStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageThreadSummary {
    pub thread_id: String,
    pub display_name: String,
    pub last_message: Option<String>,
    pub timestamp_unix_ms: Option<u64>,
    pub unread_count: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageEvent {
    pub thread_id: String,
    pub sender: String,
    pub body: String,
    pub timestamp_unix_ms: u64,
    pub attachments: Vec<SharedContent>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageSendRequest {
    pub request_id: String,
    pub thread_id: Option<String>,
    pub recipients: Vec<String>,
    pub body: String,
    pub attachments: Vec<SharedContent>,
}

/// Sent desktop → Android to request the full message history of a thread.
/// Android responds with `MessageThreadDetail`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageThreadOpen {
    pub request_id: String,
    pub thread_id: String,
    /// Maximum number of messages to return, newest-first. 0 = server default.
    pub limit: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageThreadDetail {
    pub request_id: String,
    pub thread_id: String,
    pub messages: Vec<MessageEntry>,
    pub status: FeatureStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageEntry {
    pub message_id: String,
    pub thread_id: String,
    pub sender: String,
    pub body: String,
    pub timestamp_unix_ms: u64,
    pub direction: MessageDirection,
    pub attachments: Vec<SharedContent>,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum MessageDirection {
    Inbound,
    Outbound,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageSendResponse {
    pub request_id: String,
    pub thread_id: Option<String>,
    pub result: MessageSendResult,
    pub message: Option<String>,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum MessageSendResult {
    Queued,
    Sent,
    Failed,
    PermissionDenied,
}

/// Sent desktop → Android to request download of a single file/path.
/// Android replies by starting a `FileTransferStart` (AndroidToDesktop) for that file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileTransferRequest {
    pub request_id: String,
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SharedContent {
    pub uri: Option<String>,
    pub file_name: Option<String>,
    pub mime_type: Option<String>,
    pub size_bytes: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CallState {
    pub call_id: Option<String>,
    pub state: PhoneCallState,
    pub display_name: Option<String>,
    pub phone_number: Option<String>,
    pub supported_actions: Vec<CallActionKind>,
    pub status: FeatureStatus,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PhoneCallState {
    Idle,
    Ringing,
    Dialing,
    Active,
    Held,
    Disconnected,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CallAction {
    pub call_id: Option<String>,
    pub action: CallActionKind,
    pub phone_number: Option<String>,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum CallActionKind {
    Answer,
    Decline,
    HangUp,
    Mute,
    Unmute,
    Dial,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PhotoAssetList {
    pub request_id: String,
    pub assets: Vec<PhotoAsset>,
    pub status: FeatureStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PhotoAsset {
    pub asset_id: String,
    pub display_name: String,
    pub mime_type: String,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub size_bytes: Option<u64>,
    pub created_unix_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PhotoAssetTransfer {
    pub asset_id: String,
    pub transfer_id: String,
    pub direction: TransferDirection,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RelayOffer {
    pub relay_id: String,
    pub endpoint: String,
    pub expires_unix_ms: u64,
    pub public_key_fingerprint: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RelayStatus {
    pub enabled: bool,
    pub connected: bool,
    pub remote: bool,
    pub status: FeatureStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientList {
    pub clients: Vec<ClientInfo>,
    pub input_owner_desktop_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientInfo {
    pub desktop_id: String,
    pub desktop_name: String,
    pub connected: bool,
    pub roles: Vec<ClientRole>,
    pub last_seen_unix_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientRoleUpdate {
    pub desktop_id: String,
    pub roles: Vec<ClientRole>,
    pub owns_input: bool,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClientRole {
    Viewer,
    Controller,
    FileTransfer,
    Notifications,
    Media,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthChallenge {
    pub challenge: [u8; AUTH_CHALLENGE_BYTES],
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthResponse {
    pub desktop_id: String,
    pub desktop_name: String,
    pub method: AuthMethod,
    pub response: [u8; AUTH_RESPONSE_BYTES],
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AuthMethod {
    PairingCode,
    TrustedSession,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthResult {
    pub accepted: bool,
    pub message: String,
    pub trusted: bool,
    pub desktop_id: Option<String>,
    pub desktop_name: Option<String>,
    pub session_key_fingerprint: Option<[u8; SESSION_KEY_FINGERPRINT_BYTES]>,
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

pub fn normalize_pairing_code(code: &str) -> String {
    code.chars()
        .filter(|ch| !ch.is_whitespace())
        .flat_map(|ch| ch.to_uppercase())
        .collect()
}

pub fn pairing_auth_response(
    pairing_code: &str,
    android_device_id: &str,
    desktop_id: &str,
    challenge: &[u8; AUTH_CHALLENGE_BYTES],
) -> [u8; AUTH_RESPONSE_BYTES] {
    let normalized = normalize_pairing_code(pairing_code);
    let mut message = Vec::new();
    message.extend_from_slice(PAIRING_AUTH_CONTEXT);
    append_identity_part(&mut message, android_device_id);
    append_identity_part(&mut message, desktop_id);
    message.extend_from_slice(challenge);
    hmac_sha256(normalized.as_bytes(), &message)
}

pub fn paired_secret_from_pairing_code(
    pairing_code: &str,
    android_device_id: &str,
    desktop_id: &str,
) -> [u8; PAIRED_SECRET_BYTES] {
    let normalized = normalize_pairing_code(pairing_code);
    let mut message = Vec::new();
    message.extend_from_slice(PAIRED_SECRET_CONTEXT);
    append_identity_part(&mut message, android_device_id);
    append_identity_part(&mut message, desktop_id);
    hmac_sha256(normalized.as_bytes(), &message)
}

pub fn trusted_session_auth_response(
    paired_secret: &[u8; PAIRED_SECRET_BYTES],
    android_device_id: &str,
    desktop_id: &str,
    challenge: &[u8; AUTH_CHALLENGE_BYTES],
) -> [u8; AUTH_RESPONSE_BYTES] {
    let mut message = Vec::new();
    message.extend_from_slice(TRUSTED_AUTH_CONTEXT);
    append_identity_part(&mut message, android_device_id);
    append_identity_part(&mut message, desktop_id);
    message.extend_from_slice(challenge);
    hmac_sha256(paired_secret, &message)
}

pub fn derive_session_key(
    paired_secret: &[u8; PAIRED_SECRET_BYTES],
    android_device_id: &str,
    desktop_id: &str,
    challenge: &[u8; AUTH_CHALLENGE_BYTES],
) -> [u8; SESSION_KEY_BYTES] {
    let mut message = Vec::new();
    message.extend_from_slice(SESSION_KEY_CONTEXT);
    append_identity_part(&mut message, android_device_id);
    append_identity_part(&mut message, desktop_id);
    message.extend_from_slice(challenge);
    hmac_sha256(paired_secret, &message)
}

pub fn session_key_fingerprint(
    session_key: &[u8; SESSION_KEY_BYTES],
) -> [u8; SESSION_KEY_FINGERPRINT_BYTES] {
    let digest = hmac_sha256(session_key, SESSION_KEY_FINGERPRINT_CONTEXT);
    let mut fingerprint = [0_u8; SESSION_KEY_FINGERPRINT_BYTES];
    fingerprint.copy_from_slice(&digest[..SESSION_KEY_FINGERPRINT_BYTES]);
    fingerprint
}

pub fn auth_responses_equal(
    expected: &[u8; AUTH_RESPONSE_BYTES],
    actual: &[u8; AUTH_RESPONSE_BYTES],
) -> bool {
    let mut diff = 0_u8;
    for (left, right) in expected.iter().zip(actual.iter()) {
        diff |= left ^ right;
    }
    diff == 0
}

pub fn bytes_to_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 0x0f) as usize] as char);
    }
    output
}

pub fn hex_to_fixed<const N: usize>(value: &str) -> Option<[u8; N]> {
    let clean: String = value.chars().filter(|ch| !ch.is_whitespace()).collect();
    if clean.len() != N * 2 {
        return None;
    }

    let mut output = [0_u8; N];
    for (index, pair) in clean.as_bytes().chunks_exact(2).enumerate() {
        let high = hex_digit(pair[0])?;
        let low = hex_digit(pair[1])?;
        output[index] = (high << 4) | low;
    }
    Some(output)
}

fn append_identity_part(message: &mut Vec<u8>, value: &str) {
    let bytes = value.as_bytes();
    message.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
    message.extend_from_slice(bytes);
}

fn hex_digit(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        b'A'..=b'F' => Some(value - b'A' + 10),
        _ => None,
    }
}

fn now_micros() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_micros() as u64)
        .unwrap_or_default()
}

fn hmac_sha256(key: &[u8], data: &[u8]) -> [u8; AUTH_RESPONSE_BYTES] {
    const BLOCK_BYTES: usize = 64;

    let mut key_block = [0_u8; BLOCK_BYTES];
    if key.len() > BLOCK_BYTES {
        key_block[..AUTH_RESPONSE_BYTES].copy_from_slice(&sha256(key));
    } else {
        key_block[..key.len()].copy_from_slice(key);
    }

    let mut inner_pad = [0x36_u8; BLOCK_BYTES];
    let mut outer_pad = [0x5c_u8; BLOCK_BYTES];
    for index in 0..BLOCK_BYTES {
        inner_pad[index] ^= key_block[index];
        outer_pad[index] ^= key_block[index];
    }

    let mut inner = Vec::with_capacity(BLOCK_BYTES + data.len());
    inner.extend_from_slice(&inner_pad);
    inner.extend_from_slice(data);
    let inner_hash = sha256(&inner);

    let mut outer = Vec::with_capacity(BLOCK_BYTES + inner_hash.len());
    outer.extend_from_slice(&outer_pad);
    outer.extend_from_slice(&inner_hash);
    sha256(&outer)
}

fn sha256(input: &[u8]) -> [u8; AUTH_RESPONSE_BYTES] {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];

    let mut h0 = 0x6a09e667_u32;
    let mut h1 = 0xbb67ae85_u32;
    let mut h2 = 0x3c6ef372_u32;
    let mut h3 = 0xa54ff53a_u32;
    let mut h4 = 0x510e527f_u32;
    let mut h5 = 0x9b05688c_u32;
    let mut h6 = 0x1f83d9ab_u32;
    let mut h7 = 0x5be0cd19_u32;

    let bit_len = (input.len() as u64).wrapping_mul(8);
    let mut padded = Vec::with_capacity(input.len() + 72);
    padded.extend_from_slice(input);
    padded.push(0x80);
    while (padded.len() + 8) % 64 != 0 {
        padded.push(0);
    }
    padded.extend_from_slice(&bit_len.to_be_bytes());

    for chunk in padded.chunks_exact(64) {
        let mut w = [0_u32; 64];
        for (index, word) in w.iter_mut().take(16).enumerate() {
            let offset = index * 4;
            *word = u32::from_be_bytes([
                chunk[offset],
                chunk[offset + 1],
                chunk[offset + 2],
                chunk[offset + 3],
            ]);
        }
        for index in 16..64 {
            let s0 = w[index - 15].rotate_right(7)
                ^ w[index - 15].rotate_right(18)
                ^ (w[index - 15] >> 3);
            let s1 = w[index - 2].rotate_right(17)
                ^ w[index - 2].rotate_right(19)
                ^ (w[index - 2] >> 10);
            w[index] = w[index - 16]
                .wrapping_add(s0)
                .wrapping_add(w[index - 7])
                .wrapping_add(s1);
        }

        let mut a = h0;
        let mut b = h1;
        let mut c = h2;
        let mut d = h3;
        let mut e = h4;
        let mut f = h5;
        let mut g = h6;
        let mut h = h7;

        for index in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let temp1 = h
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[index])
                .wrapping_add(w[index]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let temp2 = s0.wrapping_add(maj);

            h = g;
            g = f;
            f = e;
            e = d.wrapping_add(temp1);
            d = c;
            c = b;
            b = a;
            a = temp1.wrapping_add(temp2);
        }

        h0 = h0.wrapping_add(a);
        h1 = h1.wrapping_add(b);
        h2 = h2.wrapping_add(c);
        h3 = h3.wrapping_add(d);
        h4 = h4.wrapping_add(e);
        h5 = h5.wrapping_add(f);
        h6 = h6.wrapping_add(g);
        h7 = h7.wrapping_add(h);
    }

    let mut output = [0_u8; AUTH_RESPONSE_BYTES];
    for (index, value) in [h0, h1, h2, h3, h4, h5, h6, h7].iter().enumerate() {
        output[index * 4..index * 4 + 4].copy_from_slice(&value.to_be_bytes());
    }
    output
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
    fn mvp2_utility_payload_round_trips() {
        let envelope = Envelope::new(
            8,
            Payload::DeviceStatus(DeviceStatus {
                device_id: "phone-1".to_owned(),
                device_name: "Pixel test device".to_owned(),
                manufacturer: "Google".to_owned(),
                model: "Pixel".to_owned(),
                android_sdk: 35,
                battery_percent: Some(86),
                charging: Some(true),
                interactive: Some(true),
                features: vec![
                    FeatureStatus::available(UtilityFeature::DeviceStatus),
                    FeatureStatus::permission_required(
                        UtilityFeature::Notifications,
                        "notification listener access is not enabled",
                    ),
                ],
                wifi_state: None,
                bluetooth_state: None,
                dnd_state: None,
                volume: None,
            }),
        );

        let bytes = encode_envelope(&envelope).expect("encode");
        let decoded = decode_envelope(&bytes).expect("decode");

        assert_eq!(decoded.version, PROTOCOL_VERSION);
        assert_eq!(decoded.sequence, 8);
        assert_eq!(decoded.payload, envelope.payload);
        assert!(UtilityFeature::mvp2_all().contains(&UtilityFeature::Storage));
    }

    #[test]
    fn storage_status_round_trips() {
        let envelope = Envelope::new(
            9,
            Payload::StorageStatus(StorageStatus {
                primary: StorageBreakdown {
                    total: 128_000_000_000,
                    used: 96_000_000_000,
                    free: 32_000_000_000,
                    photos: 28_000_000_000,
                    videos: 18_000_000_000,
                    apps: 24_000_000_000,
                    music: 8_000_000_000,
                    system: 18_000_000_000,
                    other: 0,
                },
                status: FeatureStatus::available(UtilityFeature::Storage),
            }),
        );

        let bytes = encode_envelope(&envelope).expect("encode");
        let decoded = decode_envelope(&bytes).expect("decode");

        assert_eq!(decoded.version, PROTOCOL_VERSION);
        assert_eq!(decoded.sequence, 9);
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

    #[test]
    fn normalizes_pairing_codes() {
        assert_eq!(normalize_pairing_code(" 12 ab\ncd "), "12ABCD");
    }

    #[test]
    fn hmac_sha256_matches_rfc_4231_case_1() {
        let key = [0x0b_u8; 20];
        let actual = hmac_sha256(&key, b"Hi There");
        let expected = hex_bytes(
            "b0344c61d8db38535ca8afceaf0bf12b\
                                  881dc200c9833da726e9376c2e32cff7",
        );
        assert_eq!(actual, expected.as_slice());
    }

    #[test]
    fn pairing_auth_response_changes_with_code() {
        let challenge = [7_u8; AUTH_CHALLENGE_BYTES];
        let first = pairing_auth_response("123456", "android-1", "desktop-1", &challenge);
        let second = pairing_auth_response("654321", "android-1", "desktop-1", &challenge);
        assert!(auth_responses_equal(&first, &first));
        assert!(!auth_responses_equal(&first, &second));
    }

    #[test]
    fn pairing_auth_response_binds_device_identities() {
        let challenge = [7_u8; AUTH_CHALLENGE_BYTES];
        let first = pairing_auth_response("123456", "android-1", "desktop-1", &challenge);
        let second = pairing_auth_response("123456", "android-2", "desktop-1", &challenge);
        let third = pairing_auth_response("123456", "android-1", "desktop-2", &challenge);

        assert!(!auth_responses_equal(&first, &second));
        assert!(!auth_responses_equal(&first, &third));
    }

    #[test]
    fn trusted_auth_and_session_key_use_paired_secret() {
        let challenge = [9_u8; AUTH_CHALLENGE_BYTES];
        let paired_secret = paired_secret_from_pairing_code("123456", "android-1", "desktop-1");
        let auth =
            trusted_session_auth_response(&paired_secret, "android-1", "desktop-1", &challenge);
        let session_key = derive_session_key(&paired_secret, "android-1", "desktop-1", &challenge);
        let fingerprint = session_key_fingerprint(&session_key);

        assert_eq!(auth.len(), AUTH_RESPONSE_BYTES);
        assert_eq!(session_key.len(), SESSION_KEY_BYTES);
        assert_eq!(fingerprint.len(), SESSION_KEY_FINGERPRINT_BYTES);
        assert_ne!(auth, session_key);
    }

    #[test]
    fn hex_helpers_round_trip_fixed_arrays() {
        let bytes = [0xab_u8; PAIRED_SECRET_BYTES];
        let encoded = bytes_to_hex(&bytes);
        assert_eq!(hex_to_fixed::<PAIRED_SECRET_BYTES>(&encoded), Some(bytes));
        assert_eq!(hex_to_fixed::<PAIRED_SECRET_BYTES>("not hex"), None);
    }

    fn hex_bytes(input: &str) -> Vec<u8> {
        let clean: String = input.chars().filter(|ch| !ch.is_whitespace()).collect();
        assert_eq!(clean.len() % 2, 0);
        clean
            .as_bytes()
            .chunks_exact(2)
            .map(|pair| {
                let high = hex_digit(pair[0]);
                let low = hex_digit(pair[1]);
                (high << 4) | low
            })
            .collect()
    }

    fn hex_digit(value: u8) -> u8 {
        match value {
            b'0'..=b'9' => value - b'0',
            b'a'..=b'f' => value - b'a' + 10,
            b'A'..=b'F' => value - b'A' + 10,
            _ => panic!("invalid hex digit"),
        }
    }
}
