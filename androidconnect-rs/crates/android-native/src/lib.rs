use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::net::{Shutdown, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use androidconnect_protocol::{
    AUTH_CHALLENGE_BYTES, AuthChallenge, AuthMethod, AuthResult, ClipboardSource, ClipboardText,
    DeviceHello, DeviceStatus, Envelope, FeatureState, FeatureStatus, FileBrowseRequest,
    FileBrowseResponse, FileEntry, FileEntryType, FileMutation, FileMutationKind,
    FileTransferChunk, FileTransferComplete, FileTransferStart, InputEvent,
    MAX_CONTROL_FRAME_BYTES, MediaControlAction, PAIRED_SECRET_BYTES, PROTOCOL_VERSION, Payload,
    PointerButton, PointerEvent, PointerPhase, SESSION_KEY_FINGERPRINT_BYTES, SystemAction,
    TransferDirection, TransferStatus, UtilityFeature, VideoCodec, VideoFormat, VideoFrame,
    auth_responses_equal, bytes_to_hex, derive_session_key, hex_to_fixed, normalize_pairing_code,
    paired_secret_from_pairing_code, pairing_auth_response, read_length_prefixed,
    session_key_fingerprint, trusted_session_auth_response, video_flags_from_android_media_codec,
    write_length_prefixed,
};
use jni::objects::{GlobalRef, JByteArray, JClass, JString, JValue};
use jni::sys::{JNI_FALSE, JNI_TRUE, jboolean, jint, jlong, jstring};
use jni::{JNIEnv, JavaVM};
use serde::{Deserialize, Serialize};

static STATE: OnceLock<Mutex<AppState>> = OnceLock::new();

const RECONNECT_INITIAL_DELAY: Duration = Duration::from_millis(500);
const RECONNECT_MAX_DELAY: Duration = Duration::from_secs(5);

#[derive(Debug)]
struct AppState {
    sequence: u64,
    capture: Option<CaptureState>,
    stream: Option<TcpStream>,
    connected_to: Option<String>,
    connection_generation: u64,
    last_error: Option<String>,
    last_video_format: Option<VideoFormat>,
    pairing_code_set: bool,
    input_authenticated: bool,
    paired_desktop_id: Option<String>,
    paired_desktop_name: Option<String>,
    session_key_fingerprint: Option<[u8; SESSION_KEY_FINGERPRINT_BYTES]>,
    trusted_desktop_count: usize,
    android_device_id: Option<String>,
    reconnect_config: Option<SessionConnectConfig>,
    reconnect_callbacks: Option<SessionCallbacks>,
    reconnect_generation: u64,
    reconnecting: bool,
    reconnect_attempts: u64,
    last_reconnect_error: Option<String>,
    sent_envelopes: u64,
    sent_bytes: u64,
    received_pings: u64,
    sent_pongs: u64,
    sent_utility_envelopes: u64,
    status_revision: u64,
    clipboard_sequence: u64,
    incoming_transfers: HashMap<String, IncomingTransfer>,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            sequence: 1,
            capture: None,
            stream: None,
            connected_to: None,
            connection_generation: 0,
            last_error: None,
            last_video_format: None,
            pairing_code_set: false,
            input_authenticated: false,
            paired_desktop_id: None,
            paired_desktop_name: None,
            session_key_fingerprint: None,
            trusted_desktop_count: 0,
            android_device_id: None,
            reconnect_config: None,
            reconnect_callbacks: None,
            reconnect_generation: 0,
            reconnecting: false,
            reconnect_attempts: 0,
            last_reconnect_error: None,
            sent_envelopes: 0,
            sent_bytes: 0,
            received_pings: 0,
            sent_pongs: 0,
            sent_utility_envelopes: 0,
            status_revision: 0,
            clipboard_sequence: 1,
            incoming_transfers: HashMap::new(),
        }
    }
}

impl AppState {
    fn next_sequence(&mut self) -> u64 {
        let sequence = self.sequence;
        self.sequence += 1;
        sequence
    }

    fn send_payload(&mut self, payload: Payload) -> Result<(), String> {
        let envelope = Envelope::new(self.next_sequence(), payload);
        let video_bytes = match &envelope.payload {
            Payload::VideoFrame(frame) => frame.data.len() as u64,
            _ => 0,
        };
        let stream = self.stream.as_mut().ok_or("not connected")?;
        write_length_prefixed(stream, &envelope).map_err(|error| error.to_string())?;
        self.sent_envelopes += 1;
        self.sent_bytes += video_bytes;
        Ok(())
    }

    fn send_utility_payload(&mut self, payload: Payload) -> Result<(), String> {
        if !self.input_authenticated {
            return Err("desktop is not authenticated".to_owned());
        }
        self.send_payload(payload)?;
        self.sent_utility_envelopes += 1;
        Ok(())
    }

    fn clear_connection(&mut self, error: impl Into<String>) {
        self.connection_generation += 1;
        close_stream(self.stream.take());
        self.connected_to = None;
        self.input_authenticated = false;
        self.paired_desktop_id = None;
        self.paired_desktop_name = None;
        self.session_key_fingerprint = None;
        self.incoming_transfers.clear();
        self.last_error = Some(error.into());
        self.mark_status_changed();
    }

    fn reset_active_connection(&mut self) {
        self.connection_generation += 1;
        close_stream(self.stream.take());
        self.connected_to = None;
        self.input_authenticated = false;
        self.paired_desktop_id = None;
        self.paired_desktop_name = None;
        self.session_key_fingerprint = None;
        self.incoming_transfers.clear();
    }

    fn is_connected(&self) -> bool {
        self.stream.is_some()
    }

    fn mark_status_changed(&mut self) {
        self.status_revision = self.status_revision.wrapping_add(1);
    }
}

#[derive(Debug)]
struct IncomingTransfer {
    path: PathBuf,
    next_offset: u64,
}

#[derive(Debug, Clone)]
struct SessionConnectConfig {
    address: String,
    device_name: String,
    storage_dir: PathBuf,
    pairing_code: String,
}

#[derive(Debug, Clone)]
struct SessionCallbacks {
    java_vm: Arc<JavaVM>,
    input_class: GlobalRef,
    status_class: GlobalRef,
}

#[derive(Debug, Clone)]
struct ReconnectRequest {
    config: SessionConnectConfig,
    callbacks: SessionCallbacks,
    reconnect_generation: u64,
}

#[derive(Debug)]
struct CaptureState {
    started_at: Instant,
    encoded_frames: u64,
    encoded_bytes: u64,
}

