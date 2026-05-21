use std::io;
use std::net::TcpStream;
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

use androidconnect_protocol::{
    DeviceHello, Envelope, Payload, VideoCodec, VideoFormat, VideoFrame,
    video_flags_from_android_media_codec, write_length_prefixed,
};
use jni::JNIEnv;
use jni::objects::{JByteArray, JClass, JString};
use jni::sys::{JNI_FALSE, JNI_TRUE, jboolean, jint, jlong, jstring};

static STATE: OnceLock<Mutex<AppState>> = OnceLock::new();

#[derive(Debug)]
struct AppState {
    sequence: u64,
    capture: Option<CaptureState>,
    stream: Option<TcpStream>,
    connected_to: Option<String>,
    last_error: Option<String>,
    last_video_format: Option<VideoFormat>,
    sent_envelopes: u64,
    sent_bytes: u64,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            sequence: 1,
            capture: None,
            stream: None,
            connected_to: None,
            last_error: None,
            last_video_format: None,
            sent_envelopes: 0,
            sent_bytes: 0,
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
        self.stream = None;
        self.connected_to = None;
        self.last_error = Some(error.into());
    }

    fn is_connected(&self) -> bool {
        self.stream.is_some()
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

    1
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_androidconnect_NativeBridge_nativeStopSession(
    _env: JNIEnv<'_>,
    _class: JClass<'_>,
) {
    if let Ok(mut guard) = state().lock() {
        guard.capture = None;
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_androidconnect_NativeBridge_nativeConnect(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
    host: JString<'_>,
    port: jint,
    device_name: JString<'_>,
    _pairing_code: JString<'_>,
) -> jboolean {
    let host = match env.get_string(&host) {
        Ok(value) => value.to_string_lossy().into_owned(),
        Err(error) => {
            set_last_error(format!("invalid host string: {error}"));
            return JNI_FALSE;
        }
    };
    let device_name = env
        .get_string(&device_name)
        .map(|value| value.to_string_lossy().into_owned())
        .unwrap_or_else(|_| "Android device".to_owned());

    if host.trim().is_empty() || port <= 0 || port > u16::MAX as jint {
        set_last_error("invalid host or port");
        return JNI_FALSE;
    }

    let address = format!("{}:{}", host.trim(), port);
    let stream = match TcpStream::connect(&address) {
        Ok(stream) => stream,
        Err(error) => {
            set_last_error(format!("connect to {address} failed: {error}"));
            return JNI_FALSE;
        }
    };
    if let Err(error) = configure_stream(&stream) {
        set_last_error(format!("configure {address} failed: {error}"));
        return JNI_FALSE;
    }

    let mut guard = match state().lock() {
        Ok(guard) => guard,
        Err(_) => return JNI_FALSE,
    };
    guard.stream = Some(stream);
    guard.connected_to = Some(address.clone());
    guard.last_error = None;

    let hello = Payload::Hello(DeviceHello::android("android-local", device_name));
    match guard.send_payload(hello) {
        Ok(()) => {
            if let Some(format) = guard.last_video_format.clone()
                && let Err(error) = guard.send_payload(Payload::VideoFormat(format))
            {
                guard.clear_connection(format!("format send to {address} failed: {error}"));
                return JNI_FALSE;
            }
            JNI_TRUE
        }
        Err(error) => {
            guard.clear_connection(format!("hello send to {address} failed: {error}"));
            JNI_FALSE
        }
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_androidconnect_NativeBridge_nativeDisconnect(
    _env: JNIEnv<'_>,
    _class: JClass<'_>,
) {
    if let Ok(mut guard) = state().lock() {
        guard.stream = None;
        guard.connected_to = None;
        guard.last_error = None;
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_androidconnect_NativeBridge_nativePushVideoFormat(
    _env: JNIEnv<'_>,
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
            JNI_FALSE
        }
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_androidconnect_NativeBridge_nativePushVideoFrame(
    env: JNIEnv<'_>,
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
        "uptime_ms": capture.map(|capture| capture.started_at.elapsed().as_millis()).unwrap_or_default(),
        "last_error": guard.last_error,
    })
    .to_string()
}

fn configure_stream(stream: &TcpStream) -> io::Result<()> {
    stream.set_nodelay(true)
}

fn set_last_error(error: impl Into<String>) {
    if let Ok(mut guard) = state().lock() {
        guard.last_error = Some(error.into());
    }
}
