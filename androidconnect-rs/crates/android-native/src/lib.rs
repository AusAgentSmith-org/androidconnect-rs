use std::fs;
use std::io::{self, Read};
use std::net::{Shutdown, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use androidconnect_protocol::{
    AUTH_CHALLENGE_BYTES, AuthChallenge, AuthMethod, AuthResult, DeviceHello, Envelope, InputEvent,
    MAX_CONTROL_FRAME_BYTES, PAIRED_SECRET_BYTES, PROTOCOL_VERSION, Payload, PointerButton,
    PointerEvent, PointerPhase, SESSION_KEY_FINGERPRINT_BYTES, SystemAction, VideoCodec,
    VideoFormat, VideoFrame, auth_responses_equal, bytes_to_hex, derive_session_key, hex_to_fixed,
    normalize_pairing_code, paired_secret_from_pairing_code, pairing_auth_response,
    read_length_prefixed, session_key_fingerprint, trusted_session_auth_response,
    video_flags_from_android_media_codec, write_length_prefixed,
};
use jni::objects::{GlobalRef, JByteArray, JClass, JString, JValue};
use jni::sys::{JNI_FALSE, JNI_TRUE, jboolean, jint, jlong, jstring};
use jni::{JNIEnv, JavaVM};
use serde::{Deserialize, Serialize};

static STATE: OnceLock<Mutex<AppState>> = OnceLock::new();

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
    sent_envelopes: u64,
    sent_bytes: u64,
    received_pings: u64,
    sent_pongs: u64,
    status_revision: u64,
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
            sent_envelopes: 0,
            sent_bytes: 0,
            received_pings: 0,
            sent_pongs: 0,
            status_revision: 0,
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

    fn clear_connection(&mut self, error: impl Into<String>) {
        self.connection_generation += 1;
        close_stream(self.stream.take());
        self.connected_to = None;
        self.input_authenticated = false;
        self.paired_desktop_id = None;
        self.paired_desktop_name = None;
        self.session_key_fingerprint = None;
        self.last_error = Some(error.into());
        self.mark_status_changed();
    }

    fn is_connected(&self) -> bool {
        self.stream.is_some()
    }

    fn mark_status_changed(&mut self) {
        self.status_revision = self.status_revision.wrapping_add(1);
    }
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
    let trust_store = match AndroidTrustStore::load_or_create(&storage_dir) {
        Ok(store) => store,
        Err(error) => {
            set_last_error(format!("trust store unavailable: {error}"));
            notify_native_status_changed(&mut env);
            return JNI_FALSE;
        }
    };
    let android_device_id = trust_store.device_id().to_owned();
    let trusted_desktop_count = trust_store.trusted_desktop_count();
    if pairing_code.is_empty() && trusted_desktop_count == 0 {
        set_last_error("pairing code required for first desktop");
        notify_native_status_changed(&mut env);
        return JNI_FALSE;
    }

    let address = format!("{}:{}", host.trim(), port);
    let stream = match TcpStream::connect(&address) {
        Ok(stream) => stream,
        Err(error) => {
            set_last_error(format!("connect to {address} failed: {error}"));
            notify_native_status_changed(&mut env);
            return JNI_FALSE;
        }
    };
    if let Err(error) = configure_stream(&stream) {
        set_last_error(format!("configure {address} failed: {error}"));
        notify_native_status_changed(&mut env);
        return JNI_FALSE;
    }
    let input_stream = match stream.try_clone() {
        Ok(stream) => stream,
        Err(error) => {
            set_last_error(format!("clone {address} stream failed: {error}"));
            notify_native_status_changed(&mut env);
            return JNI_FALSE;
        }
    };
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

    let mut guard = match state().lock() {
        Ok(guard) => guard,
        Err(_) => return JNI_FALSE,
    };
    guard.connection_generation += 1;
    guard.stream = Some(stream);
    guard.connected_to = Some(address.clone());
    guard.pairing_code_set = !pairing_code.is_empty();
    guard.input_authenticated = false;
    guard.paired_desktop_id = None;
    guard.paired_desktop_name = None;
    guard.session_key_fingerprint = None;
    guard.trusted_desktop_count = trusted_desktop_count;
    guard.received_pings = 0;
    guard.sent_pongs = 0;
    guard.last_error = None;
    guard.mark_status_changed();
    let generation = guard.connection_generation;
    let auth_challenge = make_auth_challenge();

    let hello = Payload::Hello(DeviceHello::android(android_device_id.clone(), device_name));
    let send_result = guard
        .send_payload(hello)
        .and_then(|()| guard.send_payload(Payload::AuthChallenge(auth_challenge.clone())));
    match send_result {
        Ok(()) => {
            if let Some(format) = guard.last_video_format.clone()
                && let Err(error) = guard.send_payload(Payload::VideoFormat(format))
            {
                guard.clear_connection(format!("format send to {address} failed: {error}"));
                drop(guard);
                notify_native_status_changed_with_class(&mut env, &status_class);
                return JNI_FALSE;
            }
            drop(guard);
            notify_native_status_changed_with_class(&mut env, &status_class);
            spawn_input_reader(
                java_vm,
                input_class,
                status_class,
                input_stream,
                generation,
                address,
                AuthContext {
                    trust_store,
                    android_device_id,
                    pairing_code,
                    challenge: auth_challenge.challenge,
                },
            );
            JNI_TRUE
        }
        Err(error) => {
            guard.clear_connection(format!("hello send to {address} failed: {error}"));
            drop(guard);
            notify_native_status_changed_with_class(&mut env, &status_class);
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
        guard.connection_generation += 1;
        close_stream(guard.stream.take());
        guard.connected_to = None;
        guard.pairing_code_set = false;
        guard.input_authenticated = false;
        guard.paired_desktop_id = None;
        guard.paired_desktop_name = None;
        guard.session_key_fingerprint = None;
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
            guard.clear_connection(error);
            drop(guard);
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
            guard.clear_connection(error);
            drop(guard);
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
        "received_pings": guard.received_pings,
        "sent_pongs": guard.sent_pongs,
        "pairing_code_set": guard.pairing_code_set,
        "input_authenticated": guard.input_authenticated,
        "paired_desktop_id": guard.paired_desktop_id.clone(),
        "paired_desktop_name": guard.paired_desktop_name.clone(),
        "session_key_fingerprint": guard.session_key_fingerprint.map(|fingerprint| bytes_to_hex(&fingerprint)),
        "trusted_desktop_count": guard.trusted_desktop_count,
        "status_revision": guard.status_revision,
        "uptime_ms": capture.map(|capture| capture.started_at.elapsed().as_millis()).unwrap_or_default(),
        "last_error": guard.last_error,
    })
    .to_string()
}