fn state() -> &'static Mutex<AppState> {
    STATE.get_or_init(|| Mutex::new(AppState::default()))
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_androidconnect_NativeBridge_nativeStartSession(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
    config_json: JString<'_>,
) -> jlong {
    let _config = env
        .get_string(&config_json)
        .map(|value| value.to_string_lossy().into_owned())
        .unwrap_or_default();

    let mut guard = match state().lock() {
        Ok(guard) => guard,
        Err(_) => return 0,
    };

    guard.capture = Some(CaptureState {
        started_at: Instant::now(),
        encoded_frames: 0,
        encoded_bytes: 0,
    });
    guard.last_error = None;
    guard.mark_status_changed();
    drop(guard);
    notify_native_status_changed(&mut env);

    1
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_androidconnect_NativeBridge_nativeStopSession(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
) {
    let changed = if let Ok(mut guard) = state().lock() {
        let changed = guard.capture.take().is_some();
        if changed {
            guard.mark_status_changed();
        }
        changed
    } else {
        false
    };
    if changed {
        notify_native_status_changed(&mut env);
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_androidconnect_NativeBridge_nativeConnect(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
    host: JString<'_>,
    port: jint,
    device_name: JString<'_>,
    storage_dir: JString<'_>,
    pairing_code: JString<'_>,
) -> jboolean {
    let host = match env.get_string(&host) {
        Ok(value) => value.to_string_lossy().into_owned(),
        Err(error) => {
            set_last_error(format!("invalid host string: {error}"));
            notify_native_status_changed(&mut env);
            return JNI_FALSE;
        }
    };
    let device_name = env
        .get_string(&device_name)
        .map(|value| value.to_string_lossy().into_owned())
        .unwrap_or_else(|_| "Android device".to_owned());
    let storage_dir = match env.get_string(&storage_dir) {
        Ok(value) => PathBuf::from(value.to_string_lossy().into_owned()),
        Err(error) => {
            set_last_error(format!("invalid storage dir string: {error}"));
            notify_native_status_changed(&mut env);
            return JNI_FALSE;
        }
    };
    let pairing_code = match env.get_string(&pairing_code) {
        Ok(value) => normalize_pairing_code(&value.to_string_lossy()),
        Err(error) => {
            set_last_error(format!("invalid pairing code string: {error}"));
            notify_native_status_changed(&mut env);
            return JNI_FALSE;
        }
    };

    if host.trim().is_empty() || port <= 0 || port > u16::MAX as jint {
        set_last_error("invalid host or port");
        notify_native_status_changed(&mut env);
        return JNI_FALSE;
    }
    let address = format!("{}:{}", host.trim(), port);
    let java_vm = match env.get_java_vm() {
        Ok(vm) => vm,
        Err(error) => {
            set_last_error(format!("JavaVM unavailable: {error}"));
            notify_native_status_changed(&mut env);
            return JNI_FALSE;
        }
    };
    let status_class = match native_bridge_class(&mut env) {
        Ok(class) => class,
        Err(error) => {
            set_last_error(error);
            notify_native_status_changed(&mut env);
            return JNI_FALSE;
        }
    };
    let input_class = match remote_control_class(&mut env) {
        Ok(class) => class,
        Err(error) => {
            set_last_error(error);
            notify_native_status_changed_with_class(&mut env, &status_class);
            return JNI_FALSE;
        }
    };
    let callbacks = SessionCallbacks {
        java_vm: Arc::new(java_vm),
        input_class,
        status_class,
    };
    let config = SessionConnectConfig {
        address: address.clone(),
        device_name,
        storage_dir,
        pairing_code,
    };

    cancel_active_reconnect_and_connection();
    match connect_session(&config, &callbacks, None) {
        Ok(()) => {
            notify_native_status_changed_with_class(&mut env, &callbacks.status_class);
            JNI_TRUE
        }
        Err(error) => {
            set_last_error(error);
            notify_native_status_changed_with_class(&mut env, &callbacks.status_class);
            JNI_FALSE
        }
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_androidconnect_NativeBridge_nativeDisconnect(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
) {
    let changed = if let Ok(mut guard) = state().lock() {
        guard.reset_active_connection();
        guard.pairing_code_set = false;
        guard.reconnect_generation += 1;
        guard.reconnect_config = None;
        guard.reconnect_callbacks = None;
        guard.reconnecting = false;
        guard.reconnect_attempts = 0;
        guard.last_reconnect_error = None;
        guard.last_error = None;
        guard.mark_status_changed();
        true
    } else {
        false
    };
    if changed {
        notify_native_status_changed(&mut env);
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_androidconnect_NativeBridge_nativePushVideoFormat(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
    width: jint,
    height: jint,
    dpi: jint,
    frame_rate: jint,
    rotation_degrees: jint,
) -> jboolean {
    let mut guard = match state().lock() {
        Ok(guard) => guard,
        Err(_) => return JNI_FALSE,
    };
    if guard.capture.is_none() {
        return JNI_FALSE;
    }

    let format = VideoFormat {
        stream_id: 0,
        codec: VideoCodec::H264,
        width: width.max(0) as u32,
        height: height.max(0) as u32,
        rotation_degrees: rotation_degrees.max(0) as u16,
        dpi: dpi.max(0) as u32,
        frame_rate: frame_rate.max(0) as u32,
    };
    guard.last_video_format = Some(format.clone());

    if !guard.is_connected() {
        return JNI_FALSE;
    }

    match guard.send_payload(Payload::VideoFormat(format)) {
        Ok(()) => JNI_TRUE,
        Err(error) => {
            let error = format!("video format send failed: {error}");
            guard.clear_connection(error.clone());
            drop(guard);
            start_reconnect_worker_if_needed(error);
            notify_native_status_changed(&mut env);
            JNI_FALSE
        }
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_androidconnect_NativeBridge_nativePushVideoFrame(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
    frame: JByteArray<'_>,
    presentation_time_us: jlong,
    media_codec_flags: jint,
) -> jboolean {
    let frame = match env.convert_byte_array(frame) {
        Ok(frame) => frame,
        Err(_) => return JNI_FALSE,
    };

    let mut guard = match state().lock() {
        Ok(guard) => guard,
        Err(_) => return JNI_FALSE,
    };
    let Some(capture) = guard.capture.as_mut() else {
        return JNI_FALSE;
    };

    capture.encoded_frames += 1;
    capture.encoded_bytes += frame.len() as u64;

    if !guard.is_connected() {
        return JNI_FALSE;
    }

    let payload = Payload::VideoFrame(VideoFrame {
        stream_id: 0,
        presentation_time_us,
        flags: video_flags_from_android_media_codec(media_codec_flags),
        data: frame,
    });

    match guard.send_payload(payload) {
        Ok(()) => JNI_TRUE,
        Err(error) => {
            let error = format!("video frame send failed: {error}");
            guard.clear_connection(error.clone());
            drop(guard);
            start_reconnect_worker_if_needed(error);
            notify_native_status_changed(&mut env);
            JNI_FALSE
        }
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_androidconnect_NativeBridge_nativeStatsJson(
    env: JNIEnv<'_>,
    _class: JClass<'_>,
) -> jstring {
    let stats = current_stats_json();
    match env.new_string(stats) {
        Ok(value) => value.into_raw(),
        Err(_) => std::ptr::null_mut(),
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_androidconnect_NativeBridge_nativePushDeviceStatus(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
    status_json: JString<'_>,
) -> jboolean {
    let status_json = match jstring_to_string(&mut env, status_json, "device status") {
        Ok(value) => value,
        Err(error) => {
            set_last_error(error);
            notify_native_status_changed(&mut env);
            return JNI_FALSE;
        }
    };

    match push_device_status_json(&status_json) {
        Ok(()) => JNI_TRUE,
        Err(error) => {
            set_last_error(error);
            notify_native_status_changed(&mut env);
            JNI_FALSE
        }
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_androidconnect_NativeBridge_nativePushMediaStatus(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
    status_json: JString<'_>,
) -> jboolean {
    push_json_payload::<androidconnect_protocol::MediaStatus>(
        &mut env,
        status_json,
        "media status",
        Payload::MediaStatus,
    )
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_androidconnect_NativeBridge_nativePushStorageStatus(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
    status_json: JString<'_>,
) -> jboolean {
    push_json_payload::<androidconnect_protocol::StorageStatus>(
        &mut env,
        status_json,
        "storage status",
        Payload::StorageStatus,
    )
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_androidconnect_NativeBridge_nativePushNotificationPosted(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
    notification_json: JString<'_>,
) -> jboolean {
    push_json_payload::<androidconnect_protocol::NotificationPosted>(
        &mut env,
        notification_json,
        "notification",
        Payload::NotificationPosted,
    )
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_androidconnect_NativeBridge_nativePushNotificationRemoved(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
    notification_id: JString<'_>,
) -> jboolean {
    let notification_id = match jstring_to_string(&mut env, notification_id, "notification id") {
        Ok(value) => value,
        Err(error) => {
            set_last_error(error);
            notify_native_status_changed(&mut env);
            return JNI_FALSE;
        }
    };
    send_utility_payload(Payload::NotificationRemoved(
        androidconnect_protocol::NotificationRemoved { notification_id },
    ))
    .map(|()| JNI_TRUE)
    .unwrap_or_else(|error| {
        set_last_error(error);
        notify_native_status_changed(&mut env);
        JNI_FALSE
    })
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_androidconnect_NativeBridge_nativePushClipboardText(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
    text: JString<'_>,
) -> jboolean {
    let text = match jstring_to_string(&mut env, text, "clipboard text") {
        Ok(value) => value,
        Err(error) => {
            set_last_error(error);
            notify_native_status_changed(&mut env);
            return JNI_FALSE;
        }
    };

    match push_android_clipboard_text(text) {
        Ok(()) => JNI_TRUE,
        Err(error) => {
            set_last_error(error);
            notify_native_status_changed(&mut env);
            JNI_FALSE
        }
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_androidconnect_NativeBridge_nativePushSharedFile(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
    file_name: JString<'_>,
    mime_type: JString<'_>,
    path: JString<'_>,
    size_bytes: jlong,
) -> jboolean {
    let file_name = match jstring_to_string(&mut env, file_name, "shared file name") {
        Ok(value) => value,
        Err(error) => {
            set_last_error(error);
            notify_native_status_changed(&mut env);
            return JNI_FALSE;
        }
    };
    let mime_type = match jstring_to_string(&mut env, mime_type, "shared mime type") {
        Ok(value) => value,
        Err(error) => {
            set_last_error(error);
            notify_native_status_changed(&mut env);
            return JNI_FALSE;
        }
    };
    let path = match jstring_to_string(&mut env, path, "shared file path") {
        Ok(value) => value,
        Err(error) => {
            set_last_error(error);
            notify_native_status_changed(&mut env);
            return JNI_FALSE;
        }
    };

    match push_shared_file(file_name, mime_type, PathBuf::from(path), size_bytes, None) {
        Ok(()) => JNI_TRUE,
        Err(error) => {
            set_last_error(error);
            notify_native_status_changed(&mut env);
            JNI_FALSE
        }
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_androidconnect_NativeBridge_nativePushSharedFileForRequest(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
    file_name: JString<'_>,
    mime_type: JString<'_>,
    path: JString<'_>,
    size_bytes: jlong,
    requested_path: JString<'_>,
) -> jboolean {
    let file_name = match jstring_to_string(&mut env, file_name, "shared file name") {
        Ok(value) => value,
        Err(error) => {
            set_last_error(error);
            notify_native_status_changed(&mut env);
            return JNI_FALSE;
        }
    };
    let mime_type = match jstring_to_string(&mut env, mime_type, "shared mime type") {
        Ok(value) => value,
        Err(error) => {
            set_last_error(error);
            notify_native_status_changed(&mut env);
            return JNI_FALSE;
        }
    };
    let path = match jstring_to_string(&mut env, path, "shared file path") {
        Ok(value) => value,
        Err(error) => {
            set_last_error(error);
            notify_native_status_changed(&mut env);
            return JNI_FALSE;
        }
    };
    let requested_path = match jstring_to_string(&mut env, requested_path, "requested file path") {
        Ok(value) => value,
        Err(error) => {
            set_last_error(error);
            notify_native_status_changed(&mut env);
            return JNI_FALSE;
        }
    };

    match push_shared_file(
        file_name,
        mime_type,
        PathBuf::from(path),
        size_bytes,
        Some(requested_path),
    ) {
        Ok(()) => JNI_TRUE,
        Err(error) => {
            set_last_error(error);
            notify_native_status_changed(&mut env);
            JNI_FALSE
        }
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_androidconnect_NativeBridge_nativePushPhotoAssetList(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
    list_json: JString<'_>,
) -> jboolean {
    push_json_payload::<androidconnect_protocol::PhotoAssetList>(
        &mut env,
        list_json,
        "photo asset list",
        Payload::PhotoAssetList,
    )
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_androidconnect_NativeBridge_nativePushMessageThreadList(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
    list_json: JString<'_>,
) -> jboolean {
    push_json_payload::<androidconnect_protocol::MessageThreadList>(
        &mut env,
        list_json,
        "message thread list",
        Payload::MessageThreadList,
    )
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_androidconnect_NativeBridge_nativePushMessageThreadDetail(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
    detail_json: JString<'_>,
) -> jboolean {
    push_json_payload::<androidconnect_protocol::MessageThreadDetail>(
        &mut env,
        detail_json,
        "message thread detail",
        Payload::MessageThreadDetail,
    )
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_androidconnect_NativeBridge_nativePushMessageEvent(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
    event_json: JString<'_>,
) -> jboolean {
    push_json_payload::<androidconnect_protocol::MessageEvent>(
        &mut env,
        event_json,
        "message event",
        Payload::MessageEvent,
    )
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_androidconnect_NativeBridge_nativePushMessageSendResponse(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
    response_json: JString<'_>,
) -> jboolean {
    push_json_payload::<androidconnect_protocol::MessageSendResponse>(
        &mut env,
        response_json,
        "message send response",
        Payload::MessageSendResponse,
    )
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_androidconnect_NativeBridge_nativePushCallState(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
    state_json: JString<'_>,
) -> jboolean {
    push_json_payload::<androidconnect_protocol::CallState>(
        &mut env,
        state_json,
        "call state",
        Payload::CallState,
    )
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_androidconnect_NativeBridge_nativePushRelayStatus(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
    status_json: JString<'_>,
) -> jboolean {
    push_json_payload::<androidconnect_protocol::RelayStatus>(
        &mut env,
        status_json,
        "relay status",
        Payload::RelayStatus,
    )
}

pub fn current_stats_json() -> String {
    let Ok(guard) = state().lock() else {
        return r#"{"running":false,"error":"lock_poisoned"}"#.to_owned();
    };
    let capture = guard.capture.as_ref();

    serde_json::json!({
        "running": capture.is_some(),
        "connected": guard.stream.is_some(),
        "connected_to": guard.connected_to,
        "encoded_frames": capture.map(|capture| capture.encoded_frames).unwrap_or_default(),
        "encoded_bytes": capture.map(|capture| capture.encoded_bytes).unwrap_or_default(),
        "sent_envelopes": guard.sent_envelopes,
        "sent_bytes": guard.sent_bytes,
        "sent_utility_envelopes": guard.sent_utility_envelopes,
        "received_pings": guard.received_pings,
        "sent_pongs": guard.sent_pongs,
        "pairing_code_set": guard.pairing_code_set,
        "input_authenticated": guard.input_authenticated,
        "paired_desktop_id": guard.paired_desktop_id.clone(),
        "paired_desktop_name": guard.paired_desktop_name.clone(),
        "session_key_fingerprint": guard.session_key_fingerprint.map(|fingerprint| bytes_to_hex(&fingerprint)),
        "trusted_desktop_count": guard.trusted_desktop_count,
        "android_device_id": guard.android_device_id.clone(),
        "incoming_transfer_count": guard.incoming_transfers.len(),
        "reconnecting": guard.reconnecting,
        "reconnect_attempts": guard.reconnect_attempts,
        "last_reconnect_error": guard.last_reconnect_error.clone(),
        "status_revision": guard.status_revision,
        "uptime_ms": capture.map(|capture| capture.started_at.elapsed().as_millis()).unwrap_or_default(),
        "last_error": guard.last_error,
    })
    .to_string()
}

#[derive(Debug, Deserialize)]
struct DeviceStatusUpdate {
    device_name: String,
    manufacturer: String,
    model: String,
    android_sdk: u32,
    battery_percent: Option<u8>,
    charging: Option<bool>,
    interactive: Option<bool>,
    features: Vec<FeatureStatus>,
    #[serde(default)]
    wifi_state: Option<androidconnect_protocol::WifiState>,
    #[serde(default)]
    bluetooth_state: Option<androidconnect_protocol::BluetoothState>,
    #[serde(default)]
    dnd_state: Option<androidconnect_protocol::DndState>,
    #[serde(default)]
    volume: Option<androidconnect_protocol::VolumeState>,
}

fn push_json_payload<T>(
    env: &mut JNIEnv<'_>,
    json: JString<'_>,
    label: &str,
    wrap: impl FnOnce(T) -> Payload,
) -> jboolean
where
    T: for<'de> Deserialize<'de>,
{
    let json = match jstring_to_string(env, json, label) {
        Ok(value) => value,
        Err(error) => {
            set_last_error(error);
            notify_native_status_changed(env);
            return JNI_FALSE;
        }
    };
    let payload = match serde_json::from_str::<T>(&json) {
        Ok(value) => wrap(value),
        Err(error) => {
            set_last_error(format!("parse {label} JSON failed: {error}"));
            notify_native_status_changed(env);
            return JNI_FALSE;
        }
    };
    match send_utility_payload(payload) {
        Ok(()) => JNI_TRUE,
        Err(error) => {
            set_last_error(error);
            notify_native_status_changed(env);
            JNI_FALSE
        }
    }
}

fn push_device_status_json(json: &str) -> Result<(), String> {
    let update: DeviceStatusUpdate = serde_json::from_str(json)
        .map_err(|error| format!("parse device status failed: {error}"))?;
    let mut guard = state()
        .lock()
        .map_err(|_| "state lock poisoned while sending device status".to_owned())?;
    let device_id = guard
        .android_device_id
        .clone()
        .unwrap_or_else(|| "android-unpaired".to_owned());
    guard.send_utility_payload(Payload::DeviceStatus(DeviceStatus {
        device_id,
        device_name: update.device_name,
        manufacturer: update.manufacturer,
        model: update.model,
        android_sdk: update.android_sdk,
        battery_percent: update.battery_percent,
        charging: update.charging,
        interactive: update.interactive,
        features: update.features,
        wifi_state: update.wifi_state,
        bluetooth_state: update.bluetooth_state,
        dnd_state: update.dnd_state,
        volume: update.volume,
    }))
}

fn push_android_clipboard_text(text: String) -> Result<(), String> {
    let mut guard = state()
        .lock()
        .map_err(|_| "state lock poisoned while sending clipboard text".to_owned())?;
    let sequence = guard.clipboard_sequence;
    guard.clipboard_sequence = guard.clipboard_sequence.wrapping_add(1);
    guard.send_utility_payload(Payload::ClipboardText(ClipboardText {
        sequence,
        text,
        source: ClipboardSource::Android,
    }))
}

fn push_shared_file(
    file_name: String,
    mime_type: String,
    path: PathBuf,
    size_bytes: jlong,
    target_path: Option<String>,
) -> Result<(), String> {
    let mut file = File::open(&path)
        .map_err(|error| format!("open shared file {} failed: {error}", path.display()))?;
    let size = if size_bytes >= 0 {
        Some(size_bytes as u64)
    } else {
        file.metadata().ok().map(|metadata| metadata.len())
    };
    let transfer_id = format!("android-share-{}-{}", now_unix_ms(), generate_hex_id(4));
    send_utility_payload(Payload::FileTransferStart(FileTransferStart {
        transfer_id: transfer_id.clone(),
        direction: TransferDirection::AndroidToDesktop,
        file_name: sanitize_file_name(&file_name),
        mime_type: empty_to_none(mime_type),
        size_bytes: size,
        target_path,
    }))?;

    let mut offset = 0_u64;
    let mut buffer = vec![0_u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|error| format!("read shared file {} failed: {error}", path.display()))?;
        if read == 0 {
            break;
        }
        send_utility_payload(Payload::FileTransferChunk(FileTransferChunk {
            transfer_id: transfer_id.clone(),
            offset,
            data: buffer[..read].to_vec(),
        }))?;
        offset += read as u64;
    }

    send_utility_payload(Payload::FileTransferComplete(FileTransferComplete {
        transfer_id,
        status: TransferStatus::Completed,
        message: None,
    }))
}

fn send_utility_payload(payload: Payload) -> Result<(), String> {
    let mut guard = state()
        .lock()
        .map_err(|_| "state lock poisoned while sending utility payload".to_owned())?;
    guard.send_utility_payload(payload)
}

fn jstring_to_string(
    env: &mut JNIEnv<'_>,
    value: JString<'_>,
    label: &str,
) -> Result<String, String> {
    env.get_string(&value)
        .map(|value| value.to_string_lossy().into_owned())
        .map_err(|error| format!("invalid {label} string: {error}"))
}

fn connect_session(
    config: &SessionConnectConfig,
    callbacks: &SessionCallbacks,
    reconnect_generation: Option<u64>,
) -> Result<(), String> {
    if let Some(token) = reconnect_generation
        && !is_reconnect_generation_active(token)
    {
        return Err("stale reconnect generation".to_owned());
    }

    let trust_store = AndroidTrustStore::load_or_create(&config.storage_dir)
        .map_err(|error| format!("trust store unavailable: {error}"))?;
    let android_device_id = trust_store.device_id().to_owned();
    let trusted_desktop_count = trust_store.trusted_desktop_count();
    if config.pairing_code.is_empty() && trusted_desktop_count == 0 {
        return Err("pairing code required for first desktop".to_owned());
    }

    let stream = TcpStream::connect(&config.address)
        .map_err(|error| format!("connect to {} failed: {error}", config.address))?;
    configure_stream(&stream)
        .map_err(|error| format!("configure {} failed: {error}", config.address))?;
    let input_stream = stream
        .try_clone()
        .map_err(|error| format!("clone {} stream failed: {error}", config.address))?;
    let auth_challenge = make_auth_challenge();

    let mut guard = state()
        .lock()
        .map_err(|_| "state lock poisoned while connecting".to_owned())?;
    if let Some(token) = reconnect_generation
        && guard.reconnect_generation != token
    {
        close_stream(Some(stream));
        return Err("stale reconnect generation".to_owned());
    }

    guard.connection_generation += 1;
    guard.stream = Some(stream);
    guard.connected_to = Some(config.address.clone());
    guard.pairing_code_set = !config.pairing_code.is_empty();
    guard.input_authenticated = false;
    guard.paired_desktop_id = None;
    guard.paired_desktop_name = None;
    guard.session_key_fingerprint = None;
    guard.trusted_desktop_count = trusted_desktop_count;
    guard.android_device_id = Some(android_device_id.clone());
    guard.reconnect_config = Some(config.clone());
    guard.reconnect_callbacks = Some(callbacks.clone());
    guard.received_pings = 0;
    guard.sent_pongs = 0;
    guard.last_error = None;
    guard.mark_status_changed();
    let generation = guard.connection_generation;

    let hello = Payload::Hello(DeviceHello::android(
        android_device_id.clone(),
        config.device_name.clone(),
    ));
    let send_result = guard
        .send_payload(hello)
        .and_then(|()| guard.send_payload(Payload::AuthChallenge(auth_challenge.clone())));
    match send_result {
        Ok(()) => {
            if let Some(format) = guard.last_video_format.clone()
                && let Err(error) = guard.send_payload(Payload::VideoFormat(format))
            {
                guard
                    .clear_connection(format!("format send to {} failed: {error}", config.address));
                return Err(error);
            }
        }
        Err(error) => {
            guard.clear_connection(format!("hello send to {} failed: {error}", config.address));
            return Err(error);
        }
    }
    guard.reconnecting = false;
    guard.reconnect_attempts = 0;
    guard.last_reconnect_error = None;
    guard.mark_status_changed();
    drop(guard);

    spawn_input_reader(
        callbacks.clone(),
        input_stream,
        generation,
        config.address.clone(),
        AuthContext {
            trust_store,
            android_device_id,
            pairing_code: config.pairing_code.clone(),
            challenge: auth_challenge.challenge,
        },
    );
    Ok(())
}

fn spawn_input_reader(
    callbacks: SessionCallbacks,
    stream: TcpStream,
    generation: u64,
    address: String,
    auth_context: AuthContext,
) {
    let thread_address = address.clone();
    let callbacks_for_error = callbacks.clone();
    let spawn_result = thread::Builder::new()
        .name("AndroidConnectInputReader".to_owned())
        .spawn(move || {
            let result = input_reader_loop(&callbacks, stream, generation, auth_context);
            if let Err(error) = result {
                let error = format!("input reader from {thread_address} stopped: {error}");
                let changed = clear_connection_if_generation(generation, error.clone());
                let reconnect_started = if changed && should_reconnect_after_error(&error) {
                    start_reconnect_worker_if_needed(error)
                } else {
                    false
                };
                if changed {
                    notify_native_status_changed_from_vm(
                        &callbacks.java_vm,
                        &callbacks.status_class,
                    );
                }
                if reconnect_started {
                    notify_native_status_changed_from_vm(
                        &callbacks.java_vm,
                        &callbacks.status_class,
                    );
                }
            }
        });

    if let Err(error) = spawn_result {
        let error = format!("input reader for {address} failed to start: {error}");
        if clear_connection_if_generation(generation, error.clone()) {
            start_reconnect_worker_if_needed(error);
            notify_native_status_changed_from_vm(
                &callbacks_for_error.java_vm,
                &callbacks_for_error.status_class,
            );
        }
    }
}

fn input_reader_loop(
    callbacks: &SessionCallbacks,
    mut stream: TcpStream,
    generation: u64,
    mut auth_context: AuthContext,
) -> Result<(), String> {
    let mut env = callbacks
        .java_vm
        .attach_current_thread()
        .map_err(|error| format!("attach JNI input thread failed: {error}"))?;
    let mut dispatcher = InputDispatcher::new(callbacks.input_class.clone());
    let mut desktop_authenticated = false;

    loop {
        let envelope = read_length_prefixed(&mut stream, MAX_CONTROL_FRAME_BYTES)
            .map_err(|error| error.to_string())?;

        if envelope.version != PROTOCOL_VERSION {
            return Err(format!(
                "unsupported protocol version {}, expected {PROTOCOL_VERSION}",
                envelope.version
            ));
        }

        match envelope.payload {
            Payload::Input(event) => {
                if desktop_authenticated {
                    dispatcher.dispatch(&mut env, event);
                } else {
                    set_last_error("input ignored before pairing authentication");
                }
            }
            Payload::AuthChallenge(_) => {}
            Payload::AuthResponse(response) => {
                let auth_result = authenticate_desktop(&mut auth_context, &response)?;
                let status_changed = set_authenticated_desktop_if_generation(
                    generation,
                    auth_result.accepted,
                    auth_result.desktop_id.clone(),
                    auth_result.desktop_name.clone(),
                    auth_result.session_key_fingerprint,
                    auth_context.trust_store.trusted_desktop_count(),
                );
                send_payload_if_generation(
                    generation,
                    Payload::AuthResult(AuthResult {
                        accepted: auth_result.accepted,
                        message: auth_result.message.clone(),
                        trusted: auth_result.trusted,
                        desktop_id: auth_result.desktop_id.clone(),
                        desktop_name: auth_result.desktop_name.clone(),
                        session_key_fingerprint: auth_result.session_key_fingerprint,
                    }),
                )?;
                if status_changed {
                    notify_native_status_changed_with_class(&mut env, &callbacks.status_class);
                }
                if !auth_result.accepted {
                    return Err(auth_result.message);
                }
                desktop_authenticated = true;
            }
            Payload::AuthResult(result) => {
                if !result.accepted {
                    return Err(format!("desktop rejected pairing: {}", result.message));
                }
            }
            Payload::Ping { nonce } => {
                send_pong_if_generation(generation, nonce)?;
            }
            Payload::Pong { .. }
            | Payload::Error { .. }
            | Payload::Hello(_)
            | Payload::VideoFormat(_)
            | Payload::VideoFrame(_) => {}
            payload => {
                if is_desktop_utility_payload(&payload) {
                    if desktop_authenticated {
                        handle_desktop_utility_payload(
                            &mut env,
                            &callbacks.status_class,
                            generation,
                            payload,
                        );
                    } else {
                        set_last_error("utility command ignored before pairing authentication");
                    }
                }
            }
        }
    }
}

fn is_desktop_utility_payload(payload: &Payload) -> bool {
    matches!(
        payload,
        Payload::MediaControl(_)
            | Payload::ClipboardText(_)
            | Payload::ClipboardImage(_)
            | Payload::FileTransferStart(_)
            | Payload::FileTransferChunk(_)
            | Payload::FileTransferComplete(_)
            | Payload::FileBrowseRequest(_)
            | Payload::FileMutation(_)
            | Payload::NotificationAction(_)
            | Payload::NotificationFilterUpdate(_)
            | Payload::AudioControl(_)
            | Payload::AppWindowOpen(_)
            | Payload::AppWindowClose(_)
            | Payload::AppWindowInput(_)
            | Payload::MessageSendRequest(_)
            | Payload::MessageThreadOpen(_)
            | Payload::FileTransferRequest(_)
            | Payload::CallAction(_)
            | Payload::PhotoAssetTransfer(_)
            | Payload::RelayOffer(_)
            | Payload::ClientRoleUpdate(_)
    )
}

fn handle_desktop_utility_payload(
    env: &mut JNIEnv<'_>,
    bridge_class: &GlobalRef,
    generation: u64,
    payload: Payload,
) {
    let result = match payload {
        Payload::MediaControl(control) => call_static_void_string(
            env,
            bridge_class,
            "onMediaControl",
            media_control_action_name(control.action),
        ),
        Payload::ClipboardText(clipboard) => call_static_void_string(
            env,
            bridge_class,
            "onClipboardTextFromDesktop",
            clipboard.text,
        ),
        Payload::FileTransferStart(start) => handle_file_transfer_start(start),
        Payload::FileTransferChunk(chunk) => handle_file_transfer_chunk(chunk),
        Payload::FileTransferComplete(complete) => handle_file_transfer_complete(complete),
        Payload::FileBrowseRequest(request) => handle_file_browse_request(generation, request),
        Payload::FileMutation(mutation) => handle_file_mutation(generation, mutation),
        Payload::AudioControl(control) => call_static_void_string(
            env,
            bridge_class,
            "onAudioControl",
            format!("{:?}", control.command),
        ),
        Payload::AppWindowOpen(open) => call_static_void_strings(
            env,
            bridge_class,
            "onAppWindowOpen",
            &open.package_name,
            open.activity_name.as_deref().unwrap_or(""),
        ),
        Payload::CallAction(action) => call_static_void_strings(
            env,
            bridge_class,
            "onCallAction",
            &format!("{:?}", action.action),
            action.phone_number.as_deref().unwrap_or(""),
        ),
        Payload::MessageSendRequest(request) => call_static_void_string(
            env,
            bridge_class,
            "onMessageSendRequest",
            serde_json::to_string(&request).unwrap_or_default(),
        ),
        Payload::MessageThreadOpen(open) => call_static_void_string(
            env,
            bridge_class,
            "onMessageThreadOpen",
            serde_json::to_string(&open).unwrap_or_default(),
        ),
        Payload::FileTransferRequest(request) => call_static_void_string(
            env,
            bridge_class,
            "onFileTransferRequest",
            serde_json::to_string(&request).unwrap_or_default(),
        ),
        Payload::NotificationAction(action) => call_static_void_string(
            env,
            bridge_class,
            "onNotificationAction",
            serde_json::to_string(&action).unwrap_or_default(),
        ),
        Payload::NotificationFilterUpdate(update) => call_static_void_string(
            env,
            bridge_class,
            "onNotificationFilterUpdate",
            serde_json::to_string(&update).unwrap_or_default(),
        ),
        Payload::PhotoAssetTransfer(transfer) => call_static_void_string(
            env,
            bridge_class,
            "onPhotoAssetTransfer",
            serde_json::to_string(&transfer).unwrap_or_default(),
        ),
        Payload::RelayOffer(offer) => call_static_void_string(
            env,
            bridge_class,
            "onRelayOffer",
            serde_json::to_string(&offer).unwrap_or_default(),
        ),
        Payload::ClientRoleUpdate(update) => call_static_void_string(
            env,
            bridge_class,
            "onClientRoleUpdate",
            serde_json::to_string(&update).unwrap_or_default(),
        ),
        _ => Ok(()),
    };

    if let Err(error) = result {
        set_last_error(error);
    }
}

fn media_control_action_name(action: MediaControlAction) -> &'static str {
    match action {
        MediaControlAction::Play => "Play",
        MediaControlAction::Pause => "Pause",
        MediaControlAction::PlayPause => "PlayPause",
        MediaControlAction::Previous => "Previous",
        MediaControlAction::Next => "Next",
        MediaControlAction::Stop => "Stop",
    }
}

fn handle_file_transfer_start(start: FileTransferStart) -> Result<(), String> {
    if start.direction != TransferDirection::DesktopToAndroid {
        return Err("Android can only receive DesktopToAndroid file transfers".to_owned());
    }
    let storage_dir = active_storage_dir()?;
    let received_dir = storage_dir.join("received");
    fs::create_dir_all(&received_dir)
        .map_err(|error| format!("create {} failed: {error}", received_dir.display()))?;
    let path = unique_child_path(&received_dir, &sanitize_file_name(&start.file_name));
    File::create(&path).map_err(|error| format!("create {} failed: {error}", path.display()))?;

    let mut guard = state()
        .lock()
        .map_err(|_| "state lock poisoned while starting file transfer".to_owned())?;
    guard.incoming_transfers.insert(
        start.transfer_id,
        IncomingTransfer {
            path,
            next_offset: 0,
        },
    );
    guard.mark_status_changed();
    Ok(())
}

fn handle_file_transfer_chunk(chunk: FileTransferChunk) -> Result<(), String> {
    let (path, expected_offset) = {
        let guard = state()
            .lock()
            .map_err(|_| "state lock poisoned while writing file transfer".to_owned())?;
        let transfer = guard
            .incoming_transfers
            .get(&chunk.transfer_id)
            .ok_or_else(|| format!("unknown transfer {}", chunk.transfer_id))?;
        (transfer.path.clone(), transfer.next_offset)
    };
    if chunk.offset != expected_offset {
        return Err(format!(
            "transfer {} offset mismatch: got {}, expected {}",
            chunk.transfer_id, chunk.offset, expected_offset
        ));
    }

    let mut file = OpenOptions::new()
        .write(true)
        .open(&path)
        .map_err(|error| format!("open {} failed: {error}", path.display()))?;
    file.seek(SeekFrom::Start(chunk.offset))
        .map_err(|error| format!("seek {} failed: {error}", path.display()))?;
    file.write_all(&chunk.data)
        .map_err(|error| format!("write {} failed: {error}", path.display()))?;

    let mut guard = state()
        .lock()
        .map_err(|_| "state lock poisoned while updating file transfer".to_owned())?;
    if let Some(transfer) = guard.incoming_transfers.get_mut(&chunk.transfer_id) {
        transfer.next_offset = chunk.offset + chunk.data.len() as u64;
    }
    guard.mark_status_changed();
    Ok(())
}

fn handle_file_transfer_complete(complete: FileTransferComplete) -> Result<(), String> {
    let removed = {
        let mut guard = state()
            .lock()
            .map_err(|_| "state lock poisoned while completing file transfer".to_owned())?;
        let transfer = guard.incoming_transfers.remove(&complete.transfer_id);
        guard.mark_status_changed();
        transfer
    };
    if complete.status != TransferStatus::Completed
        && let Some(transfer) = removed
    {
        let _ = fs::remove_file(transfer.path);
    }
    Ok(())
}

fn handle_file_browse_request(generation: u64, request: FileBrowseRequest) -> Result<(), String> {
    let root = active_storage_dir()?;
    let path = match resolve_storage_path(&root, &request.path) {
        Ok(path) => path,
        Err(error) => {
            return send_file_browse_error(generation, request.request_id, request.path, error);
        }
    };
    let mut entries = Vec::new();
    let read_dir = match fs::read_dir(&path) {
        Ok(read_dir) => read_dir,
        Err(error) => {
            return send_file_browse_error(
                generation,
                request.request_id,
                request.path,
                format!("read {} failed: {error}", path.display()),
            );
        }
    };
    for entry in read_dir {
        let entry = entry.map_err(|error| format!("read directory entry failed: {error}"))?;
        let metadata = entry
            .metadata()
            .map_err(|error| format!("metadata {} failed: {error}", entry.path().display()))?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let child_path = relative_storage_path(&root, &entry.path());
        entries.push(FileEntry {
            name,
            path: child_path,
            entry_type: if metadata.is_dir() {
                FileEntryType::Directory
            } else {
                FileEntryType::File
            },
            size_bytes: if metadata.is_file() {
                Some(metadata.len())
            } else {
                None
            },
            modified_unix_ms: metadata
                .modified()
                .ok()
                .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
                .map(|duration| duration.as_millis() as u64),
            mime_type: None,
        });
    }
    entries.sort_by(|left, right| {
        file_entry_type_rank(left.entry_type)
            .cmp(&file_entry_type_rank(right.entry_type))
            .then_with(|| left.name.cmp(&right.name))
    });
    send_payload_if_generation(
        generation,
        Payload::FileBrowseResponse(FileBrowseResponse {
            request_id: request.request_id,
            path: relative_storage_path(&root, &path),
            entries,
            status: FeatureStatus::available(UtilityFeature::FileBrowser),
        }),
    )
}

fn send_file_browse_error(
    generation: u64,
    request_id: String,
    path: String,
    message: impl Into<String>,
) -> Result<(), String> {
    send_payload_if_generation(
        generation,
        Payload::FileBrowseResponse(FileBrowseResponse {
            request_id,
            path,
            entries: Vec::new(),
            status: FeatureStatus {
                feature: UtilityFeature::FileBrowser,
                state: FeatureState::Error,
                message: message.into(),
            },
        }),
    )
}

fn handle_file_mutation(generation: u64, mutation: FileMutation) -> Result<(), String> {
    let root = active_storage_dir()?;
    let path = resolve_storage_path(&root, &mutation.path)?;
    match mutation.mutation {
        FileMutationKind::CreateFolder => {
            fs::create_dir_all(&path)
                .map_err(|error| format!("create {} failed: {error}", path.display()))?;
        }
        FileMutationKind::Delete => {
            if path.is_dir() {
                fs::remove_dir_all(&path)
            } else {
                fs::remove_file(&path)
            }
            .map_err(|error| format!("delete {} failed: {error}", path.display()))?;
        }
        FileMutationKind::Rename => {
            let new_path = mutation
                .new_path
                .as_deref()
                .ok_or_else(|| "rename requires new_path".to_owned())
                .and_then(|new_path| resolve_storage_path(&root, new_path))?;
            fs::rename(&path, &new_path).map_err(|error| {
                format!(
                    "rename {} to {} failed: {error}",
                    path.display(),
                    new_path.display()
                )
            })?;
        }
    }
    let parent = path
        .parent()
        .map(|parent| relative_storage_path(&root, parent))
        .unwrap_or_default();
    handle_file_browse_request(
        generation,
        FileBrowseRequest {
            request_id: mutation.request_id,
            path: parent,
            include_thumbnails: false,
        },
    )
}

fn active_storage_dir() -> Result<PathBuf, String> {
    state()
        .lock()
        .map_err(|_| "state lock poisoned while reading storage dir".to_owned())?
        .reconnect_config
        .as_ref()
        .map(|config| config.storage_dir.clone())
        .ok_or_else(|| "storage dir unavailable until connected".to_owned())
}

fn file_entry_type_rank(entry_type: FileEntryType) -> u8 {
    match entry_type {
        FileEntryType::Directory => 0,
        FileEntryType::Media => 1,
        FileEntryType::File => 2,
    }
}

fn resolve_storage_path(root: &Path, path: &str) -> Result<PathBuf, String> {
    let mut output = root.to_path_buf();
    for component in Path::new(path).components() {
        match component {
            std::path::Component::Normal(part) => output.push(part),
            std::path::Component::CurDir => {}
            std::path::Component::RootDir => {}
            _ => return Err("path escapes AndroidConnect storage".to_owned()),
        }
    }
    Ok(output)
}

fn relative_storage_path(root: &Path, path: &Path) -> String {
    let relative = path
        .strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/");
    if relative.is_empty() {
        "/".to_owned()
    } else {
        relative
    }
}

fn unique_child_path(parent: &Path, file_name: &str) -> PathBuf {
    let mut candidate = parent.join(file_name);
    if !candidate.exists() {
        return candidate;
    }
    let path = Path::new(file_name);
    let stem = path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("file");
    let extension = path.extension().and_then(|value| value.to_str());
    for index in 1..10_000 {
        let name = if let Some(extension) = extension {
            format!("{stem}-{index}.{extension}")
        } else {
            format!("{stem}-{index}")
        };
        candidate = parent.join(name);
        if !candidate.exists() {
            return candidate;
        }
    }
    parent.join(format!("{}-{}", now_unix_ms(), file_name))
}

fn sanitize_file_name(value: &str) -> String {
    let clean: String = value
        .chars()
        .map(|ch| match ch {
            '/' | '\\' | ':' | '\0' => '_',
            ch if ch.is_control() => '_',
            ch => ch,
        })
        .collect();
    let clean = clean.trim_matches('.').trim();
    if clean.is_empty() {
        "file".to_owned()
    } else {
        clean.to_owned()
    }
}

fn empty_to_none(value: String) -> Option<String> {
    if value.trim().is_empty() {
        None
    } else {
        Some(value)
    }
}

fn cancel_active_reconnect_and_connection() {
    if let Ok(mut guard) = state().lock() {
        guard.reconnect_generation += 1;
        guard.reconnect_config = None;
        guard.reconnect_callbacks = None;
        guard.reconnecting = false;
        guard.reconnect_attempts = 0;
        guard.last_reconnect_error = None;
        guard.reset_active_connection();
        guard.last_error = None;
        guard.mark_status_changed();
    }
}

fn is_reconnect_generation_active(reconnect_generation: u64) -> bool {
    state()
        .lock()
        .map(|guard| {
            guard.reconnect_generation == reconnect_generation
                && guard.reconnecting
                && guard.reconnect_config.is_some()
        })
        .unwrap_or(false)
}

fn start_reconnect_worker_if_needed(reason: String) -> bool {
    let request = {
        let Ok(mut guard) = state().lock() else {
            return false;
        };
        if guard.stream.is_some() || guard.reconnecting {
            return false;
        }
        let Some(config) = guard.reconnect_config.clone() else {
            return false;
        };
        let Some(callbacks) = guard.reconnect_callbacks.clone() else {
            return false;
        };
        guard.reconnecting = true;
        guard.reconnect_attempts = 0;
        guard.last_reconnect_error = Some(reason);
        guard.mark_status_changed();
        ReconnectRequest {
            config,
            callbacks,
            reconnect_generation: guard.reconnect_generation,
        }
    };

    let callbacks_for_error = request.callbacks.clone();
    let spawn_result = thread::Builder::new()
        .name("AndroidConnectReconnect".to_owned())
        .spawn(move || reconnect_worker_loop(request));
    if let Err(error) = spawn_result {
        mark_reconnect_worker_stopped(format!("reconnect worker failed to start: {error}"));
        notify_native_status_changed_from_vm(
            &callbacks_for_error.java_vm,
            &callbacks_for_error.status_class,
        );
        false
    } else {
        true
    }
}

fn reconnect_worker_loop(request: ReconnectRequest) {
    let mut delay = RECONNECT_INITIAL_DELAY;
    loop {
        thread::sleep(delay);
        if !mark_reconnect_attempt(request.reconnect_generation) {
            return;
        }
        notify_native_status_changed_from_vm(
            &request.callbacks.java_vm,
            &request.callbacks.status_class,
        );

        match connect_session(
            &request.config,
            &request.callbacks,
            Some(request.reconnect_generation),
        ) {
            Ok(()) => {
                notify_native_status_changed_from_vm(
                    &request.callbacks.java_vm,
                    &request.callbacks.status_class,
                );
                return;
            }
            Err(error) => {
                if !mark_reconnect_failure(request.reconnect_generation, error) {
                    return;
                }
                notify_native_status_changed_from_vm(
                    &request.callbacks.java_vm,
                    &request.callbacks.status_class,
                );
                delay = next_reconnect_delay(delay);
            }
        }
    }
}

fn mark_reconnect_attempt(reconnect_generation: u64) -> bool {
    if let Ok(mut guard) = state().lock()
        && guard.reconnect_generation == reconnect_generation
        && guard.reconnecting
    {
        guard.reconnect_attempts += 1;
        guard.mark_status_changed();
        true
    } else {
        false
    }
}

fn mark_reconnect_failure(reconnect_generation: u64, error: String) -> bool {
    if let Ok(mut guard) = state().lock()
        && guard.reconnect_generation == reconnect_generation
        && guard.reconnecting
    {
        guard.last_reconnect_error = Some(error);
        guard.mark_status_changed();
        true
    } else {
        false
    }
}

fn mark_reconnect_worker_stopped(error: String) {
    if let Ok(mut guard) = state().lock() {
        guard.reconnecting = false;
        guard.last_reconnect_error = Some(error);
        guard.mark_status_changed();
    }
}

fn next_reconnect_delay(current: Duration) -> Duration {
    current.saturating_mul(2).min(RECONNECT_MAX_DELAY)
}

fn should_reconnect_after_error(error: &str) -> bool {
    let lower = error.to_ascii_lowercase();
    !lower.contains("pairing")
        && !lower.contains("authentication")
        && !lower.contains("rejected")
        && !lower.contains("unsupported protocol")
        && !lower.contains("stale connection generation")
}

struct AuthContext {
    trust_store: AndroidTrustStore,
    android_device_id: String,
    pairing_code: String,
    challenge: [u8; AUTH_CHALLENGE_BYTES],
}

struct AuthDecision {
    accepted: bool,
    message: String,
    trusted: bool,
    desktop_id: Option<String>,
    desktop_name: Option<String>,
    session_key_fingerprint: Option<[u8; SESSION_KEY_FINGERPRINT_BYTES]>,
}

fn authenticate_desktop(
    context: &mut AuthContext,
    response: &androidconnect_protocol::AuthResponse,
) -> Result<AuthDecision, String> {
    if response.desktop_id.trim().is_empty() {
        return Ok(AuthDecision::rejected("desktop identity missing"));
    }

    let desktop_id = response.desktop_id.clone();
    let desktop_name = if response.desktop_name.trim().is_empty() {
        "Desktop".to_owned()
    } else {
        response.desktop_name.clone()
    };

    let paired_secret = match response.method {
        AuthMethod::PairingCode => {
            if context.pairing_code.is_empty() {
                return Ok(AuthDecision::rejected(
                    "pairing code required for new desktop",
                ));
            }
            let expected = pairing_auth_response(
                &context.pairing_code,
                &context.android_device_id,
                &desktop_id,
                &context.challenge,
            );
            if !auth_responses_equal(&expected, &response.response) {
                return Ok(AuthDecision::rejected("pairing authentication failed"));
            }
            let paired_secret = paired_secret_from_pairing_code(
                &context.pairing_code,
                &context.android_device_id,
                &desktop_id,
            );
            context
                .trust_store
                .store_desktop(&desktop_id, &desktop_name, paired_secret)
                .map_err(|error| format!("pairing persistence failed: {error}"))?;
            paired_secret
        }
        AuthMethod::TrustedSession => {
            let Some(paired_desktop) = context.trust_store.paired_desktop(&desktop_id) else {
                return Ok(AuthDecision::rejected("desktop is not paired"));
            };
            let expected = trusted_session_auth_response(
                &paired_desktop.paired_secret,
                &context.android_device_id,
                &desktop_id,
                &context.challenge,
            );
            if !auth_responses_equal(&expected, &response.response) {
                return Ok(AuthDecision::rejected(
                    "trusted session authentication failed",
                ));
            }
            context
                .trust_store
                .mark_authenticated(&desktop_id, &desktop_name)
                .map_err(|error| format!("trusted session persistence failed: {error}"))?;
            paired_desktop.paired_secret
        }
    };

    let session_key = derive_session_key(
        &paired_secret,
        &context.android_device_id,
        &desktop_id,
        &context.challenge,
    );
    let fingerprint = session_key_fingerprint(&session_key);

    Ok(AuthDecision {
        accepted: true,
        message: match response.method {
            AuthMethod::PairingCode => "paired and trusted".to_owned(),
            AuthMethod::TrustedSession => "trusted session authenticated".to_owned(),
        },
        trusted: matches!(response.method, AuthMethod::TrustedSession),
        desktop_id: Some(desktop_id),
        desktop_name: Some(desktop_name),
        session_key_fingerprint: Some(fingerprint),
    })
}

impl AuthDecision {
    fn rejected(message: impl Into<String>) -> Self {
        Self {
            accepted: false,
            message: message.into(),
            trusted: false,
            desktop_id: None,
            desktop_name: None,
            session_key_fingerprint: None,
        }
    }
}

#[derive(Debug)]
struct AndroidTrustStore {
    path: PathBuf,
    data: AndroidTrustData,
}

#[derive(Debug, Clone)]
struct PairedDesktop {
    paired_secret: [u8; PAIRED_SECRET_BYTES],
}

#[derive(Debug, Serialize, Deserialize)]
struct AndroidTrustData {
    device_id: String,
    paired_desktops: Vec<PairedDesktopRecord>,
}

#[derive(Debug, Serialize, Deserialize)]
struct PairedDesktopRecord {
    desktop_id: String,
    desktop_name: String,
    paired_secret_hex: String,
    last_authenticated_unix_ms: u64,
}

impl AndroidTrustStore {
    fn load_or_create(storage_dir: &Path) -> Result<Self, String> {
        if storage_dir.as_os_str().is_empty() {
            return Err("empty storage dir".to_owned());
        }
        let path = storage_dir.join("androidconnect-trust.json");
        let data = if path.exists() {
            let raw = fs::read_to_string(&path)
                .map_err(|error| format!("read {} failed: {error}", path.display()))?;
            serde_json::from_str(&raw)
                .map_err(|error| format!("parse {} failed: {error}", path.display()))?
        } else {
            AndroidTrustData {
                device_id: String::new(),
                paired_desktops: Vec::new(),
            }
        };

        let mut store = Self { path, data };
        if store.data.device_id.trim().is_empty() {
            store.data.device_id = format!("android-{}", generate_hex_id(16));
            store.save()?;
        }
        Ok(store)
    }

    fn device_id(&self) -> &str {
        &self.data.device_id
    }

    fn trusted_desktop_count(&self) -> usize {
        self.data.paired_desktops.len()
    }

    fn paired_desktop(&self, desktop_id: &str) -> Option<PairedDesktop> {
        self.data
            .paired_desktops
            .iter()
            .find(|record| record.desktop_id == desktop_id)
            .and_then(|record| {
                let paired_secret = hex_to_fixed::<PAIRED_SECRET_BYTES>(&record.paired_secret_hex)?;
                Some(PairedDesktop { paired_secret })
            })
    }

    fn store_desktop(
        &mut self,
        desktop_id: &str,
        desktop_name: &str,
        paired_secret: [u8; PAIRED_SECRET_BYTES],
    ) -> Result<(), String> {
        let timestamp = now_unix_ms();
        let paired_secret_hex = bytes_to_hex(&paired_secret);
        if let Some(record) = self
            .data
            .paired_desktops
            .iter_mut()
            .find(|record| record.desktop_id == desktop_id)
        {
            record.desktop_name = desktop_name.to_owned();
            record.paired_secret_hex = paired_secret_hex;
            record.last_authenticated_unix_ms = timestamp;
        } else {
            self.data.paired_desktops.push(PairedDesktopRecord {
                desktop_id: desktop_id.to_owned(),
                desktop_name: desktop_name.to_owned(),
                paired_secret_hex,
                last_authenticated_unix_ms: timestamp,
            });
        }
        self.save()
    }

    fn mark_authenticated(&mut self, desktop_id: &str, desktop_name: &str) -> Result<(), String> {
        if let Some(record) = self
            .data
            .paired_desktops
            .iter_mut()
            .find(|record| record.desktop_id == desktop_id)
        {
            record.desktop_name = desktop_name.to_owned();
            record.last_authenticated_unix_ms = now_unix_ms();
            self.save()?;
        }
        Ok(())
    }

    fn save(&self) -> Result<(), String> {
        if let Some(parent) = self.path.parent()
            && !parent.as_os_str().is_empty()
        {
            fs::create_dir_all(parent)
                .map_err(|error| format!("create {} failed: {error}", parent.display()))?;
        }
        let raw = serde_json::to_string_pretty(&self.data)
            .map_err(|error| format!("encode trust store failed: {error}"))?;
        fs::write(&self.path, raw)
            .map_err(|error| format!("write {} failed: {error}", self.path.display()))
    }
}

struct InputDispatcher {
    input_class: GlobalRef,
    active_pointer: Option<ActivePointer>,
}

impl InputDispatcher {
    fn new(input_class: GlobalRef) -> Self {
        Self {
            input_class,
            active_pointer: None,
        }
    }

    fn dispatch(&mut self, env: &mut JNIEnv<'_>, event: InputEvent) -> bool {
        match event {
            InputEvent::Pointer(pointer) => self.dispatch_pointer(env, pointer),
            InputEvent::Text(text) => self.dispatch_text(env, text.text),
            InputEvent::System(action) => self.dispatch_system_action(env, action),
            InputEvent::Key(key) => self.dispatch_key(env, key),
        }
    }

    fn dispatch_pointer(&mut self, env: &mut JNIEnv<'_>, event: PointerEvent) -> bool {
        match event.phase {
            PointerPhase::Down => {
                if event.button != PointerButton::Left {
                    return false;
                }
                self.active_pointer = Some(ActivePointer {
                    pointer_id: event.pointer_id,
                    start_x: event.x,
                    start_y: event.y,
                    last_x: event.x,
                    last_y: event.y,
                    started_at: Instant::now(),
                });
                true
            }
            PointerPhase::Move => {
                if event.button != PointerButton::Left {
                    return false;
                }
                if let Some(pointer) = self.active_pointer.as_mut()
                    && pointer.pointer_id == event.pointer_id
                {
                    pointer.last_x = event.x;
                    pointer.last_y = event.y;
                }
                true
            }
            PointerPhase::Up => {
                if event.button != PointerButton::Left {
                    return false;
                }
                let Some(pointer) = self.active_pointer.take() else {
                    return self.tap(env, event.x, event.y);
                };
                if pointer.pointer_id != event.pointer_id {
                    return false;
                }

                let distance_x = event.x - pointer.start_x;
                let distance_y = event.y - pointer.start_y;
                let duration_ms = pointer.started_at.elapsed().as_millis().max(1) as jlong;
                if distance_x.abs() <= 8 && distance_y.abs() <= 8 {
                    self.tap(env, event.x, event.y)
                } else {
                    self.drag(
                        env,
                        pointer.start_x,
                        pointer.start_y,
                        event.x,
                        event.y,
                        duration_ms,
                    )
                }
            }
            PointerPhase::Wheel => {
                let delta = if event.delta_y != 0 {
                    event.delta_y
                } else {
                    event.delta_x
                };
                if delta == 0 {
                    true
                } else {
                    self.scroll(env, delta)
                }
            }
        }
    }

    fn dispatch_text(&self, env: &mut JNIEnv<'_>, text: String) -> bool {
        if text.is_empty() {
            return true;
        }

        let jtext = match env.new_string(text) {
            Ok(value) => value,
            Err(error) => {
                set_last_error(format!("input text JNI string failed: {error}"));
                clear_pending_exception(env);
                return false;
            }
        };
        call_static_bool(
            env,
            &self.input_class,
            "inputText",
            "(Ljava/lang/String;)Z",
            &[JValue::from(&jtext)],
        )
    }

    fn dispatch_system_action(&self, env: &mut JNIEnv<'_>, action: SystemAction) -> bool {
        let method = match action {
            SystemAction::Back => "systemBack",
            SystemAction::Home => "systemHome",
            SystemAction::Recents => "systemRecents",
            SystemAction::LockScreen => "lockScreen",
        };
        call_static_bool(env, &self.input_class, method, "()Z", &[])
    }

    fn dispatch_key(&self, env: &mut JNIEnv<'_>, key: androidconnect_protocol::KeyEvent) -> bool {
        if !key.pressed {
            return true;
        }

        match key.key_code {
            3 => self.dispatch_system_action(env, SystemAction::Home),
            4 => self.dispatch_system_action(env, SystemAction::Back),
            26 => self.dispatch_system_action(env, SystemAction::LockScreen),
            187 => self.dispatch_system_action(env, SystemAction::Recents),
            _ => false,
        }
    }

    fn tap(&self, env: &mut JNIEnv<'_>, x: i32, y: i32) -> bool {
        call_static_bool(
            env,
            &self.input_class,
            "tap",
            "(II)Z",
            &[JValue::Int(x), JValue::Int(y)],
        )
    }

    fn drag(
        &self,
        env: &mut JNIEnv<'_>,
        start_x: i32,
        start_y: i32,
        end_x: i32,
        end_y: i32,
        duration_ms: jlong,
    ) -> bool {
        call_static_bool(
            env,
            &self.input_class,
            "drag",
            "(IIIIJ)Z",
            &[
                JValue::Int(start_x),
                JValue::Int(start_y),
                JValue::Int(end_x),
                JValue::Int(end_y),
                JValue::Long(duration_ms),
            ],
        )
    }

    fn scroll(&self, env: &mut JNIEnv<'_>, delta_y: i32) -> bool {
        call_static_bool(
            env,
            &self.input_class,
            "scroll",
            "(I)Z",
            &[JValue::Int(delta_y)],
        )
    }
}

#[derive(Debug)]
struct ActivePointer {
    pointer_id: u32,
    start_x: i32,
    start_y: i32,
    last_x: i32,
    last_y: i32,
    started_at: Instant,
}

fn call_static_bool(
    env: &mut JNIEnv<'_>,
    class: &GlobalRef,
    name: &str,
    sig: &str,
    args: &[JValue<'_, '_>],
) -> bool {
    match env
        .call_static_method(class, name, sig, args)
        .and_then(|value| value.z())
    {
        Ok(result) => result,
        Err(error) => {
            set_last_error(format!("input dispatch {name} failed: {error}"));
            clear_pending_exception(env);
            false
        }
    }
}

fn call_static_void_string(
    env: &mut JNIEnv<'_>,
    class: &GlobalRef,
    name: &str,
    value: impl AsRef<str>,
) -> Result<(), String> {
    let jvalue = env
        .new_string(value.as_ref())
        .map_err(|error| format!("{name} JNI string failed: {error}"))?;
    env.call_static_method(
        class,
        name,
        "(Ljava/lang/String;)V",
        &[JValue::from(&jvalue)],
    )
    .map(|_| ())
    .map_err(|error| {
        clear_pending_exception(env);
        format!("{name} dispatch failed: {error}")
    })
}

fn call_static_void_strings(
    env: &mut JNIEnv<'_>,
    class: &GlobalRef,
    name: &str,
    first: &str,
    second: &str,
) -> Result<(), String> {
    let jfirst = env
        .new_string(first)
        .map_err(|error| format!("{name} first JNI string failed: {error}"))?;
    let jsecond = env
        .new_string(second)
        .map_err(|error| format!("{name} second JNI string failed: {error}"))?;
    env.call_static_method(
        class,
        name,
        "(Ljava/lang/String;Ljava/lang/String;)V",
        &[JValue::from(&jfirst), JValue::from(&jsecond)],
    )
    .map(|_| ())
    .map_err(|error| {
        clear_pending_exception(env);
        format!("{name} dispatch failed: {error}")
    })
}

fn clear_pending_exception(env: &mut JNIEnv<'_>) {
    if env.exception_check().unwrap_or(false) {
        let _ = env.exception_describe();
        let _ = env.exception_clear();
    }
}

fn notify_native_status_changed(env: &mut JNIEnv<'_>) {
    let Ok(class) = native_bridge_class(env) else {
        clear_pending_exception(env);
        return;
    };
    notify_native_status_changed_with_class(env, &class);
}

fn notify_native_status_changed_with_class(env: &mut JNIEnv<'_>, class: &GlobalRef) {
    if env
        .call_static_method(class, "onNativeStatusChanged", "()V", &[])
        .is_err()
    {
        clear_pending_exception(env);
    }
}

fn notify_native_status_changed_from_vm(java_vm: &JavaVM, class: &GlobalRef) {
    if let Ok(mut env) = java_vm.attach_current_thread() {
        notify_native_status_changed_with_class(&mut env, class);
    }
}

fn native_bridge_class(env: &mut JNIEnv<'_>) -> Result<GlobalRef, String> {
    let class = env
        .find_class("dev/androidconnect/NativeBridge")
        .map_err(|error| format!("NativeBridge class unavailable: {error}"))?;
    env.new_global_ref(class)
        .map_err(|error| format!("NativeBridge global ref failed: {error}"))
}

fn remote_control_class(env: &mut JNIEnv<'_>) -> Result<GlobalRef, String> {
    let class = env
        .find_class("dev/androidconnect/RemoteControlAccessibilityService")
        .map_err(|error| format!("RemoteControlAccessibilityService class unavailable: {error}"))?;
    env.new_global_ref(class)
        .map_err(|error| format!("RemoteControlAccessibilityService global ref failed: {error}"))
}

fn clear_connection_if_generation(generation: u64, error: impl Into<String>) -> bool {
    if let Ok(mut guard) = state().lock()
        && guard.connection_generation == generation
    {
        guard.clear_connection(error);
        true
    } else {
        false
    }
}

fn set_authenticated_desktop_if_generation(
    generation: u64,
    authenticated: bool,
    desktop_id: Option<String>,
    desktop_name: Option<String>,
    session_key_fingerprint: Option<[u8; SESSION_KEY_FINGERPRINT_BYTES]>,
    trusted_desktop_count: usize,
) -> bool {
    if let Ok(mut guard) = state().lock()
        && guard.connection_generation == generation
    {
        guard.input_authenticated = authenticated;
        guard.paired_desktop_id = if authenticated { desktop_id } else { None };
        guard.paired_desktop_name = if authenticated { desktop_name } else { None };
        guard.session_key_fingerprint = if authenticated {
            session_key_fingerprint
        } else {
            None
        };
        guard.trusted_desktop_count = trusted_desktop_count;
        guard.mark_status_changed();
        true
    } else {
        false
    }
}

fn send_payload_if_generation(generation: u64, payload: Payload) -> Result<(), String> {
    let mut guard = state()
        .lock()
        .map_err(|_| "state lock poisoned while sending auth payload".to_owned())?;
    if guard.connection_generation != generation {
        return Err("stale connection generation".to_owned());
    }
    guard.send_payload(payload)
}

fn send_pong_if_generation(generation: u64, nonce: u64) -> Result<(), String> {
    let mut guard = state()
        .lock()
        .map_err(|_| "state lock poisoned while sending pong".to_owned())?;
    if guard.connection_generation != generation {
        return Err("stale connection generation".to_owned());
    }
    guard.received_pings += 1;
    guard.send_payload(Payload::Pong { nonce })?;
    guard.sent_pongs += 1;
    Ok(())
}

fn configure_stream(stream: &TcpStream) -> io::Result<()> {
    stream.set_nodelay(true)
}

fn close_stream(stream: Option<TcpStream>) {
    if let Some(stream) = stream {
        let _ = stream.shutdown(Shutdown::Both);
    }
}

fn set_last_error(error: impl Into<String>) {
    if let Ok(mut guard) = state().lock() {
        guard.last_error = Some(error.into());
        guard.mark_status_changed();
    }
}

fn make_auth_challenge() -> AuthChallenge {
    let mut challenge = [0_u8; AUTH_CHALLENGE_BYTES];
    if fill_random(&mut challenge).is_err() {
        fill_fallback_challenge(&mut challenge);
    }
    AuthChallenge { challenge }
}

fn generate_hex_id(bytes: usize) -> String {
    let mut raw = vec![0_u8; bytes];
    if fill_random(&mut raw).is_err() {
        fill_fallback_challenge(&mut raw);
    }
    bytes_to_hex(&raw)
}

fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or_default()
}

fn fill_random(output: &mut [u8]) -> io::Result<()> {
    let mut file = std::fs::File::open("/dev/urandom")?;
    file.read_exact(output)
}

fn fill_fallback_challenge(output: &mut [u8]) {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    let mut seed = now ^ ((std::process::id() as u128) << 64);
    for chunk in output.chunks_mut(8) {
        seed ^= seed << 7;
        seed ^= seed >> 9;
        seed = seed.wrapping_mul(0x9e37_79b9_7f4a_7c15);
        let bytes = seed.to_le_bytes();
        chunk.copy_from_slice(&bytes[..chunk.len()]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stats_json_exposes_heartbeat_counters() {
        let _guard = test_lock().lock().expect("test lock");
        let mut guard = state().lock().expect("state lock");
        *guard = AppState::default();
        guard.received_pings = 3;
        guard.sent_pongs = 2;
        drop(guard);

        let stats: serde_json::Value =
            serde_json::from_str(&current_stats_json()).expect("valid stats json");
        assert_eq!(stats["received_pings"], 3);
        assert_eq!(stats["sent_pongs"], 2);
    }

    #[test]
    fn stats_json_exposes_reconnect_state() {
        let _guard = test_lock().lock().expect("test lock");
        let mut guard = state().lock().expect("state lock");
        *guard = AppState::default();
        guard.reconnecting = true;
        guard.reconnect_attempts = 3;
        guard.last_reconnect_error = Some("connect failed".to_owned());
        drop(guard);

        let stats = stats_value();
        assert_eq!(stats["reconnecting"], true);
        assert_eq!(stats["reconnect_attempts"], 3);
        assert_eq!(stats["last_reconnect_error"], "connect failed");
    }

    #[test]
    fn status_revision_changes_with_native_status() {
        let _guard = test_lock().lock().expect("test lock");
        let mut guard = state().lock().expect("state lock");
        *guard = AppState::default();
        drop(guard);

        assert_eq!(stats_value()["status_revision"], 0);
        set_last_error("connection failed");

        let stats = stats_value();
        assert_eq!(stats["status_revision"], 1);
        assert_eq!(stats["last_error"], "connection failed");
    }

    #[test]
    fn android_trust_store_persists_device_and_desktop() {
        let _guard = test_lock().lock().expect("test lock");
        let dir = test_storage_dir("android-trust-store");
        let secret = [5_u8; PAIRED_SECRET_BYTES];

        let mut store = AndroidTrustStore::load_or_create(&dir).expect("create trust store");
        let device_id = store.device_id().to_owned();
        store
            .store_desktop("desktop-1", "Desktop", secret)
            .expect("store desktop");
        drop(store);

        let store = AndroidTrustStore::load_or_create(&dir).expect("reload trust store");
        assert_eq!(store.device_id(), device_id);
        assert_eq!(store.trusted_desktop_count(), 1);
        assert_eq!(
            store
                .paired_desktop("desktop-1")
                .map(|pairing| pairing.paired_secret),
            Some(secret)
        );

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn authenticate_desktop_accepts_pairing_then_trusted_session() {
        let _guard = test_lock().lock().expect("test lock");
        let dir = test_storage_dir("android-auth");
        let store = AndroidTrustStore::load_or_create(&dir).expect("create trust store");
        let android_device_id = store.device_id().to_owned();
        let challenge = [11_u8; AUTH_CHALLENGE_BYTES];
        let desktop_id = "desktop-1";
        let desktop_name = "Test Desktop";
        let pairing_code = "123456";
        let mut context = AuthContext {
            trust_store: store,
            android_device_id: android_device_id.clone(),
            pairing_code: pairing_code.to_owned(),
            challenge,
        };

        let pairing_response = androidconnect_protocol::AuthResponse {
            desktop_id: desktop_id.to_owned(),
            desktop_name: desktop_name.to_owned(),
            method: AuthMethod::PairingCode,
            response: pairing_auth_response(
                pairing_code,
                &android_device_id,
                desktop_id,
                &challenge,
            ),
        };
        let decision = authenticate_desktop(&mut context, &pairing_response).expect("pairing auth");
        assert!(decision.accepted);
        assert!(!decision.trusted);

        let paired_secret = context
            .trust_store
            .paired_desktop(desktop_id)
            .expect("stored desktop")
            .paired_secret;
        let trusted_response = androidconnect_protocol::AuthResponse {
            desktop_id: desktop_id.to_owned(),
            desktop_name: desktop_name.to_owned(),
            method: AuthMethod::TrustedSession,
            response: trusted_session_auth_response(
                &paired_secret,
                &android_device_id,
                desktop_id,
                &challenge,
            ),
        };
        let decision = authenticate_desktop(&mut context, &trusted_response).expect("trusted auth");
        assert!(decision.accepted);
        assert!(decision.trusted);

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn reconnect_backoff_caps_at_max_delay() {
        assert_eq!(
            next_reconnect_delay(Duration::from_millis(500)),
            Duration::from_secs(1)
        );
        assert_eq!(
            next_reconnect_delay(RECONNECT_MAX_DELAY),
            RECONNECT_MAX_DELAY
        );
    }

    #[test]
    fn reconnect_skips_authentication_failures() {
        assert!(should_reconnect_after_error(
            "io error: failed to fill whole buffer"
        ));
        assert!(!should_reconnect_after_error(
            "pairing authentication failed"
        ));
        assert!(!should_reconnect_after_error(
            "unsupported protocol version 2"
        ));
    }

    fn stats_value() -> serde_json::Value {
        serde_json::from_str(&current_stats_json()).expect("valid stats json")
    }

    fn test_storage_dir(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "androidconnect-{name}-{}-{}",
            std::process::id(),
            now_unix_ms()
        ))
    }

    fn test_lock() -> &'static Mutex<()> {
        static TEST_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        TEST_LOCK.get_or_init(|| Mutex::new(()))
    }
}