fn spawn_input_reader(
    java_vm: JavaVM,
    input_class: GlobalRef,
    status_class: GlobalRef,
    stream: TcpStream,
    generation: u64,
    address: String,
    auth_context: AuthContext,
) {
    let thread_address = address.clone();
    let spawn_result = thread::Builder::new()
        .name("AndroidConnectInputReader".to_owned())
        .spawn(move || {
            let result = input_reader_loop(
                &java_vm,
                input_class,
                &status_class,
                stream,
                generation,
                auth_context,
            );
            if let Err(error) = result {
                let changed = clear_connection_if_generation(
                    generation,
                    format!("input reader from {thread_address} stopped: {error}"),
                );
                if changed {
                    notify_native_status_changed_from_vm(&java_vm, &status_class);
                }
            }
        });

    if let Err(error) = spawn_result {
        clear_connection_if_generation(
            generation,
            format!("input reader for {address} failed to start: {error}"),
        );
    }
}

fn input_reader_loop(
    java_vm: &JavaVM,
    input_class: GlobalRef,
    status_class: &GlobalRef,
    mut stream: TcpStream,
    generation: u64,
    mut auth_context: AuthContext,
) -> Result<(), String> {
    let mut env = java_vm
        .attach_current_thread()
        .map_err(|error| format!("attach JNI input thread failed: {error}"))?;
    let mut dispatcher = InputDispatcher::new(input_class);
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
                    notify_native_status_changed_with_class(&mut env, status_class);
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
        }
    }
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
